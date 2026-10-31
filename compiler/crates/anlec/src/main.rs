//! anlec —— Anle 编译器驱动
//!
//! CLI 规格（对齐 cargo 的命令形态与退出码契约）：
//!   anlec build [名] [--mode debug|release] [--opt 0-3] [--backend c|llvm]
//!   anlec run   [名] [--mode …] [--opt …] [--backend …] [-- 参数…]
//!   anlec check [名]
//!   anlec clean [名]
//!   anlec -V / -h
//!
//! - 操作数 `[名]` 可省：省略 = 当前目录的项目（有 lib.anle）或目录下唯一的 .an；
//!   给了 = 按名字找（lib.anle 的包名，或 <名>.an 文件）。
//! - 产物：`build/<模式>/<名>.exe`，中间产物 `<名>.c` 同目录（模式分顶层，目标将来叠内层）。
//! - 配置：`lib.anle`（name/main/id/v）+ `build.anle`（mode/opt/backend），
//!   `键 = 值`、`#` 注释；命令行旗子覆盖文件值。
//! - 退出码：0 成功 / 101 失败 / 1 用法错误；run 透传被运行程序的退出码。
//! - 诊断走 stderr；进度走 stdout。
//! - 编译器取 `ANLE_CC`，未设则用 PATH 里的 `clang`。MSVC target 与 GNU target 均可，
//!   前提是它找得到系统头文件与链接器

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

mod fmt;
mod lsp;

use anle_check::check;
#[cfg(feature = "backend")]
use anle_codegen::lower_to_c;
#[cfg(feature = "llvm")]
use anle_llvm::emit_object;
#[cfg(feature = "backend")]
use anle_ir::build as build_ir;
use anle_lexer::lex;
use anle_parser::Program as AstProgram;

const VERSION: &str = env!("CARGO_PKG_VERSION");

//nt: 发版时改语法元数据
//nt: 年月/日期/包
const SYNTAX: &str = "none";
const TIER: &str = "none";
const PACKS: &[&str] = &[];

//nt: 版本展示与机器行
fn version_banner() -> String {
    let 包显示 = PACKS.iter().map(|p| format!("+{}", p)).collect::<String>();
    let 包机器 = PACKS.join(",");
    let 名 = match TIER {
        "dev" => format!("Com for Anle Dev-{}", SYNTAX),
        "lts" => format!("Com for Anle {} LTS{}", SYNTAX, 包显示),
        _ => "Com for Anle unreleased".to_string(),
    };
    let 机器 = match TIER {
        "dev" => format!("syntax=Dev-{} tier=dev compiler={}", SYNTAX, VERSION),
        "lts" if 包机器.is_empty() => {
            format!("syntax={} tier=lts compiler={}", SYNTAX, VERSION)
        }
        "lts" => {
            format!("syntax={} tier=lts packs={} compiler={}", SYNTAX, 包机器, VERSION)
        }
        _ => format!("tier=none compiler={}", VERSION),
    };
    format!("{} v{}\n{}", 名, VERSION, 机器)
}

//nt: 后端编译器取哪个
//nt: 「同根」= 本可执行
#[cfg(feature = "backend")]
fn 找cc() -> String {
    if let Ok(v) = env::var("ANLE_CC") {
        if !v.trim().is_empty() {
            return v;
        }
    }
    if let Ok(exe) = env::current_exe() {
        if let Some(根) = exe.parent().and_then(|p| p.parent()) {
            for 名 in ["clang.exe", "clang"] {
                let p = 根.join("tools").join("clang").join("bin").join(名);
                if p.is_file() {
                    return p.to_string_lossy().into_owned();
                }
            }
        }
    }
    "clang".to_string()
}

const USAGE: &str = "\
anlec —— Anle 编译器

用法：
  anlec build [名] [--mode debug|release] [--opt 0-3] [--backend c|llvm]
  anlec run   [名] [--mode …] [--opt …] [--backend …] [-- 参数…]
  anlec check [名]
  anlec fmt   [路径] [--check]
  anlec clean [名]
  anlec -V | anlec -h

说明：
  [名]     可省：省略 = 当前目录的项目（有 lib.anle）或目录下唯一的 .an；
           给了 = 按名字找（lib.anle 的包名，或 <名>.an 文件）。
  --mode   debug（全护栏）/ release（优化，护栏全抽——越界等成为未定义行为）。决定产物目录。
  --opt    优化力度 0-3；缺省随模式（debug→0，release→3）。
  --backend  c（缺省）/ llvm（LLVM 18 后端，需 clang 与 LLVM-C 运行库）。
  产物     build/<模式>/<名>.exe，中间产物 <名>.c 同目录（llvm 为 <名>.ll/<名>.o）。
  退出码   0 成功 / 101 失败 / 1 用法错误；run 透传被运行程序的退出码。";

const COMMAND_HELP: &[(&str, &str)] = &[
    ("build", "把源码变成可执行文件（编译+链接一条龙）\n产物：build/<模式>/<名>.exe，中间产物 <名>.c 同目录"),
    ("run", "先 build，成功就立刻运行\n`--` 之后的参数原样传给被运行的程序"),
    ("check", "只检查代码有没有错，不产任何文件（最快）"),
    ("fmt", "只补装饰性缩进，不动 token；`--check` 只列出需要格式化的文件"),
    ("frontend", "只跑**前端**（词法→语法→检查→中间表示），不产文件、不调 clang\n用于测前端耗时：跑 3 次取最小，输出行数/字节/耗时/吞吐"),
    ("clean", "删掉 build/ 目录里所有生成的东西"),
];

fn main() -> ExitCode {
    //nt: E7 编译器内部
    std::panic::set_hook(Box::new(|info| {
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "（无消息）".to_string());
        let code = anle_error::InternalCode::E701.number();
        eprintln!(
            "{}",
            render_nofile(code, "", &format!("编译器内部错误：{}", msg), "")
        );
    }));
    let args: Vec<String> = env::args().skip(1).collect();
    dispatch(&args)
}


//nt: 失败
//nt: 已渲染的带码报错
fn fail101(msg: impl AsRef<str>) -> ExitCode {
    let m = msg.as_ref();
    if m.starts_with('[') {
        eprintln!("{}", m);
    } else {
        eprintln!("错误：{}", m);
    }
    ExitCode::from(101)
}

//nt: 无位置报错
//nt: 码级默认
fn render_nofile(code: &str, 做: &str, 缺: &str, 改: &str) -> String {
    let (de, dd) = anle_error::code_defaults(code);
    let parts = anle_error::DiagParts::full(String::new(), 做, 缺, "", 改)
        .预期(de)
        .详解(dd);
    let cx = anle_error::RenderCtx { code, file: "", src_line: "" };
    anle_error::render_full(&parts, anle_error::Tone::Fix, &cx)
}

