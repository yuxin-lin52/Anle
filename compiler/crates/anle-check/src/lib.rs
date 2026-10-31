//! anle-check —— 检查层：名字、类型、内存语义
//!
//! 分三遍：
//! - 遍 1：名字解析与可见性
//! - 遍 2：类型检查与推断
//! - 遍 3：内存语义标注（存储类 + 回收点 + 护栏）
//!
//! 遍 1 撞错时给「未知」哑元继续，只报独立成立的错。
//!
//! 错误码 `E2xx`：`E201` 名字 · `E202` 类型 · `E203` 内存语义。

use anle_error::{render_check, CheckCode, DiagParts, Tone};
use anle_types::StorageClass;
use anle_lexer::Token;
use anle_parser::{Block as AstBlock, Program as AstProgram, Stmt as AstStmt, TokStream};
use std::collections::{BTreeMap, BTreeSet};

//nt: 一条检查结果
#[derive(Debug, Clone, PartialEq)]
pub struct Diag {
    pub line: usize,
    pub col: usize,
    pub len: usize,
    pub file: String,
    pub code: String,
    //nt: 简版渲染
    pub message: String,
    //nt: 结构化原文
    pub parts: DiagParts,
    //nt: 语气
    pub tone: Tone,
    pub is_error: bool,
}

//nt: 一个已登记的val
#[derive(Debug, Clone, PartialEq)]
pub struct LocalInfo {
    pub block: String,
    pub name: String,
    pub ty: String,
    pub class: StorageClass,
    //nt: 块与名的合成键
    pub 键: std::rc::Rc<str>,
}

//nt: 符号类别
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymKind {
    //nt: add type
    Type,
    //nt: fn 声明
    Fn,
    //nt: val/fix常量
    Val,
    //nt: fn形参
    Param,
    //nt: 块标记行
    Block,
    //nt: 和类型变体
    Variant,
}

//nt: 一个符号
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolInfo {
    pub name: String,
    pub kind: SymKind,
    pub line: usize,
    pub col: usize,
    pub len: usize,
    pub block: String,
}

//nt: 检查结果
#[derive(Debug, Clone, PartialEq)]
pub struct CheckResult {
    pub diags: Vec<Diag>,
    pub locals: Vec<LocalInfo>,
    //nt: 全程序符号表
    pub symbols: Vec<SymbolInfo>,
}

impl CheckResult {
    pub fn ok(&self) -> bool {
        !self.diags.iter().any(|d| d.is_error)
    }
    pub fn errors(&self) -> Vec<&Diag> {
        self.diags.iter().filter(|d| d.is_error).collect()
    }
}

const MAX_DIAGS: usize = 20;
const UNKNOWN: &str = "未知";

//nt: 内置名
pub const BUILTINS: &[&str] = &[
    //nt: 内联汇编
    "asm",
    "print", "push", "len", "len_utf8", "empty", "has", "index", "pop", "insert", "remove",
    "clear", "sort", "slice", "concat", "code", "char", "put",
    "remove", "map", "que", "stack", "keys", "vals", "args", "read_file", "save_file", "exit",
//nt: 运行子程序
    "run",
//nt: 时间与随机
    "sle_ms", "rnd",
//nt: 裸地址读写
    "read_8", "read_16", "read_32", "read_64", "write_8", "write_16", "write_32", "write_64",
//nt: FFI 字符串转换
    "str_to_c", "str_from_c",
];

//nt: 三遍检查

//nt: 已登记类型表
type TypeTable = BTreeMap<String, (位置, Vec<(String, String)>)>;
//nt: 泛型模板
type 泛型表 = BTreeMap<
    String,
    (
        位置,
        Vec<String>,
        Vec<(String, String)>,
        Vec<(String, Vec<(String, String)>)>,
    ),
>;

struct Ctx {
    diags: Vec<Diag>,
    //nt: 当前块所属源文件
    cur_file: String,
    locals: Vec<LocalInfo>,
    //nt: locals 的索引
    local_idx: std::collections::HashMap<std::rc::Rc<str>, usize>,
    //nt: 扫描期临时「隐身」的
    suppress_key: Option<String>,
    //nt: 已登记的公式函数
    fns: BTreeMap<String, FnSig>,
    //nt: 已登记的积类型
    types: TypeTable,
    //nt: 泛型模板
    泛型: 泛型表,
    //nt: 泛型 fn 的能力声
    泛型fn能力: BTreeMap<String, (Vec<String>, Vec<Vec<String>>, 位置, Vec<Token>, Vec<String>)>,
    //nt: 类型参数作用域
    类型参数作用域: BTreeMap<String, BTreeSet<String>>,
    //nt: 常量名
    常量: BTreeSet<String>,
    //nt: 被上限截掉的条数
    suppressed: usize,
    //nt: 和类型变体名
    variants: BTreeSet<String>,
    //nt: 开块行带 unsafe
    unsafe_blocks: BTreeSet<String>,
    //nt: add val 声明行
    val_decl_line: std::collections::HashMap<std::rc::Rc<str>, usize>,
    //nt: add fn 声明行
    fn_decl_line: std::collections::HashMap<String, usize>,
    //nt: 跨模块 fn 签名
    fns_mod: BTreeMap<String, FnSig>,
    //nt: 有体的 fn
    fn_has_body: std::collections::HashSet<String>,
    //nt: 裸用合法的占位绑定
    bare_ok: BTreeSet<String>,
    //nt: 已有值的 val
    val_has_value: std::collections::HashSet<std::rc::Rc<str>>,
}

//nt: 只许出现在 unsafe 块
const UNSAFE_ONLY: &[&str] = &[
    "read_8", "read_16", "read_32", "read_64", "write_8", "write_16", "write_32", "write_64",
    "str_to_c", "str_from_c",
    //nt: 内联汇编 = uns
    "asm",
];

//nt: 公式函数签名
#[derive(Debug, Clone)]
struct FnSig {
    params: Vec<String>,
    ret: String,
    //nt: 外部函数声明
    is_outs: bool,
}

//nt: 拆函数类型
fn parse_fn_ty(s: &str) -> Option<(Vec<String>, String)> {
    let rest = s.strip_prefix("fn(")?;
    let mut 深 = 1i32;
    let mut 闭 = None;
    for (k, c) in rest.char_indices() {
        match c {
            '(' => 深 += 1,
            ')' => {
                深 -= 1;
                if 深 == 0 {
                    闭 = Some(k);
                    break;
                }
            }
            _ => {}
        }
    }
    let 闭 = 闭?;
    let 段 = &rest[..闭];
    let 参 = if 段.trim().is_empty() {
        Vec::new()
    } else {
        split_type_list(段)
    };
    let 返 = rest[闭 + 1..].trim().to_string();
    if 返.is_empty() {
        return None;
    }
    Some((参, 返))
}

