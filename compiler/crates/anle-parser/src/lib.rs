//! anle-parser —— 语法层：层级语法「分号收束」
//!
//! - 开体：以 `:` 结尾的结构行开一个体（统一规则，不逐构造特判）
//! - 收体：独占一行的 `;` 收体，强制配对
//! - 缩进纯装饰：parser 不看缩进，缩进错位不影响结构
//! - 块界：`#: 名 b` / `#: 名 d` 显式定界，块体无需 `;`
//! - 空体：开体后（仅注释 / 空行）紧跟 `;` ⇒ 报错
//! - `else`：`else:` 必须紧跟其 if 体的收束 `;`（之间仅允许注释 / 空行）
//!
//! 产出语句树；`Stmt::Line` 内的表达式解析归检查层。

use anle_error::{render_parse, ParseCode, DiagParts, Span, Tone};
use anle_lexer::{Spanned, Token};

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
    pub code: String,
//nt: 结构化原文
    pub parts: DiagParts,
    // nt: 可延后到保存
    pub 可延后: bool,
}

impl ParseError {
    pub fn with_code(line: usize, code: ParseCode, parts: &DiagParts, tone: Tone) -> Self {
        Self {
            line,
            message: render_parse(code, parts, tone),
            code: code.number().to_string(),
            parts: parts.clone(),
            可延后: false,
        }
    }

    // nt: 标记可延后
    pub fn 延后(mut self) -> Self {
        self.可延后 = true;
        self
    }
}
//nt: 记号流与位置表
//nt: 构造口
#[derive(Debug, Clone, PartialEq)]
pub struct TokStream {
    toks: Vec<Token>,
    spans: Vec<(usize, usize)>,
}

impl TokStream {
    //nt: 三元组构造
    pub fn new(三元组: Vec<(Token, usize, usize)>) -> Self {
        let mut toks = Vec::with_capacity(三元组.len());
        let mut spans = Vec::with_capacity(三元组.len());
        for (t, c, l) in 三元组 {
            toks.push(t);
            spans.push((c, l));
        }
        debug_assert_eq!(toks.len(), spans.len());
        TokStream { toks, spans }
    }

    //nt: 一次取两样
    pub fn get2(&self, i: usize) -> Option<(&Token, (usize, usize))> {
        self.toks.get(i).map(|t| (t, self.spans[i]))
    }

    //nt: 取第 i 个位置
    pub fn span(&self, i: usize) -> Option<(usize, usize)> {
        self.spans.get(i).copied()
    }

    //nt: 全部位置表
    pub fn spans(&self) -> &[(usize, usize)] {
        &self.spans
    }
}

impl std::ops::Deref for TokStream {
    type Target = [Token];
    fn deref(&self) -> &[Token] {
        &self.toks
    }
}

//nt: 语句
#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    //nt: 普通语句
    Line { line: usize, col: usize, len: usize, toks: TokStream },
    //nt: 结构语句
    Struct { line: usize, col: usize, len: usize, head: TokStream, body: Vec<Stmt> },
    //nt: 语句体函数
    Fn { line: usize, col: usize, len: usize, head: TokStream, body: Vec<Stmt> },
}

//nt: 块
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub name: String,
    //nt: 块行号
    pub line: usize,
    pub col: usize,
    pub len: usize,
    pub stmts: Vec<Stmt>,
    //nt: 块属性后缀
    pub attrs: Vec<String>,
    //nt: asm 原始行
    pub asm_lines: Vec<String>,
    //nt: 所属模块
    pub module: String,
    //nt: 源文件路径
    pub file: String,
}

//nt: 程序=若干块
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub blocks: Vec<Block>,
}

//nt: 栈帧：块体或结构体
enum Kind {
    Block,
    Struct,
    Fn,
}

struct Frame {
    kind: Kind,
    name: String,
    head: TokStream,
    body: Vec<Stmt>,
    line: usize,
    col: usize,
    len: usize,
    //nt: 块属性后缀
    attrs: Vec<String>,
    //nt: asm 原始行
    asm_lines: Vec<String>,
}

/** 块行？`#: 名 b|d [unsafe] [pub] [sle] [ABI]` → Some((名, 开块?, 属性后缀))
 *
* 后缀集合：`b`/`d` 固定；`unsafe` 紧贴 `b`；
 * `pub` / `sle` 先后随意；`ABI` 殿后。此处只**如实收集**，顺序与合法性由后续层判。*/
