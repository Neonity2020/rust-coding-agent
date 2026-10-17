//! Agent 核心：把 LLM、工具、授权审批串成一个循环。
//!
//! 循环逻辑：
//! 1. 把用户输入 push 进历史；
//! 2. 带着完整历史 + 工具定义请求 LLM（流式）；
//! 3. 转发文本增量给 [`AgentHooks`]（UI 层负责打印）；
//! 4. 若返回工具调用 → 逐个审批/执行 → 结果作为 tool 消息写回历史 → 回到 2；
//! 5. 若无工具调用 → 本轮用户输入处理完毕。

use std::path::Path;
use std::sync::Arc;

use futures::StreamExt;
use serde_json::Value;

use crate::error::{AgentError, Result};
use crate::llm::{ChatRequest, LlmProvider, Message, StreamEvent, ToolCall};
use crate::tools::ToolRegistry;
use crate::util::clip;

/// UI / 宿主环境与 Agent 的回调接口。实现它来定制输出与授权交互。
pub trait AgentHooks {
    /// 收到一段增量思考内容（推理模型思维链）。默认忽略，需要时可自行打印。
    fn on_thinking(&mut self, _delta: &str) {}

    /// 收到一段增量文本（流式"打字"输出）。
    fn on_text(&mut self, delta: &str);

    /// 即将执行工具。
    fn on_tool_start(&mut self, name: &str, args_display: &str);

    /// 工具执行结束（output 已截断）。
    fn on_tool_end(&mut self, name: &str, output: &str);

    /// 询问用户是否授权非只读工具。返回 false 表示拒绝。
    fn approve(&mut self, _name: &str, _args_display: &str) -> bool {
        true
    }
}

pub struct Agent {
    provider: Arc<dyn LlmProvider>,
    registry: ToolRegistry,
    system_prompt: String,
    model: String,
    temperature: Option<f32>,
    max_turns: usize,
    auto_approve: bool,
}

impl Agent {
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        registry: ToolRegistry,
        system_prompt: String,
        model: String,
        temperature: Option<f32>,
        max_turns: usize,
        auto_approve: bool,
    ) -> Self {
        Self { provider, registry, system_prompt, model, temperature, max_turns, auto_approve }
    }

    /// 处理一条用户输入，直到模型不再调用工具（或超出轮数上限）。
    /// 期间所有消息（含工具调用与结果）都会追加到 `history`。
    pub async fn run(
        &mut self,
        user_input: &str,
        history: &mut Vec<Message>,
        hooks: &mut dyn AgentHooks,
    ) -> Result<()> {
        history.push(Message::user(user_input));

        for _turn in 1..=self.max_turns {
            // 1. 组装请求：system 提示 + 完整历史 + 工具定义
            let mut messages = Vec::with_capacity(history.len() + 1);
            messages.push(Message::system(&self.system_prompt));
            messages.extend(history.iter().cloned());
            let req = ChatRequest::new(
                self.model.clone(),
                messages,
                Some(self.registry.specs()),
                self.temperature,
            );

            // 2. 流式请求
            let mut stream = self.provider.chat_stream(req).await?;
            let mut final_msg: Option<Message> = None;
            while let Some(ev) = stream.next().await {
                match ev? {
                    StreamEvent::Thinking { content } => hooks.on_thinking(&content),
                    StreamEvent::Delta { content } => hooks.on_text(&content),
                    StreamEvent::Completed(msg) => final_msg = Some(msg),
                }
            }
            let msg = final_msg.ok_or_else(|| AgentError::Other("模型未返回完整响应".into()))?;

            // 3. 无工具调用 → 本轮结束
            let calls = msg.tool_calls.clone().unwrap_or_default();
            history.push(msg);
            if calls.is_empty() {
                return Ok(());
            }

            // 4. 逐个执行工具调用，结果写回历史
            for call in &calls {
                self.exec_call(call, history, hooks).await;
            }
        }

        Err(AgentError::MaxTurnsExceeded(self.max_turns))
    }

    /// 执行单个工具调用（含授权审批），并把结果作为 tool 消息写回历史。
    ///
    /// 注意：任何失败（解析错误、工具报错、用户拒绝）都不会中断循环，
    /// 而是转成对模型可读的错误文本，让模型自行纠正。
    async fn exec_call(
        &self,
        call: &ToolCall,
        history: &mut Vec<Message>,
        hooks: &mut dyn AgentHooks,
    ) {
        let name = call.function.name.clone();
        let args_display = clip(&call.function.arguments, 400);
        hooks.on_tool_start(&name, &args_display);

        let outcome = match serde_json::from_str::<Value>(&call.function.arguments) {
            Err(e) => format!("参数不是合法 JSON: {e}"),
            Ok(args) => {
                let needs_approval = !self.registry.is_readonly(&name) && !self.auto_approve;
                let approved = !needs_approval || hooks.approve(&name, &args_display);
                if !approved {
                    "用户拒绝了这次工具调用。请不要重试，改为向用户说明或换一种方式。".to_string()
                } else {
                    match self.registry.execute(&name, args).await {
                        Ok(out) => out,
                        Err(e) => format!("工具执行错误: {e}"),
                    }
                }
            }
        };

        hooks.on_tool_end(&name, &clip(&outcome, 600));
        history.push(Message::tool(&call.id, &outcome));
    }
}

/// 默认系统提示：告诉模型它的角色、可用工具与工作纪律。
pub fn system_prompt(workdir: &Path) -> String {
    format!(
        r#"You are an expert coding agent operating inside a CLI harness.

Working directory: {cwd}

# Tools
You have these tools: read_file, write_file, edit_file, shell.
- Read files with read_file BEFORE editing them; never guess file contents.
- Use edit_file for surgical changes (old_string must be unique); use write_file only for new files.
- Use shell for builds, tests, git and any verification. Prefer running relevant checks after edits.
- Commands run in the working directory; output includes the exit code.

# Working discipline
- Keep changes minimal and focused on the user's request; no unrelated refactors.
- If a tool returns an error, read it carefully and correct your approach instead of repeating it.
- When the task is done, briefly summarize what changed and how you verified it.

# Language
Always respond in the user's language (Chinese if the user writes Chinese)."#,
        cwd = workdir.display()
    )
}