fn split_type_list(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut 深 = 0i32;
    for c in s.chars() {
        match c {
            '(' => {
                深 += 1;
                cur.push(c);
            }
            ')' => {
                深 -= 1;
                cur.push(c);
            }
            ',' if 深 == 0 => {
                out.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

//nt: add fn 或 add type
fn is_add_fn(toks: &[Token]) -> bool {
    matches!(toks.first(), Some(Token::Kw("add")))
        && (matches!(toks.get(1), Some(Token::Kw("fn")))
            || (matches!(toks.get(1), Some(Token::Kw("hide")))
                && matches!(toks.get(2), Some(Token::Kw("fn")))))
}

//nt: add outs f
fn is_add_outs_fn(toks: &[Token]) -> bool {
    matches!(toks.first(), Some(Token::Kw("add")))
        && matches!(toks.get(1), Some(Token::Kw("outs")))
        && matches!(toks.get(2), Some(Token::Kw("fn")))
}

//nt: 位置三分量
#[derive(Clone, Copy)]
struct 位置 {
    行: usize,
    列: usize,
    长: usize,
}

impl std::fmt::Display for 位置 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.行)
    }
}

impl Ctx {
    //nt: 统一入口
    fn add_local(&mut self, mut info: LocalInfo) -> usize {
        info.键 = std::rc::Rc::<str>::from(format!("{}|{}", info.block, info.name).as_str());
        self.local_idx.insert(info.键.clone(), self.locals.len());
        self.locals.push(info);
        self.locals.len() - 1
    }

    //nt: 按 `块|名` 查局
    fn find_local(&self, block: &str, name: &str) -> Option<&LocalInfo> {
        if self.is_suppressed(block, name) {
            return None;
        }
        let key = format!("{}|{}", block, name);
        self.local_idx.get(key.as_str()).map(|&i| &self.locals[i])
    }

    //nt: 名字是否处于「隐身」
    fn is_suppressed(&self, block: &str, name: &str) -> bool {
        match self.suppress_key.as_deref() {
            Some(k) => k == format!("{}|{}", block, name),
            None => false,
        }
    }

    //nt: 按 `块|名` 判断
    fn has_local(&self, block: &str, name: &str) -> bool {
        let key = format!("{}|{}", block, name);
        if self.suppress_key.as_deref() == Some(key.as_str()) {
            return false; // 扫描期「隐身」
        }
        if self.local_idx.contains_key(key.as_str()) {
            return true;
        }
        //nt: fn伪块再查所属块
        if let Some(plain) = block.split('#').next() {
            if plain != block && !plain.is_empty() {
                let pkey = format!("{}|{}", plain, name);
                if self.suppress_key.as_deref() != Some(pkey.as_str()) {
                    return self.local_idx.contains_key(pkey.as_str());
                }
            }
        }
        false
    }

    fn err(&mut self, 处: 位置, code: CheckCode, parts: &DiagParts, tone: Tone) {
    let line = 处.行;
        if self.diags.len() >= MAX_DIAGS {
            self.suppressed += 1;
            return; // 上限：不再新增，但**记账**，收尾时告知
        }
        //nt: 未逐条定制的「预期/
        let mut parts = parts.clone();
        let code_s = code.number().to_string();
        let (de, dd) = anle_error::code_defaults(&code_s);
        if parts.预期.is_empty() {
            parts.预期 = de.to_string();
        }
        if parts.详解.is_empty() {
            parts.详解 = dd.to_string();
        }
        let (col, len) = (处.列, 处.长);
        self.diags.push(Diag {
            line,
            col,
            len,
            file: self.cur_file.clone(),
            code: code.number().to_string(),
            message: render_check(code, &parts, tone),
            parts,
            tone,
            is_error: true,
        });
    }
}


//nt: 收集语句跨度

//nt: 检查入口：三遍
pub fn check(ast: &AstProgram) -> CheckResult {
    let mut cx = Ctx {
        diags: Vec::new(),
        cur_file: String::new(),
        locals: Vec::new(),
        local_idx: std::collections::HashMap::new(),
        suppress_key: None,
        fns: BTreeMap::new(),
        types: scan_types(ast),
        泛型: scan_generics(ast),
        泛型fn能力: scan_generic_fns(ast),
        类型参数作用域: BTreeMap::new(),
        常量: scan_fix_names(ast),
        variants: scan_variants(ast),
        unsafe_blocks: ast
            .blocks
            .iter()
            .filter(|b| b.attrs.iter().any(|a| a == "unsafe"))
            .map(|b| b.name.clone())
            .collect(),
        suppressed: 0,
        val_decl_line: std::collections::HashMap::new(),
        fn_decl_line: std::collections::HashMap::new(),
        fns_mod: BTreeMap::new(),
        fn_has_body: std::collections::HashSet::new(),
        bare_ok: BTreeSet::new(),
        val_has_value: std::collections::HashSet::new(),
    };

    //nt: kill 语句检查
    //nt: ① 仅 unsafe
    for b in &ast.blocks {
        cx.cur_file = b.file.clone();
        let is_unsafe_blk = b.attrs.iter().any(|a| a == "unsafe");
        //nt: 收集本块内 call
        //nt: 「被本链调用过」判定
        let mut called_in_block: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for st3 in &b.stmts {
            if let AstStmt::Line { line: _, col, len, toks, .. } = st3 {
                let mut i = 0usize;
                while i < toks.len() {
                    if matches!(toks.get(i), Some(Token::Kw("call"))) {
                        //nt: 跨模块调用
                        let mut k = i + 2;
                        if matches!(toks.get(i + 1), Some(Token::HashHashColon))
                            && matches!(toks.get(i + 3), Some(Token::HashColon))
                        {
                            k = i + 4;
                        }
                        if let Some(Token::Ident(n)) = toks.get(k) {
                            called_in_block.insert((*n).clone());
                        }
                    }
                    i += 1;
                }
            }
        }
        //nt: 收集本程序被 cal
        //nt: 每块只展开一次
        let mut flat_b: Vec<(位置, &TokStream, bool)> = Vec::new();
        flatten_stmts(&b.stmts, false, &mut flat_b);
        for s2 in &b.stmts {
            if let AstStmt::Line { line: _, col, len, toks, .. } = s2 {
                if !matches!(toks.first(), Some(Token::Kw("kill"))) {
                    continue;
                }
                let target = toks.iter().skip(1).filter_map(|t| match t {
                    Token::Ident(n) | Token::Str(n) => Some(n.clone()),
                    _ => None,
                }).next_back();
                let target = match target {
                    Some(t) => t,
                    None => continue,
                };
                if !is_unsafe_blk {
                    cx.err(
                        位置{ 行: b.line, 列: b.col, 长: b.len },
                        CheckCode::E201,
                        &DiagParts::full(
                            format!("第 {} 行", b.line),
                            "danger",
                            format!("块「{}」里用了 `kill #:{}`，但它不是危险块", b.name, target),
                            "",
                            "把开块行写成 `#: 块名 b unsafe`——kill 与 asm 一样，只在危险模式里放行",
                        )
                        .预期("`kill` 语句只能出现在 unsafe 块里")
                        .详解("`kill` 是 unsafe 的第四类能力：它提前回收别的块的地皮，需开发者自己担保 ⇒ 只在危险块里合法。"),
                        Tone::Fix,
                    );
                }
                //nt: ① 污点源
                //nt: ② 传播
                //nt: ③ kill 点之后
                //nt: 精确传播
                //nt: 递归展开
                let mut taint: std::collections::BTreeMap<String, bool> =
                    std::collections::BTreeMap::new();
                let mut seen_kill = false;
                let mut violations: Vec<String> = Vec::new();
                for (ln, tk, nested) in flat_b.clone() {
                    let is_kill_line = matches!(tk.first(), Some(Token::Kw("kill")));

                    let eq_pos = tk.iter().position(|t| matches!(t, Token::Sym("=")));
                    let lhs_name: Option<&String> = match tk.first() {
                        Some(Token::Kw("add")) => match tk.get(2) {
                            Some(Token::Str(v)) => Some(v),
                            _ => None,
                        },
                        Some(Token::Kw("val")) => match tk.get(1) {
                            Some(Token::Str(v)) => Some(v),
                            _ => None,
                        },
                        _ => None,
                    };
                    let rhs_start = eq_pos.map(|p| p + 1).unwrap_or(0);
                    let rhs_names: Vec<&String> = tk[rhs_start..]
                        .iter()
                        .filter_map(|t| match t {
                            Token::Str(v) => Some(v),
                            _ => None,
                        })
                        .collect();

                    let mut refs_target = false;
                    let mut ci = 0usize;
                    while ci < tk.len() {
                        if matches!(tk.get(ci), Some(Token::Kw("call"))) {
                            let mut k = ci + 2;
                            if matches!(tk.get(ci + 1), Some(Token::HashHashColon))
                                && matches!(tk.get(ci + 3), Some(Token::HashColon))
                            {
                                k = ci + 4;
                            }
                            if let Some(Token::Ident(n)) = tk.get(k) {
                                if *n == target {
                                    refs_target = true;
                                }
                            }
                        }
                        ci += 1;
                    }

                    let rhs_tainted = rhs_names.iter().any(|v| taint.get(*v) == Some(&true));

                    if let Some(name) = lhs_name {
                        if refs_target || rhs_tainted {
                            taint.insert((*name).clone(), true);
                        } else if !nested {
                            //nt: 顺序位置的干净赋值
                            //nt: 分支内的赋值**不清
                            taint.insert((*name).clone(), false);
                        }
                    }

                    if seen_kill && !is_kill_line {
                        for v in rhs_names.iter().chain(lhs_name.iter()) {
                            if taint.get(*v) == Some(&true) {
                                violations.push(format!("第 {} 行的「{}」", ln, v));
                            }
                        }
                    }
                    if is_kill_line {
                        seen_kill = true;
                    }
                }
                if !violations.is_empty() {
                    violations.dedup();
                    cx.err(
                        位置{ 行: b.line, 列: b.col, 长: b.len },
                        CheckCode::E201,
                        &DiagParts::full(
                            format!("第 {} 行", b.line),
                            "kill 追踪",
                            format!("`kill #:{}` 之后还有指向该块地皮的引用（{} 处）", target, violations.len()),
                            "",
                            "把这些引用在 kill 之前用完并清掉，或把 kill 移到它们之后",
                        )
                        .预期("kill 点之后不得存在指向待回收段的活引用")
                        .详解(&format!(
                            "编译器追踪到下面这些位置仍在引用「{}」的地皮：{}。\
                             kill 会连整棵子树一起回收 ⇒ 这些引用会变成悬空。\
                             （保守判定：流不敏感，宁可多报不可漏。）",
                            target,
                            violations.join("、")
                        )),
                        Tone::Fix,
                    );
                }

                //nt: 目标须「被本链调用过
                if target != b.name && !called_in_block.contains(&target) {
                    cx.err(
                        位置{ 行: b.line, 列: b.col, 长: b.len },
                        CheckCode::E201,
                        &DiagParts::full(
                            format!("第 {} 行", b.line),
                            "kill 目标",
                            format!("`kill #:{}` 的目标块没有被本块调用过", target),
                            "",
                            "kill 只能回收**本链已经调用过**的块的活跃实例段；若只是想清理本块自己，用 `#: 块名 d kill`",
                        )
                        .预期("kill 的目标必须是本块调用过的块")
                        .详解(
                            "块的活跃实例段由调用产生：没调用过 ⇒ 没有可回收的段 ⇒ 这条 kill 是笔误\
                             （多半是块名写错，或该块其实没在这个链上被调用）。",
                        ),
                        Tone::Fix,
                    );
                }
                if target == b.name {
                    cx.err(
                        位置{ 行: b.line, 列: b.col, 长: b.len },
                        CheckCode::E201,
                        &DiagParts::full(
                            format!("第 {} 行", b.line),
                            "danger",
                            format!("`kill #:{}` 指向当前块自己", target),
                            "",
                            "kill 要回收**别的块**的活跃实例段；要本块跑完即清理，用关块行后缀 `#: 块名 d kill`",
                        )
                        .预期("kill 的目标必须是**非当前块**")
                        .详解("语句 `kill #:块名` 回收别的块的活跃实例段；`#: 块名 d kill` 回收本块自己——两个挂载点，别混。"),
                        Tone::Fix,
                    );
                }
            }
        }
    }

    //nt: d kill 后缀检查
    //nt: ① 该块也必须是危险
    //nt: ② 开块行写法
    for b in &ast.blocks {
        cx.cur_file = b.file.clone();
        let has_kill = b.attrs.iter().any(|a| a == "kill");
        if !has_kill {
            continue;
        }
        let is_unsafe = b.attrs.iter().any(|a| a == "unsafe");
        if !is_unsafe {
            cx.err(
                位置{ 行: b.line, 列: b.col, 长: b.len },
                CheckCode::E201,
                &DiagParts::full(
                    format!("第 {} 行", b.line),
                    "danger",
                    format!("块「{}」用了 `d kill`，但它不是危险块", b.name),
                    "",
                    "把开块行写成 `#: 块名 b unsafe`——kill 与 asm 一样，只在危险模式里放行",
                )
                .预期("`d kill` 的块必须是 unsafe 块")
                .详解(
                    "`kill` 是 unsafe 的第四类能力（与裸地址读写、C 互操作、内联汇编并列）：\
                     它提前回收别的块的活跃地皮，属于需要开发者自己担保的事 ⇒ 只在危险块里合法。",
                ),
                Tone::Fix,
            );
        }
    }

    //nt: 块名唯一性
    {
        let mut seen: std::collections::BTreeMap<(String, String), usize> =
            std::collections::BTreeMap::new();
        for b in &ast.blocks {
            cx.cur_file = b.file.clone();
            let key = (b.module.clone(), b.name.clone());
            if let Some(first) = seen.get(&key) {
                cx.err(
                    位置{ 行: b.line, 列: b.col, 长: b.len },
                    CheckCode::E201,
                    &DiagParts::full(
                        format!("第 {} 行", b.line),
                        "建块",
                        format!(
                            "块「{}」重名——同模块内块不许重名（第一次在第 {} 行）",
                            b.name, first
                        ),
                        "",
                        "换个块名；跨模块的同名块是合法的（引用时写 `##:模块 #:块` 区分）",
                    )
                    .预期("同一个模块里块名唯一")
                    .详解(
                        "块名在**同模块内**必须唯一：两个同名块会被当成两个链根各跑一遍，`call #:块` 也无从判断调哪个。\
                         不同模块可以有同名块——引用时 `##:模块 #:块` 天然区分。",
                    ),
                    Tone::Fix,
                );
            } else {
                seen.insert(key, b.line);
            }
        }
    }

//nt: 块行属性合法性
    for b in &ast.blocks {
        cx.cur_file = b.file.clone();
        //nt: 判序只看开块行属性；
        let a: Vec<&String> = b.attrs.iter().filter(|x| x.as_str() != "kill").collect();
        let line = b.line;
        if let Some(pos) = a.iter().position(|x| x.as_str() == "unsafe") {
            if pos != 0 {
                cx.err(
                    位置{ 行: b.line, 列: b.col, 长: b.len },
                    CheckCode::E203,
                    &DiagParts::full(
                        format!("第 {} 行", line),
                        "块行属性",
                        format!("块「{}」属性顺序不合法：`unsafe` 未紧贴 `b`", b.name),
                        "",
                        "写成 `#: 名 b unsafe [pub|sle] [ABI]`",
                    ),
                    Tone::Fix,
                );
            }
        }
        if let Some(pos) = a.iter().position(|x| x.as_str() == "ABI") {
            if pos + 1 != a.len() {
                cx.err(
                    位置{ 行: b.line, 列: b.col, 长: b.len },
                    CheckCode::E203,
                    &DiagParts::full(
                        format!("第 {} 行", line),
                        "块行属性",
                        format!("块「{}」属性顺序不合法：`ABI` 未殿后", b.name),
                        "",
                        "把 `ABI` 写到最后一个属性位",
                    ),
                    Tone::Fix,
                );
            }
            if !a.iter().any(|x| x.as_str() == "pub") {
                cx.err(
                    位置{ 行: b.line, 列: b.col, 长: b.len },
                    CheckCode::E203,
                    &DiagParts::full(
                        format!("第 {} 行", line),
                        "块行属性",
                        format!("块「{}」标了 `ABI` 却没标 `pub`", b.name),
                        "",
                        "补上 `pub`（导出面必须显式开门，不做隐式蕴含）",
                    ),
                    Tone::Fix,
                );
            }
        }
    }

    //nt: 遍 1 收名字
    pass1_collect(ast, &mut cx);
    //nt: 能力约束
    check_caps(&mut cx);
    pass1_uses(ast, &mut cx);
    //nt: hide 可见性声明
    check_hide_decls(ast, &mut cx);
    //nt: 遍2：类型
    pass2_types(ast, &mut cx);
    //nt: 遍 2 附带
    pass2_calls(ast, &mut cx);
    //nt: 积类型环检测
    let t = cx.types.clone();
    check_type_cycles(&t, &mut cx);
    //nt: 遍 2 附带
    pass2_match_exhaustive(ast, &mut cx);
    //nt: 遍 3 标内存
    pass3_memory(ast, &mut cx);
    //nt: sle 块时序检查
    pass_sle_calls(ast, &mut cx);

    //nt: 超上限 ⇒ 补一条提
    if cx.suppressed > 0 {
        cx.diags.push(Diag {
            line: 0,
            col: 0,
            len: 0,
            file: String::new(),
            code: String::new(),
            message: format!(
                "提示：还有 {} 条未报告（单次诊断上限 {} 条）—— 先修上面这些，再编一次会露出后面的",
                cx.suppressed, MAX_DIAGS
            ),
            parts: DiagParts::default(),
            tone: Tone::Suggest,
            is_error: false,
        });
    }

    CheckResult { diags: cx.diags, locals: cx.locals, symbols: collect_symbols(ast) }
}


//nt: 逐条语句找声明，产「
//nt: 位置直接取自 AST
pub fn collect_symbols(ast: &AstProgram) -> Vec<SymbolInfo> {
    let mut out = Vec::new();
    for b in &ast.blocks {
        out.push(SymbolInfo {
            name: b.name.clone(),
            kind: SymKind::Block,
            line: b.line,
            col: b.col,
            len: b.len,
            block: b.name.clone(),
        });
        collect_symbols_in(&b.stmts, &b.name, &mut out);
    }
    out
}

//nt: 在一组语句里找声明
fn collect_symbols_in(stmts: &[AstStmt], 块: &str, out: &mut Vec<SymbolInfo>) {
    for s in stmts {
        let (line, col, len, toks): (usize, usize, usize, &TokStream) = match s {
            AstStmt::Line { line, col, len, toks, .. } => (*line, *col, *len, toks),
            AstStmt::Struct { line, col, len, head, body, .. } | AstStmt::Fn { line, col, len, head, body, .. } => {
                登记头部(head, 位置{ 行: *line, 列: *col, 长: *len }, 块, out);
                collect_symbols_in(body, 块, out);
                continue;
            }
        };
        登记头部(toks, 位置{ 行: line, 列: col, 长: len }, 块, out);
    }
}

//nt: 从一条声明语句的 t
fn 登记头部(toks: &TokStream, 处: 位置, 块: &str, out: &mut Vec<SymbolInfo>) {
    let line = 处.行;
    let kind = match (toks.first(), toks.get(1)) {
        (Some(Token::Kw("add")), Some(Token::Kw("fn"))) => SymKind::Fn,
        (Some(Token::Kw("add")), Some(Token::Kw("val"))) => SymKind::Val,
        (Some(Token::Kw("add")), Some(Token::Kw("fix"))) => SymKind::Val,
        (Some(Token::Kw("add")), Some(Token::Kw("type"))) => SymKind::Type,
        _ => return,
    };
    let Some((Token::Str(名) | Token::Ident(名), (col, len))) = toks.get2(2) else {
        return;
    };
    out.push(SymbolInfo {
        name: 名.clone(),
        kind,
        line,
        col,
        len,
        block: 块.to_string(),
    });
}


//nt: 块是否带 unsafe
fn block_is_unsafe(block: &AstBlock, ast: &AstProgram) -> bool {
    //nt: 块行属性在语法层已消
    //nt: 这里以「块名 + 属
//nt: 遍 3 由块内是否出
    let _ = (block, ast);
    false
}

fn pass1_collect(ast: &AstProgram, cx: &mut Ctx) {
    for b in &ast.blocks {
        cx.cur_file = b.file.clone();
        collect_block(b, cx);
    }
}

fn collect_block(b: &AstBlock, cx: &mut Ctx) {
    collect_stmts(&b.stmts, b, cx);
}

//nt: 语句体 fn 的头部
fn stmt_fn_head(head: &[Token]) -> Vec<Token> {
    head.iter().cloned().filter(|t| *t != Token::Sym(":")).collect()
}

//nt: 语句体 fn 的函数
fn stmt_fn_name(head: &[Token]) -> Option<String> {
    let h = stmt_fn_head(head);
    let at = match (h.first(), h.get(1)) {
        (Some(Token::Kw("fn")), _) => 1,
        (Some(Token::Kw("add")), Some(Token::Kw("hide"))) => 3,
        _ => 2,
    };
    match h.get(at) {
        Some(Token::Str(n)) | Some(Token::Ident(n)) => Some(n.clone()),
        _ => None,
    }
}

//nt: 解析泛型语句头
fn 泛型语句头(h: &[Token]) -> Option<(Vec<(String, Vec<String>)>, Vec<(String, String)>)> {
    let at = match (h.first(), h.get(1)) {
        (Some(Token::Kw("fn")), _) => 1,
        (Some(Token::Kw("add")), Some(Token::Kw("hide"))) => 3,
        _ => 2,
    };
    if !matches!(h.get(at + 1), Some(Token::Sym("("))) {
        return None;
    }
    let 止 = 配对括号(h, at + 1)?;
    let mut 参数: Vec<(String, Vec<String>)> = Vec::new();
    for 段 in h[at + 2..止].split(|t| *t == Token::Sym(",")) {
        match 段 {
            [Token::Ident(n)] => 参数.push((n.clone(), Vec::new())),
            [Token::Ident(n), Token::Sym(op)] => 参数.push((n.clone(), vec![(*op).to_string()])),
            _ => return None,
        }
    }
    if 参数.is_empty() {
        return None;
    }
    let (形参, _) = parse_params(&h[止 + 1..]).ok()?;
    Some((参数, 形参))
}

//nt: 找出全部 give
fn 找give(stmts: &[AstStmt], 出: &mut Vec<(usize, Vec<Token>)>) {
    for st in stmts {
        match st {
            AstStmt::Line { line, col, len, toks, .. } if matches!(toks.first(), Some(Token::Kw("give"))) => {
                出.push((*line, toks.to_vec()));
            }
            AstStmt::Fn { body, .. } | AstStmt::Struct { body, .. } => 找give(body, 出),
            _ => {}
        }
    }
}

//nt: 函数体独立作用域
fn fn_scope_block(b: &AstBlock, fname: &str, body: &[AstStmt]) -> AstBlock {
    AstBlock {
        name: format!("{}#{}", b.name, fname),
        line: b.line,
        col: b.col,
        len: b.len,
        stmts: body.to_vec(),
        attrs: b.attrs.clone(),
        asm_lines: Vec::new(),
        module: b.module.clone(),
        file: b.file.clone(),
    }
}

//nt: 伪块名还原
fn 原块(block: &str) -> &str {
    block.split('#').next().unwrap_or(block)
}

//nt: 块内语句**递归
fn collect_stmts(stmts: &[AstStmt], b: &AstBlock, cx: &mut Ctx) {
    for s in stmts {
        match s {
            AstStmt::Fn { line, col, len, head, body, .. } => {
                let h = stmt_fn_head(head);
                if matches!(h.first(), Some(Token::Kw("fn"))) {
                    collect_rebind_fn(b, 位置{ 行: *line, 列: *col, 长: *len }, &h, cx);
                } else {
                    collect_fn(b, 位置{ 行: *line, 列: *col, 长: *len }, &h, cx);
                }
                if let Some(n) = stmt_fn_name(head) {
                    cx.fn_has_body.insert(format!("{}|{}", b.name, n.clone()));
                    let pb = fn_scope_block(b, &n, body);
                    let 形参 = match 泛型语句头(&h) {
                        Some((类型参数, ps)) => {
                            cx.类型参数作用域.insert(
                                pb.name.clone(),
                                类型参数.iter().map(|(n, _)| n.clone()).collect(),
                            );
                            Some(ps)
                        }
                        None => parse_params(&h).ok().map(|(ps, _)| ps),
                    };
                    if let Some(params) = 形参 {
                        for (pn, pt) in params {
                            let key = std::rc::Rc::<str>::from(format!("{}|{}", pb.name, pn).as_str());
                            if !cx.local_idx.contains_key(&*key) {
                                cx.local_idx.insert(key.clone(), cx.locals.len());
                                cx.locals.push(LocalInfo {
                                    block: pb.name.clone(),
                                    name: pn.clone(),
                                    ty: pt,
                                    class: StorageClass::Tmp,
                                    键: key.clone(),
                                });
                            }
                        }
                    }
                    collect_stmts(body, &pb, cx);
                }
            }
            AstStmt::Line { line, col, len, toks, .. } => {
                //nt: 单行绑定
                collect_inline_case_bindings(b, toks, 0, cx);
                if is_add_val(toks) {
                    collect_val(b, 位置{ 行: *line, 列: *col, 长: *len }, toks, cx);
                }
                if is_add_fn(toks) {
                    collect_fn(b, 位置{ 行: *line, 列: *col, 长: *len }, toks, cx);
                }
                if is_add_outs_fn(toks) {
                    collect_outs_fn(b, 位置{ 行: *line, 列: *col, 长: *len }, toks, cx);
                }
                //nt: 补体/重绑定行
                if matches!(toks.first(), Some(Token::Kw("fn"))) {
                    collect_rebind_fn(b, 位置{ 行: *line, 列: *col, 长: *len }, toks, cx);
                }
                //nt: 改值/重绑定行
                if matches!(toks.first(), Some(Token::Kw("val"))) {
                    collect_val_rebind(b, 位置{ 行: *line, 列: *col, 长: *len }, toks, cx);
                }
            }
            AstStmt::Struct { line, col, len, head, body, .. } => {
                //nt: 判别统一带括号
                if matches!(head.first(), Some(Token::Kw("case"))) {
                    if let Some(Token::Ident(n)) = head.get(1) {
                        if n != "_" && !matches!(head.get(2), Some(Token::Sym("("))) {
                            cx.err(
                                位置{ 行: *line, 列: *col, 长: *len },
                                CheckCode::E204,
                                &DiagParts::full(
                                    format!("第 {} 行", line),
                                    "match 判别",
                                    format!("无字段变体的判别也要写括号——`case {}()`", n),
                                    "",
                                    "判别统一带括号：有字段写 `case 变体(绑定)`，无字段写 `case 变体()`，兜底写 `case _:`",
                                ),
                                Tone::Fix,
                            );
                        }
                    }
                }
                //nt: case 变体名
                collect_case_bindings(b, head, cx);
                //nt: for 循环变量
                if matches!(head.first(), Some(Token::Kw("for"))) {
                    if let Some(Token::Ident(n)) = head.get(1) {
                        cx.err(
                            位置{ 行: *line, 列: *col, 长: *len },
                            CheckCode::E201,
                            &DiagParts::full(
                                format!("第 {} 行", line),
                                "循环变量",
                                format!("循环变量「{}」没加引号——自定义名一律带引号（块名除外）", n),
                                "",
                                format!("写成 `for \"{}\" in …`；体内读取写 val \"{}\"", n, n),
                            ),
                            Tone::Fix,
                        );
                    }
                    if let Some(Token::Str(n)) = head.get(1) {
                        let vname = (*n).clone();
                        if !cx.has_local(&b.name, &vname) {
                            let ty = iter_elem_ty(head, &cx.locals, &cx.local_idx, &b.name);
                            let 新号 = cx.add_local(LocalInfo {
                                block: b.name.clone(),
                                name: vname.clone(),
                                ty,
                                键: std::rc::Rc::from(""),
                                    class: StorageClass::Tmp,
                            });
                            //nt: 循环变量也显式
                        }
                    }
                }
                collect_stmts(body, b, cx);
            }
        }
    }
}

//nt: 登记 add val
fn collect_val(b: &AstBlock, 处: 位置, toks: &[Token], cx: &mut Ctx) {
    let line = 处.行;
    let name = match toks.get(2) {
        Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
        _ => return,
    };
    //nt: 记声明行
    let 新键 = std::rc::Rc::<str>::from(format!("{}|{}", b.name, name).as_str());
    let 键 = match cx.local_idx.get(&*新键) {
        Some(&i) => cx.locals[i].键.clone(),
        None => 新键,
    };
    cx.val_decl_line.entry(键.clone()).or_insert(line);
    //nt: 有值 / 无值
    if toks.iter().any(|t| matches!(t, Token::Sym("="))) {
        cx.val_has_value.insert(键);
    }
    if let Some(ty) = container_ty_at(toks, 3) {
//nt: 纯下标槽禁装容器
        let head = ty.split('(').next().unwrap_or("").trim().to_string();
        let mut bad: Option<String> = None;
        //nt: list / que
        if matches!(head.as_str(), "list" | "que" | "stack") {
            if let Some(e) = elem_ty_str(&ty) {
                if is_container_ty(&e) {
                    bad = Some(format!("元素是 {}", e));
                }
            }
        }
        //nt: map 的**键
        if head == "map" {
            if let Some(inner) = ty.strip_prefix("map(").and_then(|x| x.strip_suffix(')')) {
                let k = inner.split(',').next().unwrap_or("").trim();
//nt: 键不能是把手
                if is_container_ty(k) {
                    bad = Some(format!("键是容器 {}", k));
                } else if k.ends_with('*') {
                    bad = Some(format!("键是把手 {}", k));
                }
            }
        }
        if let Some(why) = bad {
            cx.err(
                处,
                CheckCode::E203,
                &DiagParts::full(
                    format!("第 {} 行", line),
                    "建 val",
                    format!("容器里不能套容器——{}", why),
                    "",
                    "改成一维扁平 + 手工索引；要装容器就放进 map 的值或结构体字段（有名字的地方可以装）",
                ),
                Tone::Fix,
            );
        }
    }
    let ty = match container_ty_at(toks, 3) {
        Some(t) => t,
        None => match toks.get(3) {
            Some(Token::Kw(t)) => {
//nt: 内置类型加星 ⇒ 编
                if matches!(toks.get(4), Some(Token::Sym("*"))) {
                    cx.err(
                        处,
                        CheckCode::E203,
                        &DiagParts::full(
                            format!("第 {} 行", line),
                            "建 val",
                            format!("「{}」是内置类型，加不了星", t),
                            "",
                            "内置类型本来就是值；要引用语义请用自定义类型（如 node*）",
                        ),
                        Tone::Fix,
                    );
                    return;
                }
                (*t).to_string()
            }
            //nt: 自定义类型名是 Id
            Some(Token::Ident(n)) => {
                if matches!(toks.get(4), Some(Token::Sym("*"))) {
                    format!("{}*", n)
                } else {
                    n.clone()
                }
            }
            //nt: 无类型标注
            _ => match toks.iter().position(|t| *t == Token::Sym("=")) {
                Some(eq) => call_ret(&toks[eq + 1..], &b.name, &cx.fns, &cx.locals)
                    .or_else(|| infer(&toks[eq + 1..], &cx.locals, &cx.local_idx, &b.name, &cx.types).map(|s| s.to_string()))
                    .unwrap_or_else(|| UNKNOWN.to_string()),
                None => return,
            },
        },
    };
    if cx.has_local(&b.name, &name) {
        //nt: 分同fn重名与撞块级
        let 同fn = cx.find_local(&b.name, &name).is_some();
        let (标题, 正文, 改法) = if 同fn {
            (
                format!("块「{}」里已经有 val「{}」", b.name, name),
                "",
                "换个名字（同一个块里 val 不许重名）".to_string(),
            )
        } else {
            (
                format!("fn 局部「{}」不许与块级 val 重名", name),
                "fn 的局部和形参有自己的名字空间，但不得占用块级 val 的名字",
                "给 fn 局部换个名字，与块级 val 错开".to_string(),
            )
        };
        cx.err(
            处,
            CheckCode::E201,
            &DiagParts::full(format!("第 {} 行", line), "建 val", 标题, 正文, 改法),
            Tone::Fix,
        );
        return;
    }
    cx.add_local(LocalInfo { block: b.name.clone(), name, ty, 键: std::rc::Rc::from(""),
                                    class: StorageClass::Tmp });
}

//nt: for 名 in 源
fn iter_elem_ty(head: &[Token], locals: &[LocalInfo], idx: &std::collections::HashMap<std::rc::Rc<str>, usize>, block: &str) -> String {
    let mut src: &[Token] = if head.len() >= 4 { &head[3..head.len() - 1] } else { return "int".into() };
    //nt: 剥掉 call 前缀
    if matches!(src.first(), Some(Token::Kw("call")))
        && matches!(src.get(1), Some(Token::Kw("val")))
    {
        src = &src[1..];
    }
    if src.len() >= 2
        && matches!(src[0], Token::Ident(ref n) if n == "keys")
        && matches!(src[1], Token::Sym("("))
    {
        return "str".into();
    }
    //nt: 显式取值形态
    if let [Token::Kw("val"), rest @ (Token::Str(_) | Token::Ident(_))] = src {
        let n = match rest {
            Token::Str(n) | Token::Ident(n) => n,
            _ => unreachable!(),
        };
        if let Some(l) = idx.get(format!("{}|{}", block, n).as_str()).map(|&i| &locals[i]) {
            if let Some(e) = elem_ty_str(&l.ty) {
                return e;
            }
        }
    }
    if let [Token::Ident(n)] = src {
        if let Some(l) = idx.get(format!("{}|{}", block, n).as_str()).map(|&i| &locals[i]) {
            if let Some(e) = elem_ty_str(&l.ty) {
                return e;
            }
        }
    }
    "int".into()
}

//nt: 容器类型串 ⇒ 元素
fn elem_ty_str(ty: &str) -> Option<String> {
    let (head, inner) = ty.split_once('(')?;
    let inner = inner.strip_suffix(')')?;
    match head {
        "list" | "que" | "stack" => Some(inner.trim().to_string()),
        "map" => {
            let mut depth = 0i32;
            for (i, c) in inner.char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    ',' if depth == 0 => return Some(inner[i + 1..].trim().to_string()),
                    _ => {}
                }
            }
            None
        }
        _ => None,
    }
}

//nt: 类型串是否是容器
fn is_container_ty(t: &str) -> bool {
    matches!(t.split('(').next().unwrap_or("").trim(), "list" | "que" | "stack" | "map")
}

fn is_add_val(toks: &[Token]) -> bool {
    matches!(toks.first(), Some(Token::Kw("add")))
        && matches!(toks.get(1), Some(Token::Kw("val")) | Some(Token::Kw("fix")))
}

//nt: add fix "名
fn is_add_fix(toks: &[Token]) -> bool {
    matches!(toks.first(), Some(Token::Kw("add"))) && matches!(toks.get(1), Some(Token::Kw("fix")))
}


