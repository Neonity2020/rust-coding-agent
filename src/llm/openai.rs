//! OpenAI 兼容的 Chat Completions 客户端（支持 SSE 流式）。
//!
//! 适用于所有实现了 `/chat/completions` 协议的服务：
//! OpenAI、DeepSeek、Moonshot、Ollama、vLLM、以及自建网关。

use std::collections::VecDeque;
use std::pin::Pin;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;
use futures::{Stream, StreamExt};
use serde::Deserialize;

use super::{ChatRequest, ChatResponse, LlmProvider, Message, StreamEvent, ToolCall};
use crate::error::{AgentError, Result};

pub struct OpenAiProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl OpenAiProvider {
    pub fn new(base_url: &str, api_key: &str) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("构建 HTTP 客户端失败");
        Self { client, base_url: base_url.trim_end_matches('/').to_string(), api_key: api_key.to_string() }
    }

    async fn send(&self, req: ChatRequest) -> Result<reqwest::Response> {
        let url = format!("{}/chat/completions", self.base_url);
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .header("Accept", "text/event-stream")
            .json(&req)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(AgentError::Api(format!("{status} {body}")));
        }
        Ok(resp)
    }
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    async fn chat(&self, mut req: ChatRequest) -> Result<ChatResponse> {
        req.stream = None;
        let resp = self.send(req).await?;
        Ok(resp.json().await?)
    }

    async fn chat_stream(&self, mut req: ChatRequest) -> Result<BoxStream<'static, Result<StreamEvent>>> {
        req.stream = Some(true);
        let resp = self.send(req).await?;
        Ok(sse_stream(resp))
    }
}

// ---------------------------------------------------------------------------
// SSE 解析：字节流 -> StreamEvent 流
// ---------------------------------------------------------------------------

fn sse_stream(resp: reqwest::Response) -> BoxStream<'static, Result<StreamEvent>> {
    let state = SseState {
        resp: Box::pin(resp.bytes_stream()),
        buf: String::new(),
        pending: VecDeque::new(),
        acc: MessageAcc::default(),
        finished: false,
    };
    Box::pin(futures::stream::unfold(state, |mut st| async move {
        loop {
            // 先把已经解析出来的事件吐出去
            if let Some(ev) = st.pending.pop_front() {
                return Some((Ok(ev), st));
            }
            if st.finished {
                return None;
            }
            match st.resp.next().await {
                Some(Ok(bytes)) => {
                    st.buf.push_str(&String::from_utf8_lossy(&bytes));
                    while let Some((pos, len)) = find_sep(&st.buf) {
                        let evt: String = st.buf.drain(..pos + len).collect();
                        st.handle_event(&evt);
                    }
                }
                Some(Err(e)) => {
                    st.finished = true;
                    return Some((Err(e.into()), st));
                }
                None => {
                    // 流结束（有些服务不发 [DONE] 就直接断开）
                    st.finish();
                }
            }
        }
    }))
}

/// 找到一个完整 SSE 事件的分隔符位置（兼容 LF 与 CRLF）。
fn find_sep(buf: &str) -> Option<(usize, usize)> {
    let a = buf.find("\n\n").map(|p| (p, 2));
    let b = buf.find("\r\n\r\n").map(|p| (p, 4));
    match (a, b) {
        (Some(x), Some(y)) => Some(if x.0 <= y.0 { x } else { y }),
        (Some(x), None) => Some(x),
        (None, Some(y)) => Some(y),
        _ => None,
    }
}

/// 聚合流式增量，最终还原成完整消息。
#[derive(Default)]
struct MessageAcc {
    content: String,
    reasoning: String,
    tool_calls: Vec<ToolCall>,
    completed: bool,
}

impl MessageAcc {
    fn into_message(&self) -> Message {
        Message {
            role: super::Role::Assistant,
            content: if self.content.is_empty() { None } else { Some(self.content.clone()) },
            tool_calls: if self.tool_calls.is_empty() { None } else { Some(self.tool_calls.clone()) },
            reasoning_content: if self.reasoning.is_empty() { None } else { Some(self.reasoning.clone()) },
            tool_call_id: None,
        }
    }

    fn push_tool_delta(&mut self, d: &ToolCallDelta) {
        while self.tool_calls.len() <= d.index {
            self.tool_calls.push(ToolCall {
                id: String::new(),
                kind: "function".into(),
                function: super::FunctionCall { name: String::new(), arguments: String::new() },
            });
        }
        let tc = &mut self.tool_calls[d.index];
        if let Some(id) = d.id.as_deref() {
            if !id.is_empty() {
                tc.id = id.to_string();
            }
        }
        if let Some(f) = &d.function {
            if let Some(n) = &f.name {
                tc.function.name.push_str(n);
            }
            if let Some(a) = &f.arguments {
                tc.function.arguments.push_str(a);
            }
        }
    }
}

struct SseState {
    resp: Pin<Box<dyn Stream<Item = std::result::Result<Bytes, reqwest::Error>> + Send>>,
    buf: String,
    pending: VecDeque<StreamEvent>,
    acc: MessageAcc,
    finished: bool,
}

impl SseState {
    fn handle_event(&mut self, evt: &str) {
        // 一个 SSE 事件可能有多行，只有 `data:` 行是我们的数据
        let mut data = String::new();
        for line in evt.lines() {
            if let Some(rest) = line.strip_prefix("data:") {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(rest.trim_start());
            }
        }
        if data.is_empty() {
            return; // 仅含 event:/id: 行，忽略
        }
        self.handle_data(&data);
    }

    fn handle_data(&mut self, data: &str) {
        if data.trim() == "[DONE]" {
            self.finish();
            return;
        }
        let chunk: Chunk = match serde_json::from_str(data) {
            Ok(c) => c,
            Err(e) => {
                tracing::debug!("跳过无法解析的 SSE chunk: {e}");
                return;
            }
        };
        for choice in chunk.choices {
            let delta = choice.delta;
            // 思考内容：转发事件供 UI 可选展示，并聚合进最终消息（MiMo 多轮回传必需）
            if let Some(text) = delta.reasoning_content {
                if !text.is_empty() {
                    self.acc.reasoning.push_str(&text);
                    self.pending.push_back(StreamEvent::Thinking { content: text });
                }
            }
            if let Some(text) = delta.content {
                if !text.is_empty() {
                    self.pending.push_back(StreamEvent::Delta { content: text });
                }
            }
            if let Some(tds) = delta.tool_calls {
                for td in tds {
                    self.acc.push_tool_delta(&td);
                }
            }
        }
    }

    fn finish(&mut self) {
        if !self.acc.completed {
            self.acc.completed = true;
            self.pending.push_back(StreamEvent::Completed(self.acc.into_message()));
        }
        self.finished = true;
    }
}

// ---------------------------------------------------------------------------
// 流式 chunk 的数据结构（带 default 容错，不同服务字段略有差异）
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct Chunk {
    #[serde(default)]
    choices: Vec<ChunkChoice>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct ChunkChoice {
    delta: Delta,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Delta {
    content: Option<String>,
    reasoning_content: Option<String>,
    tool_calls: Option<Vec<ToolCallDelta>>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct ToolCallDelta {
    index: usize,
    id: Option<String>,
    function: Option<FunctionDelta>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct FunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}
