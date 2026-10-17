# rust-coding-agent

用 Rust 构建的 Coding Agent 框架：流式输出、工具调用、授权审批、会话持久化，可对接任何 OpenAI 兼容的 API。

## 架构

```
src/
├── main.rs          CLI / REPL 终端交互（实现 AgentHooks）
├── config.rs        配置（环境变量 + CLI 参数）
├── error.rs         统一错误类型（thiserror）
├── util.rs          截断、路径解析等工具函数
├── agent/
│   └── mod.rs       ★ Agent 核心循环 + AgentHooks 接口
├── llm/
│   ├── mod.rs       ★ LlmProvider trait、Message/ToolCall/ChatRequest 类型
│   └── openai.rs    OpenAI 兼容客户端（SSE 流式解析、tool_calls 增量聚合）
├── tools/
│   ├── mod.rs       ★ Tool trait + ToolRegistry（注册/分发/只读判断）
│   ├── shell.rs     shell 工具（异步执行、超时、输出截断）
│   └── fs.rs        read_file / write_file / edit_file 工具
└── session.rs       会话 JSONL 持久化（每条消息一行，可 --resume）
```

核心循环（`agent/mod.rs`）：

```
用户输入 → [组装 system + 历史 + 工具定义]
        → LLM（流式）→ 文本增量转发给 Hooks（终端打字输出）
        → 有 tool_calls？ ──是──→ 审批（只读免审/其余询问用户）→ 执行 → 结果写回历史 → 继续请求 LLM
                        └──否──→ 本轮结束，自动保存会话
```

设计要点：

- **错误不中断循环**：工具报错、参数非法、用户拒绝，都转成对模型可读的文本写回历史，让模型自行纠正。
- **流式在 Provider 内聚合**：`chat_stream` 吐 `Delta`（文本增量）和 `Completed`（聚合完 tool_calls 的完整消息），Agent 层不关心 SSE 分片细节。
- **只读判断控制授权**：`Tool::is_readonly()` 为 true 的工具（读文件）免确认；写文件、执行命令需要用户 y/N 确认。
- **edit_file 要求唯一匹配**：old_string 多处匹配时报错并提示模型提供更多上下文，避免误改。

## 运行

> 推荐：复制 `.env.example` 为 `.env` 填入凭据，程序启动时自动加载，无需 `export`。
> 实际 shell 环境变量优先于 `.env`，且 `.env` 已被 gitignore。

### 方式一：MiMo 国内订阅（Token Plan）

```bash
# Token Plan 控制台（https://platform.xiaomimimo.com/#/console/plan-manage）
# 获取专属 Key（个人版 tp-xxxxx / 团队版 tttp-xxxxx）
export MIMO_API_KEY=tp-...   # 或写入 .env
cargo run -- "读一下 Cargo.toml，告诉我这个项目用了哪些依赖"
```

设置 `MIMO_API_KEY` 后自动使用国内订阅预设：

| 项目 | 预设值 |
|---|---|
| Base URL | `https://token-plan-cn.xiaomimimo.com/v1`（中国集群） |
| 模型 | `mimo-v2.6-pro`（可用 `MIMO_MODEL=mimo-v2.6-flash` 切换） |
| 鉴权 | `Authorization: Bearer`（MiMo 官方 OpenAI 兼容协议） |

其他集群/按量付费可覆盖：

```bash
export MIMO_BASE_URL=https://token-plan-sgp.xiaomimimo.com/v1  # 新加坡集群
export MIMO_BASE_URL=https://token-plan-ams.xiaomimimo.com/v1  # 欧洲集群
export MIMO_BASE_URL=https://api.xiaomimimo.com/v1             # 按量付费（Key 格式 sk-xxxxx）
```

> **推理模型兼容已处理**：MiMo 的 `reasoning_content`（思维链）会在流式响应中聚合，
> 原样保留在历史里并完整回传（Agent 多轮工具调用场景不回传会报 400），会话文件也会持久化该字段。

### 方式二：任意 OpenAI 兼容端点

```bash
export OPENAI_API_KEY=sk-...            # 本地端点可不设
export OPENAI_BASE_URL=https://api.openai.com/v1   # 或 DeepSeek/Ollama/vLLM/自建网关
export OPENAI_MODEL=gpt-4o

cargo run -- "读一下 Cargo.toml，告诉我这个项目用了哪些依赖"   # 单次执行
cargo run -- --auto-approve --model deepseek-chat "..."       # 跳过授权确认
cargo run -- --resume sessions/1700000000.jsonl               # 恢复会话
```

优先级：CLI 参数 > `OPENAI_*` 环境变量 > `MIMO_*` 环境变量 > 端点预设默认值。
REPL 内每轮自动把历史写入 `sessions/<时间戳>.jsonl`（`/clear` `/save` `/exit`）。

## 扩展指南

### 新增一个工具

```rust
pub struct GrepTool { workdir: PathBuf }

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &'static str { "grep" }
    fn description(&self) -> &'static str { "在文件中搜索正则，返回 文件:行号:内容" }
    fn parameters(&self) -> serde_json::Value {
        json!({ "type": "object",
                "properties": { "pattern": { "type": "string" } },
                "required": ["pattern"] })
    }
    fn is_readonly(&self) -> bool { true }   // 只读 → 免授权
    async fn execute(&self, args: serde_json::Value) -> Result<String> { /* ... */ }
}
```

然后在 `main.rs::build_agent` 里 `ToolRegistry::new(vec![..., Arc::new(GrepTool { ... })])` 即可，模型会自动看到新工具。

### 接入新的 LLM 后端

实现 `LlmProvider` 的两个方法（`chat` / `chat_stream`），在 `build_agent` 中替换 `OpenAiProvider`。只要是 OpenAI 兼容协议，直接改 `OPENAI_BASE_URL` 就行，无需写代码。

### 接入 MCP

MCP 协议本质是"远程版 Tool trait"：写一个启动时拉取 MCP server 工具列表的适配器，把每个远端工具包装成 `Arc<dyn Tool>`（`execute` 转发 JSON-RPC 调用）注册进 `ToolRegistry` 即可。

## 后续路线（按优先级）

1. **上下文压缩**：历史过长时用 LLM 摘要旧消息，只保留最近窗口 + 摘要。
2. **diff 编辑**：接入 `similar`/`tree-sitter`，支持按 diff 应用编辑，减少 old_string 不唯一问题。
3. **子任务并行**：独立任务拆给 sub-agent（各自独立历史），结果汇总。
4. **TUI**：用 `ratatui`/`crossterm` 替换终端打印，支持 Markdown 渲染、diff 高亮。
5. **沙箱**：shell 工具放进容器/bubblewrap 级隔离，替代 y/N 授权。
6. **检查点**：每次写操作前 git commit/stash，支持一键回滚。

## 测试

```bash
cargo test    # 工具唯一性编辑、行号读取、会话 roundtrip、截断/路径等 6 个测试
```
