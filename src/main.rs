//! CLI 入口：单次执行或交互式 REPL。

mod agent;
mod config;
mod error;
mod llm;
mod session;
mod tools;
mod util;

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;

use agent::{system_prompt, Agent, AgentHooks};
use config::Settings;
use error::Result;
use llm::openai::OpenAiProvider;
use llm::LlmProvider;
use tools::fs::{EditFileTool, ReadFileTool, WriteFileTool};
use tools::shell::ShellTool;
use tools::ToolRegistry;
use util::clip;

#[derive(Parser, Debug)]
#[command(name = "coding-agent", version, about = "用 Rust 构建的 Coding Agent 框架")]
struct Cli {
    /// 模型名（覆盖环境变量 OPENAI_MODEL）
    #[arg(long)]
    model: Option<String>,

    /// OpenAI 兼容 API 地址（覆盖环境变量 OPENAI_BASE_URL）
    #[arg(long)]
    base_url: Option<String>,

    /// 最大 Agent 循环轮数（默认 30）
    #[arg(long)]
    max_turns: Option<usize>,

    /// 跳过工具授权确认（危险：请只在可信目录使用）
    #[arg(long)]
    auto_approve: bool,

    /// 从会话文件恢复历史，例如 sessions/1700000000.jsonl
    #[arg(long)]
    resume: Option<PathBuf>,

    /// 单次执行的 prompt；省略则进入交互模式
    prompt: Option<String>,
}

/// CLI 的钩子实现：终端打印 + y/N 授权确认。
struct CliHooks {
    auto_approve: bool,
    wrote_text: bool,
}

impl CliHooks {
    fn new(auto_approve: bool) -> Self {
        Self { auto_approve, wrote_text: false }
    }

    /// 确保当前输出行结束。
    fn finish_line(&mut self) {
        if self.wrote_text {
            println!();
            self.wrote_text = false;
        }
    }
}

impl AgentHooks for CliHooks {
    fn on_text(&mut self, delta: &str) {
        print!("{delta}");
        let _ = std::io::stdout().flush();
        self.wrote_text = true;
    }

    fn on_tool_start(&mut self, name: &str, args_display: &str) {
        self.finish_line();
        println!("⚙ {name} {}", clip(args_display, 300));
    }

    fn on_tool_end(&mut self, _name: &str, output: &str) {
        for line in output.lines().take(12) {
            println!("  │ {line}");
        }
    }

    fn approve(&mut self, name: &str, args_display: &str) -> bool {
        if self.auto_approve {
            return true;
        }
        println!("⚠ 需要授权: {name} {}", clip(args_display, 300));
        print!("   允许执行? [y/N] ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
    }
}

fn build_agent(settings: &Settings) -> Agent {
    let provider: Arc<dyn LlmProvider> =
        Arc::new(OpenAiProvider::new(&settings.base_url, &settings.api_key));
    let registry = ToolRegistry::new(vec![
        Arc::new(ReadFileTool::new(settings.workdir.clone())),
        Arc::new(WriteFileTool::new(settings.workdir.clone())),
        Arc::new(EditFileTool::new(settings.workdir.clone())),
        Arc::new(ShellTool::new(settings.workdir.clone(), 120)),
    ]);
    Agent::new(
        provider,
        registry,
        system_prompt(&settings.workdir),
        settings.model.clone(),
        settings.temperature,
        settings.max_turns,
        settings.auto_approve,
    )
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_target(false)
        .init();

    if let Err(e) = run().await {
        eprintln!("错误: {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    let settings = Settings::load(cli.model, cli.base_url, cli.max_turns, cli.auto_approve)?;
    let mut agent = build_agent(&settings);
    let mut hooks = CliHooks::new(settings.auto_approve);

    // 会话：--resume 恢复，否则新建
    let (session_path, mut history) = match cli.resume {
        Some(p) => {
            let h = session::load(&p)?;
            println!("已恢复会话 {}（{} 条消息）", p.display(), h.len());
            (p, h)
        }
        None => (session::default_session_path(), Vec::new()),
    };

    println!("endpoint  : {}", settings.provider_label);
    println!("model     : {}", settings.model);
    println!("base_url : {}", settings.base_url);
    println!("workdir  : {}", settings.workdir.display());
    println!("session  : {}", session_path.display());

    match cli.prompt {
        // 单次执行模式
        Some(prompt) => {
            agent.run(&prompt, &mut history, &mut hooks).await?;
            hooks.finish_line();
            session::save(&session_path, &history)?;
        }
        // 交互模式
        None => {
            println!("\n输入问题开始，/clear 清空上下文，/exit 退出，Ctrl-D 结束。\n");
            loop {
                print!("> ");
                let _ = std::io::stdout().flush();
                let mut line = String::new();
                if std::io::stdin().read_line(&mut line)? == 0 {
                    println!("再见！");
                    break;
                }
                let input = line.trim();
                if input.is_empty() {
                    continue;
                }
                match input {
                    "/exit" | "/quit" => break,
                    "/clear" => {
                        history.clear();
                        println!("上下文已清空。");
                        continue;
                    }
                    "/save" => {
                        session::save(&session_path, &history)?;
                        println!("会话已保存到 {}", session_path.display());
                        continue;
                    }
                    _ => {}
                }
                if let Err(e) = agent.run(input, &mut history, &mut hooks).await {
                    eprintln!("错误: {e}");
                }
                hooks.finish_line();
                // 每轮自动保存，崩溃也不丢历史
                let _ = session::save(&session_path, &history);
            }
        }
    }
    Ok(())
}
