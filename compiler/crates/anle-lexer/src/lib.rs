//! anle-lexer —— 词法分析器
//!
//! 处理层级语法「分号收束」的词法形态：关键字表 / `nt` 注释 / 符号与字面量。
//!
//! - 行结构：每行末产 `Newline`（`;` 独占行、`nt` 第 0 列都靠它判）
//! - `;` = 收束符号（`Sym(";")`）
//! - `;` 后接数字 ⇒ E001
//! - `nt` 注释 = 第 0 列裸 `nt` + 空格（前有任何字符，含缩进，⇒ E002）
//! - 缩进纯装饰（跳过行首空格 / 制表）
//! - 字面量：int（`0x` / `0b` / `0o` + 下划线）/ float / str（转义 `\n` `\t` `\\` `\"`）/ `@"…"` 原样串
//! - 源文件：UTF-8 无 BOM、LF

use anle_error::{render_lex, DiagParts, LexCode, Tone};

pub mod asm;

//nt: 词法错误
#[derive(Debug, Clone, PartialEq)]
pub struct LexError {
    pub line: usize,
    pub message: String,
    pub code: String,
    //nt: 结构化原文
    pub parts: DiagParts,
}

impl LexError {
    pub fn with_code(line: usize, code: LexCode, parts: &DiagParts, tone: Tone) -> Self {
        Self {
            line,
            message: render_lex(code, parts, tone),
            code: code.number().to_string(),
            parts: parts.clone(),
        }
    }
}

