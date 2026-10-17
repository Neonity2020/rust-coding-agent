//! LLM 抽象层：消息类型、请求/响应结构、Provider trait。
//!
//! 任何 OpenAI 兼容的 API（OpenAI、DeepSeek、Ollama、vLLM、以及你自己的
//! free-router 等网关）都可以通过实现 [`LlmProvider`] 接入。

pub mod openai;

use async_trait::async_trait;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};

use crate::error::Result;

// ---------------------------------------------------------------------------
// 消息类型
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// 对话消息。与 OpenAI Chat Completions 的消息格式一一对应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// assistant 消息携带的工具调用请求（仅 assistant）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// 推理内容（MiMo 等推理模型返回）。Agent 多轮工具调用场景必须原样回传，
    /// 否则 MiMo API 会返回 400。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    /// 工具结果消息对应的 tool_call id（仅 tool）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    pub fn system(text: &str) -> Self {
        Self { role: Role::System, content: Some(text.into()), tool_calls: None, reasoning_content: None, tool_call_id: None }
    }
    pub fn user(text: &str) -> Self {
        Self { role: Role::User, content: Some(text.into()), tool_calls: None, reasoning_content: None, tool_call_id: None }
    }
    pub fn tool(tool_call_id: &str, content: &str) -> Self {
        Self {
            role: Role::Tool,
            content: Some(content.into()),
            tool_calls: None,
            reasoning_content: None,
            tool_call_id: Some(tool_call_id.into()),
        }
    }
}

/// 模型发起的一次工具调用。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", default = "default_tool_type")]
    pub kind: String,
    pub function: FunctionCall,
}

fn default_tool_type() -> String {
    "function".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    /// JSON 字符串形式的参数（模型生成的原始文本，执行前需要解析）。
    pub arguments: String,
}

/// 工具定义（以 JSON Schema 描述参数）。
#[derive(Debug, Clone, Serialize)]
pub struct ToolSpec {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: FunctionSpec,
}

#[derive(Debug, Clone, Serialize)]
pub struct FunctionSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

impl ToolSpec {
    pub fn function(name: &str, description: &str, parameters: serde_json::Value) -> Self {
        Self {
            kind: "function".into(),
            function: FunctionSpec { name: name.into(), description: description.into(), parameters },
        }
    }
}

// ---------------------------------------------------------------------------
// 请求 / 响应
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolSpec>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
}

impl ChatRequest {
    pub fn new(model: String, messages: Vec<Message>, tools: Option<Vec<ToolSpec>>, temperature: Option<f32>) -> Self {
        Self { model, messages, tools, temperature, stream: None }
    }
}

/// 一次性响应。当前 Agent 循环使用流式接口，此结构保留给非流式场景。
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct ChatResponse {
    pub choices: Vec<Choice>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct Choice {
    pub message: Message,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

// ---------------------------------------------------------------------------
// 流式事件
// ---------------------------------------------------------------------------

/// 流式响应产生的事件。
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// 一段增量思考内容（推理模型的思维链，默认不在终端打印）。
    Thinking { content: String },
    /// 一段增量文本（assistant 正在"打字"）。
    Delta { content: String },
    /// 流结束，附带聚合完成的完整 assistant 消息（含工具调用）。
    Completed(Message),
}

// ---------------------------------------------------------------------------
// Provider trait
// ---------------------------------------------------------------------------

/// LLM 提供方抽象。实现它即可接入任何后端。
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// 一次性请求（非流式）。当前主循环走流式，此方法供非流式场景使用。
    #[allow(dead_code)]
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse>;

    /// 流式请求。返回的流只产生 [`StreamEvent`]，Provider 内部负责把
    /// tool_calls 增量聚合成完整消息，Agent 层无需关心分片细节。
    async fn chat_stream(&self, req: ChatRequest) -> Result<BoxStream<'static, Result<StreamEvent>>>;
}
