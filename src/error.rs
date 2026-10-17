use thiserror::Error;

/// 框架统一错误类型。
#[derive(Error, Debug)]
pub enum AgentError {
    #[error("HTTP 请求失败: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON 序列化失败: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),

    #[error("配置错误: {0}")]
    Config(String),

    #[error("工具执行失败 [{tool}]: {message}")]
    Tool { tool: String, message: String },

    #[error("LLM API 返回错误: {0}")]
    Api(String),

    #[error("达到最大 Agent 循环轮数 ({0})，已中止")]
    MaxTurnsExceeded(usize),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, AgentError>;
