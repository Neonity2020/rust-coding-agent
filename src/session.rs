//! 会话持久化：每条消息一行 JSON（JSONL），可随时恢复。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::Result;
use crate::llm::Message;

/// 生成一个默认会话文件路径：`sessions/<unix 时间戳>.jsonl`。
pub fn default_session_path() -> PathBuf {
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    PathBuf::from("sessions").join(format!("{ts}.jsonl"))
}

/// 保存完整历史（整文件重写，保持简单可靠）。
pub fn save(path: &Path, history: &[Message]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut buf = String::with_capacity(history.len() * 128);
    for m in history {
        buf.push_str(&serde_json::to_string(m)?);
        buf.push('\n');
    }
    std::fs::write(path, buf)?;
    Ok(())
}

/// 从 JSONL 恢复历史。
pub fn load(path: &Path) -> Result<Vec<Message>> {
    let text = std::fs::read_to_string(path)?;
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| Ok(serde_json::from_str(l)?))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::Role;

    #[test]
    fn roundtrip() {
        let dir = std::env::temp_dir().join(format!("rca-sess-{}", std::process::id()));
        let path = dir.join("t.jsonl");
        // 模拟推理模型的 assistant 消息：带 reasoning_content（MiMo 多轮回传必需）
        let assistant = Message {
            role: Role::Assistant,
            content: Some("答案".into()),
            tool_calls: None,
            reasoning_content: Some("思考过程...".into()),
            tool_call_id: None,
        };
        let history = vec![Message::user("hi"), assistant];
        save(&path, &history).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].content.as_deref(), Some("hi"));
        assert_eq!(loaded[1].reasoning_content.as_deref(), Some("思考过程..."));
        assert_eq!(loaded[1].content.as_deref(), Some("答案"));
        std::fs::remove_file(&path).unwrap();
    }
}