//nt: 原子token
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    HashColon,         // #:   块标记
    HashHashColon,     // ##:  模块引用
    HashHashHashColon, // ###: 包前缀
    Kw(&'static str),  // 关键字
    Ident(String),     // 裸名（可中文）
    Str(String),       // "…"（已解转义）
    RawStr(String),    // @"…"（原样，不解转义）
    Int(i64),
    Float(f64),
    Sym(&'static str), // 运算符 / 标点
    RawAsm(String),    // asm 块体的原始行（整行不解析，原文保留）
    Newline,           // 行界
}

//nt: 带行号的token
#[derive(Debug, Clone, PartialEq)]
pub struct Spanned {
    pub tok: Token,
    pub line: usize,
    //nt: 一基列号
    pub col: usize,
    //nt: 跨度
    pub len: usize,
}

impl Spanned {
    pub fn span(&self) -> anle_error::Span {
        anle_error::Span { line: self.line, col: self.col, len: self.len }
    }
}

//nt: 关键字表
//nt: 关键字表
//nt: nt 不入表
const KEYWORDS: &[&str] = &[
    //nt: 动作与结构
    "add", "call", "fn", "val", "type", "hide", "give",
    //nt: 跨块取值动词
    "take",
    //nt: 外部函数声明
    "outs",
    //nt: 控制流
    "if", "else", "for", "while", "in", "max",
    //nt: 和类型判别
    "match", "case",
    //nt: 块行属性后缀
    "unsafe", "pub", "sle", "ABI", "kill", "asm",
    //nt: 常量（编译期求值）
    "fix",
    //nt: 值类型（定长族）
    "int", "float", "bool", "str", "int_8", "int_16", "int_32", "int_64", "uint_8", "uint_16",
    "uint_32", "uint_64",
    //nt: 集合
    "list", "map", "que", "stack",
    //nt: 逻辑与空值
    "and", "or", "not", "null",
    //nt: 真假值
    "re", "fa",
];

//nt: 双字符符号
const SYMBOLS2: &[&str] = &["==", "!=", "<=", ">=", "<<", ">>"];
//nt: 单字符符号
const SYMBOLS1: &[&str] = &[
    "=", ":", ";", "~", ",", ".", "(", ")", "[", "]", "{", "}", "+", "-", "*", "/", "%", "<", ">",
    "?", "&", "|", "^", "!",
];

fn is_ident_start(c: char) -> bool {
    c == '_' || c.is_alphabetic()
}
fn is_ident_cont(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

//nt: nt 注释行判定
fn is_comment_line(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

//nt: asm 开块行
fn asm_open_block(line: &str) -> Option<String> {
    let t = line.trim_start();
    if !t.starts_with("#:") {
        return None;
    }
    let w: Vec<&str> = t[2..].split_whitespace().collect();
    if w.len() < 3 || w[1] != "b" || !w[2..].iter().any(|x| *x == "asm") {
        return None;
    }
    Some(w[0].to_string())
}

//nt: 关块行
fn is_asm_close_line(line: &str, name: &str) -> bool {
    let t = line.trim_start();
    if !t.starts_with("#:") {
        return false;
    }
    let w: Vec<&str> = t[2..].split_whitespace().collect();
    w.len() >= 2 && w[0] == name && w[1] == "d"
}

//nt: 词法入口
pub fn lex(src: &str) -> Result<Vec<Spanned>, LexError> {
    let mut out = Vec::new();
    let mut lines: Vec<&str> = src.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop(); // 末行换行不算新行
    }

    let mut asm_blk: Option<String> = None;
    for (idx, raw) in lines.into_iter().enumerate() {
        let line_no = idx + 1;
        //nt: asm 原始行模式
        if let Some(name) = asm_blk.clone() {
            if is_asm_close_line(raw, &name) {
                asm_blk = None; // 关块行本身按正常词法处理
            } else {
                out.push(Spanned { tok: Token::RawAsm(raw.to_string()), line: line_no, col: 1, len: raw.chars().count() });
                out.push(Spanned { tok: Token::Newline, line: line_no, col: raw.chars().count() + 1, len: 0 });
                continue;
            }
        }
        //nt: 进入原始行模式
        if asm_blk.is_none() {
            if let Some(n) = asm_open_block(raw) {
                asm_blk = Some(n);
            }
        }
        //nt: 换行契约
        if raw.contains('\r') {
            return Err(LexError::with_code(
                line_no,
                LexCode::E006,
                &DiagParts::full(
                    format!("第 {} 行", line_no),
                    "读源文件",
                    "发现 CR 字符",
                    "CRLF 换行不合契约（须纯 LF）",
                    "把文件转成纯 LF 换行",
                ),
                Tone::Fix,
            ));
        }
        let line = raw;

        //nt: 整行注释
        if is_comment_line(line) {
            out.push(Spanned { tok: Token::Newline, line: line_no, col: line.chars().count() + 1, len: 0 });
            continue;
        }

        lex_line(line, line_no, &mut out)?;
    }

    Ok(out)
}

//nt: 词法化单行
fn lex_line(line: &str, line_no: usize, out: &mut Vec<Spanned>) -> Result<(), LexError> {
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0usize;

    while i < chars.len() {
        let c = chars[i];

        //nt: 空白（缩进纯装饰）
        if c == ' ' || c == '\t' {
            i += 1;
            continue;
        }

        //nt: 字符串"…"
        if c == '"' {
            let (s, ni) = read_string(&chars, i, line_no, false)?;
            out.push(Spanned { tok: Token::Str(s), line: line_no, col: i + 1, len: ni - i });
            i = ni;
            continue;
        }

        //nt: 原样串@"…"
        if c == '@' {
            if chars.get(i + 1) == Some(&'"') {
                let (s, ni) = read_string(&chars, i + 1, line_no, true)?;
                out.push(Spanned { tok: Token::RawStr(s), line: line_no, col: i + 1, len: ni - i });
                i = ni;
                continue;
            }
            return Err(LexError::with_code(
                line_no,
                LexCode::E005,
                &DiagParts::full(
                    format!("第 {} 行", line_no),
                    "写原样串",
                    "@ 后不是引号",
                    "@ 只能接双引号",
                    "写成 @\"…\"（原样字符串）",
                ),
                Tone::Suggest,
            ));
        }

        //nt: 数字
        if c.is_ascii_digit() {
            let (tok, ni) = read_number(&chars, i, line_no)?;
            out.push(Spanned { tok, line: line_no, col: i + 1, len: ni - i });
            i = ni;
            continue;
        }

        //nt: 标识符/关键字
        if is_ident_start(c) {
            let mut j = i;
            while j < chars.len() && is_ident_cont(chars[j]) {
                j += 1;
            }
            let w: String = chars[i..j].iter().collect();
            let tok = match KEYWORDS.iter().find(|k| **k == w) {
            //nt: size 同 int
                Some(kw) => Token::Kw(kw),
                None => Token::Ident(w),
            };
            out.push(Spanned { tok, line: line_no, col: i + 1, len: j - i });
            i = j;
            continue;
        }

        //nt: 块标记
        if c == '#' {
            let rest: String = chars[i..].iter().collect();
            if rest.starts_with("###:") {
                out.push(Spanned { tok: Token::HashHashHashColon, line: line_no, col: i + 1, len: 4 });
                i += 4;
                continue;
            }
            if rest.starts_with("##:") {
                out.push(Spanned { tok: Token::HashHashColon, line: line_no, col: i + 1, len: 3 });
                i += 3;
                continue;
            }
            if rest.starts_with("#:") {
                out.push(Spanned { tok: Token::HashColon, line: line_no, col: i + 1, len: 2 });
                i += 2;
                continue;
            }
            return Err(unexpected_char(line_no, c));
        }

        //nt: 收束符
        if c == ';' {
            if chars.get(i + 1).is_some_and(|d| d.is_ascii_digit()) {
                return Err(LexError::with_code(
                    line_no,
                    LexCode::E001,
                    &DiagParts::full(
                        format!("第 {} 行", line_no),
                        "写层级标记",
                        "行首 ;数字",
                        ";数字 深度前缀已废止",
                        "层级改用「; 收束」：结构行冒号开体、独占一行 ; 收体",
                    ),
                    Tone::Fix,
                ));
            }
            out.push(Spanned { tok: Token::Sym(";"), line: line_no, col: i + 1, len: 1 });
            i += 1;
            continue;
        }

//nt: 双字符符号
        if i + 1 < chars.len() {
            let two: String = chars[i..i + 2].iter().collect();
            if let Some(s) = SYMBOLS2.iter().find(|s| **s == two) {
                out.push(Spanned { tok: Token::Sym(s), line: line_no, col: i + 1, len: 2 });
                i += 2;
                continue;
            }
        }

        //nt: 单字符符号
        let one = c.to_string();
        if let Some(s) = SYMBOLS1.iter().find(|s| **s == one) {
            out.push(Spanned { tok: Token::Sym(s), line: line_no, col: i + 1, len: 1 });
            i += 1;
            continue;
        }

        return Err(unexpected_char(line_no, c));
    }

    out.push(Spanned { tok: Token::Newline, line: line_no, col: chars.len() + 1, len: 0 });
    Ok(())
}

fn unexpected_char(line_no: usize, c: char) -> LexError {
    LexError::with_code(
        line_no,
        LexCode::E001,
        &DiagParts::full(
            format!("第 {} 行", line_no),
            "解析",
            format!("不认识的字符「{}」", c),
            "不在 Anle 的词法表里",
            "检查拼写（中文近形字：报告 ≠ 报吿）或改用合法符号",
        ),
        Tone::Suggest,
    )
}

//nt: 读字符串
fn read_string(
    chars: &[char],
    quote_at: usize,
    line_no: usize,
    raw: bool,
) -> Result<(String, usize), LexError> {
    let mut i = quote_at + 1;
    let mut s = String::new();
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
        //nt: 原样串
            if raw && chars.get(i + 1) == Some(&'"') {
                s.push('"');
                i += 2;
                continue;
            }
            return Ok((s, i + 1));
        }
        if !raw && c == '\\' {
            let e = chars.get(i + 1).copied().ok_or_else(|| {
                LexError::with_code(
                    line_no,
                    LexCode::E004,
                    &DiagParts::full(
                        format!("第 {} 行", line_no),
                        "写字符串",
                        "转义符后无字符",
                        "\\ 悬空",
                        "转义只支持 \\n \\t \\\\ \\\"",
                    ),
                    Tone::Suggest,
                )
            })?;
            s.push(match e {
                'n' => '\n',
                't' => '\t',
                '\\' => '\\',
                '"' => '"',
                _ => {
                    return Err(LexError::with_code(
                        line_no,
                        LexCode::E004,
                        &DiagParts::full(
                            format!("第 {} 行", line_no),
                            "写字符串",
                            format!("不支持的转义 \\{}", e),
                            "转义只有四种",
                            "只写 \\n \\t \\\\ \\\"（原样串用 @\"…\"）",
                        ),
                        Tone::Suggest,
                    ))
                }
            });
            i += 2;
            continue;
        }
        s.push(c);
        i += 1;
    }
    Err(LexError::with_code(
        line_no,
        LexCode::E004,
        &DiagParts::full(
            format!("第 {} 行", line_no),
            "写字符串",
            "缺收尾双引号",
            "字符串没闭合",
            "补上收尾的 \"",
        ),
        Tone::Suggest,
    ))
}

