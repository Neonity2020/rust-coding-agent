//! `shell` 工具：在工作目录执行 shell 命令（构建、测试、git 等）。

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{tool_err, Tool};
use crate::error::Result;
use crate::util::clip;

const NAME: &str = "shell";
const MAX_OUTPUT: usize = 20_000;

pub struct ShellTool {
    workdir: PathBuf,
    timeout: Duration,
}

impl ShellTool {
    pub fn new(workdir: PathBuf, timeout_secs: u64) -> Self {
        Self { workdir, timeout: Duration::from_secs(timeout_secs) }
    }
}

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &'static str {
        NAME
    }

    fn description(&self) -> &'static str {
        "在工作目录中执行 shell 命令并返回退出码、stdout、stderr。\
         构建、运行测试、git 操作等都应使用此工具，而不是猜测结果。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "要执行的 shell 命令，例如 `cargo test`"
                }
            },
            "required": ["command"]
        })
    }

    fn is_readonly(&self) -> bool {
        false
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let cmd = args
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| tool_err(NAME, "缺少 `command` 参数"))?;

        let fut = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(&self.workdir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output();

        let out = tokio::time::timeout(self.timeout, fut)
            .await
            .map_err(|_| tool_err(NAME, format!("命令超时（{}s）被终止", self.timeout.as_secs())))??;

        let mut result = format!("exit code: {}\n", out.status.code().unwrap_or(-1));
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        if !stdout.is_empty() {
            result.push_str("stdout:\n");
            result.push_str(&stdout);
        }
        if !stderr.is_empty() {
            result.push_str("\nstderr:\n");
            result.push_str(&stderr);
        }
        if stdout.is_empty() && stderr.is_empty() {
            result.push_str("(无输出)");
        }
        Ok(clip(&result, MAX_OUTPUT))
    }
}