//nt: 预扫全程序类型声明
fn scan_types(ast: &AstProgram) -> TypeTable {
    let mut out: TypeTable = BTreeMap::new();
    for b in &ast.blocks {
        let mut flat: Vec<(位置, &TokStream, bool)> = Vec::new();
        flatten_stmts(&b.stmts, false, &mut flat);
        for (处, toks, _nested) in flat {
            let _line = 处.行;
                if is_add_type(toks) {
                    //nt: 泛型模板不进普通类型
                    if matches!(toks.get(3), Some(Token::Sym("("))) {
                        continue;
                    }
                    //nt: 积类型：收字段表
                    if let Some((name, fields)) = parse_type_def(toks) {
                        out.entry(name).or_insert((处, fields));
                        continue;
                    }
                    //nt: 和类型
                    if let Some(name) = toks.get(2).and_then(|t| match t {
                        Token::Str(n) | Token::Ident(n) => Some(n.clone()),
                        _ => None,
                    }) {
                        if toks.iter().any(|t| *t == Token::Sym("|")) {
                            out.entry(name).or_insert((处, Vec::new()));
                        }
                    }
                }
                    }
    }
    out
}

//nt: 把类型串里的类型参数
fn 替换类型参数(s: &str, 参数: &[String], 实参: &[String]) -> String {
    let mut 出 = String::new();
    let 字 = |c: char| c.is_alphanumeric() || c == '_';
    let cs: Vec<char> = s.chars().collect();
    let mut i = 0usize;
    while i < cs.len() {
        if 字(cs[i]) {
            let 起 = i;
            while i < cs.len() && 字(cs[i]) {
                i += 1;
            }
            let 词: String = cs[起..i].iter().collect();
            match 参数.iter().position(|p| *p == 词) {
                Some(k) => 出.push_str(&实参[k]),
                None => 出.push_str(&词),
            }
            continue;
        }
        出.push(cs[i]);
        i += 1;
    }
    出
}