//nt: 带文件报错
fn render_file(code: &str, 做: &str, 缺: &str, 改: &str, src: &std::path::Path) -> String {
    let file = src
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (de, dd) = anle_error::code_defaults(code);
    let parts = anle_error::DiagParts::full(String::new(), 做, 缺, "", 改)
        .预期(de)
        .详解(dd);
    let cx = anle_error::RenderCtx { code, file: &file, src_line: "" };
    anle_error::render_full(&parts, anle_error::Tone::Fix, &cx)
}

//nt: 用法错误
fn usage_err(rendered: impl AsRef<str>) -> ExitCode {
    eprintln!("{}", rendered.as_ref());
    eprintln!();
    eprintln!("{}", USAGE);
    ExitCode::from(1)
}


fn dispatch(args: &[String]) -> ExitCode {
    if args.is_empty() {
        eprintln!("{}", USAGE);
        return ExitCode::from(1);
    }
    match args[0].as_str() {
        "-V" | "--version" => {
            println!("{}", version_banner());
            ExitCode::SUCCESS
        }
        "-h" | "--help" => {
            println!("{}", USAGE);
            ExitCode::SUCCESS
        }
        "build" | "run" | "check" | "clean" | "frontend" => run_command(&args[0], &args[1..]),
        "fmt" => do_fmt(&args[1..]),
        "lsp" => {
            // nt: 语言服务器入口
            let 码 = lsp::服务();
            if 码 == 0 { ExitCode::SUCCESS } else { ExitCode::from(1) }
        }
        other => usage_err(render_nofile(
            anle_error::CliCode::E501.number(),
            "敲命令",
            &format!("不认识的命令「{}」", other),
            "用 `anlec -h` 看可用命令（build / run / check / clean）",
        )),
    }
}


struct Opts {
    name: Option<String>,
    mode: Option<String>,
    opt: Option<u32>,
    incremental: Option<bool>,
    backend: Option<String>,
    pass_args: Vec<String>,
    dump_ir: bool, // 未公开的调试开关
    //nt: 词法快照开关
    dump_tokens: bool,
    //nt: 语法快照开关
    dump_ast: bool,
    help: bool,
}

impl Opts {
    fn new() -> Self {
        Self {
            name: None,
            mode: None,
            opt: None,
            incremental: None,
            backend: None,
            pass_args: Vec::new(),
            dump_ir: false,
            dump_tokens: false,
            dump_ast: false,
            help: false,
        }
    }
}

fn parse_opts(rest: &[String]) -> Result<Opts, String> {
    let mut o = Opts::new();
    let mut i = 0usize;
    while i < rest.len() {
        let a = rest[i].as_str();
        if a == "--" {
            o.pass_args = rest[i + 1..].to_vec();
            break;
        }
        if let Some(body) = a.strip_prefix("--") {
            let (k, inline) = match body.split_once('=') {
                Some((k, v)) => (k, Some(v.to_string())),
                None => (body, None),
            };
            //nt: 取旗子的值
            macro_rules! val {
                () => {{
                    match &inline {
                        Some(v) => {
                            i += 1;
                            v.clone()
                        }
                        None => {
                            i += 1;
                            match rest.get(i) {
                                Some(v) => v.clone(),
                                None => return Err(format!("--{} 缺值", k)),
                            }
                        }
                    }
                }};
            }
            match k {
                "mode" => {
                    let v = val!();
                    if v != "debug" && v != "release" {
                        return Err(render_nofile(
                            anle_error::CliCode::E503.number(),
                            "写选项",
                            &format!("`--mode` 只收 debug|release，给了 {}", v),
                            "改成 `--mode debug` 或 `--mode release`",
                        ));
                    }
                    o.mode = Some(v);
                }
                "incremental" => { o.incremental = Some(true); }
                "no-cache" => { o.incremental = Some(false); }
                "opt" => {
                    let v = val!();
                    let n: u32 = v.parse().map_err(|_| {
                        render_nofile(
                            anle_error::CliCode::E503.number(),
                            "写选项",
                            &format!("`--opt` 只收 0-3，给了 {}", v),
                            "改成 0-3 之间的数",
                        )
                    })?;
                    if n > 3 {
                        return Err(render_nofile(
                            anle_error::CliCode::E503.number(),
                            "写选项",
                            &format!("`--opt` 只收 0-3，给了 {}", n),
                            "改成 0-3 之间的数",
                        ));
                    }
                    o.opt = Some(n);
                }
                "backend" => {
                    let v = val!();
                    if v != "c" && v != "llvm" {
                        return Err(render_nofile(
                            anle_error::CliCode::E503.number(),
                            "写选项",
                            &format!("`--backend` 只收 c|llvm，给了 {}", v),
                            "改成 `--backend c`（缺省）或 `--backend llvm`",
                        ));
                    }
                    o.backend = Some(v);
                }
                "dump-ir" => o.dump_ir = true,
                "dump-tokens" => o.dump_tokens = true,
                "dump-ast" => o.dump_ast = true,
                "help" => o.help = true,
                other => {
                    return Err(render_nofile(
                        anle_error::CliCode::E502.number(),
                        "写选项",
                        &format!("不认识的选项 --{}", other),
                        "用 `anlec build -h` 看该命令支持的选项",
                    ))
                }
            }
        } else if a.starts_with('-') && a.len() > 1 {
            match a {
                "-h" => o.help = true,
                other => {
                    return Err(render_nofile(
                        anle_error::CliCode::E502.number(),
                        "写选项",
                        &format!("不认识的选项 {}", other),
                        "用 `anlec build -h` 看该命令支持的选项",
                    ))
                }
            }
        } else if o.name.is_none() {
            o.name = Some(a.to_string());
        } else {
            return Err(render_nofile(
                anle_error::CliCode::E504.number(),
                "写命令行",
                &format!("多余的参数「{}」", a),
                "去掉它——一个命令最多给一个名字",
            ));
        }
        i += 1;
    }
    Ok(o)
}

fn command_help(cmd: &str) -> String {
    let what = COMMAND_HELP
        .iter()
        .find(|(c, _)| *c == cmd)
        .map(|(_, w)| (*w).to_string())
        .unwrap_or_default();
    format!("anlec {} —— {}", cmd, what)
}


struct Target {
    //nt: 项目根
    root: PathBuf,
    //nt: 产物名
    name: String,
    //nt: 源文件
    src: PathBuf,
}