fn is_block_line(t: &[Token]) -> Option<(String, bool, Vec<String>)> {
    if t.first() != Some(&Token::HashColon) {
        return None;
    }
    let name = match t.get(1) {
        Some(Token::Ident(n)) | Some(Token::Str(n)) => n.clone(),
        _ => return None,
    };
    let (open, rest) = match t.get(2) {
        Some(Token::Ident(w)) if w == "b" => (true, &t[3..]),
        Some(Token::Ident(w)) if w == "d" => (false, &t[3..]),
        _ => return None,
    };
    let mut attrs: Vec<String> = Vec::new();
    for a in rest {
        match a {
            Token::Kw(k) if *k == "unsafe" || *k == "pub" || *k == "sle" || *k == "ABI" || *k == "kill" || *k == "asm" => {
                attrs.push((*k).to_string())
            }
            _ => return None, // 认不出的后缀 ⇒ 不是块行
        }
    }
    Some((name, open, attrs))
}

//nt: fn 定义行
fn is_fn_head(t: &[Token]) -> bool {
    match (t.first(), t.get(1), t.get(2)) {
        (Some(Token::Kw("fn")), _, _) => true,
        (Some(Token::Kw("add")), Some(Token::Kw("fn")), _) => true,
        (Some(Token::Kw("add")), Some(Token::Kw("hide")), Some(Token::Kw("fn"))) => true,
        _ => false,
    }
}

//nt: 结构行
fn is_struct_head(t: &[Token]) -> bool {
    matches!(t.last(), Some(Token::Sym(":")))
}

//nt: 独占一行的`;`
fn is_semi_line(t: &[Token]) -> bool {
    t.len() == 1 && t[0] == Token::Sym(";")
}

//nt: 语法入口
pub fn parse(spanned: Vec<Spanned>) -> Result<Program, ParseError> {
    let (ast, errs) = parse_all(spanned);
    match errs.into_iter().next() {
        Some(e) => Err(e),
        None => Ok(ast),
    }
}

