use std::path::{Path, PathBuf};

/// 截断字符串到最多 `max` 个字符，并标注原始长度。
pub fn clip(s: &str, max: usize) -> String {
    let total = s.chars().count();
    if total <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push_str(&format!("\n...[已截断，共 {total} 字符]"));
    out
}

/// 把用户/模型给出的路径解析为绝对路径（相对路径基于工作目录）。
pub fn resolve_path(workdir: &Path, raw: &str) -> PathBuf {
    let p = Path::new(raw);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        workdir.join(p)
    }
}

/// 展示用路径：优先显示相对工作目录的路径。
pub fn display_path(workdir: &Path, p: &Path) -> String {
    p.strip_prefix(workdir)
        .map(|r| r.display().to_string())
        .unwrap_or_else(|_| p.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_short_unchanged() {
        assert_eq!(clip("hello", 10), "hello");
    }

    #[test]
    fn clip_long_truncated() {
        let s = "a".repeat(100);
        let out = clip(&s, 10);
        assert!(out.starts_with(&"a".repeat(10)));
        assert!(out.contains("已截断"));
    }

    #[test]
    fn resolve_relative() {
        let wd = Path::new("/tmp/proj");
        assert_eq!(resolve_path(wd, "src/main.rs"), PathBuf::from("/tmp/proj/src/main.rs"));
        assert_eq!(resolve_path(wd, "/etc/hosts"), PathBuf::from("/etc/hosts"));
    }
}