fn resolve(name: Option<&str>) -> Result<Target, String> {
    let cwd = env::current_dir().map_err(|e| {
        render_nofile(
            anle_error::EnvCode::E602.number(),
            "读当前目录",
            &format!("拿不到当前目录：{}", e),
            "检查当前目录是否存在、是否有权限",
        )
    })?;

    if let Some(n) = name {
        let as_file = cwd.join(format!("{}.an", n));
        if as_file.is_file() {
            return single(as_file);
        }
        let as_dir = cwd.join(n);
//nt: 目录工程
        if as_dir.is_dir() {
            if as_dir.join("main.an").is_file() {
                //nt: 产物名取目录 bas
                let base = as_dir
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| n.to_string());
                return Ok(Target { root: as_dir.clone(), name: base, src: as_dir });
            }
            if as_dir.join("lib.anle").is_file() {
                return project(as_dir, n.to_string());
            }
            return Err(render_nofile(
                anle_error::CliCode::E505.number(),
                "找项目",
                &format!("目录「{}」里没有 main.an——工程入口固定是 main.an", n),
                "在目录根放一个 main.an，或把名字指向别的工程",
            ));
        }
        //nt: 当前目录本身就是项目
        if cwd.join("lib.anle").is_file() {
            let kv = read_kv(&cwd.join("lib.anle"));
            if kv_get(&kv, "name") == Some(n) {
                return project(cwd.clone(), n.to_string());
            }
        }
        return Err(render_nofile(
            anle_error::CliCode::E505.number(),
            "找项目",
            &format!("找不到「{}」——当前目录下没有 {}.an，也没有同名项目目录", n, n),
            &format!("检查名字拼写，或先建 {}.an", n),
        ));
    }

    //nt: 读库根配置
    if cwd.join("lib.anle").is_file() {
        let kv = read_kv(&cwd.join("lib.anle"));
        let pkg = kv_get(&kv, "name")
            .map(String::from)
            .unwrap_or_else(|| file_name_of(&cwd));
        return project(cwd.clone(), pkg);
    }
    let mut ans = list_an(&cwd)?;
    match ans.len() {
        1 => single(ans.remove(0)),
        0 => Err(render_nofile(
            anle_error::CliCode::E505.number(),
            "找源文件",
            "当前目录既没有 lib.anle，也没有 .an 源文件——不知道要编什么",
            "给个名字（如 `anlec build hello`），或先建一个 .an",
        )),
        _ => Err(render_nofile(
            anle_error::CliCode::E505.number(),
            "找源文件",
            "当前目录有多个 .an 文件——不知道编哪个",
            "给个名字（如 `anlec build hello`）",
        )),
    }
}

fn single(f: PathBuf) -> Result<Target, String> {
    let name = f
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .ok_or_else(|| {
            render_nofile(
                anle_error::InternalCode::E701.number(),
                "",
                &format!("拿不到文件名：{}", f.display()),
                "",
            )
        })?;
    let root = f
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    Ok(Target { root, name, src: f })
}

fn project(root: PathBuf, name: String) -> Result<Target, String> {
    //nt: 约定一文件一程序
    let mut ans = list_an(&root)?;
    let src = match ans.len() {
        1 => ans.remove(0),
        0 => {
            return Err(render_nofile(
                anle_error::CliCode::E505.number(),
                "找源文件",
                &format!("项目「{}」里没有 .an 源文件", name),
                "项目根下要有一个 .an",
            ))
        }
        _ => {
            return Err(render_nofile(
                anle_error::CliCode::E505.number(),
                "找源文件",
                &format!("项目「{}」里有多个 .an——一文件一程序", name),
                "只留一个 .an",
            ))
        }
    };
    Ok(Target { root, name, src })
}

fn list_an(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let rd = fs::read_dir(dir).map_err(|e| {
        render_nofile(
            anle_error::EnvCode::E602.number(),
            "读目录",
            &format!("读不了 {}：{}", dir.display(), e),
            "检查目录是否存在、是否有读权限",
        )
    })?;
    let mut out = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().map(|x| x == "an").unwrap_or(false) {
            out.push(p);
        }
    }
    out.sort();
    Ok(out)
}

fn file_name_of(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}


fn read_kv(p: &Path) -> Vec<(String, String)> {
    let text = match fs::read_to_string(p) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            out.push((k.trim().to_string(), v.trim().trim_matches('"').to_string()));
        }
    }
    out
}

fn kv_get<'a>(kv: &'a [(String, String)], k: &str) -> Option<&'a str> {
    kv.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str())
}

//nt: 配置收敛
fn resolve_config(t: &Target, o: &Opts) -> Result<(String, u32, String), ExitCode> {
    let kv = read_kv(&t.root.join("build.anle"));
    let f_mode = kv_get(&kv, "mode").map(String::from);
    let f_opt = kv_get(&kv, "opt").and_then(|v| v.parse::<u32>().ok());
    let f_backend = kv_get(&kv, "backend").map(String::from);

    let mode = o.mode.clone().or(f_mode).unwrap_or_else(|| "debug".into());
    if mode != "debug" && mode != "release" {
        return Err(usage_err(render_nofile(
            anle_error::CliCode::E503.number(),
            "写选项",
            &format!("`--mode` 只收 debug|release，给了 {}（`build.anle` 里也一样）", mode),
            "改成 `--mode debug` 或 `--mode release`",
        )));
    }
    let opt = o.opt.or(f_opt).unwrap_or(if mode == "debug" { 0 } else { 3 });
    if opt > 3 {
        return Err(usage_err(render_nofile(
            anle_error::CliCode::E503.number(),
            "写选项",
            &format!("`--opt` 只收 0-3，给了 {}", opt),
            "改成 0-3 之间的数",
        )));
    }
    let backend = o.backend.clone().or(f_backend).unwrap_or_else(|| "c".into());
    Ok((mode, opt, backend))
}


///nt: 格式化命令
fn do_fmt(args: &[String]) -> ExitCode {
    let 只查 = args.iter().any(|a| a == "--check");
    let 名 = args.iter().find(|a| !a.starts_with('-'));
    let 路径 = match 名 {
        Some(s) => {
            let p = PathBuf::from(s);
            if p.exists() {
                p
            } else {
                let 加扩展 = p.with_extension("an");
                if 加扩展.exists() {
                    加扩展
                } else {
                    match resolve(Some(s)) {
                        Ok(t) => t.src,
                        Err(e) => return fail101(e),
                    }
                }
            }
        }
        None => match resolve(None) {
            Ok(t) => t.src,
            Err(e) => return fail101(e),
        },
    };
    match fmt::执行(&路径, 只查) {
        Ok((数, 改)) => {
            for f in &改 {
                println!("{}", f.display());
            }
            if 只查 {
                if 改.is_empty() {
                    println!("{} 个文件已符合格式", 数);
                    ExitCode::SUCCESS
                } else {
                    println!("{} / {} 个文件需要格式化", 改.len(), 数);
                    ExitCode::from(1)
                }
            } else {
                println!("已格式化 {} / {} 个文件", 改.len(), 数);
                ExitCode::SUCCESS
            }
        }
        Err(e) => fail101(render_nofile(
            anle_error::EnvCode::E602.number(),
            "格式化",
            &e,
            "检查路径是否存在、是否有写权限",
        )),
    }
}

