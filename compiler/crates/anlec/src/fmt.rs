//nt: 装饰性缩进
//nt: 块体一级
//nt: 冒号加一级
//nt: 分号减一级
//nt: asm 原样

use std::fs;
use std::path::{Path, PathBuf};

pub const 步长: usize = 4;

///nt: 块行解析
fn 块行(t: &str) -> Option<(bool, bool)> {
    if !t.starts_with("#:") {
        return None;
    }
    let mut 词 = t[2..].split_whitespace();
    词.next()?;
    let 开关 = 词.next()?;
    let 开 = match 开关 {
        "b" => true,
        "d" => false,
        _ => return None,
    };
    Some((开, 词.any(|x| x == "asm")))
}

fn 行首(级: usize, t: &str) -> String {
    let mut s = String::with_capacity(级 * 步长 + t.len());
    for _ in 0..级 {
        s.push_str("    ");
    }
    s.push_str(t);
    s
}

///nt: 全文格式化
pub fn format_source(src: &str) -> String {
    let mut 行: Vec<&str> = src.split('\n').collect();
    if 行.last() == Some(&"") {
        行.pop();
    }
    let mut 出: Vec<String> = Vec::with_capacity(行.len());
    let mut 级 = 0usize;
    let mut asm = false;
    for 原 in 行 {
        let 原 = 原.trim_end_matches('\r');
        let t = 原.trim_start();
        if asm {
            if let Some((false, _)) = 块行(t) {
                asm = false;
                级 = 0;
                出.push(t.to_string());
            } else {
                出.push(原.to_string());
            }
            continue;
        }
        if t.is_empty() {
            出.push(String::new());
            continue;
        }
        if let Some((开, 是asm)) = 块行(t) {
            if 开 {
                级 = 1;
                出.push(t.to_string());
                asm = 是asm;
            } else {
                级 = 0;
                出.push(t.to_string());
            }
            continue;
        }
        if t == ";" {
            级 = 级.saturating_sub(1);
            出.push(行首(级, t));
            continue;
        }
        if t.starts_with("else:") {
            出.push(行首(级, t));
            级 += 1;
            continue;
        }
        let 注释 = t.starts_with("//");
        let 开体 = t.ends_with(':') && !注释;
        出.push(行首(级, t));
        if 开体 {
            级 += 1;
        }
    }
    let mut s = 出.join("\n");
    s.push('\n');
    s
}

fn 收文件(p: &Path, 出: &mut Vec<PathBuf>) {
    if p.is_file() {
        if p.extension().map(|x| x == "an").unwrap_or(false) {
            出.push(p.to_path_buf());
        }
        return;
    }
    let 跳过 = ["build", "target", ".git", "node_modules", "项目回收站"];
    let rd = match fs::read_dir(p) {
        Ok(r) => r,
        Err(_) => return,
    };
    let mut 项: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    项.sort();
    for e in 项 {
        if e.is_dir() {
            let 名 = e.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            if 跳过.contains(&名.as_str()) {
                continue;
            }
            收文件(&e, 出);
        } else if e.extension().map(|x| x == "an").unwrap_or(false) {
            出.push(e);
        }
    }
}

///nt: 格式化文件或目录
pub fn 执行(p: &Path, 只查: bool) -> Result<(usize, Vec<PathBuf>), String> {
    let mut 文件 = Vec::new();
    收文件(p, &mut 文件);
    let mut 改 = Vec::new();
    for f in &文件 {
        let 源 = fs::read_to_string(f).map_err(|e| format!("读不到 {}：{}", f.display(), e))?;
        let 新 = format_source(&源);
        if 新 != 源 {
            改.push(f.clone());
            if !只查 {
                fs::write(f, 新).map_err(|e| format!("写不了 {}：{}", f.display(), e))?;
            }
        }
    }
    Ok((文件.len(), 改))
}