//nt: 读数字字面量
fn read_number(chars: &[char], start: usize, line_no: usize) -> Result<(Token, usize), LexError> {
    let mut i = start;
    //nt: 进制前缀
    if chars[i] == '0' {
        if let Some(&p) = chars.get(i + 1) {
            let radix = match p {
                'x' | 'X' => Some(16),
                'b' | 'B' => Some(2),
                'o' | 'O' => Some(8),
                _ => None,
            };
            if let Some(radix) = radix {
                i += 2;
                let mut digits = String::new();
                while i < chars.len() && (chars[i] == '_' || chars[i].is_digit(radix)) {
                    if chars[i] != '_' {
                        digits.push(chars[i]);
                    }
                    i += 1;
                }
                let v = i64::from_str_radix(&digits, radix).map_err(|_| num_err(line_no))?;
                return Ok((Token::Int(v), i));
            }
        }
    }

    //nt: 十进制：整数部分
    let mut text = String::new();
    while i < chars.len() && (chars[i] == '_' || chars[i].is_ascii_digit()) {
        if chars[i] != '_' {
            text.push(chars[i]);
        }
        i += 1;
    }
    let mut is_float = false;
    //nt: 小数部分
    if chars.get(i) == Some(&'.') && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit()) {
        is_float = true;
        text.push('.');
        i += 1;
        while i < chars.len() && (chars[i] == '_' || chars[i].is_ascii_digit()) {
            if chars[i] != '_' {
                text.push(chars[i]);
            }
            i += 1;
        }
    }
    //nt: 指数部分
    if matches!(chars.get(i), Some('e') | Some('E')) {
        let mut j = i + 1;
        if matches!(chars.get(j), Some('+') | Some('-')) {
            j += 1;
        }
        if chars.get(j).is_some_and(|d| d.is_ascii_digit()) {
            is_float = true;
            text.push('e');
            //nt: 补记正负号
            if chars.get(i + 1) == Some(&'+') || chars.get(i + 1) == Some(&'-') {
                text.push(chars[i + 1]);
            }
            let mut k = j;
            while k < chars.len() && (chars[k] == '_' || chars[k].is_ascii_digit()) {
                if chars[k] != '_' {
                    text.push(chars[k]);
                }
                k += 1;
            }
            i = k;
        }
    }

    if is_float {
        let v: f64 = text.parse().map_err(|_| num_err(line_no))?;
        Ok((Token::Float(v), i))
    } else {
        let v: i64 = text.parse().map_err(|_| num_err(line_no))?;
        Ok((Token::Int(v), i))
    }
}

fn num_err(line_no: usize) -> LexError {
    LexError::with_code(
        line_no,
        LexCode::E003,
        &DiagParts::full(
            format!("第 {} 行", line_no),
            "写数字字面量",
            "数字格式不对",
            "无法解析",
            "检查进制前缀（0x/0b/0o）与下划线分隔",
        ),
        Tone::Suggest,
    )
}