fn run_command(cmd: &str, rest: &[String]) -> ExitCode {
    let o = match parse_opts(rest) {
        Ok(o) => o,
        Err(e) => return usage_err(e),
    };
    if o.help {
        println!("{}", command_help(cmd));
        return ExitCode::SUCCESS;
    }

    let t = match resolve(o.name.as_deref()) {
        Ok(t) => t,
        Err(e) => return fail101(e),
    };

    match cmd {
        "check" => {
            if o.dump_tokens {
                do_dump_tokens(&t)
            } else if o.dump_ast {
                do_dump_ast(&t)
            } else {
                do_check(&t)
            }
        }
        "frontend" => do_frontend(&t),
        "clean" => do_clean(&t),
        "build" | "run" => {
            let (mode, opt, backend) = match resolve_config(&t, &o) {
                Ok(v) => v,
                Err(e) => return e,
            };
            //nt: ⚠️ 实验性
            //nt: 待 P1 完整版
            let incr = o.incremental.unwrap_or(false);
            let exe = match do_build(&t, &mode, opt, o.dump_ir, incr, &backend) {
                Ok((_, exe)) => exe,
                Err(e) => return e,
            };
            if cmd == "build" {
                ExitCode::SUCCESS
            } else {
                do_run(&exe, &o.pass_args)
            }
        }
        _ => unreachable!(),
    }
}

//nt: 词法 → 语法
//nt: 多文件工程
//nt: 编译单位 = 目录；
//nt: 取一段记号
fn collect_module_refs(toks: &[anle_lexer::Token], out: &mut Vec<String>) {
    let mut i = 0usize;
    while i < toks.len() {
        if matches!(toks[i], anle_lexer::Token::HashHashColon) {
            //nt: 模块前缀名
            let mut name = String::new();
            let mut j = i + 1;
            while let Some(tok) = toks.get(j) {
                match tok {
                    anle_lexer::Token::Ident(n) | anle_lexer::Token::Str(n) => {
                        if !name.is_empty() {
                            name.push('.');
                        }
                        name.push_str(n);
                        j += 1;
                        if matches!(toks.get(j), Some(anle_lexer::Token::Sym("."))) {
                            j += 1;
                        } else {
                            break;
                        }
                    }
                    _ => break,
                }
            }
            if !name.is_empty() {
                out.push(name);
            }
            i = j.max(i + 1);
            continue;
        }
        i += 1;
    }
}

//nt: 递归走语句
fn stmts_module_refs(stmts: &[anle_parser::Stmt], out: &mut Vec<String>) {
    for s in stmts {
        match s {
            anle_parser::Stmt::Line { toks, .. } => collect_module_refs(toks, out),
            anle_parser::Stmt::Struct { head, body, .. }
            | anle_parser::Stmt::Fn { head, body, .. } => {
                collect_module_refs(head, out);
                stmts_module_refs(body, out);
            }
        }
    }
}

//nt: 递归扫工程目录，收集
//nt: 按尾名收集
fn collect_by_tail(root: &Path, tail: &str, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let rd = match fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return,
    };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            collect_by_tail(root, tail, &p, out);
        } else if p.extension().map(|x| x == "an").unwrap_or(false) {
            if p.file_name().map(|s| s == "main.an").unwrap_or(false) {
                continue;
            }
            let rel = match p.strip_prefix(root) {
                Ok(r) => r,
                Err(_) => continue,
            };
            //nt: 全路径 = 相对工程
            let mut parts: Vec<String> = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            let stem = parts.pop().unwrap_or_default();
            let stem = stem.trim_end_matches(".an").to_string();
            parts.push(stem);
            let full = parts.join(".");
            if full.rsplit('.').next().unwrap_or(&full) == tail {
                out.push((full, p));
            }
        }
    }
}

//nt: 标准库根
fn std_root() -> Option<PathBuf> {
    if let Ok(v) = env::var("ANLE_STD") {
        if !v.trim().is_empty() {
            return Some(PathBuf::from(v));
        }
    }
    let exe = env::current_exe().ok()?;
    let dir = exe.parent()?;
    for p in [
        dir.join("..").join("lib").join("anle"),
        dir.join("..").join("..").join("std"),
    ] {
        if p.join("strings.an").is_file() {
            return Some(p);
        }
    }
    None
}

