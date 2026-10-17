//! 配置：环境变量 + CLI 参数（优先级：CLI > OPENAI_* > MIMO_* > 默认值）。
//!
//! 支持两种预设：
//! - **OpenAI 兼容默认**：`OPENAI_API_KEY`（或都不设时用占位 key，适合本地端点）
//! - **MiMo 国内订阅（Token Plan）**：设置 `MIMO_API_KEY`（格式 `tp-xxxxx` / `ttp-xxxxx`）
//!   即自动切换到国内集群 `https://token-plan-cn.xiaomimimo.com/v1` 和 MiMo 模型。

use std::path::PathBuf;

use crate::error::{AgentError, Result};

pub struct Settings {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub temperature: Option<f32>,
    pub max_turns: usize,
    pub auto_approve: bool,
    pub workdir: PathBuf,
    /// 用于 banner 展示的端点描述。
    pub provider_label: String,
}

impl Settings {
    pub fn load(
        model: Option<String>,
        base_url: Option<String>,
        max_turns: Option<usize>,
        auto_approve: bool,
    ) -> Result<Self> {
        let openai_key = std::env::var("OPENAI_API_KEY").ok().filter(|s| !s.is_empty());
        let mimo_key = std::env::var("MIMO_API_KEY").ok().filter(|s| !s.is_empty());

        // 端点预设：OPENAI_API_KEY 优先，其次 MIMO_API_KEY（国内订阅），最后占位 key
        let (api_key, label, default_base, default_model) = match (openai_key, mimo_key) {
            (Some(k), _) => (k, "OpenAI 兼容端点".to_string(),
                "https://api.openai.com/v1".to_string(), "gpt-4o".to_string()),
            (None, Some(k)) => (k, "MiMo Token Plan（国内订阅）".to_string(),
                "https://token-plan-cn.xiaomimimo.com/v1".to_string(), "mimo-v2.6-pro".to_string()),
            (None, None) => {
                eprintln!("提示: 未设置 OPENAI_API_KEY / MIMO_API_KEY，使用 \"EMPTY\"（适用于本地/免鉴权端点）");
                ("EMPTY".to_string(), "未配置端点".to_string(),
                "https://api.openai.com/v1".to_string(), "gpt-4o".to_string())
            }
        };

        let base_url = base_url
            .or_else(|| std::env::var("OPENAI_BASE_URL").ok().filter(|s| !s.is_empty()))
            .or_else(|| std::env::var("MIMO_BASE_URL").ok().filter(|s| !s.is_empty()))
            .unwrap_or(default_base);

        let model = model
            .or_else(|| std::env::var("OPENAI_MODEL").ok().filter(|s| !s.is_empty()))
            .or_else(|| std::env::var("MIMO_MODEL").ok().filter(|s| !s.is_empty()))
            .unwrap_or(default_model);

        let temperature = std::env::var("OPENAI_TEMPERATURE")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .or(Some(0.0));

        let max_turns = max_turns.unwrap_or(30);

        let workdir = std::env::current_dir().map_err(|e| AgentError::Config(format!("无法确定工作目录: {e}")))?;

        Ok(Self { api_key, base_url, model, temperature, max_turns, auto_approve, workdir, provider_label: label })
    }
}
