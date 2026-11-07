//! 分配统计
//!
//! 包一层全局分配器，统计分配次数与字节数；只在 `ANLE_ALLOC_STAT=1` 时输出。
//! 计数用原子操作，开销远小于一次分配，可以常开。

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

//nt: 分配次数
static 次数: AtomicUsize = AtomicUsize::new(0);
//nt: 分配字节
static 字节: AtomicUsize = AtomicUsize::new(0);
//nt: 释放次数
static 释放: AtomicUsize = AtomicUsize::new(0);

//nt: 计数分配器
pub struct 计数;

unsafe impl GlobalAlloc for 计数 {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        次数.fetch_add(1, Ordering::Relaxed);
        字节.fetch_add(l.size(), Ordering::Relaxed);
        System.alloc(l)
    }

    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        释放.fetch_add(1, Ordering::Relaxed);
        System.dealloc(p, l)
    }

    unsafe fn realloc(&self, p: *mut u8, l: Layout, 新: usize) -> *mut u8 {
        次数.fetch_add(1, Ordering::Relaxed);
        字节.fetch_add(新, Ordering::Relaxed);
        System.realloc(p, l, 新)
    }
}

//nt: 打印统计
pub fn 报告() {
    if std::env::var("ANLE_ALLOC_STAT").is_err() {
        return;
    }
    let a = 次数.load(Ordering::Relaxed);
    let b = 字节.load(Ordering::Relaxed);
    let d = 释放.load(Ordering::Relaxed);
    eprintln!(
        "[分配统计] 次数 {}，字节 {}（{:.1} MB），释放 {}",
        a,
        b,
        b as f64 / 1048576.0,
        d
    );
}