fn load_project(dir: &Path) -> Result<AstProgram, ExitCode> {
    let entry = dir.join("main.an");
    if !entry.is_file() {
        return Err(fail101(render_nofile(
            anle_error::CliCode::E505.number(),
            "找工程入口",
            &format!("目录「{}」里找不到 main.an——工程入口固定是 main.an", dir.display()),
            "在目录根放一个 main.an",
        )));
    }
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut blocks: Vec<anle_parser::Block> = Vec::new();
    let mut queue: std::collections::VecDeque<(String, PathBuf)> = std::collections::VecDeque::new();
    queue.push_back((String::new(), entry));
    while let Some((modname, path)) = queue.pop_front() {
        if !seen.insert(modname.clone()) {
            continue; // 同一模块只编一次（循环引用天然无害）
        }
        let text = fs::read_to_string(&path).map_err(|e| {
            fail101(render_nofile(
                anle_error::EnvCode::E602.number(),
                "读源文件",
                &format!("读不到 {}：{}", path.display(), e),
                "检查文件是否存在、是否有读权限",
            ))
        })?;
        let toks = lex(&text)
            .map_err(|e| fail101(render_front(&e.code, &e.parts, &path, &text, e.line)))?;
        let (ast, errs) = anle_parser::parse_all(toks);
        if !errs.is_empty() {
            let 全: Vec<String> = errs
                .iter()
                .map(|e| render_front(&e.code, &e.parts, &path, &text, e.line))
                .collect();
            return Err(fail101(格式语法错(&全)));
        }
//nt: 依赖收齐
        let mut deps: Vec<String> = Vec::new();
        for b in &ast.blocks {
            stmts_module_refs(&b.stmts, &mut deps);
        }
        //nt: 合源
        //nt: 依赖模块 = 相对工
        let mut ast = ast;
        for b in &mut ast.blocks {
            b.module = modname.clone();
            //nt: 源文件路径
            b.file = path.display().to_string();
        }
        blocks.extend(ast.blocks); // main 的块在前，依赖按引用顺序追加
        for dep in deps {
            //nt: std 前缀查表
            if let Some(rest) = dep.strip_prefix("std.") {
                let root = std_root().ok_or_else(|| {
                    fail101(render_nofile(
                        anle_error::CliCode::E505.number(),
                        "找标准库",
                        "找不到标准库根——设 `ANLE_STD`，或按 SDK 布局放 `<SDK>/lib/anle`",
                        "把标准库目录放到上述位置，或设 ANLE_STD",
                    ))
                })?;
                let p = root
                    .join(rest.replace('.', std::path::MAIN_SEPARATOR_STR))
                    .with_extension("an");
                if p.is_file() {
                    queue.push_back((dep.clone(), p));
                    continue;
                }
                return Err(fail101(render_nofile(
                    anle_error::CliCode::E505.number(),
                    "找标准库",
                    &format!("标准库里找不到模块「{}」", dep),
                    &format!("确认 {} 存在", p.display()),
                )));
            }
            let rel = dep.replace('.', std::path::MAIN_SEPARATOR_STR);
            let direct = dir.join(&rel).with_extension("an");
            let want = direct.display().to_string();
//nt: 模块路径尾名退回
            //nt: 先按给定名当全路径找
            let resolved: Option<(String, std::path::PathBuf)> = if direct.is_file() {
                Some((dep.clone(), direct))
            } else {
                let tail = dep.rsplit('.').next().unwrap_or(&dep).to_string();
                let mut hits: Vec<(String, std::path::PathBuf)> = Vec::new();
                collect_by_tail(dir, &tail, dir, &mut hits);
                match hits.len() {
                    1 => hits.into_iter().next(),
                    0 => None,
                    _ => {
                        let names: Vec<String> = hits.iter().map(|(m, _)| m.clone()).collect();
                        return Err(fail101(render_nofile(
                            anle_error::CliCode::E505.number(),
                            "找模块",
                            &format!(
                                "模块「{}」有歧义——{} 都叫这名，请写全路径",
                                dep,
                                names.join("、")
                            ),
                            &format!("把引用改成 `##:{}`（或别的全路径）", names[0]),
                        )));
                    }
                }
            };
            match resolved {
                Some((full, p)) => queue.push_back((full, p)),
                None => {
                    return Err(fail101(render_nofile(
                        anle_error::CliCode::E505.number(),
                        "找模块",
                        &format!("模块「{}」找不到——模块名按目录路径映射", dep),
                        &format!("确认 {} 存在，或把引用写全", want),
                    )))
                }
            }
        }
    }
    Ok(AstProgram { blocks })
}

fn front(t: &Target) -> Result<AstProgram, ExitCode> {
//nt: 目录 = 多文件工程
    if t.src.is_dir() {
        return load_project(&t.src);
    }
    let text = match fs::read_to_string(&t.src) {
        Ok(x) => x,
        Err(e) => {
            return Err(fail101(render_nofile(
                anle_error::EnvCode::E602.number(),
                "读源文件",
                &format!("读不到 {}：{}", t.src.display(), e),
                "检查文件是否存在、是否有读权限",
            )))
        }
    };
    if text.starts_with('\u{feff}') {
        return Err(fail101(render_file(
            anle_error::LexCode::E007.number(),
            "读源文件",
            "源文件带 BOM——契约要求 UTF-8 无 BOM",
            "用编辑器把编码改成「UTF-8（不带 BOM）」再编",
            &t.src,
        )));
    }
    let toks = match lex(&text) {
        Ok(x) => x,
        Err(e) => return Err(fail101(render_front(&e.code, &e.parts, &t.src, &text, e.line))),
    };
    let (ast, errs) = anle_parser::parse_all(toks);
    if !errs.is_empty() {
        let 全: Vec<String> = errs
            .iter()
            .map(|e| render_front(&e.code, &e.parts, &t.src, &text, e.line))
            .collect();
        return Err(fail101(格式语法错(&全)));
    }
    Ok(ast)
}

//nt: 构建错误
fn render_coded(code: &str, message: &str, line: usize, err_file: &std::path::Path, text: &str) -> String {
    //nt: 显示名 = 错误实际
    let file = err_file
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    //nt: 源码行
    let eff_text = if err_file.as_os_str().is_empty() {
        text.to_string()
    } else {
        std::fs::read_to_string(err_file).unwrap_or_default()
    };
    let lines: Vec<&str> = eff_text.lines().collect();
    let src_line = if line > 0 && line <= lines.len() {
        lines[line - 1]
    } else {
        ""
    };
    let 哪 = if line > 0 {
        format!("第 {} 行", line)
    } else {
        String::new()
    };
    let (de, dd) = anle_error::code_defaults(code);
    let parts = anle_error::DiagParts::full(哪, "", message, "", "")
        .预期(de)
        .详解(dd);
    let cx = anle_error::RenderCtx { code, file: &file, src_line };
    anle_error::render_full(&parts, anle_error::Tone::Fix, &cx)
}

//nt: 词法 / 语法错误
fn render_front(
    code: &str,
    parts: &anle_error::DiagParts,
    src: &std::path::Path,
    text: &str,
    line: usize,
) -> String {
    let file = src
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    let src_line = if line > 0 && line <= lines.len() {
        lines[line - 1]
    } else {
        ""
    };
    let (de, dd) = anle_error::code_defaults(code);
    let mut p = parts.clone();
    if p.预期.is_empty() {
        p.预期 = de.to_string();
    }
    if p.详解.is_empty() {
        p.详解 = dd.to_string();
    }
    let cx = anle_error::RenderCtx { code, file: &file, src_line };
    anle_error::render_full(&p, anle_error::Tone::Fix, &cx)
}

fn print_diags(checks: &anle_check::CheckResult, src_path: &std::path::Path, src_text: &str) {
    eprintln!("检查未通过，共 {} 个错误：", checks.errors().len());
    let mut cache: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for d in &checks.diags {
        //nt: 错误归自身文件
        let (file, text): (String, &str) = if d.file.is_empty() {
            (
                src_path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                src_text,
            )
        } else {
            let key = d.file.clone();
            if !cache.contains_key(&key) {
                let t = fs::read_to_string(&key).unwrap_or_default();
                cache.insert(key.clone(), t);
            }
            let name = std::path::Path::new(&key)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| key.clone());
            (name, cache.get(&key).map(|s| s.as_str()).unwrap_or(""))
        };
        let lines: Vec<&str> = text.lines().collect();
        let src_line = if d.line > 0 && d.line <= lines.len() {
            lines[d.line - 1]
        } else {
            ""
        };
        let 哪 = if d.col > 0 {
            format!("第 {} 行第 {} 列", d.line, d.col)
        } else if d.line > 0 {
            format!("第 {} 行", d.line)
        } else {
            String::new()
        };
        let mut parts = d.parts.clone();
        if !哪.is_empty() {
            parts.哪 = 哪;
        }
        let ctx = anle_error::RenderCtx { code: &d.code, file: &file, src_line };
        let full = anle_error::render_full(&parts, d.tone, &ctx);
        if full.trim().is_empty() || parts.缺.is_empty() {
            eprintln!("  {}", d.message); // 非结构化诊断（如上限提示）：原样输出
        } else {
            eprintln!("  {}", full);
        }
    }
}