//nt: 扫泛型模板
fn scan_generics(ast: &AstProgram) -> 泛型表 {
    let mut out: 泛型表 = BTreeMap::new();
    for b in &ast.blocks {
        let mut flat: Vec<(位置, &TokStream, bool)> = Vec::new();
        flatten_stmts(&b.stmts, false, &mut flat);
        for (处, toks, _nested) in flat {
            let _line = 处.行;
            if !is_add_type(toks) || !matches!(toks.get(3), Some(Token::Sym("("))) {
                continue;
            }
            let name = match toks.get(2) {
                Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
                _ => continue,
            };
            let mut 深 = 0i32;
            let mut 尽 = None;
            for (k, t) in toks.iter().enumerate().skip(3) {
                match t {
                    Token::Sym("(") => 深 += 1,
                    Token::Sym(")") => {
                        深 -= 1;
                        if 深 == 0 {
                            尽 = Some(k);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let 尽 = match 尽 {
                Some(k) => k,
                None => continue,
            };
            let mut 参数: Vec<String> = Vec::new();
            for 段 in toks[4..尽].split(|t| *t == Token::Sym(",")) {
                if let [Token::Ident(n)] = 段 {
                    参数.push(n.clone());
                }
            }
            if 参数.is_empty() {
                continue;
            }
            let 字段 = match parse_type_def(toks) {
                Some((_名, f)) => f,
                None => Vec::new(),
            };
            //nt: 和类型的变体表
            let mut 变体表: Vec<(String, Vec<(String, String)>)> = Vec::new();
            if 字段.is_empty() {
                if let Some(eq) = toks.iter().position(|t| *t == Token::Sym("=")) {
                    for 段 in toks[eq + 1..].split(|t| *t == Token::Sym("|")) {
                        if let Some(Token::Ident(vn)) | Some(Token::Str(vn)) = 段.first() {
                            let mut 载: Vec<(String, String)> = Vec::new();
                            if matches!(段.get(1), Some(Token::Sym("("))) {
                                let mut 深 = 0i32;
                                let mut 止 = None;
                                for (k, t) in 段.iter().enumerate().skip(1) {
                                    match t {
                                        Token::Sym("(") => 深 += 1,
                                        Token::Sym(")") => {
                                            深 -= 1;
                                            if 深 == 0 {
                                                止 = Some(k);
                                                break;
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                                if let Some(止) = 止 {
                                    for (i, a) in 段[2..止]
                                        .split(|t| *t == Token::Sym(","))
                                        .enumerate()
                                    {
                                        let 型: String = a
                                            .iter()
                                            .filter_map(|t| match t {
                                                Token::Ident(x) => Some(x.clone()),
                                                Token::Kw(k) => Some((*k).to_string()),
                                                Token::Sym("*") => Some("*".to_string()),
                                                _ => None,
                                            })
                                            .collect();
                                        if !型.is_empty() {
                                            载.push((format!("t{}", i), 型));
                                        }
                                    }
                                }
                            }
                            变体表.push((vn.clone(), 载));
                        }
                    }
                }
            }
            out.insert(name, (处, 参数, 字段, 变体表));
        }
    }
    out
}

//nt: 扫常量名
fn scan_fix_names(ast: &AstProgram) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for b in &ast.blocks {
        let mut flat: Vec<(位置, &TokStream, bool)> = Vec::new();
        flatten_stmts(&b.stmts, false, &mut flat);
        for (_l, toks, _nested) in flat {
            if is_add_fix(toks) {
                if let Some(Token::Str(n)) | Some(Token::Ident(n)) = toks.get(2) {
                    out.insert(format!("{}|{}", b.name, n));
                }
            }
        }
    }
    out
}

//nt: 扫泛型 fn 的能力
fn scan_generic_fns(ast: &AstProgram)
    -> BTreeMap<String, (Vec<String>, Vec<Vec<String>>, 位置, Vec<Token>, Vec<String>)> {
    let mut out: BTreeMap<String, (Vec<String>, Vec<Vec<String>>, 位置, Vec<Token>, Vec<String>)> =
        BTreeMap::new();
    for b in &ast.blocks {
        let mut flat: Vec<(位置, &TokStream, bool)> = Vec::new();
        flatten_stmts(&b.stmts, false, &mut flat);
        for (_line, toks, _nested) in flat {
            let 名位 = if matches!(toks.first(), Some(Token::Kw("add")))
                && matches!(toks.get(1), Some(Token::Kw("fn")))
            {
                2
            } else if matches!(toks.first(), Some(Token::Kw("fn"))) {
                1
            } else {
                continue;
            };
            if !matches!(toks.get(名位 + 1), Some(Token::Sym("("))) {
                continue;
            }
            let mut 深 = 0i32;
            let mut 止 = None;
            for (k, t) in toks.iter().enumerate().skip(名位 + 1) {
                match t {
                    Token::Sym("(") => 深 += 1,
                    Token::Sym(")") => {
                        深 -= 1;
                        if 深 == 0 {
                            止 = Some(k);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let 止 = match 止 {
                Some(k) => k,
                None => continue,
            };
            let mut 参数: Vec<String> = Vec::new();
            let mut 能力: Vec<Vec<String>> = Vec::new();
            let mut 合规 = true;
            for 段 in toks[名位 + 2..止].split(|t| *t == Token::Sym(",")) {
                match 段 {
                    [Token::Ident(n)] => {
                        参数.push(n.clone());
                        能力.push(Vec::new());
                    }
                    [Token::Ident(n), Token::Sym(op)] => {
                        参数.push(n.clone());
                        能力.push(vec![(*op).to_string()]);
                    }
                    _ => 合规 = false,
                }
            }
            if 合规 && !参数.is_empty() {
                if let Some(Token::Str(n)) | Some(Token::Ident(n)) = toks.get(名位) {
                    let 体 = match toks.iter().position(|t| *t == Token::Sym("=")) {
                        Some(eq) => toks[eq + 1..].to_vec(),
                        None => Vec::new(),
                    };
                    //nt: 形参名
                    let mut 形参名: Vec<String> = Vec::new();
                    if let Some(开2) = 配对括号(toks, 名位 + 1).map(|k| k + 1) {
                        if let Some(止2) = 配对括号(toks, 开2) {
                            for 段 in toks[开2 + 1..止2].split(|t| *t == Token::Sym(",")) {
                                if let Some(Token::Ident(pn)) | Some(Token::Str(pn)) = 段.first() {
                                    形参名.push(pn.clone());
                                }
                            }
                        }
                    }
                    out.insert(n.clone(), (参数, 能力, _line, 体, 形参名));
                }
            }
        }
    }
    for b in &ast.blocks {
        walk_generic_stmt_fns(&b.stmts, &mut out);
    }
    out
}

//nt: 收集模板体
fn walk_generic_stmt_fns(
    stmts: &[AstStmt],
    out: &mut BTreeMap<String, (Vec<String>, Vec<Vec<String>>, 位置, Vec<Token>, Vec<String>)>,
) {
    for s in stmts {
        match s {
            AstStmt::Fn { line, col, len, head, body, .. } => {
                let h: Vec<Token> =
                    head.iter().cloned().filter(|t| *t != Token::Sym(":")).collect();
                if let Some((参数能力, 形参)) = 泛型语句头(&h) {
                    if let Some(n) = stmt_fn_name(head) {
                        let mut flat = Vec::new();
                        flatten_stmts(body, true, &mut flat);
                        let mut 体: Vec<Token> = Vec::new();
                        for (_, t, _) in flat {
                            体.extend(t.iter().cloned());
                        }
                        let 参数 = 参数能力.iter().map(|(n, _)| n.clone()).collect();
                        let 能力 = 参数能力.iter().map(|(_, c)| c.clone()).collect();
                        let 形参名 = 形参.iter().map(|(n, _)| n.clone()).collect();
                        out.insert(n, (参数, 能力, 位置{ 行: *line, 列: *col, 长: *len }, 体, 形参名));
                    }
                }
                walk_generic_stmt_fns(body, out);
            }
            AstStmt::Struct { body, .. } => walk_generic_stmt_fns(body, out),
            _ => {}
        }
    }
}

//nt: 扫和类型变体名
fn scan_variants(ast: &AstProgram) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for b in &ast.blocks {
        let mut flat: Vec<(位置, &TokStream, bool)> = Vec::new();
        flatten_stmts(&b.stmts, false, &mut flat);
        for (_line, toks, _nested) in flat {
                if !is_add_type(toks) {
                    continue;
                }
                if let Some(eq) = toks.iter().position(|t| *t == Token::Sym("=")) {
                    //nt: 积类型
                    if matches!(toks.get(eq + 1), Some(Token::Sym("("))) {
                        continue;
                    }
                    let mut i = eq + 1;
                    while i + 1 < toks.len() {
                        if let Token::Ident(n) = &toks[i] {
                            if matches!(toks.get(i + 1), Some(Token::Sym("("))) {
                                out.insert(n.clone());
                            }
                        }
                        i += 1;
                    }
                }
                    }
    }
    out
}

//nt: 登记 case 绑定
fn collect_case_bindings(b: &AstBlock, head: &[Token], cx: &mut Ctx) {
    if !matches!(head.first(), Some(Token::Kw("case")))
        || !matches!(head.get(2), Some(Token::Sym("(")))
    {
        return;
    }
    let mut d = 0i32;
    let mut end = None;
    for (k, tk) in head.iter().enumerate().skip(2) {
        match tk {
            Token::Sym("(") => d += 1,
            Token::Sym(")") => {
                d -= 1;
                if d == 0 {
                    end = Some(k);
                    break;
                }
            }
            _ => {}
        }
    }
    if let Some(end) = end {
        for a in split_commas(&head[3..end]) {
            if let Some(Token::Ident(n)) = a.first() {
                cx.err(
                    位置{ 行: b.line, 列: b.col, 长: b.len },
                    CheckCode::E201,
                    &DiagParts::full(
                        format!("第 {} 行", b.line),
                        "case 绑定",
                        format!("解构绑定「{}」没加引号——自定义名一律带引号（块名除外）", n),
                        "",
                        format!("写成 `case 变体(\"{}\")：`；体内读取写 val \"{}\"", n, n),
                    ),
                    Tone::Fix,
                );
            }
            if let Some(Token::Str(n)) = a.first() {
                if !cx.has_local(&b.name, n) {
                    cx.add_local(LocalInfo {
                        block: b.name.clone(),
                        name: n.clone(),
                        ty: UNKNOWN.to_string(),
                        键: std::rc::Rc::from(""),
                                    class: StorageClass::Tmp,
                    });
                    //nt: 绑定名也显式
                }
            }
        }
    }
}

fn is_add_type(toks: &[Token]) -> bool {
    matches!(toks.first(), Some(Token::Kw("add"))) && matches!(toks.get(1), Some(Token::Kw("type")))
}

fn parse_type_def(toks: &[Token]) -> Option<(String, Vec<(String, String)>)> {
    let name = match toks.get(2) {
        Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
        _ => return None,
    };
    //nt: 积类型
    let eq = toks.iter().position(|t| *t == Token::Sym("="))?;
    if !matches!(toks.get(eq + 1), Some(Token::Sym("("))) {
        return None;
    }
    let open = eq + 1;
    let mut depth = 0i32;
    let mut end = None;
    for (k, t) in toks.iter().enumerate().skip(open) {
        match t {
            Token::Sym("(") => depth += 1,
            Token::Sym(")") => {
                depth -= 1;
                if depth == 0 {
                    end = Some(k);
                    break;
                }
            }
            _ => {}
        }
    }
    let end = end?;
    let mut fields = Vec::new();
    for a in split_commas(&toks[open + 1..end]) {
        let fname = match a.first() {
            Some(Token::Ident(n)) | Some(Token::Str(n)) => n.clone(),
            _ => return None,
        };
        //nt: 完整类型序
        if a.len() < 2 {
            return None;
        }
        let mut fty = String::new();
        let mut depth = 0i32;
        for tk in &a[1..] {
            match tk {
                Token::Sym("(") => {
                    depth += 1;
                    fty.push('(');
                }
                Token::Sym(")") => {
                    depth -= 1;
                    fty.push(')');
                }
                Token::Sym(",") if depth > 0 => fty.push(','),
                Token::Sym("*") => fty.push('*'),
                Token::Kw(t) => fty.push_str(t),
                Token::Ident(n) => fty.push_str(n),
                Token::Str(n) => fty.push_str(n),
                _ => {}
            }
        }
        fields.push((fname, fty));
    }
    Some((name, fields))
}

//nt: 环检测
fn check_type_cycles(types: &TypeTable, cx: &mut Ctx) {
    for (name, (tline, fields)) in types {
        //nt: 直接自指
        for (fname, fty) in fields {
            if !fty.ends_with('*') && fty == name.as_str() {
                cx.err(
                    *tline,
                    CheckCode::E203,
                    &DiagParts::full(
                        "类型定义",
                        "写字段",
                        format!("裸环：字段「{}」的类型回到自己", fname),
                        "",
                        format!("写 {}*（把手）就不成环", name),
                    ),
                    Tone::Fix,
                );
            }
        }
        //nt: 互指环
        if reaches_self(types, name) {
            cx.err(
                *tline,
                CheckCode::E203,
                &DiagParts::full(
                    "类型定义",
                    "写字段",
                    format!("互指成环：{} 沿值字段又能回到自己", name),
                    "",
                    "把回环的那一端改成把手（加 *）",
                ),
                Tone::Fix,
            );
        }
    }
}

//nt: 从起点查可达
fn reaches_self(types: &TypeTable, start: &str) -> bool {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut stack: Vec<String> = Vec::new();
    if let Some((_, fs)) = types.get(start) {
        for (_, t) in fs {
            if !t.ends_with('*') {
                stack.push(t.clone());
            }
        }
    }
    while let Some(n) = stack.pop() {
        if n == start {
            return true;
        }
        if !seen.insert(n.clone()) {
            continue;
        }
        if let Some((_, fs)) = types.get(&n) {
            for (_, t) in fs {
                if !t.ends_with('*') {
                    stack.push(t.clone());
                }
            }
        }
    }
    false
}


//nt: 登记 add fn
fn collect_fn(b: &AstBlock, 处: 位置, toks: &[Token], cx: &mut Ctx) {
    let _line = 处.行;
    //nt: add hide f
    let name_at = if matches!(toks.get(1), Some(Token::Kw("hide"))) { 3 } else { 2 };
    register_fn(b, 处, toks, name_at, false, false, cx);
}

//nt: 登记 fn 重绑定行
fn collect_rebind_fn(b: &AstBlock, 处: 位置, toks: &[Token], cx: &mut Ctx) {
    let _line = 处.行;
    register_fn(b, 处, toks, 1, false, true, cx);
}

//nt: val "名" =
fn collect_val_rebind(b: &AstBlock, 处: 位置, toks: &[Token], cx: &mut Ctx) {
    let line = 处.行;
    let name = match toks.get(1) {
        Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
        _ => return,
    };
    let key = format!("{}|{}", b.name, name);
    let eq = match toks.iter().position(|t| *t == Token::Sym("=")) {
        Some(e) => e,
        None => return,
    };
    let rhs = &toks[eq + 1..];
    let is_delta = match (rhs.first(), rhs.get(1)) {
        (Some(Token::Kw("val")), Some(Token::Str(n) | Token::Ident(n))) => {
            n == &name && matches!(rhs.get(2), Some(Token::Sym("(")))
        }
        _ => false,
    };
    if is_delta && !cx.val_has_value.contains(key.as_str()) {
        cx.err(
            处,
            CheckCode::E201,
            &DiagParts::full(
                format!("第 {} 行", line),
                "改值",
                format!("取不到「{}」的当前值——先给它一个值", name),
                "",
                "delta 是「取当前值再改」：先 `add val \"名\" 类型 = 初值`，或先用 `val \"名\" = 值` 给过值",
            ),
            Tone::Fix,
        );
        return;
    }
    cx.val_has_value.insert(std::rc::Rc::<str>::from(key.as_str()));
}

//nt: add hide f
fn check_hide_decls(ast: &AstProgram, cx: &mut Ctx) {
    for b in &ast.blocks {
        let is_pub = b.attrs.iter().any(|a| a == "pub");
        let mut flat: Vec<(位置, &TokStream, bool)> = Vec::new();
        flatten_stmts(&b.stmts, false, &mut flat);
        for (处, toks, _nested) in flat {
            let line = 处.行;
            if !matches!(toks.first(), Some(Token::Kw("add")))
                || !matches!(toks.get(1), Some(Token::Kw("hide")))
            {
                continue;
            }
            let cat = match toks.get(2) {
                Some(Token::Kw("fn")) => "fn",
                Some(Token::Kw("type")) => "type",
                _ => {
                    cx.err(
                        处,
                        CheckCode::E203,
                        &DiagParts::full(
                            format!("第 {} 行", line),
                            "hide 可见性",
                            "hide 只能对 fn / type 写（`add hide fn \"名\"` / `add hide type \"名\"`）"
                                .to_string(),
                            "",
                            "val 本就在公开面里，不许单独藏",
                        ),
                        Tone::Fix,
                    );
                    continue;
                }
            };
            if !is_pub {
                cx.err(
                    处,
                    CheckCode::E203,
                    &DiagParts::full(
                        format!("第 {} 行", line),
                        "hide 可见性",
                        format!("块「{}」不是 pub 块，不许 hide", b.name),
                        "",
                        "hide 是给 pub 块开窗又拉帘用的；普通块本来就不对外",
                    ),
                    Tone::Fix,
                );
                continue;
            }
            if !matches!(toks.get(3), Some(Token::Str(_)) | Some(Token::Ident(_))) {
                cx.err(
                    处,
                    CheckCode::E203,
                    &DiagParts::full(
                        format!("第 {} 行", line),
                        "hide 可见性",
                        format!(
                            "`add hide {}` 后应接{}名",
                            cat,
                            if cat == "fn" { "fn" } else { "类型" }
                        ),
                        "",
                        "写成 `add hide fn \"名\"` / `add hide type \"名\"`",
                    ),
                    Tone::Fix,
                );
            }
        }
    }
}

//nt: 登记 add outs fn
fn collect_outs_fn(b: &AstBlock, 处: 位置, toks: &[Token], cx: &mut Ctx) {
    let _line = 处.行;
    register_fn(b, 处, toks, 3, true, false, cx);
}

//nt: 登记一个 fn 签名
//nt: rebind = t
fn register_fn(
    b: &AstBlock,
    处: 位置,
    toks: &[Token],
    name_at: usize,
    is_outs: bool,
    rebind: bool,
    cx: &mut Ctx,
) {
    let line = 处.行;
    let name = match toks.get(name_at) {
        Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
        _ => return,
    };
//nt: 内置名保护
    if BUILTINS.contains(&name.as_str()) {
        cx.err(
            处,
            CheckCode::E201,
            &DiagParts::full(
                format!("第 {} 行", line),
                "建 fn",
                format!("「{}」是内置函数名，不能拿来当 fn 名", name),
                "",
                "换个名字（内置名开箱即用，不允许被覆盖）",
            ),
            Tone::Suggest,
        );
        return;
    }
    //nt: 记定义行
    cx.fn_decl_line
        .entry(format!("{}|{}", b.name, name))
        .or_insert(line);
    if cx.fns.contains_key(&format!("{}|{}", b.name, name)) && !rebind {
        cx.err(
            处,
            CheckCode::E201,
            &DiagParts::full(
                format!("第 {} 行", line),
                "建 fn",
                format!("fn「{}」已经建过", name),
                "",
                "换个名字（同一个块里 fn 不许重名；补体/重绑定写 `fn \"名\" (参) 返 = 体`）",
            ),
            Tone::Fix,
        );
        return;
    }
    let (params, after) = match parse_params(toks) {
        Ok(v) => v,
        Err(_) => return,
    };
//nt: 返回类型
    let ret = match parse_type_ref(toks, after) {
        Ok((t, _)) => t,
        Err(_) => return,
    };
    //nt: 外部函数签名只收标量
    if is_outs {
        for (pname, pty) in &params {
            if !matches!(pty.as_str(), "int" | "float" | "bool") {
                cx.err(
                    处,
                    CheckCode::E202,
                    &DiagParts::full(
                        format!("第 {} 行", line),
                        "声明外部函数",
                        format!("参数「{}」是 {}——外部函数只收标量", pname, pty),
                        "",
                        "字符串进出写 to_c_str / from_c_str 显式转换",
                    )
                    .预期("外部函数的参数只能是 int / float / bool 标量")
                .详解("字符串进出 C 必须显式转换：进去写 to_c_str(串)，出来写 from_c_str(指针)——这是安全边界，不能让指针悄悄穿过。"),
                    Tone::Fix,
                );
                return;
            }
        }
        if !matches!(ret.as_str(), "int" | "float" | "bool") {
            cx.err(
                处,
                CheckCode::E202,
                &DiagParts::full(
                    format!("第 {} 行", line),
                    "声明外部函数",
                    format!("返回类型 {}——外部函数只收标量", ret),
                    "",
                    "字符串进出写 to_c_str / from_c_str 显式转换",
                )
                .预期("外部函数的返回只能是 int / float / bool 标量")
                .详解("字符串进出 C 必须显式转换：进去写 to_c_str(串)，出来写 from_c_str(指针)。"),
                Tone::Fix,
            );
            return;
        }
    }
    let key = format!("{}|{}", b.name, name);
    cx.fns.insert(
        key.clone(),
        FnSig {
            params: params.iter().map(|(_, t)| t.clone()).collect(),
            ret,
            is_outs,
        },
    );
    //nt: 有体 / 无体
    if toks.iter().any(|t| matches!(t, Token::Sym("="))) {
        cx.fn_has_body.insert(key.clone());
    }
    //nt: 跨模块调用核对
    if !b.module.is_empty() {
        if let Some(sig) = cx.fns.get(&key) {
            cx.fns_mod
                .insert(format!("{}|{}|{}", b.module, b.name, name), sig.clone());
        }
    }
    //nt: 普通 fn 的形参名
    //nt: 形参归fn不撞块级
    if !is_outs {
        for (pname, pty) in &params {
            let 局部块 = format!("{}#{}", b.name, name);
            let 块键 = format!("{}|{}", b.name, pname);
            if cx.local_idx.contains_key(块键.as_str()) {
                cx.err(
                    处,
                    CheckCode::E201,
                    &DiagParts::full(
                        format!("第 {} 行", 处.行),
                        "fn 形参",
                        format!("形参「{}」与块级 val 重名", pname),
                        "fn 局部与形参不许和块级 val 同名——名字空间分开，歧义在编译期消灭",
                        "给形参换个名字",
                    ),
                    Tone::Fix,
                );
            }
            cx.add_local(LocalInfo {
                block: 局部块,
                name: pname.clone(),
                ty: pty.clone(),
                键: std::rc::Rc::from(""),
                                    class: StorageClass::Tmp,
            });
        }
    }
}

//nt: 解析类型引用
fn parse_type_ref(toks: &[Token], at: usize) -> Result<(String, usize), String> {
    match toks.get(at) {
        Some(Token::Kw(t)) => {
            let 基 = (*t).to_string();
            if matches!(基.as_str(), "list" | "que" | "stack" | "map")
                && matches!(toks.get(at + 1), Some(Token::Sym("(")))
            {
                let mut 深 = 0i32;
                let mut 尽 = None;
                for (k, tk) in toks.iter().enumerate().skip(at + 1) {
                    match tk {
                        Token::Sym("(") => 深 += 1,
                        Token::Sym(")") => {
                            深 -= 1;
                            if 深 == 0 {
                                尽 = Some(k);
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                let 尽 = 尽.ok_or_else(|| format!("容器类型「{}」的括号没闭合", 基))?;
                let mut 实参: Vec<String> = Vec::new();
                for 段 in split_commas(&toks[at + 2..尽]) {
                    let (ty, _) = parse_type_ref(&段, 0)?;
                    实参.push(ty);
                }
                Ok((format!("{}({})", 基, 实参.join(", ")), 尽 + 1))
            } else if 基 == "fn" && matches!(toks.get(at + 1), Some(Token::Sym("("))) {
                let mut 深 = 0i32;
                let mut 尽 = None;
                for (k, tk) in toks.iter().enumerate().skip(at + 1) {
                    match tk {
                        Token::Sym("(") => 深 += 1,
                        Token::Sym(")") => {
                            深 -= 1;
                            if 深 == 0 {
                                尽 = Some(k);
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                let 尽 = 尽.ok_or_else(|| "函数类型「fn」的括号没闭合".to_string())?;
                let mut 实参: Vec<String> = Vec::new();
                for 段 in split_commas(&toks[at + 2..尽]) {
                    let (ty, _) = parse_type_ref(&段, 0)?;
                    实参.push(ty);
                }
                let (ret, 止) = parse_type_ref(toks, 尽 + 1)?;
                Ok((format!("fn({}) {}", 实参.join(", "), ret), 止))
            } else {
                Ok((基, at + 1))
            }
        }
        Some(Token::Ident(n)) => {
            if matches!(toks.get(at + 1), Some(Token::Sym("*"))) {
                Ok((format!("{}*", n), at + 2))
            } else {
                Ok((n.clone(), at + 1))
            }
        }
        Some(Token::HashColon) | Some(Token::HashHashColon) => {
            let cross_mod = matches!(toks.get(at), Some(Token::HashHashColon));
            let colon_at = if cross_mod {
                let mut j = at + 1;
                while let Some(tok) = toks.get(j) {
                    match tok {
                        Token::Ident(_) | Token::Str(_) => {
                            j += 1;
                            if matches!(toks.get(j), Some(Token::Sym("."))) {
                                j += 1;
                            } else {
                                break;
                            }
                        }
                        _ => break,
                    }
                }
                if !matches!(toks.get(j), Some(Token::HashColon)) {
                    return Err("`##:` 后应接模块名再接 `#:块`——形如 `##:前端.拆词 #:分句`".into());
                }
                j
            } else {
                at
            };
            if !matches!(toks.get(colon_at + 2), Some(Token::Kw("type"))) {
                return Err("跨块取类型要写 `#:块 type \"名\"`".into());
            }
            let tname = match toks.get(colon_at + 3) {
                Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
                _ => return Err("`#:块 type` 后应接类型名".into()),
            };
            if matches!(toks.get(colon_at + 4), Some(Token::Sym("*"))) {
                Ok((format!("{}*", tname), colon_at + 5))
            } else {
                Ok((tname, colon_at + 4))
            }
        }
        _ => Err("这里应当是类型（内置类型名 / 自定义类型名 / `#:块 type \"名\"`）".into()),
    }
}

fn parse_params(toks: &[Token]) -> Result<(Vec<(String, String)>, usize), String> {
    let open = toks
        .iter()
        .position(|t| *t == Token::Sym("("))
        .ok_or("缺参数表")?;
    let mut depth = 0i32;
    let mut end = None;
    for (k, t) in toks.iter().enumerate().skip(open) {
        match t {
            Token::Sym("(") => depth += 1,
            Token::Sym(")") => {
                depth -= 1;
                if depth == 0 {
                    end = Some(k);
                    break;
                }
            }
            _ => {}
        }
    }
    let end = end.ok_or("参数表没闭合")?;
    let mut out = Vec::new();
    for a in split_commas(&toks[open + 1..end]) {
        let name = match a.first() {
            Some(Token::Ident(n)) | Some(Token::Str(n)) => n.clone(),
            _ => return Err("参数缺名字".into()),
        };
        let (ty, _) = match a.get(1) {
            Some(_) => parse_type_ref(&a, 1)?,
            None => return Err(format!("参数「{}」缺类型", name)),
        };
        out.push((name, ty));
    }
    Ok((out, end + 1))
}

fn split_commas(inner: &[Token]) -> Vec<Vec<Token>> {
    let mut out = Vec::new();
    let mut cur = Vec::new();
    let mut depth = 0i32;
    for t in inner {
        match t {
            Token::Sym("(") => depth += 1,
            Token::Sym(")") => depth -= 1,
            Token::Sym("[") => depth += 1,
            Token::Sym("]") => depth -= 1,
            Token::Sym(",") if depth == 0 => {
                out.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(t.clone());
    }
    if !cur.is_empty() || !out.is_empty() {
        out.push(cur);
    }
    out.into_iter().filter(|a| !a.is_empty()).collect()
}

//nt: 取调用点的实参
fn call_args(toks: &[Token], open: usize) -> Vec<Vec<Token>> {
    let mut depth = 0i32;
    let mut inner: Vec<Token> = Vec::new();
    let mut i = open;
    while i < toks.len() {
        match &toks[i] {
            Token::Sym("(") => depth += 1,
            Token::Sym(")") => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        if depth >= 1 {
            inner.push(toks[i].clone());
        }
        i += 1;
    }
    if matches!(inner.first(), Some(Token::Sym("("))) {
        inner.remove(0);
    }
    split_commas(&inner)
}

//nt: 从 `开` 处的
fn 配对括号(toks: &[Token], 开: usize) -> Option<usize> {
    let mut 深 = 0i32;
    for (k, t) in toks.iter().enumerate().skip(开) {
        match t {
            Token::Sym("(") => 深 += 1,
            Token::Sym(")") => {
                深 -= 1;
                if 深 == 0 {
                    return Some(k);
                }
            }
            _ => {}
        }
    }
    None
}

//nt: 定义点核对
//nt: 核对定义点
fn check_caps(cx: &mut Ctx) {
    let 清单: Vec<(String, Vec<String>, Vec<Vec<String>>, 位置, Vec<Token>, Vec<String>)> = cx
        .泛型fn能力
        .iter()
        .map(|(k, v)| (k.clone(), v.0.clone(), v.1.clone(), v.2, v.3.clone(), v.4.clone()))
        .collect();
    for (名, 参数, 能力, 处, 体, 形参名) in 清单 {
        let 行 = 处.行;
        let 声明: Vec<String> = 能力.iter().flatten().cloned().collect();
        //nt: 泛型 fn 的形参一
        let 泛型型参数: Vec<String> = 形参名
            .iter()
            .filter(|_| true)
            .cloned()
            .collect();
        let mut 可用 = 声明.clone();
        if 声明.iter().any(|x| x == "<") {
            可用.extend([">", "<=", ">="].iter().map(|x| x.to_string()));
        }
        if 声明.iter().any(|x| x == "==") {
            可用.push("!=".to_string());
        }
        for (k, t) in 体.iter().enumerate() {
            let op = match t {
                Token::Sym(o) => o.to_string(),
                _ => continue,
            };
            if !["<", "<=", ">", ">=", "==", "!=", "+", "-", "*", "/", "%"]
                .contains(&op.as_str())
            {
                continue;
            }
            //nt: 邻域内出现「类型参数
            let 邻域: Vec<&Token> = 体.iter().skip(k.saturating_sub(3)).take(6).collect();
            let 作用在参数 = 邻域.iter().any(|t| match t {
                //nt: 裸名既指类型参数名
                Token::Ident(n) => 参数.contains(n) || 形参名.contains(n),
                _ => false,
            }) || 体
                .iter()
                .skip(k.saturating_sub(4))
                .take(8)
                .any(|t| match t {
                    Token::Str(n) => 泛型型参数.contains(n),
                    _ => false,
                });
            if 作用在参数 && !可用.contains(&op) {
                cx.err(
                    处,
                    CheckCode::E201,
                    &DiagParts::full(
                        format!("第 {} 行", 行),
                        "能力约束",
                        format!(
                            "泛型 fn「{}」的体里对类型参数用了「{}」，参数表没声明这个能力",
                            名, op
                        ),
                        "能力集 = 运算符集，声明了才能用",
                        format!(
                            "在类型参数上标注（当前已声明：{}）",
                            if 可用.is_empty() {
                                "无".to_string()
                            } else {
                                可用.join(" ")
                            }
                        ),
                    ),
                    Tone::Fix,
                );
            }
        }
    }
}

//nt: 调用点核对
fn check_call(b: &AstBlock, 处: 位置, name: &str, args: &[Vec<Token>], cx: &mut Ctx) {
    let line = 处.行;
    //nt: 键只拼一次，下面两处
    //nt: 语句体 fn 的体内
    let mut sig_key = format!("{}|{}", b.name, name);
    if !cx.fns.contains_key(&sig_key) {
        let alt = format!("{}|{}", 原块(&b.name), name);
        if cx.fns.contains_key(&alt) {
            sig_key = alt;
        }
    }
    let sig = match cx.fns.get(&sig_key) {
        Some(s) => s.clone(),
        None => {
            //nt: 泛型 fn
            if cx.泛型fn能力.contains_key(name) {
                return;
            }
            cx.err(
                处,
                CheckCode::E201,
                &DiagParts::full(
                    format!("第 {} 行", line),
                    "调函数",
                    format!("fn「{}」还没建过", name),
                    "",
                    "先用 add fn \"名\" (参数) 返回类型 = 表达式 建",
                ),
                Tone::Fix,
            );
            //nt: 占位签名
            cx.fns.insert(
                sig_key,
                FnSig { params: Vec::new(), ret: UNKNOWN.to_string(), is_outs: false },
            );
            return;
        }
    };
//nt: 「先建后用」
    //nt: add type
    if let Some(&decl) = cx.fn_decl_line.get(&sig_key) {
        if decl > line {
            cx.err(
                处,
                CheckCode::E201,
                &DiagParts::full(
                    format!("第 {} 行", line),
                    "调函数",
                    format!("fn「{}」用在它被建出来之前——它到第 {} 行才建", name, decl),
                    "",
                    format!("把建「{}」的那行 `add fn` 移到这一行之前", name),
                )
                .预期("函数要先建出来，再调它")
                .详解(
                    "Anle 是「先建后用」：调用要排在 `add fn` 之后。`add type`（类型）不受这条约束，可以先用后写。",
                ),
                Tone::Fix,
            );
        }
    }
    //nt: 纯声明
    if !sig.is_outs && !cx.fn_has_body.contains(&sig_key) {
        cx.err(
            处,
            CheckCode::E201,
            &DiagParts::full(
                format!("第 {} 行", line),
                "调函数",
                format!("fn「{}」只有声明，还没有体", name),
                "",
                format!("用 `fn \"{}\" (参数) 返回类型 = 表达式` 给它补体", name),
            )
            .预期("声明过的 fn 要补体才能调用"),
            Tone::Fix,
        );
        return;
    }
    check_sig_args(b, 处, name, &sig, args, cx);
}

//nt: 按签名核对实参
fn check_sig_args(
    b: &AstBlock,
    处: 位置,
    name: &str,
    sig: &FnSig,
    args: &[Vec<Token>],
    cx: &mut Ctx,
) {
    let line = 处.行;
    if sig.params.len() != args.len() {
        cx.err(
            处,
            CheckCode::E202,
            &DiagParts::full(
                format!("第 {} 行", line),
                "调函数",
                format!("fn「{}」要 {} 个参数，给了 {} 个", name, sig.params.len(), args.len()),
                "",
                "按定义给够参数",
            )
            .预期(format!("按定义给够 {} 个参数", sig.params.len()))
                .详解("参数个数由 add fn 的声明说了算；给多了给少了都拦——调用和定义必须对得上。"),
            Tone::Fix,
        );
        return;
    }
    for (k, a) in args.iter().enumerate() {
        //nt: 裸名实参先按声明类型
        let got = infer_named(a, &cx.locals, &cx.local_idx, &b.name, &cx.types)
            .or_else(|| infer(a, &cx.locals, &cx.local_idx, &b.name, &cx.types).map(|s| s.to_string()));
        if let Some(got) = got {
            let want = &sig.params[k];
            //nt: 「未知」
            if got != UNKNOWN && !类型相容(want, &got) {
                cx.err(
                    处,
                    CheckCode::E202,
                    &DiagParts::full(
                        format!("第 {} 行", line),
                        "调函数",
                        format!("fn「{}」第 {} 个参数要 {}，给了 {}", name, k + 1, want, got),
                        "",
                        format!("改成 {} 的值", want),
                    )
                    .预期(format!("第 {} 个参数应当是 {} 型", k + 1, want))
                .详解("实参类型要和形参声明一致；类型不合时用 int(值) / str(值) / float(值) / bool(值) 显式转换再传。"),
                    Tone::Fix,
                );
            }
        }
    }
}

//nt: 容器类型相容
//nt: 两侧都写了就必须相同
fn 类型相容(want: &str, got: &str) -> bool {
    if want == got {
        return true;
    }
    let 基 = |s: &str| s.split('(').next().unwrap_or(s).trim().to_string();
    let wb = 基(want);
    let gb = 基(got);
    wb == gb
        && matches!(wb.as_str(), "list" | "que" | "stack" | "map")
        && !(want.contains('(') && got.contains('('))
}

//nt: 右边是「直接调用」
fn call_ret(
    toks: &[Token],
    block: &str,
    fns: &BTreeMap<String, FnSig>,
    locals: &[LocalInfo],
) -> Option<String> {
    //nt: 同块直调
    if let (Some(Token::Ident(n)), Some(Token::Sym("("))) = (toks.first(), toks.get(1)) {
        return fns.get(&format!("{}|{}", block, n)).map(|s| s.ret.clone());
    }
    //nt: 同块调用
    if matches!(toks.first(), Some(Token::Kw("call"))) && matches!(toks.get(1), Some(Token::Kw("fn"))) {
        let n = match toks.get(2) {
            Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
            _ => return None,
        };
        return fns.get(&format!("{}|{}", block, n)).map(|s| s.ret.clone());
    }
    //nt: take #
    if matches!(toks.first(), Some(Token::Kw("take")))
        && matches!(toks.get(1), Some(Token::HashColon))
        && matches!(toks.get(3), Some(Token::Kw("val")))
    {
        let blk = match toks.get(2) {
            Some(Token::Ident(n)) => n.clone(),
            Some(Token::Kw(k)) => (*k).to_string(),
            _ => return None,
        };
        let vname = match toks.get(4) {
            Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
            _ => return None,
        };
        return locals
            .iter()
            .find(|l| l.block == blk && l.name == vname)
            .map(|l| l.ty.clone());
    }
    //nt: 跨块取 val
    if matches!(toks.first(), Some(Token::Kw("call")))
        && matches!(toks.get(1), Some(Token::HashColon))
        && matches!(toks.get(3), Some(Token::Kw("val")))
    {
        let blk = match toks.get(2) {
            Some(Token::Ident(n)) => n.clone(),
            Some(Token::Kw(k)) => (*k).to_string(),
            _ => return None,
        };
        let vname = match toks.get(4) {
            Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
            _ => return None,
        };
        return locals
            .iter()
            .find(|l| l.block == blk && l.name == vname)
            .map(|l| l.ty.clone());
    }
    //nt: 跨块调用
    let base = if matches!(toks.first(), Some(Token::Kw("call"))) { 1 } else { 0 };
    if matches!(toks.get(base), Some(Token::HashColon)) {
        let blk = match toks.get(base + 1) {
            Some(Token::Ident(n)) => n.clone(),
            Some(Token::Kw(k)) => (*k).to_string(),
            _ => return None,
        };
        let fname = match toks.get(base + 3) {
            Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
            _ => return None,
        };
        return fns.get(&format!("{}|{}", blk, fname)).map(|s| s.ret.clone());
    }
    None
}

//nt: 遍 2 的调用核对
//nt: 遍 2 附带
fn pass2_match_exhaustive(ast: &AstProgram, cx: &mut Ctx) {
    //nt: 类型名 → 变体名表
    let mut ty_variants: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut var_owner: BTreeMap<String, String> = BTreeMap::new();
    for b in &ast.blocks {
        cx.cur_file = b.file.clone();
    let mut flat: Vec<(位置, &TokStream, bool)> = Vec::new();
    flatten_stmts(&b.stmts, false, &mut flat);
        for (_line, toks, _nested) in flat {
                if !is_add_type(toks) {
                    continue;
                }
                if let Some(eq) = toks.iter().position(|t| *t == Token::Sym("=")) {
                    let tname = match toks.get(2) {
                        Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
                        _ => continue,
                    };
                    let mut vs = Vec::new();
                    let mut i = eq + 1;
                    while i + 1 < toks.len() {
                        if let Token::Ident(n) = &toks[i] {
                            if matches!(toks.get(i + 1), Some(Token::Sym("("))) {
                                vs.push(n.clone());
                            }
                        }
                        i += 1;
                    }
                    if !vs.is_empty() {
                        for v in &vs {
                            var_owner.insert(v.clone(), tname.clone());
                        }
                        ty_variants.insert(tname, vs);
                    }
                }
                    }
    }
    //nt: 不提前退
    for b in &ast.blocks {
        cx.cur_file = b.file.clone();
        check_match_exhaustive_block(b, &ty_variants, &var_owner, cx);
        check_expr_matches_block(b, &ty_variants, &var_owner, cx);
    }
}

//nt: 递归收集块内的 ma
//nt: 只收 match 头
fn collect_match_structs<'a>(stmts: &'a [AstStmt], out: &mut Vec<&'a AstStmt>) {
    for s in stmts {
        if let AstStmt::Struct { head, body, .. } = s {
            if matches!(head.first(), Some(Token::Kw("match"))) {
                out.push(s);
            }
            collect_match_structs(body, out);
        }
    }
}

//nt: 块内「裸用合法」的绑
//nt: 与 IR 的 bar
//nt: 两边豁免集不一致就会
fn collect_bare_ok_names(stmts: &[AstStmt], out: &mut BTreeSet<String>) {
    for s in stmts {
        if let AstStmt::Struct { head, body, .. } = s {
            match head.first() {
                Some(Token::Kw("for")) => {
                    if let Some(Token::Ident(n)) | Some(Token::Str(n)) = head.get(1) {
                        out.insert((*n).clone());
                    }
                }
                Some(Token::Kw("case")) => {
                    //nt: case 变体
                    if matches!(head.get(2), Some(Token::Sym("("))) {
                        for t in head.iter().skip(3) {
                            match t {
                                Token::Ident(n) | Token::Str(n) => {
                                    out.insert((*n).clone());
                                }
                                Token::Sym(")") => break,
                                _ => {}
                            }
                        }
                    }
                }
                _ => {}
            }
            collect_bare_ok_names(body, out);
        }
    }
}

//nt: match 对象的类
//nt: 推断来源
//nt: params 匹配
fn match_object_ty(
    b: &AstBlock,
    obj: &[Token],
    bare_ok: &BTreeSet<String>,
    params: &BTreeMap<String, String>,
    var_owner: &BTreeMap<String, String>,
    处: 位置,
    cx: &mut Ctx,
) -> Option<String> {
    let line = 处.行;
    //nt: 剥掉 call 前缀
    let obj: &[Token] = if matches!(obj.first(), Some(Token::Kw("call")))
        && matches!(obj.get(1), Some(Token::Kw("val")))
    {
        &obj[1..]
    } else {
        obj
    };
    //nt: ① 显式取值
    if let [Token::Kw("val"), rest @ (Token::Str(_) | Token::Ident(_))] = obj {
        let n = match rest {
            Token::Str(n) | Token::Ident(n) => n,
            _ => unreachable!(),
        };
        let key = format!("{}|{}", b.name, n);
        return match cx.local_idx.get(key.as_str()) {
            Some(&i) => Some(cx.locals[i].ty.trim_end_matches('*').to_string()),
            None => {
                cx.err(
                    处,
                    CheckCode::E201,
                    &DiagParts::full(
                        format!("第 {} 行", line),
                        "match 判别",
                        format!("val「{}」还没建过", n),
                        "",
                        "先用 add val \"名\" 和类型 = … 建，再 match",
                    ),
                    Tone::Fix,
                );
                None
            }
        };
    }
    //nt: ② 显式调用
    if let [Token::Kw("call"), Token::Kw("fn"), rest @ (Token::Str(_) | Token::Ident(_)), Token::Sym("("), ..] = obj {
        let n = match rest {
            Token::Str(n) | Token::Ident(n) => n,
            _ => unreachable!(),
        };
        let key = format!("{}|{}", b.name, n);
        if let Some(sig) = cx.fns.get(&key) {
            return Some(sig.ret.trim_end_matches('*').to_string());
        }
        return None; // fn 没建过——pass2_calls 已报，这里不重复
    }
    //nt: ③ 变体构造
    if let [Token::Ident(n), Token::Sym("("), ..] = obj {
        if let Some(owner) = var_owner.get(n) {
            return Some(owner.clone());
        }
        return None; // 不是变体构造（内置调用/裸调用等）——由别的遍报
    }
    //nt: ④ 裸名
    if let Some(Token::Ident(n)) = obj.first() {
        if let Some(t) = params.get(n) {
            //nt: fn 参数
            return Some(t.trim_end_matches('*').to_string());
        }
        if !bare_ok.contains(n) {
            let key = format!("{}|{}", b.name, n);
            if cx.local_idx.contains_key(&*key) {
                cx.err(
                    处,
                    CheckCode::E201,
                    &DiagParts::full(
                        format!("第 {} 行", line),
                        "match 判别",
                        format!("match 对象里的「{}」是裸名——取 val 请写 val \"{}\"", n, n),
                        "",
                        "match 对象必须显式：写 val \"名\"，或直接给一个表达式（如 call fn \"名\"(…)）",
                    )
                    .预期("match 对象不留裸名别名（全面显式：fn 与 val 可重名，裸名消不了歧义）"),
                    Tone::Fix,
                );
                return None;
            }
            cx.err(
                处,
                CheckCode::E201,
                &DiagParts::full(
                    format!("第 {} 行", line),
                    "match 判别",
                    format!("「{}」还没建过", n),
                    "",
                    "先建再用：add val \"名\" 和类型 = …，match 对象写 val \"名\"",
                ),
                Tone::Fix,
            );
            return None;
        }
        //nt: 占位绑定
        let key = format!("{}|{}", b.name, n);
        return cx.local_idx.get(key.as_str()).map(|&i| cx.locals[i].ty.trim_end_matches('*').to_string());
    }
    //nt: ⑤ 其余表达式
    cx.err(
        处,
        CheckCode::E202,
        &DiagParts::full(
            format!("第 {} 行", line),
            "match 判别",
            "match 对象的类型推不出来",
            "",
            "先存带类型 val 再 match（add val \"名\" 和类型 = …），或换成一个能看出类型的表达式",
        )
        .预期("match 对象的类型必须编译期可推：val 声明类型 / fn 声明返回类型 / 变体构造"),
        Tone::Fix,
    );
    None
}

//nt: match 穷尽性
//nt: 对象形态
//nt: 裸名一律报错，不留「
fn check_match_exhaustive_block(
    b: &AstBlock,
    ty_variants: &BTreeMap<String, Vec<String>>,
    var_owner: &BTreeMap<String, String>,
    cx: &mut Ctx,
) {
    let mut found: Vec<&AstStmt> = Vec::new();
    collect_match_structs(&b.stmts, &mut found);
    if found.is_empty() {
        return;
    }
    let mut bare_ok: BTreeSet<String> = BTreeSet::new();
    collect_bare_ok_names(&b.stmts, &mut bare_ok);

    for s in found {
        let (head, body, line, col, len) = match s {
            AstStmt::Struct { head, body, line, col, len, .. } => (head, body, line, col, len),
            _ => continue,
        };
        //nt: 对象 = 去掉开头的
        let obj = if head.len() >= 2 { &head[1..head.len() - 1] } else { &[][..] };
        if obj.is_empty() {
            continue;
        }
        let Some(ty) = match_object_ty(b, obj, &bare_ok, &BTreeMap::new(), var_owner, 位置{ 行: *line, 列: *col, 长: *len }, cx) else {
            continue;
        };
        //nt: 推断出的类型不是和类
        let Some(all) = ty_variants.get(&ty) else {
            continue;
        };
        let mut covered: Vec<String> = Vec::new();
        let mut has_wild = false;
        for s2 in body {
            if let AstStmt::Struct { head: h2, .. } = s2 {
                if !matches!(h2.first(), Some(Token::Kw("case"))) {
                    continue;
                }
                match h2.get(1) {
                    Some(Token::Ident(n)) if n == "_" => has_wild = true,
                    Some(Token::Ident(n)) => covered.push(n.clone()),
                    _ => {}
                }
            }
        }
        if !has_wild {
            let missing: Vec<String> = all
                .iter()
                .filter(|v| !covered.contains(v))
                .map(|v| format!("「{}」", v))
                .collect();
            if !missing.is_empty() {
                cx.err(
                    位置{ 行: *line, 列: *col, 长: *len },
                    CheckCode::E204,
                    &DiagParts::full(
                        format!("第 {} 行", line),
                        "match 判别",
                        format!("没覆盖变体 {}", missing.join("、")),
                        "",
                        "补上缺的 case，或写 `case _:` 兜底",
                    ),
                    Tone::Fix,
                );
            }
        }
    }
}

//nt: 表达式形态 matc
//nt: fn 体内参数裸用合
fn check_expr_matches_block(
    b: &AstBlock,
    ty_variants: &BTreeMap<String, Vec<String>>,
    var_owner: &BTreeMap<String, String>,
    cx: &mut Ctx,
) {
    let mut bare_ok: BTreeSet<String> = BTreeSet::new();
    collect_bare_ok_names(&b.stmts, &mut bare_ok);
    let mut flat: Vec<(位置, &TokStream, bool)> = Vec::new();
    flatten_stmts(&b.stmts, false, &mut flat);
    for (处, toks, _nested) in flat {
            let _line = 处.行;
        //nt: fn 声明/补体行
        let mut params: BTreeMap<String, String> = BTreeMap::new();
        let mut scan: &[Token] = toks;
        if is_add_fn(toks) || matches!(toks.first(), Some(Token::Kw("fn"))) {
            if let Ok((ps, _after)) = parse_params(toks) {
                for (n, t) in ps {
                    params.insert(n, t);
                }
            }
            if let Some(eq) = toks.iter().position(|t| *t == Token::Sym("=")) {
                scan = &toks[eq + 1..];
            }
        }
        scan_expr_matches(b, 处, scan, &bare_ok, &params, ty_variants, var_owner, cx);
    }
}

//nt: 在表达式 token
//nt: 语句形态 match
fn scan_expr_matches(
    b: &AstBlock,
    处: 位置,
    toks: &[Token],
    bare_ok: &BTreeSet<String>,
    params: &BTreeMap<String, String>,
    ty_variants: &BTreeMap<String, Vec<String>>,
    var_owner: &BTreeMap<String, String>,
    cx: &mut Ctx,
) {
    let line = 处.行;
    let mut k = 0usize;
    while k < toks.len() {
        if !matches!(toks.get(k), Some(Token::Kw("match"))) {
            k += 1;
            continue;
        }
        //nt: 对象字面量
        let mut depth = 0i32;
        let mut brace = None;
        let mut j = k + 1;
        while j < toks.len() {
            match &toks[j] {
                Token::Sym("(") | Token::Sym("[") => depth += 1,
                Token::Sym(")") | Token::Sym("]") => depth -= 1,
                Token::Sym("{") if depth == 0 => {
                    brace = Some(j);
                    break;
                }
                _ => {}
            }
            j += 1;
        }
        let Some(brace) = brace else {
            k += 1;
            continue;
        };
        let obj: Vec<Token> = toks[k + 1..brace].to_vec();
        if obj.is_empty() {
            k += 1;
            continue;
        }
        //nt: 单行也要登记
        collect_inline_case_bindings(b, toks, brace + 1, cx);
        let (covered, has_wild, bad) = collect_case_coverage(toks, brace + 1);
        for n in &bad {
            cx.err(
                处,
                CheckCode::E204,
                &DiagParts::full(
                    format!("第 {} 行", line),
                    "match 判别",
                    format!("无字段变体的判别也要写括号——`case {}()`", n),
                    "",
                    "判别统一带括号：有字段写 `case 变体(绑定)`，无字段写 `case 变体()`，兜底写 `case _:`",
                ),
                Tone::Fix,
            );
        }
        if let Some(ty) = match_object_ty(b, &obj, bare_ok, params, var_owner, 处, cx) {
            if let Some(all) = ty_variants.get(&ty) {
                if !has_wild {
                    let missing: Vec<String> = all
                        .iter()
                        .filter(|v| !covered.contains(v))
                        .map(|v| format!("「{}」", v))
                        .collect();
                    if !missing.is_empty() {
                        cx.err(
                            处,
                            CheckCode::E204,
                            &DiagParts::full(
                                format!("第 {} 行", line),
                                "match 判别",
                                format!("没覆盖变体 {}", missing.join("、")),
                                "",
                                "补上缺的 case，或写 `case _:` 兜底",
                            ),
                            Tone::Fix,
                        );
                    }
                }
            }
        }
        k += 1; // 继续扫（嵌套 match 也要查）
    }
}

//nt: 从 `{` 之后收
//nt: 单行绑定登记
fn collect_inline_case_bindings(b: &AstBlock, toks: &[Token], from: usize, cx: &mut Ctx) {
    let mut k = from;
    while k < toks.len() {
        if matches!(toks.get(k), Some(Token::Kw("match"))) {
            //nt: 找到配对的 {
            let mut depth = 0i32;
            let mut brace = None;
            let mut j = k + 1;
            while j < toks.len() {
                match &toks[j] {
                    Token::Sym("(") | Token::Sym("[") => depth += 1,
                    Token::Sym(")") | Token::Sym("]") => depth -= 1,
                    Token::Sym("{") if depth == 0 => {
                        brace = Some(j);
                        break;
                    }
                    _ => {}
                }
                j += 1;
            }
            if let Some(brace) = brace {
                collect_inline_case_arms(b, &toks[brace + 1..], cx);
                k = brace;
            }
        }
        k += 1;
    }
}

fn collect_inline_case_arms(b: &AstBlock, toks: &[Token], cx: &mut Ctx) {
    let mut k = 0;
    while k < toks.len() {
        if !matches!(toks.get(k), Some(Token::Kw("case"))) {
            k += 1;
            continue;
        }
        let mut end = k;
        while end < toks.len() && !matches!(toks.get(end), Some(Token::Sym(":"))) {
            end += 1;
        }
        collect_case_bindings(b, &toks[k..end], cx);
        k = end + 1;
    }
}

fn collect_case_coverage(toks: &[Token], mut j: usize) -> (Vec<String>, bool, Vec<String>) {
    let mut covered = Vec::new();
    let mut has_wild = false;
    let mut bad = Vec::new();
    loop {
        if j >= toks.len() || matches!(&toks[j], Token::Sym("}")) {
            break;
        }
        if !matches!(&toks[j], Token::Kw("case")) {
            j += 1;
            continue;
        }
        j += 1;
        //nt: 模式：到顶层`:`
        let mut d = 0i32;
        let pat_start = j;
        while j < toks.len() {
            match &toks[j] {
                Token::Sym("(") => d += 1,
                Token::Sym(")") => d -= 1,
                Token::Sym(":") if d == 0 => break,
                _ => {}
            }
            j += 1;
        }
        let pat = &toks[pat_start..j];
        match pat.first() {
            Some(Token::Ident(n)) if n == "_" => has_wild = true,
            Some(Token::Ident(n)) => {
                covered.push(n.clone());
                if pat.len() == 1 {
                    bad.push(n.clone());
                }
            }
            _ => {}
        }
        if j < toks.len() {
            j += 1; // 跳过 `:`
        }
        //nt: 分支表达式
        let mut d3 = 0i32;
        while j < toks.len() {
            match &toks[j] {
                Token::Sym("(") | Token::Sym("[") => d3 += 1,
                Token::Sym(")") | Token::Sym("]") => d3 -= 1,
                Token::Sym("{") => d3 += 1,
                Token::Sym("}") if d3 == 0 => break,
                Token::Sym("}") => d3 -= 1,
                Token::Sym(",") if d3 == 0 => break,
                _ => {}
            }
            j += 1;
        }
        if matches!(toks.get(j), Some(Token::Sym(","))) {
            j += 1;
        }
    }
    (covered, has_wild, bad)
}

fn pass2_calls(ast: &AstProgram, cx: &mut Ctx) {
    for b in &ast.blocks {
        cx.cur_file = b.file.clone();
        calls_block(b, cx);
    }
}

fn calls_block(b: &AstBlock, cx: &mut Ctx) {
    calls_stmts(&b.stmts, b, cx);
}

//nt: 调用核对也**递归
fn calls_stmts(stmts: &[AstStmt], b: &AstBlock, cx: &mut Ctx) {
    for s in stmts {
        match s {
            AstStmt::Line { line, col, len, toks, .. } => {
                //nt: 公式补体行查伪块
                //nt: 形参已归伪块
                let fn名 = if is_add_fn(toks) || matches!(toks.first(), Some(Token::Kw("fn"))) {
                    //nt: 形参解析失败不查体
                    //nt: 泛型/能力型历史盲区
                    if is_add_fn(toks) && parse_params(toks).is_err() {
                        None
                    } else {
                        stmt_fn_name(toks)
                    }
                } else {
                    None
                };
                if let Some(n) = fn名 {
                    let pb = fn_scope_block(b, &n, &[]);
                    calls_expr(&pb, 位置{ 行: *line, 列: *col, 长: *len }, toks, cx);
                } else {
                    calls_expr(b, 位置{ 行: *line, 列: *col, 长: *len }, toks, cx);
                }
            }
            AstStmt::Struct { line, col, len, head, body, .. } => {
                calls_expr(b, 位置{ 行: *line, 列: *col, 长: *len }, head, cx);
                calls_stmts(body, b, cx);
            }
            AstStmt::Fn { head, body, .. } => {
                if let Some(n) = stmt_fn_name(head) {
                    let pb = fn_scope_block(b, &n, body);
                    calls_stmts(body, &pb, cx);
                }
            }
        }
    }
}

//nt: 取一行里的「表达式部
fn calls_expr(b: &AstBlock, 处: 位置, toks: &[Token], cx: &mut Ctx) {
    let line = 处.行;
    //nt: add type
    if is_add_type(toks) {
        return;
    }
    //nt: 写位置要显式
    if let Some(Token::Ident(n)) = toks.first() {
        let 是写位置 = matches!(toks.get(1), Some(Token::Sym(".")) | Some(Token::Sym("[")))
            && toks.iter().any(|t| *t == Token::Sym("="));
        if 是写位置 {
            let 写法 = if matches!(toks.get(1), Some(Token::Sym("."))) {
                format!("val \"{}\".字段 = 值", n)
            } else {
                format!("val \"{}\"[下标] = 值", n)
            };
            cx.err(
                处,
                CheckCode::E201,
                &DiagParts::full(
                    format!("第 {} 行", line),
                    "写位置",
                    format!("「{}」左边的裸名没加 val——写 `{}`", n, 写法),
                    "",
                    "写位置也要显式：字段写与下标写都写成 `val \"名\"…` 形态，不留裸名",
                )
                .预期("写位置与读位置同一套名字规则，只有一种写法"),
                Tone::Fix,
            );
        }
    }
    let expr: &[Token] = if is_add_val(toks) || is_add_fn(toks) || is_add_outs_fn(toks) {
        match toks.iter().position(|t| *t == Token::Sym("=")) {
            Some(eq) => &toks[eq + 1..],
            None => &[],
        }
    } else if matches!(toks.first(), Some(Token::Kw("val"))) {
        match toks.iter().position(|t| *t == Token::Sym("=")) {
            Some(eq) => &toks[eq + 1..],
            None => &[],
        }
    } else if matches!(toks.last(), Some(Token::Sym(":"))) && toks.len() >= 2 {
        &toks[1..toks.len() - 1]
    } else {
        toks
    };
    let mut i = 0usize;
    //nt: take 是动词
    for w in expr.windows(2) {
        if matches!(w[0], Token::Kw("call")) && matches!(w[1], Token::Kw("take")) {
            cx.err(
                处,
                CheckCode::E201,
                &DiagParts::full(
                    format!("第 {} 行", line),
                    "跨块取值",
                    "call 与 take 不得同时出现".to_string(),
                    "",
                    "要引用槽写 `call #:块 val \"名\"`；要复印写 `take #:块 val \"名\"`",
                ),
                Tone::Fix,
            );
            break;
        }
    }
    if expr.iter().any(|t| matches!(t, Token::Kw("take")))
        && !(is_add_val(toks) && matches!(expr.first(), Some(Token::Kw("take"))))
    {
        cx.err(
            处,
            CheckCode::E201,
            &DiagParts::full(
                format!("第 {} 行", line),
                "take 复印",
                "take 只配 `add val \"名\" 类型 = take #:块 val \"名\"` 用".to_string(),
                "",
                "take 是把别的块的 val 当时的值复印进本块新变量——写成 add val 的右值",
            ),
            Tone::Fix,
        );
    }
    while i < expr.len() {
        //nt: 内联 asm
        if anle_lexer::asm::是asm词(&expr[i])
            && matches!(expr.get(i + 1), Some(Token::Sym("(")))
        {
            if !cx.unsafe_blocks.contains(&b.name) {
                cx.err(
                    处,
                    CheckCode::E201,
                    &DiagParts::full(
                        format!("第 {} 行", line),
                        "危险模式",
                        "「asm」是危险模式专属——当前块未进入危险模式",
                        "",
                        "把这块的开块行写成 `#: 块名 b unsafe`（危险模式），再在这里用它",
                    )
                    .预期("内联汇编只能出现在 unsafe 块里")
                    .详解(
                        "asm 是 unsafe 的第四类能力（与裸地址读写、C 互操作并列）：\
                         汇编绕开了 Anle 的全部类型与内存规则，只在 `#: 块名 b unsafe` 的块里放行。",
                    ),
                    Tone::Fix,
                );
            }
            if let Some(内区) = anle_lexer::asm::括号内(expr, i + 1) {
                if let Err(原因) = anle_lexer::asm::解析隔离区(内区) {
                    cx.err(
                        处,
                        CheckCode::E201,
                        &DiagParts::full(
                            format!("第 {} 行", line),
                            "内联汇编",
                            format!("asm 隔离区不合规：{}", 原因),
                            "括号内是隔离区，按 asm 自己的微语法写",
                            anle_lexer::asm::映射表说明(),
                        ),
                        Tone::Fix,
                    );
                }
                let mut depth = 0i32;
                let mut j = i + 1;
                while j < expr.len() {
                    match &expr[j] {
                        Token::Sym("(") => depth += 1,
                        Token::Sym(")") => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    j += 1;
                }
                i = j + 1;
                continue;
            }
        }
        match &expr[i] {
            //nt: call fn "名
            Token::Kw("call") => {
                if matches!(expr.get(i + 1), Some(Token::Kw("fn"))) {
                    if let Some(Token::Str(n) | Token::Ident(n)) = expr.get(i + 2) {
                        if matches!(expr.get(i + 3), Some(Token::Sym("("))) {
                            let args = call_args(expr, i + 3);
                            check_call(b, 处, n, &args, cx);
                        }
                    }
                } else if let Some((module, blk, fname, at)) = parse_qualified_call(expr, i) {
                    //nt: 跨块/跨模块 fn
                    let args = call_args(expr, at);
                    let sig = match &module {
                        Some(m) => cx.fns_mod.get(&format!("{}|{}|{}", m, blk, fname)).cloned(),
                        None => cx.fns.get(&format!("{}|{}", blk, fname)).cloned(),
                    };
                    if let Some(sig) = sig {
                        if module.is_none()
                            && !sig.is_outs
                            && !cx.fn_has_body.contains(&format!("{}|{}", blk, fname))
                        {
                            cx.err(
                                处,
                                CheckCode::E201,
                                &DiagParts::full(
                                    format!("第 {} 行", line),
                                    "调函数",
                                    format!("fn「{}」只有声明，还没有体", fname),
                                    "",
                                    format!("用 `fn \"{}\" (参数) 返回类型 = 表达式` 给它补体", fname),
                                )
                                .预期("声明过的 fn 要补体才能调用"),
                                Tone::Fix,
                            );
                        } else {
                            check_sig_args(b, 处, &fname, &sig, &args, cx);
                        }
                    }
                } else if matches!(expr.get(i + 1), Some(Token::Kw("val"))) {
                    if let (Some(Token::Str(n) | Token::Ident(n)), Some(Token::Sym("("))) =
                        (expr.get(i + 2), expr.get(i + 3))
                    {
                        let fty = cx.find_local(&b.name, n).map(|l| l.ty.clone());
                        if let Some((params, ret)) = fty.as_deref().and_then(parse_fn_ty) {
                            let args = call_args(expr, i + 3);
                            let sig = FnSig { params, ret, is_outs: false };
                            check_sig_args(b, 处, n, &sig, &args, cx);
                        }
                    }
                }
            }
            Token::Kw("fn") => {
                //nt: 核对函数值名
                //nt: fn 名按块作用域
                if let Some(Token::Str(n) | Token::Ident(n)) = expr.get(i + 1) {
                    let fn块 = if b.name.contains('#') {
                        b.name.split('#').next().unwrap_or(&b.name).to_string()
                    } else {
                        b.name.clone()
                    };
                    if !matches!(expr.get(i + 2), Some(Token::Sym("(")))
                        && !matches!(expr.get(i.wrapping_sub(1)), Some(Token::Kw("call")))
                        && !cx.fns.contains_key(&format!("{}|{}", fn块, n))
                    {
                        cx.err(
                            处,
                            CheckCode::E201,
                            &DiagParts::full(
                                format!("第 {} 行", line),
                                "函数值",
                                format!("fn「{}」还没建过", n),
                                "",
                                format!("先用 `add fn \"{}\" (参数) 返回类型 = …` 建出来", n),
                            )
                            .预期("函数值只能取自本块已声明的 fn"),
                            Tone::Fix,
                        );
                    }
                }
            }
            Token::Ident(n) => {
//nt: 危险模式专属词
//nt: 报错要**指明"未进
                if UNSAFE_ONLY.contains(&n.as_str()) && !cx.unsafe_blocks.contains(&b.name) {
                    cx.err(
                        处,
                        CheckCode::E201,
                        &DiagParts::full(
                            format!("第 {} 行", line),
                            "危险模式",
                            format!("「{}」是危险模式专属——当前块未进入危险模式", n),
                            "",
                            "把这块的开块行写成 `#: 块名 b unsafe`（危险模式），再在这里用它",
                        )
                        .预期("危险模式专属词只能出现在 unsafe 块里")
                        .详解(
                            "「未进入危险模式」和「这个名字不存在」是两回事：\
                             `read_8` / `write_8` 这类裸地址读写、以及 `str_to_c` / `str_from_c` 这类 C 互操作词，\
                             只在 `#: 块名 b unsafe` 的块里放行；在普通块里它们是**被模式挡住了**，不是没定义。",
                        ),
                        Tone::Fix,
                    );
                }
                //nt: 内联汇编
            }
            _ => {}
        }
        if let Token::Ident(n) = &expr[i] {
            if BUILTINS.contains(&n.as_str()) {
                i += 1;
                continue; // 内置名：不按 fn 查
            }
            if matches!(expr.get(i + 1), Some(Token::Sym("("))) {
                //nt: 泛型构造
                let mut 本次实例: Option<String> = None;
                if !cx.types.contains_key(n) {
                    if let Some((_l, 参数, 字段, _变体)) = cx.泛型.get(n).cloned() {
                        let args = call_args(expr, i + 1);
                        let mut 绑定: Vec<Option<String>> = vec![None; 参数.len()];
                        for (k, a) in args.iter().enumerate() {
                            if k >= 字段.len() {
                                break;
                            }
                            let got = infer_named(a, &cx.locals, &cx.local_idx, &b.name, &cx.types)
                                .or_else(|| infer(a, &cx.locals, &cx.local_idx, &b.name, &cx.types).map(|s| s.to_string()));
                            let got = match got {
                                Some(g) => g,
                                None => continue,
                            };
                            let pat = 字段[k].1.clone();
                            if let Some(pi) = 参数.iter().position(|p| *p == pat) {
                                绑定[pi] = Some(got);
                            } else if let Some(pi) = 参数.iter().position(|p| pat == format!("{}*", p)) {
                                if let Some(内) = got.strip_suffix('*') {
                                    绑定[pi] = Some(内.to_string());
                                }
                            }
                        }
                        if 绑定.iter().all(|x| x.is_some()) {
                            let 实参: Vec<String> = 绑定.into_iter().map(|x| x.unwrap()).collect();
                            let 实例名 = format!("{}({})", n, 实参.join(", "));
                            let 新字段: Vec<(String, String)> = 字段
                                .iter()
                                .map(|(f, t)| (f.clone(), 替换类型参数(t, &参数, &实参)))
                                .collect();
                            cx.types.insert(实例名.clone(), (处, 新字段));
                            本次实例 = Some(实例名);
                        }
                    }
                }
                let 构造键: String = if cx.types.contains_key(n) {
                    n.clone()
                } else if let Some(名) = 本次实例 {
                    名
                } else {
                    //nt: 泛型但参数反解不全
                    String::new()
                };
                if !构造键.is_empty() {
                    //nt: 构造核对
                    let fields = cx.types.get(&构造键).unwrap().1.clone();
                    let args = call_args(expr, i + 1);
                    if args.len() != fields.len() {
                        cx.err(
                            处,
                            CheckCode::E202,
                            &DiagParts::full(
                                format!("第 {} 行", line),
                                "构造",
                                format!("类型「{}」有 {} 个字段，给了 {} 个", 构造键, fields.len(), args.len()),
                                "",
                                "按定义给够字段",
                            ),
                            Tone::Fix,
                        );
                    } else {
                        for (k, a) in args.iter().enumerate() {
                            let got = infer_named(a, &cx.locals, &cx.local_idx, &b.name, &cx.types)
                                .or_else(|| infer(a, &cx.locals, &cx.local_idx, &b.name, &cx.types).map(|s| s.to_string()));
                            if let Some(got) = got {
                                //nt: 字段类型照原样比对
                                let want = fields[k].1.clone();
                                if !ty_compatible(&want, &got) {
                                    cx.err(
                                        处,
                                        CheckCode::E202,
                                        &DiagParts::full(
                                            format!("第 {} 行", line),
                                            "构造",
                                            format!("字段「{}」要 {}，给了 {}", fields[k].0, want, got),
                                            "",
                                            format!("改成 {} 的值", want),
                                        ),
                                        Tone::Fix,
                                    );
                                }
                            }
                        }
                    }
                } else if !cx.variants.contains(n) {
//nt: 外部函数
                    let is_outs = cx
                        .fns
                        .get(&format!("{}|{}", b.name, n))
                        .map(|s| s.is_outs)
                        .unwrap_or(false);
                    if is_outs && !cx.unsafe_blocks.contains(&b.name) {
                        cx.err(
                            处,
                            CheckCode::E201,
                            &DiagParts::full(
                                format!("第 {} 行", line),
                                "调外部函数",
                                format!("外部函数「{}」只能在 unsafe 块里调", n),
                                "",
                                "把这块的开块行写成 `#: 块名 b unsafe`",
                            ),
                            Tone::Fix,
                        );
                    } else {
                        //nt: 二值化
                        let key = format!("{}|{}", b.name, n);
                        if cx.fns.contains_key(&key) {
                            cx.err(
                                处,
                                CheckCode::E201,
                                &DiagParts::full(
                                    format!("第 {} 行", line),
                                    "调函数",
                                    format!("调用要写 `call`——形如 `call fn \"{}\"(…)`", n),
                                    "",
                                    "把 `名(…)` 改成 `call fn \"名\"(…)`（同块调用也要写）",
                                )
                                .预期("调用户 fn 必须写 `call fn \"名\"(…)`（全面显式）"),
                                Tone::Fix,
                            );
                        } else {
                            check_call(b, 处, n, &call_args(expr, i + 1), cx);
                        }
                    }
                }
            }
        }
        i += 1;
    }
    //nt: 裸读/裸改禁令
    scan_bare_reads(b, 处, expr, toks, cx);
}

//nt: 顶层赋值位置
fn top_assign_pos(toks: &[Token]) -> Option<usize> {
    let mut depth = 0i32;
    for (k, t) in toks.iter().enumerate() {
        match t {
            Token::Sym("(") | Token::Sym("[") | Token::Sym("{") => depth += 1,
            Token::Sym(")") | Token::Sym("]") | Token::Sym("}") => depth -= 1,
            Token::Sym("=") if depth == 0 => return Some(k),
            _ => {}
        }
    }
    None
}

//nt: call #
//nt: 解析限定调用
fn parse_qualified_call(expr: &[Token], i: usize) -> Option<(Option<String>, String, String, usize)> {
    let mut j = i + 1;
    let mut module: Option<String> = None;
    match expr.get(j) {
        Some(Token::HashColon) => j += 1,
        Some(Token::HashHashColon) => {
            j += 1;
            let mut path = match expr.get(j) {
                Some(Token::Ident(n)) => {
                    j += 1;
                    n.clone()
                }
                _ => return None,
            };
            while matches!(expr.get(j), Some(Token::Sym("."))) {
                j += 1;
                if let Some(Token::Ident(n)) = expr.get(j) {
                    path.push('.');
                    path.push_str(n);
                    j += 1;
                } else {
                    return None;
                }
            }
            if !matches!(expr.get(j), Some(Token::HashColon)) {
                return None;
            }
            j += 1;
            module = Some(path);
        }
        _ => return None,
    }
    let blk = match expr.get(j) {
        Some(Token::Ident(n)) => n.clone(),
        Some(Token::Kw(k)) => (*k).to_string(),
        _ => return None,
    };
    j += 1;
    //nt: 只核对 fn 形态
    if !matches!(expr.get(j), Some(Token::Kw("fn"))) {
        return None;
    }
    j += 1;
    let fname = match expr.get(j) {
        Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
        _ => return None,
    };
    j += 1;
    if !matches!(expr.get(j), Some(Token::Sym("("))) {
        return None;
    }
    Some((module, blk, fname, j))
}

//nt: 裸读禁令
//nt: 扫裸名读写
fn scan_bare_reads(b: &AstBlock, 处: 位置, expr: &[Token], full: &[Token], cx: &mut Ctx) {
    let line = 处.行;
    if matches!(full.first(), Some(Token::Kw("case"))) {
        return; // case 头是模式，不是表达式
    }
    //nt: 读值要带 call
    for k in 0..expr.len() {
        if !matches!(expr[k], Token::Kw("val")) || k == 0 {
            continue;
        }
        if matches!(expr[k - 1], Token::Kw("call")) {
            continue;
        }
        //nt: 跨块形态自带前缀
        let mut j = k;
        while j > 0 && matches!(expr[j - 1], Token::Ident(_) | Token::Str(_)) {
            j -= 1;
        }
        if j > 0 && matches!(expr[j - 1], Token::HashColon | Token::HashHashColon) {
            continue;
        }
        let n = match expr.get(k + 1) {
            Some(Token::Str(x)) | Some(Token::Ident(x)) => x.clone(),
            _ => continue,
        };
        cx.err(
            处,
            CheckCode::E201,
            &DiagParts::full(
                format!("第 {} 行", line),
                "读值",
                format!("「{}」前面缺 call —— 读值写 `call val \"{}\"`", n, n),
                "",
                format!("改成 `call val \"{}\"`；改值仍是 `val \"{}\" = 值`（= 已承担动词）", n, n),
            )
            .预期("读 val 是动作，要带 call；改 val 有 = 当动词，不再带 call"),
            Tone::Fix,
        );
    }
    //nt: 内联汇编
    let mut k = 0usize;
    while k < expr.len() {
        if anle_lexer::asm::是asm词(&expr[k]) && matches!(expr.get(k + 1), Some(Token::Sym("("))) {
            return;
        }
        k += 1;
    }
    //nt: 裸重绑定
    if let (Some(Token::Ident(n)), Some(Token::Sym("="))) = (expr.first(), expr.get(1)) {
        if cx.local_idx.contains_key(format!("{}|{}", b.name, n).as_str()) {
            cx.err(
                处,
                CheckCode::E201,
                &DiagParts::full(
                    format!("第 {} 行", line),
                    "改值",
                    format!("改值要写 `val \"{}\" = 值`——裸名「{}」消不了歧义", n, n),
                    "",
                    "改值的行首写 `val \"名\" = 新值`；声明才用 `add val`",
                )
                .预期("改值必须显式写 `val \"名\" = 值`"),
                Tone::Fix,
            );
            return;
        }
    }
    //nt: val 读
    let mut vi = 0usize;
    while vi + 1 < expr.len() {
        if matches!(expr[vi], Token::Kw("val")) {
            if let Some(Token::Str(n)) = expr.get(vi + 1) {
                let cur = format!("{}|{}", b.name, n);
                let is_write = matches!(expr.get(vi + 2), Some(Token::Sym("=")));
                let qualified = vi > 0 && matches!(expr[vi - 1], Token::HashColon);
                if !is_write && !qualified && !cx.local_idx.contains_key(cur.as_str()) {
                    let prefix = format!("{}#", b.name);
                    let tail = format!("|{}", n);
                    if cx.local_idx.keys().any(|k| k.starts_with(&prefix) && k.ends_with(&tail)) {
                        cx.err(
                            处,
                            CheckCode::E201,
                            &DiagParts::full(
                                format!("第 {} 行", line),
                                "取值",
                                format!("「{}」是语句体 fn 里的局部——函数外不能用", n),
                                "fn 的局部只活在该函数体里",
                                format!("在块里另建一个 val，或用 `call fn \"…\"` 把值取出来"),
                            )
                            .预期("fn 局部不能在函数体外读"),
                            Tone::Fix,
                        );
                        return;
                    }
                    // nt: 名字不存在
                    let 别处有 = cx.local_idx.keys().any(|k| k.ends_with(&tail));
                    if !别处有 {
                        cx.err(
                            处,
                            CheckCode::E201,
                            &DiagParts::full(
                                format!("第 {} 行", line),
                                "取值",
                                format!("「{}」没建过——本块和引用链里都没有这个名字", n),
                                "",
                                "先 `add val \"名\" 类型 = 值` 建它；跨块读要写 `call #:块名 val \"名\"`",
                            )
                            .预期("读的 val 必须先在当前块或引用链里建过"),
                            Tone::Fix,
                        );
                        return;
                    }
                }
            }
            vi += 2;
            continue;
        }
        vi += 1;
    }
    let mut params: BTreeSet<String> = BTreeSet::new();
    if is_add_fn(full) || matches!(full.first(), Some(Token::Kw("fn"))) {
        if let Ok((ps, _)) = parse_params(full) {
            for (n, _) in ps {
                params.insert(n);
            }
        }
    }
    let for_head = matches!(full.first(), Some(Token::Kw("for")));
    let eq = top_assign_pos(expr);
    let mut i = 0usize;
    while i < expr.len() {
        let n = match &expr[i] {
            Token::Ident(n) => n,
            _ => {
                i += 1;
                continue;
            }
        };
        //nt: .字段` / `#
        if i > 0 {
            match expr.get(i - 1) {
                Some(Token::Sym(".")) | Some(Token::HashColon) | Some(Token::HashHashColon)
                | Some(Token::HashHashHashColon) => {
                    i += 1;
                    continue;
                }
                _ => {}
            }
        }
        //nt: 跳过调用名
        if matches!(expr.get(i + 1), Some(Token::Sym("("))) {
            i += 1;
            continue;
        }
        //nt: 合法裸用
        if BUILTINS.contains(&n.as_str()) || UNSAFE_ONLY.contains(&n.as_str()) {
            i += 1;
            continue;
        }
        if cx.types.contains_key(n) || cx.variants.contains(n) {
            i += 1;
            continue;
        }
        if cx.fns.contains_key(&format!("{}|{}", b.name, n))
            || cx.fns.contains_key(&format!("{}|{}", 原块(&b.name), n))
        {
            i += 1;
            continue;
        }
        if params.contains(n) {
            i += 1;
            continue;
        }
        //nt: for 变量 / c
        if cx.bare_ok.contains(&format!("{}|{}", b.name, n)) {
            i += 1;
            continue;
        }
        if for_head && i == 0 {
            i += 1;
            continue;
        }
        //nt: 写位置
        if let Some(e) = eq {
            if i < e {
                i += 1;
                continue;
            }
        }
        //nt: 已知局部
        //nt: 裸名不放行
        let 是fn内 = b.name.contains('#');
        let 原块有 = 是fn内 && {
            let plain = b.name.split('#').next().unwrap_or("");
            cx.local_idx.contains_key(format!("{}|{}", plain, n).as_str())
        };
        if 原块有 {
            cx.err(
                处,
                CheckCode::E201,
                &DiagParts::full(
                    format!("第 {} 行", line),
                    "取值",
                    format!("裸名「{}」——取 val 请写 val \"{}\"", n, n),
                    "",
                    "全面显式：fn 与 val 可重名，裸名消不了歧义。把读取处写成 val \"名\"",
                )
                .预期("读 val 必须写 val \"名\""),
                Tone::Fix,
            );
            return;
        }
        if cx.local_idx.contains_key(format!("{}|{}", b.name, n).as_str()) {
            cx.err(
                处,
                CheckCode::E201,
                &DiagParts::full(
                    format!("第 {} 行", line),
                    "取值",
                    format!("裸名「{}」——取 val 请写 val \"{}\"", n, n),
                    "",
                    "全面显式：fn 与 val 可重名，裸名消不了歧义。把读取处写成 val \"名\"",
                )
                .预期("读 val 必须写 val \"名\""),
                Tone::Fix,
            );
            return;
        }
        i += 1;
    }
}
fn pass1_uses(ast: &AstProgram, cx: &mut Ctx) {
    for b in &ast.blocks {
        cx.cur_file = b.file.clone();
        uses_block(b, cx);
    }
}

fn uses_block(b: &AstBlock, cx: &mut Ctx) {
    uses_stmts(&b.stmts, b, cx);
}

//nt: 名字使用核对也**递
fn uses_stmts(stmts: &[AstStmt], b: &AstBlock, cx: &mut Ctx) {
    for s in stmts {
        match s {
            AstStmt::Line { line, col, len, toks, .. } => {
                //nt: fn 补体行查伪块
                //nt: 形参已归伪块
                if matches!(toks.first(), Some(Token::Kw("fn"))) {
                    if let Some(n) = stmt_fn_name(toks) {
                        let pb = fn_scope_block(b, &n, &[]);
                        uses_line(&pb, 位置{ 行: *line, 列: *col, 长: *len }, toks, cx);
                        continue;
                    }
                }
                uses_line(b, 位置{ 行: *line, 列: *col, 长: *len }, toks, cx)
            }
            AstStmt::Struct { line, col, len, head, body, .. } => {
                uses_expr_tokens(b, 位置{ 行: *line, 列: *col, 长: *len }, head, cx);
                uses_stmts(body, b, cx);
            }
            AstStmt::Fn { head, body, .. } => {
                if let Some(n) = stmt_fn_name(head) {
                    let pb = fn_scope_block(b, &n, body);
                    uses_stmts(body, &pb, cx);
                }
            }
        }
    }
}

//nt: 递归展开语句序列
//nt: 把手追踪需要它
fn flatten_stmts<'a>(
    stmts: &'a [AstStmt],
    nested: bool,
    out: &mut Vec<(位置, &'a TokStream, bool)>,
) {
    for s in stmts {
        match s {
            AstStmt::Line { line, col, len, toks, .. } => {
                out.push((位置{ 行: *line, 列: *col, 长: *len }, toks, nested))
            }
            AstStmt::Struct { line, col, len, head, body, .. } | AstStmt::Fn { line, col, len, head, body, .. } => {
                out.push((位置{ 行: *line, 列: *col, 长: *len }, head, nested));
                flatten_stmts(body, true, out);
            }
        }
    }
}

fn uses_line(b: &AstBlock, 处: 位置, toks: &[Token], cx: &mut Ctx) {
    let line = 处.行;
    //nt: add type
    if is_add_type(toks) || is_add_fn(toks) || is_add_outs_fn(toks) {
        return;
    }
    if is_add_val(toks) {
        //nt: 声明句
        //nt: 名字在参数里
        //nt: add val "x
        if let Some(eq) = toks.iter().position(|t| *t == Token::Sym("=")) {
            let decl_name = match toks.get(2) {
                Some(Token::Str(n)) | Some(Token::Ident(n)) => Some(n.clone()),
                _ => None,
            };
            //nt: 原实现
            //nt: 改为 suppres
            let prev_suppress = cx.suppress_key.take();
            if let Some(n) = decl_name.as_ref() {
                cx.suppress_key = Some(format!("{}|{}", b.name, n));
            }
            uses_expr_tokens(b, 处, &toks[eq + 1..], cx);
            cx.suppress_key = prev_suppress;
        }
        return;
    }
    //nt: val "名" =
    if matches!(toks.first(), Some(Token::Kw("val"))) {
        if let Some(Token::Str(n)) | Some(Token::Ident(n)) = toks.get(1) {
            if let Some(eq) = toks.iter().position(|t| *t == Token::Sym("=")) {
                if cx.常量.contains(&format!("{}|{}", b.name, n)) {
                    cx.err(
                        处,
                        CheckCode::E201,
                        &DiagParts::full(
                            format!("第 {} 行", line),
                            "改常量",
                            format!("常量「{}」不许改赋", n),
                            "它是 add fix 声明的编译期常量，值在编译期定死",
                            "要能改的值就改用 add val 声明",
                        ),
                        Tone::Fix,
                    );
                    return;
                }
                require_known(b, 处, n, cx);
                uses_expr_tokens(b, 处, &toks[eq + 1..], cx);
            } else {
                require_known(b, 处, n, cx);
            }
        }
        return;
    }
    uses_expr_tokens(b, 处, toks, cx);
}

fn require_known(b: &AstBlock, 处: 位置, name: &str, cx: &mut Ctx) {
    let line = 处.行;
    //nt: _` 是 `case
    if name == "_" {
        return;
    }
    let known = cx.has_local(&b.name, &name);
    if known {
//nt: 「先建后用」
        //nt: 声明在用之后
        if let Some(&decl) = cx.val_decl_line.get(format!("{}|{}", b.name, name).as_str()) {
            if decl > line {
                cx.err(
                    处,
                    CheckCode::E201,
                    &DiagParts::full(
                        format!("第 {} 行", line),
                        "用名字",
                        format!("val「{}」用在它被建出来之前——它到第 {} 行才建", name, decl),
                        "",
                        format!("把建「{}」的那行 `add val` 移到这一行之前", name),
                    )
                    .预期("名字要先建出来，再用它")
                    .详解(
                        "Anle 是「先建后用」：val 的读和写都要排在它的 `add val` 之后——排在前面会读到还没放值的内存。`add fn`（函数）与 `add type`（类型）是声明，不受这条约束，可以先用后写。",
                    ),
                    Tone::Fix,
                );
            }
        }
        return;
    }
    cx.err(
        处,
        CheckCode::E201,
        &DiagParts::full(
            format!("第 {} 行", line),
            "用名字",
            format!("val「{}」还没建过", name),
            "",
            format!("在用它之前先建：add val \"{}\" <类型> = <初值>", name),
        )
        .预期("这里的名字应当是【本块已建过】的 val / 函数")
        .详解(
 "Anle 的规则是「先建后用」：名字要先由 `add val`（值）/ `add fn`（函数）/ `add type`（类型）建出来，才能在表达式里读；如果名字在别的块里，跨块取 val 要写 `call #:块 val \"名\"`；跨块调函数要写 `call #:块 fn \"名\"(…)`；同块直接用。",
        ),
        Tone::Fix,
    );
    //nt: 登记局部
    cx.add_local(LocalInfo {
        block: b.name.clone(),
        name: name.to_string(),
        ty: UNKNOWN.to_string(),
        键: std::rc::Rc::from(""),
                                    class: StorageClass::Tmp,
    });
}

//nt: 摘掉 asm 区
fn 摘掉asm区(toks: &[Token]) -> Vec<Token> {
    let mut 出 = Vec::with_capacity(toks.len());
    let mut i = 0usize;
    while i < toks.len() {
        if anle_lexer::asm::是asm词(&toks[i]) && matches!(toks.get(i + 1), Some(Token::Sym("("))) {
            let mut depth = 0i32;
            let mut j = i + 1;
            while j < toks.len() {
                match &toks[j] {
                    Token::Sym("(") => depth += 1,
                    Token::Sym(")") => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            i = j + 1;
            continue;
        }
        出.push(toks[i].clone());
        i += 1;
    }
    出
}

//nt: 表达式里的标识符都要
fn uses_expr_tokens(b: &AstBlock, 处: 位置, toks: &[Token], cx: &mut Ctx) {
    let _line = 处.行;
    //nt: 内联汇编隔离区整段摘
    let 净: Vec<Token> = 摘掉asm区(toks);
    let toks: &[Token] = &净;
    let mut skip_paren = 0i32;
    let mut field_skip: Vec<usize> = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        match t {
            Token::Sym("(") => skip_paren += 1,
            Token::Sym(")") => skip_paren -= 1,
            //nt: 名.字段
            Token::Sym(".") => {
                if matches!(toks.get(i + 1), Some(Token::Ident(_)) | Some(Token::Str(_))) {
                    field_skip.push(i + 1);
                }
            }
            //nt: 块名前缀
            Token::HashColon => {
                if matches!(toks.get(i + 1), Some(Token::Ident(_))) {
                    field_skip.push(i + 1);
                }
            }
            //nt: 模块前缀
            Token::HashHashColon => {
                if matches!(toks.get(i + 1), Some(Token::Ident(_)) | Some(Token::Str(_))) {
                    field_skip.push(i + 1);
                }
            }
            Token::Ident(n) => {
                //nt: 调用点
                if matches!(toks.get(i + 1), Some(Token::Sym("("))) {
                    continue;
                }
                if field_skip.contains(&i) {
                    continue; // 字段名
                }
                //nt: case 变体名
                if i > 0 && matches!(toks.get(i - 1), Some(Token::Kw("case"))) {
                    continue;
                }
                require_known(b, 处, n, cx);
            }
            _ => {}
        }
        let _ = skip_paren;
    }
}


//nt: 推断一段 token
//nt: 容器类型位
fn container_ty_at(toks: &[Token], i: usize) -> Option<String> {
    let name = match toks.get(i)? {
        Token::Kw(k) => (*k).to_string(),
        _ => return None,
    };
    if name == "fn" {
        return parse_type_ref(toks, i).ok().map(|(t, _)| t);
    }
    if name != "list" && name != "que" && name != "stack" && name != "map" {
        return None;
    }
    if !matches!(toks.get(i + 1), Some(Token::Sym("("))) {
        return Some(name);
    }
    let mut depth = 0i32;
    let mut end = None;
    for (k, tk) in toks.iter().enumerate().skip(i + 1) {
        match tk {
            Token::Sym("(") => depth += 1,
            Token::Sym(")") => {
                depth -= 1;
                if depth == 0 {
                    end = Some(k);
                    break;
                }
            }
            _ => {}
        }
    }
    let end = end?;
    let inner = &toks[i + 2..end];
    let arg_name = |ts: &[Token]| -> Option<String> {
        if matches!(ts.first(), Some(Token::Kw("fn"))) && matches!(ts.get(1), Some(Token::Sym("("))) {
            return parse_type_ref(ts, 0).ok().map(|(t, _)| t);
        }
        let base = match ts.first()? {
            Token::Kw(t) => (*t).to_string(),
            Token::Ident(n) => n.clone(),
            _ => return None,
        };
        if matches!(ts.get(1), Some(Token::Sym("*"))) {
            Some(format!("{}*", base))
        } else {
            Some(base)
        }
    };
    if name == "map" {
        let args = split_commas(inner);
        if args.len() != 2 {
            return None;
        }
        let k = arg_name(&args[0])?;
        let v = arg_name(&args[1])?;
        return Some(format!("map({}, {})", k, v));
    }
    let e = arg_name(inner)?;
    Some(format!("{}({})", name, e))
}

//nt: 类型串的基名
fn base_ty(s: &str) -> &str {
    s.split('(').next().unwrap_or(s)
}

//nt: 赋值类型是否相容
//nt: 定长族名
const WIDTHS: &[&str] = &[
    "int_8", "int_16", "int_32", "int_64", "uint_8", "uint_16", "uint_32", "uint_64",
];

fn ty_compatible(decl: &str, got: &str) -> bool {
    if decl == got {
        return true;
    }
//nt: null 只配引用
    if got == "null" {
        return decl.ends_with('*');
    }
    if decl == "null" {
        return got.ends_with('*');
    }
    //nt: 定长族互通
    if WIDTHS.contains(&decl) || WIDTHS.contains(&got) {
        return (WIDTHS.contains(&decl) && WIDTHS.contains(&got))
            || decl == "int"
            || got == "int";
    }
    let (bd, bg) = (base_ty(decl), base_ty(got));
    if bd != bg || !matches!(bd, "list" | "map" | "que" | "stack") {
        return false;
    }
    decl == bd || got == bg
}

//nt: 右值是**单个裸名
fn infer_named(
    toks: &[Token],
    locals: &[LocalInfo],
    idx: &std::collections::HashMap<std::rc::Rc<str>, usize>,
    block: &str,
    types: &TypeTable,
) -> Option<String> {
    //nt: call前缀读
    if matches!(toks.first(), Some(Token::Kw("call")))
        && matches!(toks.get(1), Some(Token::Kw("val")))
        && !matches!(toks.get(3), Some(Token::Sym("(")))
    {
        return infer_named(&toks[1..], locals, idx, block, types);
    }
//nt: 顶层的逻辑/比较运算
    if matches!(top_op(toks).0, 1 | 2 | 3) {
        return Some("bool".into());
    }
//nt: 列表字面量 ⇒ li
    if matches!(toks.first(), Some(Token::Sym("["))) {
        let 内 = if toks.len() >= 2 && matches!(toks.last(), Some(Token::Sym("]"))) {
            &toks[1..toks.len() - 1]
        } else {
            &toks[1..]
        };
        if let Some(首) = split_commas(内).into_iter().next() {
            if let Some(t) = infer_named(&首, locals, idx, block, types)
                .or_else(|| infer(&首, locals, idx, block, types))
            {
                return Some(format!("list({})", t));
            }
        }
        return Some("list".into());
    }
//nt: 显式取 val
    if let [Token::Kw("val"), rest @ (Token::Str(_) | Token::Ident(_)), ..] = toks {
        //nt: val 开头后跟比较
        if matches!(
            toks.get(2),
            Some(Token::Sym("==")) | Some(Token::Sym("!=")) | Some(Token::Sym("<"))
                | Some(Token::Sym("<=")) | Some(Token::Sym(">")) | Some(Token::Sym(">="))
        ) {
            return Some("bool".to_string());
        }
        let n = match rest {
            Token::Str(n) | Token::Ident(n) => n.clone(),
            _ => unreachable!(),
        };
        let key = format!("{}|{}", block, n);
        let base = idx.get(key.as_str()).map(|&i| locals[i].ty.clone());
        //nt: val "名"[…]
        //nt: 切片引用
        if let Some(t) = &base {
            if toks.iter().any(|x| *x == Token::Sym("~")) {
                return Some(t.clone());
            }
            //nt: 后缀链
            let mut cur = Some(t.clone());
            let mut k = 2usize;
            loop {
                if matches!(toks.get(k), Some(Token::Sym("."))) {
                    let Some(Token::Ident(f) | Token::Str(f)) = toks.get(k + 1) else { break };
                    cur = match cur {
                        Some(b) => 字段型(types, &b, f),
                        None => None,
                    };
                    k += 2;
                    continue;
                }
                if matches!(toks.get(k), Some(Token::Sym("["))) {
                    cur = match cur {
                        Some(b) if b == "str" => Some("str".to_string()),
                        Some(b) => elem_ty_str(&b),
                        None => None,
                    };
                    let mut 深 = 0i32;
                    while k < toks.len() {
                        match &toks[k] {
                            Token::Sym("[") => 深 += 1,
                            Token::Sym("]") => {
                                深 -= 1;
                                if 深 == 0 {
                                    k += 1;
                                    break;
                                }
                            }
                            _ => {}
                        }
                        k += 1;
                    }
                    continue;
                }
                break;
            }
            return cur;
        }
        return base;
    }
    if let [Token::Ident(n)] = toks {
        let key = format!("{}|{}", block, n);
        return idx.get(key.as_str()).map(|&i| locals[i].ty.clone());
    }
    //nt: 名[下标]
//nt: 普通下标 ⇒ 元素类
    if let Some(Token::Ident(n)) = toks.first() {
        let key = format!("{}|{}", block, n);
        if let Some(base) = idx.get(key.as_str()).map(|&i| locals[i].ty.clone()) {
            if toks.iter().any(|t| *t == Token::Sym("~")) {
                return Some(base);
            }
            if matches!(toks.get(1), Some(Token::Sym("["))) {
                return Some(elem_ty_str(&base).unwrap_or_else(|| "int".into()));
            }
        }
    }
    None
}

//nt: 整体加括号
fn paren_spans_all(toks: &[Token]) -> bool {
    let mut depth = 0i32;
    for (i, t) in toks.iter().enumerate() {
        match t {
            Token::Sym("(") => depth += 1,
            Token::Sym(")") => {
                depth -= 1;
                if depth == 0 {
                    return i == toks.len() - 1;
                }
            }
            _ => {}
        }
    }
    false
}

//nt: 字段类型查表
fn 字段型(types: &TypeTable, 基: &str, 字段: &str) -> Option<String> {
    //nt: 把手基剥星查表
    let plain = 基.strip_suffix('*').unwrap_or(基);
    let r = types
        .get(plain)
        .and_then(|(_, fs)| fs.iter().find(|(n, _)| n == 字段).map(|(_, t)| t.clone()));
    r
}

fn infer(
    toks: &[Token],
    locals: &[LocalInfo],
    idx: &std::collections::HashMap<std::rc::Rc<str>, usize>,
    block: &str,
    types: &TypeTable,
) -> Option<String> {
    //nt: call前缀读
    if matches!(toks.first(), Some(Token::Kw("call")))
        && matches!(toks.get(1), Some(Token::Kw("val")))
        && !matches!(toks.get(3), Some(Token::Sym("(")))
    {
        return infer(&toks[1..], locals, idx, block, types);
    }
    //nt: 顶层的逻辑/比较运算
    if matches!(top_op(toks).0, 1 | 2 | 3) {
        return Some("bool".to_string());
    }
    //nt: 显式取 val
    if let [Token::Kw("val"), rest @ (Token::Str(_) | Token::Ident(_)), ..] = toks {
        //nt: val 开头后跟比较
        if matches!(
            toks.get(2),
            Some(Token::Sym("==")) | Some(Token::Sym("!=")) | Some(Token::Sym("<"))
                | Some(Token::Sym("<=")) | Some(Token::Sym(">")) | Some(Token::Sym(">="))
        ) {
            return Some("bool".to_string());
        }
        let n = match rest {
            Token::Str(n) | Token::Ident(n) => n.clone(),
            _ => unreachable!(),
        };
        if let Some(l) = idx.get(format!("{}|{}", block, n).as_str()).map(|&i| &locals[i]) {
            //nt: 按后缀细化型
            let mut cur: Option<String> = Some(l.ty.clone());
            let mut k = 2usize;
            let mut 走过 = false;
            loop {
                if matches!(toks.get(k), Some(Token::Sym("."))) {
                    let Some(Token::Ident(f) | Token::Str(f)) = toks.get(k + 1) else { break };
                    走过 = true;
                    cur = match cur {
                        Some(t) => 字段型(types, &t, f),
                        None => None,
                    };
                    k += 2;
                    continue;
                }
                if matches!(toks.get(k), Some(Token::Sym("["))) {
                    走过 = true;
                    cur = match cur {
                        Some(t) if t == "str" => Some("str".to_string()),
                        Some(t) => elem_ty_str(&t),
                        None => None,
                    };
                    let mut 深 = 0i32;
                    while k < toks.len() {
                        match &toks[k] {
                            Token::Sym("[") => 深 += 1,
                            Token::Sym("]") => {
                                深 -= 1;
                                if 深 == 0 {
                                    k += 1;
                                    break;
                                }
                            }
                            _ => {}
                        }
                        k += 1;
                    }
                    continue;
                }
                break;
            }
            if 走过 && cur.is_some() {
                return cur;
            }
            let t = l.ty.as_str();
            //nt: 容器类型
            //nt: 否则 len/emp
            if is_container_ty(t) {
                return Some("list".to_string());
            }
            return Some(match t {
                "float" => "float",
                "bool" => "bool",
                "str" => "str",
                _ => "int",
            }
            .to_string());
        }
    }
    //nt: 同块取 val
    //nt: 不做这一步
    if let [Token::Kw("call"), Token::Kw("val"), Token::Str(n)] = toks {
        if let Some(l) = idx.get(format!("{}|{}", block, n).as_str()).map(|&i| &locals[i]) {
            return Some(
                match l.ty.as_str() {
                    "float" => "float",
                    "bool" => "bool",
                    "str" => "str",
                    _ => "int",
                }
                .to_string(),
            );
        }
    }
//nt: 内置 / 类型转换调
    let head_name = match toks.first() {
        Some(Token::Ident(n)) => Some(n.clone()),
        Some(Token::Kw(k)) => Some((*k).to_string()),
        _ => None,
    };
    if matches!(toks.get(1), Some(Token::Sym("("))) && paren_spans_all(toks) {
        if let Some(name) = head_name {
            match name.as_str() {
                "int" | "code" | "len" | "len_utf8" | "index" | "rnd" | "str_to_c" => {
                    return Some("int".to_string())
                }
                "empty" | "has" => return Some("bool".to_string()),
                "map" => return Some("map".to_string()),
                //nt: 队列/栈构造与环境函
                "que" => return Some("que".to_string()),
                "stack" => return Some("stack".to_string()),
                "args" => return Some("list(str)".to_string()),
                "read_file" => return Some("str".to_string()),
                "keys" => {
                    //nt: 键元素随键型
                    //nt: 同块val取全型
                    let inner = &toks[2..toks.len() - 1];
                    let mut got = None;
                    if let [Token::Kw("call"), Token::Kw("val"), Token::Str(n)] = inner {
                        if let Some(l) = idx.get(format!("{}|{}", block, n).as_str()).map(|&i| &locals[i]) {
                            got = Some(l.ty.to_string());
                        }
                    }
                    let mt = got.or_else(|| infer(inner, locals, idx, block, types));
                    if let Some(mt) = mt {
                        if let Some(rest) = mt.strip_prefix("map(") {
                            let k = rest.split(',').next().unwrap_or("").trim();
                            return Some(format!("list({})", k));
                        }
                    }
                    return Some("list(str)".to_string());
                },
                //nt: values 的元素
                "vals" => return Some("list".to_string()),
                "str" | "concat" | "char" | "str_from_c" => {
                    return Some("str".to_string())
                }
                "slice" => return Some("list".to_string()),
                "float" => return Some("float".to_string()),
                "bool" => return Some("bool".to_string()),
                "list" => return Some("list".to_string()),
                _ => {}
            }
        }
    }
    //nt: 列表 / map 字
    //nt: 下标读
    if matches!(toks.first(), Some(Token::Sym("["))) {
        return Some("list".to_string());
    }
    if matches!(toks.first(), Some(Token::Sym("{"))) {
        return Some("map".to_string());
    }
    //nt: 顶层最低优先级的运算
    //nt: 避免"扫到最后一个标
    match top_op(toks).0 {
        1 | 2 | 3 => return Some("bool".to_string()), // or / and / 比较
        4..=7 => return Some("int".to_string()),      // 位运算 / 移位
        8 => {
            //nt: 赋值/加减
            if top_has_str(toks, locals, block) {
                return Some("str".to_string());
            }
            if top_has_float(toks, locals, block) {
                return Some("float".to_string());
            }
            return Some("int".to_string());
        }
        9 => {
            return Some(if top_has_float(toks, locals, block) { "float" } else { "int" }.to_string())
        }
        _ => {}
    }
    //nt: 无顶层运算符 ⇒ 单
    operand_ty(toks, locals, idx, block).map(|s| s.to_string())
}

//nt: 顶层运算符
fn top_op(toks: &[Token]) -> (u8, usize) {
    let mut depth = 0i32;
    let mut best = 0u8;
    let mut best_i = 0usize;
    let mut prev_end = false; // 上一个顶层 token 是否结束一个操作数（判一元负号）
    for (i, t) in toks.iter().enumerate() {
        match t {
            Token::Sym("(") | Token::Sym("[") => {
                depth += 1;
                prev_end = false;
                continue;
            }
            Token::Sym(")") | Token::Sym("]") => {
                depth -= 1;
                if depth == 0 {
                    prev_end = true;
                }
                continue;
            }
            _ => {}
        }
        if depth != 0 {
            continue;
        }
        let c = match t {
            Token::Kw("or") => 1,
            Token::Kw("and") => 2,
            Token::Sym("==") | Token::Sym("!=") | Token::Sym("<") | Token::Sym("<=")
            | Token::Sym(">") | Token::Sym(">=") => 3,
            Token::Sym("|") => 4,
            Token::Sym("^") => 5,
            Token::Sym("&") => 6,
            Token::Sym("<<") | Token::Sym(">>") => 7,
            Token::Sym("+") => 8,
            Token::Sym("-") if prev_end => 8,
            Token::Sym("*") | Token::Sym("/") | Token::Sym("%") => 9,
            _ => 0,
        };
        if c > 0 && (best == 0 || c < best) {
            best = c;
            best_i = i;
        }
        prev_end = matches!(
            t,
            Token::Ident(_)
                | Token::Int(_)
                | Token::Float(_)
                | Token::Str(_)
                | Token::Kw("re")
                | Token::Kw("fa")
                | Token::Kw("null")
        );
    }
    (best, best_i)
}

//nt: 顶层是否出现 str
fn top_has_str(toks: &[Token], locals: &[LocalInfo], block: &str) -> bool {
    let mut depth = 0i32;
    let mut i = 0usize;
    while i < toks.len() {
        match &toks[i] {
            Token::Sym("(") | Token::Sym("[") => depth += 1,
            Token::Sym(")") | Token::Sym("]") => depth -= 1,
            //nt: call val "
            //nt: 不是 str 字面量
            Token::Kw("call")
                if depth == 0
                    && matches!(
                        toks.get(i + 1),
                        Some(Token::Kw("val")) | Some(Token::Kw("fn")) | Some(Token::Kw("type"))
                    )
                    && matches!(toks.get(i + 2), Some(Token::Str(_))) =>
            {
                i += 3;
                continue;
            }
            Token::Str(_) if depth == 0 => return true,
            Token::Ident(n) if depth == 0 => {
                if locals
                    .iter()
                    .any(|l| l.block == block && l.name == *n && l.ty == "str")
                {
                    return true;
                }
            }
            _ => {}
        }
        i += 1;
    }
    false
}

//nt: 顶层是否出现 flo
fn top_has_float(toks: &[Token], locals: &[LocalInfo], block: &str) -> bool {
    let mut depth = 0i32;
    for t in toks {
        match t {
            Token::Sym("(") | Token::Sym("[") => depth += 1,
            Token::Sym(")") | Token::Sym("]") => depth -= 1,
            Token::Float(_) if depth == 0 => return true,
            Token::Ident(n) if depth == 0 => {
                if locals
                    .iter()
                    .any(|l| l.block == block && l.name == *n && l.ty == "float")
                {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

//nt: 单一操作数
fn operand_ty(toks: &[Token], locals: &[LocalInfo], idx: &std::collections::HashMap<std::rc::Rc<str>, usize>, block: &str) -> Option<&'static str> {
    match toks.first() {
        Some(Token::Int(_)) => Some("int"),
        Some(Token::Float(_)) => Some("float"),
        Some(Token::Str(_)) => Some("str"),
        Some(Token::Kw("re")) | Some(Token::Kw("fa")) => Some("bool"),
        Some(Token::Sym("-")) => match toks.get(1) {
            //nt: 一元负号随字面量
            Some(Token::Float(_)) => Some("float"),
            _ => Some("int"),
        }, // 一元负号 + 数字字面量
        Some(Token::Ident(n)) => {
            if matches!(toks.get(1), Some(Token::Sym("("))) {
                None // 调用：类型由调用侧另判
            } else {
                idx.get(format!("{}|{}", block, n).as_str()).map(|&i| &locals[i])
                    .filter(|l| l.ty != UNKNOWN)
                    .map(|l| match l.ty.as_str() {
                        "float" => "float",
                        "bool" => "bool",
                        "str" => "str",
                        _ => "int",
                    })
            }
        }
        _ => None,
    }
}

//nt: 类型名是否已知
fn is_known_type(cx: &Ctx, block: &str, t: &str) -> bool {
    let base = t.trim_end_matches('*');
    let base = base.split('(').next().unwrap_or(base);
    const BUILTIN: &[&str] = &["int", "float", "bool", "str", "list", "map", "que", "stack"];
    BUILTIN.contains(&base)
        || WIDTHS.contains(&base)
        || cx.types.contains_key(base)
        || cx.泛型.contains_key(base)
        || cx
            .类型参数作用域
            .get(block)
            .map(|s| s.contains(base))
            .unwrap_or(false)
}

fn pass2_types(ast: &AstProgram, cx: &mut Ctx) {
    for b in &ast.blocks {
        cx.cur_file = b.file.clone();
        types_block(b, cx);
    }
}

fn types_block(b: &AstBlock, cx: &mut Ctx) {
    for s in &b.stmts {
        match s {
            AstStmt::Line { line, col, len, toks, .. } => {
                //nt: add val "名
                if is_add_val(toks) {
                    let name = match toks.get(2) {
                        Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
                        _ => continue,
                    };
                    //nt: 声明类型
                    //nt: 其余标量类型取关键字
                    let decl = match container_ty_at(toks, 3) {
                        Some(t) => t,
                        None => match toks.get(3) {
                            //nt: 真假值字面量 re
                            Some(Token::Kw(t)) if *t == "re" || *t == "fa" => {
                                cx.err(
                                    位置{ 行: *line, 列: *col, 长: *len },
                                    CheckCode::E201,
                                    &DiagParts::full(
                                        format!("第 {} 行", line),
                                        "类型名",
                                        format!("没有类型「{}」", t),
                                        "re / fa 是真假值的字面量，不是类型名",
                                        "类型名写 bool",
                                    ),
                                    Tone::Fix,
                                );
                                return;
                            }
                            Some(Token::Kw(t)) => (*t).to_string(),
                            //nt: 自定义类型名
                            //nt: 未知类型名
                            Some(Token::Ident(t)) => {
                                //nt: 自定义名补星
                                let t = if matches!(toks.get(4), Some(Token::Sym("*"))) {
                                    format!("{}*", t)
                                } else {
                                    t.clone()
                                };
                                if !is_known_type(cx, &b.name, &t) {
                                    cx.err(
                                        位置{ 行: *line, 列: *col, 长: *len },
                                        CheckCode::E201,
                                        &DiagParts::full(
                                            format!("第 {} 行", line),
                                            "类型名",
                                            format!("没有类型「{}」", t),
                                            "",
                                            "内置类型：int / float / bool / str / list(T) / map(K,V) / que(T) / stack(T) \
                                             / int_8…int_64 / uint_8…uint_64；自定义类型要先 `add type \"名\" (…)`。\
                                             旧名 int8 / uint8 / size 已废止（2026-10-08 词名重构）。",
                                        )
                                        .预期("类型名必须是内置类型或已 add type 的类型")
                                        .详解(
                                            "类型名做**存在性校验**：不在内置表、又没有 `add type` 声明过 ⇒ 报错。\
                                             这样废止名与被拼错的类型名都会当场暴露，不需要维护废止名单。",
                                        ),
                                        Tone::Fix,
                                    );
                                    continue;
                                }
                                t.clone()
                            }
                            _ => continue,
                        },
                    };
                    //nt: 键型白名单
                    if let Some(rest) = decl.strip_prefix("map(") {
                        let k = rest.split(',').next().unwrap_or("").trim();
                        if k != "str" && k != "int" {
                            cx.err(
                                位置{ 行: *line, 列: *col, 长: *len },
                                CheckCode::E201,
                                &DiagParts::full(
                                    format!("第 {} 行", line),
                                    "map 键型",
                                    format!("map 的键型「{}」不支持", k),
                                    "map 的键只支持 str 或 int",
                                    "把键型改成 str 或 int",
                                ),
                                Tone::Fix,
                            );
                            continue;
                        }
                    }
                    if let Some(eq) = toks.iter().position(|t| *t == Token::Sym("=")) {
                        let rhs = &toks[eq + 1..];
                        let got: Option<String> = if rhs.len() == 2 && matches!(rhs[0], Token::Kw("fn")) {
                            let f = match &rhs[1] {
                                Token::Str(f) | Token::Ident(f) => f.clone(),
                                _ => String::new(),
                            };
                            cx.fns.get(&format!("{}|{}", b.name, f)).map(|s| {
                                let ps: Vec<String> = s.params.clone();
                                format!("fn({}) {}", ps.join(", "), s.ret)
                            })
                        } else {
                            call_ret(rhs, &b.name, &cx.fns, &cx.locals)
                                .or_else(|| infer_named(rhs, &cx.locals, &cx.local_idx, &b.name, &cx.types))
                                .or_else(|| infer(rhs, &cx.locals, &cx.local_idx, &b.name, &cx.types).map(|s| s.to_string()))
                        };
                        if let Some(got) = got {
                            if !ty_compatible(&decl, &got) {
                                cx.err(
                                    位置{ 行: *line, 列: *col, 长: *len },
                                    CheckCode::E202,
                                    &DiagParts::full(
                                        format!("第 {} 行", line),
                                        "赋值",
                                        format!("「{}」声明是 {}，右边是 {}", name, decl, got),
                                        "",
                                        format!("改成 {} 的值，或把声明改成 {}", decl, got),
                                    )
                                    .预期(format!("右边的值应当是 {} 型（左边声明说了算）", decl))
                .详解("Anle 不悄悄转换类型：int 不会自动变 str。要换类型请显式写 int(值) / str(值) / float(值) / bool(值) 转换；若是声明写错了，就把左边类型改成右边的型。"),
                                    Tone::Fix,
                                );
                            }
                        }
                    }
                }
                //nt: val"名"=表达式
                if matches!(toks.first(), Some(Token::Kw("val")))
                    && !matches!(toks.get(2), Some(Token::Sym("[")) | Some(Token::Sym(".")))
                {
                    if let (Some(Token::Str(n)) | Some(Token::Ident(n)), Some(eq)) = (
                        toks.get(1),
                        toks.iter().position(|t| *t == Token::Sym("=")),
                    ) {
                        let decl = cx
                            .find_local(&b.name, n)
                            .map(|l| l.ty.clone())
                            .unwrap_or_else(|| UNKNOWN.to_string());
                        if decl != UNKNOWN {
                            //nt: 先认具名读
                            let got: Option<String> = infer_named(
                                &toks[eq + 1..],
                                &cx.locals,
                                &cx.local_idx,
                                &b.name,
                                &cx.types,
                            )
                            .or_else(|| call_ret(&toks[eq + 1..], &b.name, &cx.fns, &cx.locals))
                            .or_else(|| infer(&toks[eq + 1..], &cx.locals, &cx.local_idx, &b.name, &cx.types).map(|s| s.to_string()));
                            if let Some(got) = got {
                                if !ty_compatible(&decl, &got) {
                                    cx.err(
                                        位置{ 行: *line, 列: *col, 长: *len },
                                        CheckCode::E202,
                                        &DiagParts::full(
                                        format!("第 {} 行", line),
                                        "改值",
                                        format!("「{}」是 {}，右边是 {}", n, decl, got),
                                        "",
                                        format!("改成 {} 的值", decl),
                                        )
                                        .预期(format!("新值应当是 {} 型", decl))
                .详解("改值必须和它当初声明的类型一致——同一条 val 不改类型，要另一种型请用转换函数。"),
                                        Tone::Fix,
                                    );
                                }
                            }
                        }
                    }
                }
            }
            AstStmt::Fn { line, col, len, head, body, .. } => {
                //nt: 末尾必须是 give
                let fname = stmt_fn_name(head).unwrap_or_default();
                if !fname.is_empty() {
                    let ret = cx
                        .fns
                        .get(&format!("{}|{}", b.name, fname))
                        .or_else(|| cx.fns.get(&format!("{}|{}", 原块(&b.name), fname)))
                        .map(|s| s.ret.clone())
                        .unwrap_or_default();
                    let pb = fn_scope_block(b, &fname, body);
                    match body.last() {
                        Some(AstStmt::Line { line: gl, col, len, toks, .. }) if matches!(toks.first(), Some(Token::Kw("give"))) => {
                            //nt: give同规则
                            let mut 早: Vec<(usize, Vec<Token>)> = Vec::new();
                            找give(&body[..body.len() - 1], &mut 早);
                            for (el, toks) in 早 {
                                let expr = &toks[1..];
                                let got = infer_named(expr, &cx.locals, &cx.local_idx, &pb.name, &cx.types)
                                    .or_else(|| {
                                        infer(expr, &cx.locals, &cx.local_idx, &pb.name, &cx.types)
                                            .map(|s| s.to_string())
                                    });
                                if let Some(got) = got {
                                    if !ret.is_empty() && !ty_compatible(&ret, &got) {
                                        cx.err(
                                            位置{ 行: el, 列: *col, 长: *len },
                                            CheckCode::E202,
                                            &DiagParts::full(
                                                format!("第 {} 行", el),
                                                "写 give",
                                                format!("fn「{}」声明返回 {}，中途 give 的是 {}", fname, ret, got),
                                                "",
                                                format!("give 一个 {} 的值", ret),
                                            )
                                            .预期("中途 give 与末尾 give 同型")
                                            .详解("中途 give 即立即返回；所有 give 都要和声明的返回类型一致。"),
                                            Tone::Fix,
                                        );
                                    }
                                }
                            }
                            let expr = &toks[1..];
                            let got = infer_named(expr, &cx.locals, &cx.local_idx, &pb.name, &cx.types)
                                .or_else(|| infer(expr, &cx.locals, &cx.local_idx, &pb.name, &cx.types).map(|s| s.to_string()));
                            if let Some(got) = got {
                                if !ret.is_empty() && !ty_compatible(&ret, &got) {
                                    cx.err(
                                        位置{ 行: *gl, 列: *col, 长: *len },
                                        CheckCode::E202,
                                        &DiagParts::full(
                                            format!("第 {} 行", gl),
                                            "写 give",
                                            format!("fn「{}」声明返回 {}，give 的是 {}", fname, ret, got),
                                            "",
                                            format!("give 一个 {} 的值", ret),
                                        )
                                        .预期(format!("give 的表达式应当是 {} 型", ret)),
                                        Tone::Fix,
                                    );
                                }
                            }
                        }
                        _ => {
                            cx.err(
                                位置{ 行: *line, 列: *col, 长: *len },
                                CheckCode::E202,
                                &DiagParts::full(
                                    format!("第 {} 行", line),
                                    "写语句体 fn",
                                    format!("fn「{}」的最后一条不是 give", fname),
                                    "语句体必须有返回值",
                                    "在函数体末尾写 `give 表达式`",
                                )
                                .预期("语句体 fn 末尾要写 give 表达式")
                                .详解("fn 是算值的：语句体允许中间步骤，但最后要用 give 把值交回调用者。"),
                                Tone::Fix,
                            );
                        }
                    }
                    types_block(&pb, cx);
                }
            }
            AstStmt::Struct { line, col, len, head, body, .. } => {
                //nt: 结构行条件/匹配值
                let kind = head.first().and_then(|t| match t {
                    Token::Kw(k) => Some(*k),
                    _ => None,
                });
                match kind {
                    Some("if") | Some("while") => {
                        let cond = if head.len() >= 2 { &head[1..head.len() - 1] } else { &[] };
                        if let Some(got) = infer(cond, &cx.locals, &cx.local_idx, &b.name, &cx.types) {
                            if got != "bool" {
                                cx.err(
                                    位置{ 行: *line, 列: *col, 长: *len },
                                    CheckCode::E202,
                                    &DiagParts::full(
                                        format!("第 {} 行", line),
                                        "写条件",
                                        format!("{} 的条件不是真假值（是 {}）", kind.unwrap(), got),
                                        "",
                                        "写成比较式（如 x == 1），或直接用 re / fa",
                                    ),
                                    Tone::Fix,
                                );
                            }
                        }
                    }
                    _ => {}
                }
                for s2 in body {
                    if let AstStmt::Line { line, col: _, len, toks, .. } = s2 {
                        //nt: 造伪块
                        let pseudo = AstBlock {
            name: b.name.clone(),
            line: *line,
            col: 0,
            len: 0,
            stmts: vec![AstStmt::Line { line: *line, col: 0, len: 0, toks: toks.clone() }],
            attrs: Vec::new(),
            asm_lines: Vec::new(),
            module: b.module.clone(),
            file: b.file.clone(),
        };
                        types_block(&pseudo, cx);
                    } else if let AstStmt::Struct { line, col, len, head, body } = s2 {
                        let pseudo = AstBlock {
                            name: b.name.clone(),
                            line: *line,
                            col: *col,
                            len: *len,
                            stmts: vec![AstStmt::Struct { line: *line, col: 0, len: 0, head: head.clone(), body: body.clone() }],
                            attrs: Vec::new(),
                            asm_lines: Vec::new(),
                            module: b.module.clone(),
                            file: b.file.clone(),
                        };
                        types_block(&pseudo, cx);
                    }
                }
            }
        }
    }
}


//nt: 存储类标注
//nt: -其余⇒`tmp`
//nt: 遍 3 标注
//nt: sle 块时序检查
//nt: 被取用前得有人显式跑
fn pass_sle_calls(ast: &AstProgram, cx: &mut Ctx) {
    //nt: 收 sle 块
    let sle: std::collections::BTreeSet<String> = ast
        .blocks
        .iter()
        .filter(|b| b.attrs.iter().any(|a| a == "sle"))
        .map(|b| b.name.clone())
        .collect();
    if sle.is_empty() {
        return;
    }
    //nt: 跑整块语句集与取用处
    let mut ran: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    //nt: 取用处记录
    let mut 用处: Vec<(String, String, String, usize, usize, usize)> = Vec::new();
    for b in &ast.blocks {
        for s in &b.stmts {
            if let AstStmt::Line { line, col, len, toks, .. } = s {
                let mut i = 0usize;
                while i < toks.len() {
                    if matches!(toks.get(i), Some(Token::Kw("call")))
                        && matches!(toks.get(i + 1), Some(Token::HashColon))
                    {
                        let blk = match toks.get(i + 2) {
                            Some(Token::Ident(n)) => n.clone(),
                            Some(Token::Kw(k)) => (*k).to_string(),
                            _ => {
                                i += 1;
                                continue;
                            }
                        };
                        match toks.get(i + 3) {
                            //nt: 跑整块
                            None => {
                                ran.insert(blk);
                            }
                            //nt: 取 val 须先跑
                            Some(Token::Kw("val")) if sle.contains(&blk) => {
                                用处.push((blk, b.name.clone(), b.file.clone(), *line, *col, *len));
                            }
                            _ => {}
                        }
                        i += 3;
                        continue;
                    }
                    i += 1;
                }
            }
        }
    }
    //nt: 每块只报首处
    let mut 已报: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for (blk, _from, file, line, col, len) in 用处 {
        if ran.contains(&blk) || !已报.insert(blk.clone()) {
            continue;
        }
        cx.cur_file = file;
        cx.err(
            位置 { 行: line, 列: col, 长: len },
            CheckCode::E203,
            &DiagParts::full(
                format!("第 {} 行", line),
                "sle 块时序",
                format!("sle 块「{}」没人显式跑过，它的 val 还不存在", blk),
                "sle 块不自动跑；取用它的 val 前，程序里要有一句 `call #:块名` 把它跑一遍",
                "先写一句 `call #:块名`，再取用它的 val",
            ),
            Tone::Fix,
        );
    }
}

fn pass3_memory(ast: &AstProgram, cx: &mut Ctx) {
    for b in &ast.blocks {
        cx.cur_file = b.file.clone();
        let unsafe_block = block_is_unsafe(b, ast);
        for l in cx.locals.iter_mut() {
            if l.block == b.name {
                l.class = if unsafe_block { StorageClass::Unsafe } else { StorageClass::Tmp };
            }
        }
        //nt: 护栏标注
        let _ = b;
    }
}