//! 文件工具：读、写、精确编辑。

use std::fs;
use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{tool_err, Tool};
use crate::error::Result;
use crate::util::{display_path, resolve_path};

const MAX_READ: usize = 400_000; // 单文件读取上限（字符）

fn get_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key).and_then(|v| v.as_str()).ok_or_else(|| tool_err("fs", format!("缺少 `{key}` 参数")))
}

// ---------------------------------------------------------------------------
// read_file
// ---------------------------------------------------------------------------

pub struct ReadFileTool {
    workdir: PathBuf,
}

impl ReadFileTool {
    pub fn new(workdir: PathBuf) -> Self {
        Self { workdir }
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn description(&self) -> &'static str {
        "读取文件内容。输出带行号（行号\\t内容），编辑文件前必须先读取。\
         大文件可通过 offset/limit 分页读取。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "文件路径（相对工作目录或绝对路径）" },
                "offset": { "type": "integer", "description": "起始行号（1 开始，默认 1）" },
                "limit": { "type": "integer", "description": "最多返回的行数（默认 2000）" }
            },
            "required": ["path"]
        })
    }

    fn is_readonly(&self) -> bool {
        true
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let raw = get_str(&args, "path")?;
        let path = resolve_path(&self.workdir, raw);
        let content =
            fs::read_to_string(&path).map_err(|e| tool_err("read_file", format!("{raw}: {e}")))?;
        if content.chars().count() > MAX_READ {
            return Err(tool_err("read_file", format!("文件过大（> {MAX_READ} 字符），请用 offset/limit 分页")));
        }
        if content.is_empty() {
            return Ok("(空文件)".to_string());
        }

        let lines: Vec<&str> = content.lines().collect();
        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(1).max(1) as usize;
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(2000) as usize;

        let start = (offset - 1).min(lines.len());
        let end = (start + limit).min(lines.len());

        let mut out = String::new();
        for (i, line) in lines[start..end].iter().enumerate() {
            out.push_str(&format!("{:>6}\t{}\n", start + i + 1, line));
        }
        if end < lines.len() {
            out.push_str(&format!("...[共 {} 行，已显示 {}-{}]\n", lines.len(), start + 1, end));
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// write_file
// ---------------------------------------------------------------------------

pub struct WriteFileTool {
    workdir: PathBuf,
}

impl WriteFileTool {
    pub fn new(workdir: PathBuf) -> Self {
        Self { workdir }
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn description(&self) -> &'static str {
        "创建或整体覆写一个文件。适合新建文件；修改已有文件优先用 edit_file。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "文件路径" },
                "content": { "type": "string", "description": "文件完整内容" }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let raw = get_str(&args, "path")?;
        let content = get_str(&args, "content")?;
        let path = resolve_path(&self.workdir, raw);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| tool_err("write_file", format!("{raw}: {e}")))?;
        }
        fs::write(&path, content).map_err(|e| tool_err("write_file", format!("{raw}: {e}")))?;
        Ok(format!("已写入 {}（{} 字节）", display_path(&self.workdir, &path), content.len()))
    }
}

// ---------------------------------------------------------------------------
// edit_file
// ---------------------------------------------------------------------------

pub struct EditFileTool {
    workdir: PathBuf,
}

impl EditFileTool {
    pub fn new(workdir: PathBuf) -> Self {
        Self { workdir }
    }
}

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &'static str {
        "edit_file"
    }

    fn description(&self) -> &'static str {
        "对已有文件做精确字符串替换。old_string 必须在文件中唯一出现（除非 replace_all=true），\
         并保留周围缩进以保证唯一性；修改前必须先用 read_file 读到准确内容。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "文件路径" },
                "old_string": { "type": "string", "description": "要被替换的原文（需唯一匹配）" },
                "new_string": { "type": "string", "description": "替换后的新文本" },
                "replace_all": { "type": "boolean", "description": "替换所有匹配（默认 false）" }
            },
            "required": ["path", "old_string", "new_string"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let raw = get_str(&args, "path")?;
        let old = get_str(&args, "old_string")?;
        let new = get_str(&args, "new_string")?;
        let replace_all = args.get("replace_all").and_then(|v| v.as_bool()).unwrap_or(false);
        if old.is_empty() {
            return Err(tool_err("edit_file", "old_string 不能为空"));
        }

        let path = resolve_path(&self.workdir, raw);
        let content =
            fs::read_to_string(&path).map_err(|e| tool_err("edit_file", format!("{raw}: {e}")))?;

        let hits = content.matches(old).count();
        match hits {
            0 => Err(tool_err("edit_file", format!(
                "old_string 在 {raw} 中未找到。请重新 read_file 获取准确内容后再编辑。"
            ))),
            1 => {
                let updated = content.replacen(old, new, 1);
                fs::write(&path, &updated)
                    .map_err(|e| tool_err("edit_file", format!("{raw}: {e}")))?;
                Ok("已替换 1 处".to_string())
            }
            n if !replace_all => Err(tool_err("edit_file", format!(
                "old_string 在 {raw} 中出现 {n} 次，不唯一。请提供更多上下文使其唯一，或设置 replace_all=true。"
            ))),
            n => {
                let updated = content.replace(old, new);
                fs::write(&path, &updated)
                    .map_err(|e| tool_err("edit_file", format!("{raw}: {e}")))?;
                Ok(format!("已替换 {n} 处"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tmp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rca-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn edit_requires_unique_match() {
        let dir = tmp_dir();
        let file = dir.join("a.txt");
        fs::write(&file, "foo bar foo").unwrap();

        let tool = EditFileTool::new(dir.clone());
        // 两处匹配 → 报错
        let err = tool.execute(json!({"path": "a.txt", "old_string": "foo", "new_string": "baz"})).await;
        assert!(err.is_err());
        // replace_all → 成功
        let ok = tool
            .execute(json!({"path": "a.txt", "old_string": "foo", "new_string": "baz", "replace_all": true}))
            .await
            .unwrap();
        assert!(ok.contains("2 处"));
        assert_eq!(fs::read_to_string(&file).unwrap(), "baz bar baz");
    }

    #[tokio::test]
    async fn read_shows_line_numbers() {
        let dir = tmp_dir();
        fs::write(dir.join("b.txt"), "line1\nline2").unwrap();
        let tool = ReadFileTool::new(dir);
        let out = tool.execute(json!({"path": "b.txt"})).await.unwrap();
        assert!(out.contains("     1\tline1"));
        assert!(out.contains("     2\tline2"));
    }
}