//nt: 只跑前端
//nt: 用于性能测量
fn do_frontend(t: &Target) -> ExitCode {
    use std::time::Instant;
    let src_file = if t.src.is_dir() {
        t.src.join("main.an")
    } else {
        t.src.clone()
    };
    let src_text = fs::read_to_string(&src_file).unwrap_or_default();
    let lines = src_text.lines().count();
    let bytes = src_text.len();

    let mut best = f64::MAX;
    #[allow(unused_mut)]
    let mut n_fn = 0usize;
    for _ in 0..3 {
        let t0 = Instant::now();
        let ast = match front(t) {
            Ok(a) => a,
            Err(e) => return e,
        };
        let t_parse = t0.elapsed().as_secs_f64();
        let checks = check(&ast);
        let t_check = t0.elapsed().as_secs_f64();
        if !checks.ok() {
            print_diags(&checks, &src_file, &src_text);
            return ExitCode::from(101);
        }
        #[cfg(feature = "backend")]
        let ir = match build_ir(&ast) {
            Ok(p) => p,
            Err(e) => {
                return fail101(render_coded(
                    &e.code,
                    &e.message,
                    e.line,
                    std::path::Path::new(&e.file),
                    &src_text,
                ))
            }
        };
        let dt = t0.elapsed().as_secs_f64();
        eprintln!("[分段] 语法 {:.3}s | 检查 {:.3}s | 中间表示 {:.3}s", t_parse, t_check - t_parse, dt - t_check);
        if dt < best {
            best = dt;
            #[cfg(feature = "backend")]
            {
                n_fn = ir.functions.len();
            }
        }
    }
    println!(
        "[前端] 耗时 {:.3}s | 行数 {} | 字节 {} | 函数 {} | 吞吐 {:.0} 行/秒",
        best,
        lines,
        bytes,
        n_fn,
        if best > 0.0 { lines as f64 / best } else { 0.0 }
    );
    ExitCode::SUCCESS
}

//nt: 令牌类码
fn 令牌码(t: &anle_lexer::Token) -> u32 {
    use anle_lexer::Token as T;
    match t {
        T::Newline => 0,
        T::RawAsm(_) => 1,
        T::Str(_) => 2,
        T::RawStr(_) => 3,
        T::Int(_) => 4,
        T::Float(_) => 5,
        T::Ident(_) => 6,
        T::Kw(_) => 7,
        T::Sym(_) => 8,
        T::HashColon => 9,
        T::HashHashColon => 10,
        T::HashHashHashColon => 11,
    }
}

//nt: 词法快照
fn do_dump_tokens(t: &Target) -> ExitCode {
    let src_file = if t.src.is_dir() { t.src.join("main.an") } else { t.src.clone() };
    let src_text = match fs::read_to_string(&src_file) {
        Ok(s) => s,
        Err(e) => {
            return fail101(render_nofile(
                anle_error::EnvCode::E602.number(),
                "读源文件",
                &format!("读不了 {}：{}", src_file.display(), e),
                "检查路径是否存在、是否有读权限",
            ))
        }
    };
    match anle_lexer::lex(&src_text) {
        Ok(toks) => {
            let mut 出 = String::new();
            for t in toks {
                //nt: 一项四字段
                出.push_str(&format!("{}\n{}\n{}\n{}\n", t.line, t.col, t.len, 令牌码(&t.tok)));
            }
            print!("{}", 出);
            ExitCode::SUCCESS
        }
        Err(e) => {
            println!("ERR\n{}\n{}", e.line, e.code);
            ExitCode::from(101)
        }
    }
}

//nt: 语法快照
fn do_dump_ast(t: &Target) -> ExitCode {
    let src_file = if t.src.is_dir() { t.src.join("main.an") } else { t.src.clone() };
    let src_text = match fs::read_to_string(&src_file) {
        Ok(s) => s,
        Err(e) => {
            return fail101(render_nofile(
                anle_error::EnvCode::E602.number(),
                "读源文件",
                &format!("读不了 {}：{}", src_file.display(), e),
                "检查路径是否存在、是否有读权限",
            ))
        }
    };
    let toks = match anle_lexer::lex(&src_text) {
        Ok(v) => v,
        Err(e) => {
            println!("ERR\n{}\n{}", e.line, e.code);
            return ExitCode::from(101);
        }
    };
    let (ast, errs) = anle_parser::parse_all(toks);
    if let Some(e) = errs.first() {
        println!("ERR\n{}\n{}", e.line, e.code);
        return ExitCode::from(101);
    }
    let mut 出 = String::new();
    for b in &ast.blocks {
        快照块(b, &mut 出);
    }
    print!("{}", 出);
    ExitCode::SUCCESS
}

//nt: 块后序
fn 快照块(b: &anle_parser::Block, out: &mut String) {
    for s in &b.stmts {
        快照句(s, 1, out);
    }
    let attrs = b.attrs.join(",");
    out.push_str(&format!(
        "B\t0\t{}\t{}\t{}\t{}\t{}\t{}\n",
        b.line,
        b.col,
        b.len,
        b.name,
        attrs,
        b.asm_lines.len()
    ));
}

//nt: 语句后序
fn 快照句(s: &anle_parser::Stmt, d: usize, out: &mut String) {
    use anle_parser::Stmt as S;
    let (kind, line, col, len, head, body) = match s {
        S::Line { line, col, len, toks } => ("L", *line, *col, *len, toks, None),
        S::Struct { line, col, len, head, body } => ("S", *line, *col, *len, head, Some(body)),
        S::Fn { line, col, len, head, body } => ("F", *line, *col, *len, head, Some(body)),
    };
    if let Some(body) = body {
        for x in body {
            快照句(x, d + 1, out);
        }
    }
    out.push_str(&format!("{}\t{}\t{}\t{}\t{}\t{}", kind, d, line, col, len, head.len()));
    for i in 0..head.len() {
        let (c, l) = head.span(i).unwrap_or((0, 0));
        out.push_str(&format!("\t{}\t{}\t{}", c, l, 令牌码(&head[i])));
    }
    out.push('\n');
}

