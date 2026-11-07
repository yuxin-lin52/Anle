//! 编译期求值配额
//!
//! 两道闸门：深度防栈溢出，步数防挂死。两者都可用命令行调高，0 表示不设上限。
//! 计的是一次编译的累计量，每次 build 开头重置。

use std::cell::Cell;

//nt: 默认深度
pub const 默认深度: usize = 256;
//nt: 默认步数
pub const 默认步数: usize = 1_280_000;

thread_local! {
    static 深限: Cell<usize> = const { Cell::new(默认深度) };
    static 步限: Cell<usize> = const { Cell::new(默认步数) };
    static 已用: Cell<usize> = const { Cell::new(0) };
    static 超额: Cell<Option<&'static str>> = const { Cell::new(None) };
}

//nt: 设配额并重置
pub fn 设(深: usize, 步: usize) {
    深限.with(|v| v.set(深));
    步限.with(|v| v.set(步));
    已用.with(|v| v.set(0));
    超额.with(|v| v.set(None));
}

//nt: 记若干步
pub fn 记步(n: usize) {
    已用.with(|v| v.set(v.get().saturating_add(n)));
    let 限 = 步限.with(|v| v.get());
    if 限 != 0 && 已用.with(|v| v.get()) > 限 {
        超额.with(|v| v.set(Some("步数")));
    }
}

//nt: 进一层调用
pub fn 可入(深: usize) -> bool {
    let 限 = 深限.with(|v| v.get());
    if 限 != 0 && 深 > 限 {
        超额.with(|v| v.set(Some("深度")));
        return false;
    }
    记步(1);
    超额.with(|v| v.get()).is_none()
}

//nt: 超限项与上限
pub fn 超限() -> Option<(&'static str, usize)> {
    let 项 = 超额.with(|v| v.get())?;
    let 限 = if 项 == "深度" {
        深限.with(|v| v.get())
    } else {
        步限.with(|v| v.get())
    };
    Some((项, 限))
}