// nt: 多错恢复入口
pub fn parse_all(spanned: Vec<Spanned>) -> (Program, Vec<ParseError>) {
    let mut errs: Vec<ParseError> = Vec::new();
    // nt: 记录并跳行
    macro_rules! 报错 {
        ($e:expr) => {{
            errs.push($e);
            continue;
        }};
    }
    let mut blocks: Vec<Block> = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();
    let mut after_semi = false; // 上一个有意义行是否为 `;`（供 else 规则）

    //nt: 按换行切分行
    //nt: 记号与位置同收
    let mut lines: Vec<(Span, TokStream)> = Vec::new();
    let mut cur: Vec<(Token, usize, usize)> = Vec::new();
    let mut cur_span = Span::default();
    for sp in spanned {
        if sp.tok == Token::Newline {
            if !cur.is_empty() {
                lines.push((cur_span, TokStream::new(std::mem::take(&mut cur))));
            }
            continue;
        }
        if cur.is_empty() {
            cur_span = sp.span();
        } else {
            cur_span.len = sp.col + sp.len - cur_span.col;
        }
        cur.push((sp.tok, sp.col, sp.len));
    }
    if !cur.is_empty() {
        lines.push((cur_span, TokStream::new(cur)));
    }

    for (span, toks) in lines {
        let line_no = span.line;
        if toks.is_empty() {
            continue;
        }

        //nt: 1)块开
        if let Some((name, true, attrs)) = is_block_line(&toks) {
//nt: 块不许嵌套
            if let Some(top) = stack.last() {
                if matches!(top.kind, Kind::Block) {
                    报错!(ParseError::with_code(
                        line_no,
                        ParseCode::E104,
                        &DiagParts::full(
                            format!("第 {} 行", line_no),
                            "开块",
                            format!("块「{}」里又开了块「{}」——块是平的，不许嵌套", top.name, name),
                            "",
                            "把内块移出来平铺：先 `#: 名 d` 关掉当前块，再开新块",
                        ),
                        Tone::Fix,
                    ));
                }
            }
//nt: kill 作关块后缀
            if attrs.iter().any(|a| a == "kill") {
                报错!(ParseError::with_code(
                    line_no,
                    ParseCode::E101,
                    &DiagParts::full(
                        format!("第 {} 行", line_no),
                        "写块行",
                        format!("块「{}」的开块行写了 `kill`", name),
                        "`kill` 只能作关块行后缀",
                        "要本块跑完即清理 ⇒ 写 `#: 块名 d kill`；要回收别的块 ⇒ 在块体内写 `kill #:块名`",
                    ),
                    Tone::Fix,
                ));
            }
            stack.push(Frame { kind: Kind::Block, name, head: TokStream::new(Vec::new()), body: Vec::new(), line: line_no, col: span.col, len: span.len, attrs, asm_lines: Vec::new() });
            after_semi = false;
            continue;
        }

        //nt: 2)块关
        if let Some((name, false, close_attrs)) = is_block_line(&toks) {
        //nt: 体未收束报错
            if let Some(top) = stack.last() {
                if matches!(top.kind, Kind::Struct | Kind::Fn) {
                    let l = top.line;
                    报错!(ParseError::with_code(
                        l,
                        ParseCode::E101,
                        &DiagParts::full(
                            format!("第 {} 行", l),
                            "开体",
                            "体没收束就关块了",
                            "缺独占一行的 ;",
                            "结构行开体后，用独占一行的 ; 收体，再 #: 名 d 关块",
                        ),
                        Tone::Fix,
                    ));
                }
            }
            let Some(f) = stack.pop() else {
                报错!(ParseError::with_code(
                    line_no,
                    ParseCode::E104,
                    &DiagParts::full(
                        format!("第 {} 行", line_no),
                        "关块",
                        "没有对应的开块",
                        name.clone(),
                        "检查 #: 名 b / d 是否成对",
                    ),
                    Tone::Fix,
                ))
            };
            if !matches!(f.kind, Kind::Block) || f.name != name {
                报错!(ParseError::with_code(
                    line_no,
                    ParseCode::E104,
                    &DiagParts::full(
                        format!("第 {} 行", line_no),
                        "关块",
                        "关块对不上",
                        format!("开的是「{}」，关的是「{}」", f.name, name),
                        "块名要相同，且先收完体内的体",
                    ),
                    Tone::Fix,
                ));
            }
//nt: 合并块属性
            let mut all_attrs = f.attrs;
            all_attrs.extend(close_attrs.iter().cloned());
            blocks.push(Block {
                name: f.name,
                line: f.line,
                col: f.col,
                len: f.len,
                stmts: f.body,
                attrs: all_attrs,
                asm_lines: f.asm_lines,
                module: String::new(),
                file: String::new(),
            });
            after_semi = false;
            continue;
        }

        //nt: 3)`;`收体
        if is_semi_line(&toks) {
            let Some(f) = stack.pop() else {
                报错!(ParseError::with_code(
                    line_no,
                    ParseCode::E102,
                    &DiagParts::full(
                        format!("第 {} 行", line_no),
                        "收束",
                        "没有可收的体",
                        "这里多了一个 ;",
                        "只有结构行（以 : 结尾）才需要 ; 收束",
                    ),
                    Tone::Fix,
                ))
            };
            if !matches!(f.kind, Kind::Struct | Kind::Fn) {
                报错!(ParseError::with_code(
                    line_no,
                    ParseCode::E102,
                    &DiagParts::full(
                        format!("第 {} 行", line_no),
                        "收束",
                        "块体不需要 ;",
                        "块界由 #: 名 b / d 定",
                        "去掉这个 ;",
                    ),
                    Tone::Fix,
                ));
            }
            if f.body.is_empty() {
                报错!(ParseError::with_code(
                    line_no,
                    ParseCode::E103,
                    &DiagParts::full(
                        format!("第 {} 行", line_no),
                        "开体",
                        "体里是空的",
                        "开体后紧接着收束",
                        "体里至少写一条语句，或删掉这个结构行",
                    ),
                    Tone::Fix,
                ));
            }
            let st = match f.kind {
                Kind::Fn => Stmt::Fn { line: f.line, col: f.col, len: f.len, head: f.head, body: f.body },
                _ => Stmt::Struct { line: f.line, col: f.col, len: f.len, head: f.head, body: f.body },
            };
            match stack.last_mut() {
                Some(p) => p.body.push(st),
                None => {
                    报错!(ParseError::with_code(
                        line_no,
                        ParseCode::E104,
                        &DiagParts::full(
                            format!("第 {} 行", line_no),
                            "收束",
                            "收束到了块外",
                            "体不在块内",
                            "代码须写在 #: 名 b … d 之间",
                        ),
                        Tone::Fix,
                    ))
                }
            }
            after_semi = true;
            continue;
        }

        //nt: 4)结构行开体
        if is_struct_head(&toks) {
            if let Some(Token::Kw("else")) = toks.first() {
                if !after_semi {
                    报错!(ParseError::with_code(
                        line_no,
                        ParseCode::E105,
                        &DiagParts::full(
                            format!("第 {} 行", line_no),
                            "写 else",
                            "else 没紧跟上一个体的收束",
                            "中间夹了别的内容",
                            "else: 必须紧跟其 if 体的 ; 之后（中间只允许注释 / 空行）",
                        ),
                        Tone::Fix,
                    ));
                }
            }
            let kind = if is_fn_head(&toks) { Kind::Fn } else { Kind::Struct };
            stack.push(Frame { kind, name: String::new(), head: toks, body: Vec::new(), line: line_no, col: span.col, len: span.len, attrs: Vec::new(), asm_lines: Vec::new() });
            after_semi = false;
            continue;
        }

        //nt: 5)普通语句
        //nt: kill 限顺序体
        //nt: 体内出现即报错
        if matches!(toks.first(), Some(Token::Kw("kill"))) {
            if let Some(p) = stack.last() {
                if matches!(p.kind, Kind::Struct) {
                    报错!(ParseError::with_code(
                        line_no,
                        ParseCode::E101,
                        &DiagParts::full(
                            format!("第 {} 行", line_no),
                            "写 kill",
                            "`kill` 不能写在 if / while 体里",
                            "`kill` 只能在块的顺序语句位置",
                            "把 kill 移到块体的顺序位置（不在任何 if / while 内）；条件释放的需求在追踪到位后会蒸发",
                        )
                        .详解("条件 kill 会让「这段地皮死不死」依赖运行期才知道的值，与「回收点编译期定死」直接冲突"),
                        Tone::Fix,
                    ));
                }
            }
        }
        //nt: asm 原始行入表
        if let [Token::RawAsm(s)] = &toks[..] {
            match stack.last_mut() {
                Some(p) if matches!(p.kind, Kind::Block) => p.asm_lines.push(s.clone()),
                Some(_) => {
                    报错!(ParseError::with_code(
                        line_no,
                        ParseCode::E101,
                        &DiagParts::full(
                            format!("第 {} 行", line_no),
                            "写 asm 原文",
                            "asm 原文行跑进了结构体",
                            "它只能在 asm 块体里",
                            "把原文行移进 `#: 名 b asm` … `#: 名 d` 之间",
                        ),
                        Tone::Fix,
                    ));
                }
                None => {
                    报错!(ParseError::with_code(
                        line_no,
                        ParseCode::E104,
                        &DiagParts::full(
                            format!("第 {} 行", line_no),
                            "写 asm 原文",
                            "asm 原文行不在块内",
                            "上面没有开着的 asm 块",
                            "先把块开起来：`#: 名 b asm`",
                        ),
                        Tone::Fix,
                    ));
                }
            }
            after_semi = false;
            continue;
        }

        //nt: 5)普通语句
        match stack.last_mut() {
            Some(p) => p.body.push(Stmt::Line { line: line_no, col: span.col, len: span.len, toks }),
            None => {
                报错!(ParseError::with_code(
                    line_no,
                    ParseCode::E104,
                    &DiagParts::full(
                        format!("第 {} 行", line_no),
                        "写语句",
                        "语句不在块内",
                        "代码须在块内",
                        "把语句放进 #: 名 b … d 之间",
                    ),
                    Tone::Fix,
                ))
            }
        }
        after_semi = false;
    }

//nt: 未收的体
    while let Some(f) = stack.pop() {
        errs.push(match f.kind {
            Kind::Struct | Kind::Fn => ParseError::with_code(
                f.line,
                ParseCode::E101,
                &DiagParts::full(
                    format!("第 {} 行", f.line),
                    "开体",
                    "体没有收束",
                    "缺独占一行的 ;",
                    "结构行开体后，用独占一行的 ; 收体",
                ),
                Tone::Fix,
            ),
            Kind::Block => {
                blocks.push(Block {
                    name: f.name.clone(),
                    line: f.line,
                    col: f.col,
                    len: f.len,
                    stmts: f.body,
                    attrs: f.attrs,
                    asm_lines: f.asm_lines,
                    module: String::new(),
                    file: String::new(),
                });
                ParseError::with_code(
                    f.line,
                    ParseCode::E104,
                    &DiagParts::full(
                        format!("第 {} 行", f.line),
                        "开块",
                        "块没有关",
                        format!("块「{}」缺 #: {} d", f.name, f.name),
                        "补上收尾的 #: 名 d",
                    ),
                    Tone::Fix,
                )
                // nt: 到文件尾才算
                .延后()
            }
        });
    }

    (Program { blocks }, errs)
}