fn do_check(t: &Target) -> ExitCode {
    let ast = match front(t) {
        Ok(a) => a,
        Err(e) => return e,
    };
    let checks = check(&ast);
    if checks.ok() {
        println!("检查通过：{}", t.src.display());
        ExitCode::SUCCESS
    } else {
        let src_file = if t.src.is_dir() { t.src.join("main.an") } else { t.src.clone() };
        let src_text = fs::read_to_string(&src_file).unwrap_or_default();
        print_diags(&checks, &src_file, &src_text);
        ExitCode::from(101)
    }
}

//nt: 编译+链接；成功返回
//nt: 调试模式编译参数
fn cc_opt_args(mode: &str, opt: u32) -> Vec<String> {
    if mode == "debug" {
        vec!["-O0".to_string(), "-g".to_string()]
    } else {
        //nt: release 抽护栏
        vec![format!("-O{}", opt), "-DANLE_NOGUARD".to_string(), "-s".to_string()]
    }
}

#[cfg(feature = "llvm")]
fn do_build_llvm(
    ir: &anle_ir::Program,
    src_text: &str,
    src_file: &str,
    mode: &str,
    opt: u32,
    dir: &Path,
    exe: &Path,
    name: &str,
) -> Result<(), ExitCode> {
    let cc = 找cc();
    let rt_c = dir.join("anle_rt.c");
    let rt_o = dir.join("anle_rt.o");
    if let Err(e) = fs::write(&rt_c, anle_codegen::runtime_c_for(src_file, src_text)) {
        return Err(fail101(render_nofile(
            anle_error::EnvCode::E602.number(),
            "写运行时",
            &format!("写不出 {}：{}", rt_c.display(), e),
            "检查路径是否有写权限",
        )));
    }
    let st = Command::new(&cc)
        .arg("-O2")
        //nt: release 抽护栏
        .args(if mode == "release" { vec!["-DANLE_NOGUARD".to_string()] } else { vec![] })
        .arg("-c")
        .arg(&rt_c)
        .arg("-o")
        .arg(&rt_o)
        .status();
    match st {
        Ok(s) if s.success() => {}
        Ok(_) => return Err(fail101(render_nofile(
            anle_error::GenCode::E401.number(),
            "编译运行时",
            "C 运行时编译失败",
            "检查 clang 是否可用",
        ))),
        Err(e) => return Err(fail101(render_nofile(
            anle_error::EnvCode::E601.number(),
            "调编译器",
            &format!("调不起编译器「{}」：{}", cc, e),
            "设 ANLE_CC 指向可用的 clang",
        ))),
    }
    let obj = dir.join(format!("{}.o", name));
    if let Err(e) = emit_object(ir, &obj, opt, mode == "debug", src_file) {
        return Err(fail101(render_nofile(
            anle_error::GenCode::E401.number(),
            "LLVM 后端",
            &e,
            "修正源码或改用 --backend c",
        )));
    }
    let st = Command::new(&cc).arg(&obj).arg(&rt_o).arg("-o").arg(exe).status();
    match st {
        Ok(s) if s.success() => Ok(()),
        Ok(_) => Err(fail101(render_nofile(
            anle_error::GenCode::E401.number(),
            "链接",
            "LLVM 目标文件链接失败",
            "检查 clang 是否可用",
        ))),
        Err(e) => Err(fail101(render_nofile(
            anle_error::EnvCode::E601.number(),
            "调编译器",
            &format!("调不起编译器「{}」：{}", cc, e),
            "设 ANLE_CC 指向可用的 clang",
        ))),
    }
}


// nt: 精简构建不含后端
#[cfg(not(feature = "backend"))]
fn do_build(
    _t: &Target,
    _mode: &str,
    _opt: u32,
    _dump_ir: bool,
    _incr: bool,
    _backend: &str,
) -> Result<(PathBuf, PathBuf), ExitCode> {
    Err(fail101(
        "这个 anlec 是精简构建（只有诊断与语言服务），不含代码生成——编译请换完整版",
    ))
}

