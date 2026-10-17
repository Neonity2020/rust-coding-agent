//! 工具层：[`Tool`] trait 与 [`ToolRegistry`]。
//!
//! 新增一个工具只需要：定义结构体 → 实现 [`Tool`] → 注册进 [`ToolRegistry`]。

pub mod fs;
pub mod shell;

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::{AgentError, Result};
use crate::llm::ToolSpec;

/// Agent 可调用的工具。
#[async_trait]
pub trait Tool: Send + Sync {
    /// 工具名（模型用它来发起调用，需全局唯一）。
    fn name(&self) -> &'static str;

    /// 工具描述（模型据此决定何时使用）。
    fn description(&self) -> &'static str;

    /// 参数的 JSON Schema。
    fn parameters(&self) -> Value;

    /// 是否只读。只读工具跳过用户授权（如读文件），写操作/执行命令需要确认。
    fn is_readonly(&self) -> bool {
        false
    }

    /// 执行工具。参数是模型生成、已解析的 JSON。
    async fn execute(&self, args: Value) -> Result<String>;
}

/// 构造一个对模型可读的工具错误。
pub fn tool_err(tool: &str, msg: impl std::fmt::Display) -> AgentError {
    AgentError::Tool { tool: tool.to_string(), message: msg.to_string() }
}

/// 工具注册表：负责向模型暴露工具列表，以及按名字分发调用。
pub struct ToolRegistry {
    tools: Vec<Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new(tools: Vec<Arc<dyn Tool>>) -> Self {
        Self { tools }
    }

    /// 供请求使用的工具定义列表。
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools
            .iter()
            .map(|t| ToolSpec::function(t.name(), t.description(), t.parameters()))
            .collect()
    }

    pub fn is_readonly(&self, name: &str) -> bool {
        self.tools.iter().find(|t| t.name() == name).map(|t| t.is_readonly()).unwrap_or(false)
    }

    /// 按名字执行工具。
    pub async fn execute(&self, name: &str, args: Value) -> Result<String> {
        match self.tools.iter().find(|t| t.name() == name) {
            Some(tool) => tool.execute(args).await,
            None => {
                let available: Vec<&str> = self.tools.iter().map(|t| t.name()).collect();
                Err(AgentError::Other(format!(
                    "未知工具 '{name}'，可用工具: {}",
                    available.join(", ")
                )))
            }
        }
    }
}