#[cfg(feature = "backend")]
fn do_build(t: &Target, mode: &str, opt: u32, dump_ir: bool, incr: bool, backend: &str) -> Result<(PathBuf, PathBuf), ExitCode> {
    let ast = front(t)?;
    //nt: 目录工程 ⇒ 报错以
    let src_file = if t.src.is_dir() { t.src.join("main.an") } else { t.src.clone() };
    let src_text = fs::read_to_string(&src_file).unwrap_or_default();
    let checks = check(&ast);
    if !checks.ok() {
        print_diags(&checks, &src_file, &src_text);
        return Err(ExitCode::from(101));
    }
    let ir = match build_ir(&ast) {
        Ok(p) => p,
        Err(e) => return Err(fail101(render_coded(&e.code, &e.message, e.line, std::path::Path::new(&e.file), &src_text))),
    };
    if dump_ir {
        println!("{}", ir.dump());
    }
    let dir = t.root.join("build").join(mode);
    if let Err(e) = fs::create_dir_all(&dir) {
        return Err(fail101(render_nofile(
            anle_error::EnvCode::E602.number(),
            "建产物目录",
            &format!("建不了产物目录 {}：{}", dir.display(), e),
            "检查路径是否有写权限",
        )));
    }
    let c_path = dir.join(format!("{}.c", t.name));
    let mut exe = dir.join(&t.name);
    if cfg!(windows) {
        exe.set_extension("exe");
    }

    if backend == "llvm" {
        #[cfg(feature = "llvm")]
        {
            let fname = src_file.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            do_build_llvm(&ir, &src_text, &fname, mode, opt, &dir, &exe, &t.name)?;
            return Ok((dir, exe));
        }
        #[cfg(not(feature = "llvm"))]
        {
            return Err(fail101(
                "这个 anlec 构建未启用 LLVM 后端——请改用 --backend c，或用默认特性重新构建",
            ));
        }
    }

    let c_src = match lower_to_c(
        &ir,
        &src_file.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
        &src_text,
        mode == "debug",
    ) {
        Ok(s) => s,
        Err(e) => return Err(fail101(render_coded(&e.code, &e.message, e.line, std::path::Path::new(""), &src_text))),
    };

    if incr {
        let (prelude, units) = anle_codegen::split_parts(&c_src);
        let cache = dir.join(".cache");
        if let Err(e) = fs::create_dir_all(&cache) {
            return Err(fail101(render_nofile(
                anle_error::EnvCode::E602.number(),
                "建缓存目录",
                &format!("建不了 {}：{}", cache.display(), e),
                "检查路径是否有写权限",
            )));
        }
        let cc = 找cc();
        let ver = env!("CARGO_PKG_VERSION");
        let mut objs: Vec<PathBuf> = Vec::new();
        let mut hit = 0usize;
        for (idx, code) in &units {
            let key = cache_unit_key(&[&prelude_iface(&prelude), code, &opt.to_string(), ver]);
            let obj = cache.join(format!("u{}-{}.o", idx, key));
            if obj.exists() {
                hit += 1;
            } else {
                let uc = cache.join(format!("u{}-{}.c", idx, key));
                if let Err(e) = fs::write(&uc, format!("{}{}", prelude, code)) {
                    return Err(fail101(render_nofile(
                        anle_error::EnvCode::E602.number(),
                        "写编译单元",
                        &format!("写不出 {}：{}", uc.display(), e),
                        "检查路径是否有写权限",
                    )));
                }
                let st = match Command::new(&cc)
                    .args(cc_opt_args(mode, opt))
                    .arg("-c")
                    .arg(&uc)
                    .arg("-o")
                    .arg(&obj)
                    .status()
                {
                    Ok(s) => s,
                    Err(e) => {
                        return Err(fail101(render_nofile(
                            anle_error::EnvCode::E601.number(),
                            "调编译器",
                            &format!("调不起编译器「{}」：{}", cc, e),
                            "设 `ANLE_CC` 指向可用的 clang（装了 SDK 的 clang 组件会自动用上）",
                        )))
                    }
                };
                if !st.success() {
                    return Err(fail101(render_nofile(
                        anle_error::GenCode::E403.number(),
                        "编编译单元",
                        &format!("单元 {} 编译失败", idx),
                        "把源码与 C 报错一起提交到 issue",
                    )));
                }
            }
            objs.push(obj);
        }
        println!("[增量] 单元 {} 个｜缓存命中 {}｜新编译 {}", units.len(), hit, units.len() - hit);
        let mut cmd = Command::new(&cc);
        for o in &objs {
            cmd.arg(o);
        }
        let st = match cmd.arg("-o").arg(&exe).status() {
            Ok(s) => s,
            Err(e) => {
                return Err(fail101(render_nofile(
                    anle_error::EnvCode::E601.number(),
                    "链接",
                    &format!("调不起链接器「{}」：{}", cc, e),
                    "设 `ANLE_CC` 指向可用的 clang（装了 SDK 的 clang 组件会自动用上）",
                )))
            }
        };
        if !st.success() {
            return Err(fail101(render_nofile(
                anle_error::GenCode::E403.number(),
                "链接产物",
                "链接失败",
                "把源码与上面的 C 报错一起提交到项目 issue",
            )));
        }
        println!("可执行：{}", exe.display());
        return Ok((c_path, exe));
    }
    if let Err(e) = fs::write(&c_path, &c_src) {
        return Err(fail101(render_nofile(
            anle_error::EnvCode::E602.number(),
            "写中间产物",
            &format!("写不出 {}：{}", c_path.display(), e),
            "检查路径是否有写权限、文件是否被占用",
        )));
    }
    println!("中间产物：{}", c_path.display());

    let cc = 找cc();
    let status = match Command::new(&cc)
        .args(cc_opt_args(mode, opt))
        .arg(&c_path)
        .arg("-o")
        .arg(&exe)
        .status()
    {
        Ok(s) => s,
        Err(e) => {
            return Err(fail101(render_nofile(
                anle_error::EnvCode::E601.number(),
                "调编译器",
            &format!("调不起编译器「{}」：{}", cc, e),
            "设 `ANLE_CC` 指向可用的 clang（MSVC target 与 GNU target 均可；装了 SDK 的 clang 组件会自动用上）",
        )))
        }
    };
    if !status.success() {
        return Err(fail101(render_nofile(
            anle_error::GenCode::E403.number(),
            "编产物",
            &format!("编译器失败（退出码 {:?}）", status.code()),
            "把源码与上面的 C 报错一起提交到项目 issue",
        )));
    }
    println!("可执行：{}", exe.display());
    Ok((c_path, exe))
}

//nt: 运行被编译的程序；退
fn do_run(exe: &Path, args: &[String]) -> ExitCode {
    let st = match Command::new(exe).args(args).status() {
        Ok(s) => s,
        Err(e) => {
            return fail101(render_nofile(
                anle_error::EnvCode::E603.number(),
                "跑程序",
                &format!("跑不起 {}：{}", exe.display(), e),
                "检查产物是否存在、是否有可执行权限",
            ))
        }
    };
    match st.code() {
        Some(0) => ExitCode::SUCCESS,
        Some(c) if (1..=255).contains(&c) => ExitCode::from(c as u8),
        Some(c) => fail101(render_nofile(
            anle_error::EnvCode::E604.number(),
            "交退出码",
            &format!("程序退出码 {}（超出 8 位，无法透传）", c),
            "程序退出码要在 0–255 之间才能原样交出去",
        )),
        None => fail101(render_nofile(
            anle_error::EnvCode::E604.number(),
            "交退出码",
            "程序被信号终止（没有可透传的退出码）",
            "看程序是不是被 Ctrl-C 打断、被系统杀掉或自己崩了",
        )),
    }
}

//nt: 去掉 prelude
//nt: 但它只影响报错行号、
fn prelude_iface(prelude: &str) -> String {
    const MARK: &str = "ANLE_SRC[] = {";
    match prelude.find(MARK) {
        None => prelude.to_string(),
        Some(i) => {
            let start = prelude[..i].rfind('\n').map(|x| x).unwrap_or(i);
            match prelude[i..].find("};") {
                None => prelude.to_string(),
                Some(j) => {
                    let end = i + j + 2;
                    format!("{}{}", &prelude[..start], &prelude[end..])
                }
            }
        }
    }
}

//nt: 编译单元的缓存键
fn cache_unit_key(parts: &[&str]) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    for p in parts {
        p.hash(&mut h);
    }
    format!("{:016x}", h.finish())
}

fn do_clean(t: &Target) -> ExitCode {
    let bd = t.root.join("build");
    if !bd.exists() {
        println!("没有可清理的（{} 不存在）", bd.display());
        return ExitCode::SUCCESS;
    }
    match fs::remove_dir_all(&bd) {
        Ok(_) => {
            println!("已清理：{}", bd.display());
            ExitCode::SUCCESS
        }
        Err(e) => fail101(render_nofile(
            anle_error::EnvCode::E602.number(),
            "清产物目录",
            &format!("清不了 {}：{}", bd.display(), e),
            "检查是否有权限、目录是否被别的程序占用",
        )),
    }
}
// nt: 多错输出
fn 格式语法错(各: &[String]) -> String {
    if 各.len() == 1 {
        return 各[0].clone();
    }
    format!("{}（共 {} 处语法错）", 各.join("\n"), 各.len())
}
