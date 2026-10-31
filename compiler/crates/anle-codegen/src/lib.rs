//! anle-codegen —— 后端之一：中间表示 → C 源码
//!
//! 另一条 lower 在 `anle-llvm`。
//!
//! 覆盖：算术、控制流、容器、字符串、结构体、链式执行、运行子程序。
//! 编码约定：`print` 走 libc 的 `printf`；局部变量映射为 `v<序号>`。

use anle_ir::{
    BinOp, Block, Exit, Function, Local, LocalId, Place, Proj, Program, Rvalue, Stmt, Ty, Value,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

// nt: 后端错误
#[derive(Debug, Clone, PartialEq)]
pub struct CodegenError {
    pub message: String,
// nt: 错误码 E4xx 段
    pub code: String,
    // nt: 源行号
    pub line: usize,
}

// nt: 积类型表
type TypeMap = BTreeMap<String, (String, Vec<(String, Ty)>)>;

thread_local! {
    // nt: 调试模式插行号
    static DEBUG_LINES: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    // nt: 源文件名
    static SRC_FILE: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    // nt: 各文件源码行
    static SRC_LINES: std::cell::RefCell<BTreeMap<String, Vec<String>>> =
        const { std::cell::RefCell::new(BTreeMap::new()) };
}

// nt: C 类型映射
fn c_ty_in(t: &Ty, types: &TypeMap) -> String {
    let n = t.0.as_str();
    if let Some(base) = n.strip_suffix('*') {
        if let Some((c, _)) = types.get(base) {
            return format!("{}*", c); // 把手
        }
    }
    if let Some((c, _)) = types.get(n) {
        return c.clone(); // 积类型（值聚合）
    }
    match n {
        "int" => "long long".to_string(),
        // nt: 定长族 → C 原生
        "int_8" => "signed char".to_string(),
        "uint_8" => "unsigned char".to_string(),
        "int_16" => "short".to_string(),
        "uint_16" => "unsigned short".to_string(),
        "int_32" => "int".to_string(),
        "uint_32" => "unsigned int".to_string(),
        "int_64" => "long long".to_string(),
        "uint_64" => "unsigned long long".to_string(),
        "float" => "double".to_string(),
        "bool" => "long long".to_string(),
        "str" => "const char*".to_string(),
        // nt: 指针
        n if n.ends_with('*') => {
            format!("{}*", c_ty_in(&Ty(n[..n.len() - 1].to_string()), types))
        }
        // nt: 容器
        n if n.starts_with("list") || n.starts_with("que") || n.starts_with("stack") => {
            "AnleList*".to_string()
        }
        n if n.starts_with("map") => "AnleMap*".to_string(),
        _ => "long long".to_string(),
    }
}

// nt: list 的最小 C
const ANLE_LIST_RT: &str = r#"const char* anle_str_copy(const char* s);
\
\

\

     typedef struct { long long len; long long cap; long long esz; long long sgn; char* d; } AnleList;
\
     static long long anle_f2s(double d) { long long v; memcpy(&v, &d, 8); return v; }
     static double anle_s2f(long long v) { double d; memcpy(&d, &v, 8); return d; }
\
     long long anle_slot_get(const char* p, long long esz, long long sgn) {
  long long v = 0;
  memcpy(&v, p, (size_t)esz);
  if (sgn && esz < 8) { long long sh = 64 - esz * 8; v = (long long)(((unsigned long long)v) << sh) >> sh; }
  return v;
}
\
     void anle_slot_put(char* p, long long esz, long long v) { memcpy(p, &v, (size_t)esz); }
\
     AnleList* anle_list_new(long long esz, long long sgn) {
  AnleList* l = (AnleList*)anle_region_alloc(anle_region_cur, (long long)sizeof(AnleList));
  l->len = 0; l->cap = 4; l->esz = esz; l->sgn = sgn;
  l->d = (char*)anle_region_alloc(anle_region_cur, (long long)(esz * 4));
  if (!l || !l->d) { anle_die_nc(6, "内存不够——程序申请的空间超出可用内存", "缩小数据量，或换更省内存的写法"); }
  return l;
}
\
     char* anle_list_slot(AnleList* l, long long i) { return l->d + i * l->esz; }
\
     AnleList* anle_list_dc_parent(AnleList* h) {
  if (!h) return 0;
  AnleRegion* t = anle_region_cur ? anle_region_cur->parent : 0;
  if (!t) t = anle_region_cur;
  AnleList* n = (AnleList*)anle_region_alloc(t, (long long)sizeof(AnleList));
  n->len = h->len; n->cap = h->cap; n->esz = h->esz; n->sgn = h->sgn;
  n->d = (char*)anle_region_alloc(t, (long long)(h->esz * (h->cap > 0 ? h->cap : 1)));
  if (h->len > 0) memcpy(n->d, h->d, (size_t)(h->esz * h->len));
  return n;
}
\
     void anle_list_die(long long i, long long n) {
  char b[192];
  snprintf(b, sizeof b, "位置 %lld 越界——这个容器只有 %lld 个位置", i, n);
  anle_die_nc(4, b, "取之前先用 len(容器) 查一下个数");
}
\
     long long anle_list_len(AnleList* l) { return l ? l->len : 0; }
\
     /* 聚合元素：整块字节拷 */
     void anle_list_copy_at(AnleList* l, long long i, char* dst) {
  ANLE_GUARD(if (!l || i < 1 || i > l->len) anle_list_die(i, l ? l->len : 0);)
  memcpy(dst, anle_list_slot(l, i - 1), (size_t)l->esz);
}

     void anle_list_push_from(AnleList* l, const char* src) {
  if (l->len >= l->cap) { long long ncap = l->cap * 2; char* nd = (char*)anle_region_alloc(anle_region_cur, (long long)(l->esz * ncap)); if (l->d && l->len > 0) memcpy(nd, l->d, (size_t)(l->esz * l->len)); l->cap = ncap; l->d = nd; }
  memcpy(anle_list_slot(l, l->len), src, (size_t)l->esz);
  l->len = l->len + 1;
}

     void anle_list_setat_from(AnleList* l, long long i, const char* src) {
  ANLE_GUARD(if (!l || i < 1 || i > l->len) anle_list_die(i, l ? l->len : 0);)
  memcpy(anle_list_slot(l, i - 1), src, (size_t)l->esz);
}


     AnleList* anle_list_lit_from(long long n, const char* v, long long esz) {
  AnleList* l = anle_list_new(esz, 0);
  long long i = 0;
  while (i < n) { memcpy(anle_list_slot(l, i), v + i * esz, (size_t)esz); i = i + 1; }
  l->len = n;
  return l;
}
\
     long long anle_list_at(AnleList* l, long long i) {
  ANLE_GUARD(if (!l || i < 1 || i > l->len) anle_list_die(i, l ? l->len : 0);)
  return anle_slot_get(anle_list_slot(l, i - 1), l->esz, l->sgn);
}
\
     void anle_list_setat(AnleList* l, long long i, long long v) {
  ANLE_GUARD(if (!l || i < 1 || i > l->len) anle_list_die(i, l ? l->len : 0);)
  anle_slot_put(anle_list_slot(l, i - 1), l->esz, v);
}
\
     long long anle_list_push(AnleList* l, long long v) {
  if (l->len >= l->cap) { long long ncap = l->cap * 2; char* nd = (char*)anle_region_alloc(anle_region_cur, (long long)(l->esz * ncap)); if (l->d && l->len > 0) memcpy(nd, l->d, (size_t)(l->esz * l->len)); l->cap = ncap; l->d = nd; }
  anle_slot_put(anle_list_slot(l, l->len), l->esz, v);
  l->len = l->len + 1;
  return l->len;
}
\
     long long anle_list_insert_from(AnleList* l, long long pos, const char* src) {
  ANLE_GUARD(if (!l || pos < 1 || pos > l->len + 1) anle_list_die(pos, l ? l->len + 1 : 0);)
  anle_list_push(l, 0);
  long long i = l->len - 1;
  while (i >= pos) { memmove(anle_list_slot(l, i), anle_list_slot(l, i - 1), (size_t)l->esz); i = i - 1; }
  memcpy(anle_list_slot(l, pos - 1), src, (size_t)l->esz);
  return l->len;
}
     AnleList* anle_list_lit(long long n, const long long* v, long long esz, long long sgn) {
  AnleList* l = anle_list_new(esz, sgn);
  long long i = 0;
  while (i < n) { anle_list_push(l, v[i]); i = i + 1; }
  return l;
}
\
     void anle_list_print(AnleList* l) {
  printf("[");
  long long i = 0;
  while (i < anle_list_len(l)) { if (i > 0) printf(", "); printf("%lld", anle_list_at(l, i + 1)); i = i + 1; }
  printf("]");
}
\
     void anle_list_print_tag(AnleList* l, const char* tag) {
  printf("%s[", tag);
  long long i = 0;
  while (i < anle_list_len(l)) { if (i > 0) printf(", "); printf("%lld", anle_list_at(l, i + 1)); i = i + 1; }
  printf("]");
}
\
     void anle_list_print_f(AnleList* l) {
  printf("[");
  long long i = 0;
  while (i < anle_list_len(l)) { if (i > 0) printf(", "); printf("%g", anle_s2f(anle_list_at(l, i + 1))); i = i + 1; }
  printf("]");
}
\
     void anle_list_print_tag_f(AnleList* l, const char* tag) {
  printf("%s[", tag);
  long long i = 0;
  while (i < anle_list_len(l)) { if (i > 0) printf(", "); printf("%g", anle_s2f(anle_list_at(l, i + 1))); i = i + 1; }
  printf("]");
}
\
     void anle_list_print_tag_s(AnleList* l, const char* tag) {
  printf("%s[", tag);
  long long i = 0;
  while (i < anle_list_len(l)) { if (i > 0) printf(", "); printf("\"%s\"", (const char*)(size_t)anle_list_at(l, i + 1)); i = i + 1; }
  printf("]");
}
\
     long long anle_list_insert(AnleList* l, long long pos, long long v) {
  ANLE_GUARD(if (!l || pos < 1 || pos > l->len + 1) anle_list_die(pos, l ? l->len + 1 : 0);)
  anle_list_push(l, 0);
  long long i = l->len - 1;
  while (i >= pos) { memmove(anle_list_slot(l, i), anle_list_slot(l, i - 1), (size_t)l->esz); i = i - 1; }
  anle_slot_put(anle_list_slot(l, pos - 1), l->esz, v);
  return l->len;
}
\
     long long anle_list_remove(AnleList* l, long long pos) {
  ANLE_GUARD(if (!l || pos < 1 || pos > l->len) anle_list_die(pos, l ? l->len : 0);)
  long long v = anle_slot_get(anle_list_slot(l, pos - 1), l->esz, l->sgn);
  memmove(anle_list_slot(l, pos - 1), anle_list_slot(l, pos), (size_t)(l->esz * (l->len - pos)));
  l->len = l->len - 1;
  return v;
}
\
     long long anle_list_pop(AnleList* l) {
  ANLE_GUARD(if (!l || l->len <= 0) anle_list_die(0, l ? l->len : 0);)
  l->len = l->len - 1;
  return anle_slot_get(anle_list_slot(l, l->len), l->esz, l->sgn);
}
\
     void anle_list_clear(AnleList* l) { if (l) l->len = 0; }
\
     long long anle_list_contains(AnleList* l, long long v) {
  long long i = 0;
  while (i < (l ? l->len : 0)) { if (anle_slot_get(anle_list_slot(l, i), l->esz, l->sgn) == v) return 1; i = i + 1; }
  return 0;
}
\
     long long anle_list_index(AnleList* l, long long v) {
  long long i = 0;
  while (i < (l ? l->len : 0)) { if (anle_slot_get(anle_list_slot(l, i), l->esz, l->sgn) == v) return i + 1; i = i + 1; }
  return 0;
}
\
     long long anle_sort_esz = 8;
\
     long long anle_sort_sgn = 1;
\
     static int anle_cmp_slot(const void* a, const void* b) {
  long long x = anle_slot_get((const char*)a, anle_sort_esz, anle_sort_sgn);
  long long y = anle_slot_get((const char*)b, anle_sort_esz, anle_sort_sgn);
  if (anle_sort_sgn) return (x > y) ? 1 : ((x < y) ? -1 : 0);
  { unsigned long long ux = (unsigned long long)x, uy = (unsigned long long)y;
    return (ux > uy) ? 1 : ((ux < uy) ? -1 : 0); }
}
\
     void anle_list_sort(AnleList* l) { anle_sort_esz = l->esz; anle_sort_sgn = l->sgn; qsort(l->d, (size_t)l->len, (size_t)l->esz, anle_cmp_slot); }
     static int anle_cmp_slot_s(const void* a, const void* b) {
  const char* x = (const char*)(size_t)anle_slot_get((const char*)a, anle_sort_esz, anle_sort_sgn);
  const char* y = (const char*)(size_t)anle_slot_get((const char*)b, anle_sort_esz, anle_sort_sgn);
  return strcmp(x ? x : "", y ? y : "");
}
     void anle_list_sort_s(AnleList* l) { anle_sort_esz = l->esz; anle_sort_sgn = l->sgn; qsort(l->d, (size_t)l->len, (size_t)l->esz, anle_cmp_slot_s); }
\
     AnleList* anle_list_lit_s(long long n, const char* const* v) {
  AnleList* l = anle_list_new(8, 0);
  long long i = 0;
  while (i < n) { anle_list_push(l, (long long)(size_t)anle_str_copy(v[i])); i = i + 1; }
  return l;
}
\
     void anle_list_print_s(AnleList* l) {
  printf("[");
  long long i = 0;
  while (i < anle_list_len(l)) { if (i > 0) printf(", "); printf("\"%s\"", (const char*)(size_t)anle_list_at(l, i + 1)); i = i + 1; }
  printf("]");
}
\
     long long anle_list_contains_s(AnleList* l, const char* v) {
  long long i = 0;
  while (i < (l ? l->len : 0)) { const char* e = (const char*)(size_t)anle_list_at(l, i + 1); if (e && v && strcmp(e, v) == 0) return 1; i = i + 1; }
  return 0;
}
\
     long long anle_list_index_s(AnleList* l, const char* v) {
  long long i = 0;
  while (i < (l ? l->len : 0)) { const char* e = (const char*)(size_t)anle_list_at(l, i + 1); if (e && v && strcmp(e, v) == 0) return i + 1; i = i + 1; }
  return 0;
}
\
     AnleList* anle_dc_list(AnleList* h, long long str_elem) {
  if (!h) return 0;
  AnleList* n = (AnleList*)anle_region_alloc(anle_region_cur, (long long)sizeof(AnleList));
  n->len = h->len; n->cap = h->cap; n->esz = h->esz; n->sgn = h->sgn;
  n->d = (char*)anle_region_alloc(anle_region_cur, (long long)(h->esz * h->cap));
  if (!n || !n->d) { anle_die_nc(6, "内存不够——程序申请的空间超出可用内存", "缩小数据量，或换更省内存的写法"); }
  long long i = 0;
  while (i < h->len) { if (str_elem) anle_slot_put(anle_list_slot(n, i), 8, (long long)(size_t)anle_str_copy((const char*)(size_t)anle_slot_get(anle_list_slot(h, i), 8, 0))); else memcpy(anle_list_slot(n, i), anle_list_slot(h, i), (size_t)h->esz); i = i + 1; }
  return n;
}
\
     AnleList* anle_list_slice(AnleList* l, long long a, long long b) {
  ANLE_GUARD(if (!l || a < 1 || b < a || b > l->len) anle_list_die(b, l ? l->len : 0);)
  AnleList* r = anle_list_new(l->esz, l->sgn);
  long long i = a;
  while (i <= b) { anle_list_push_from(r, anle_list_slot(l, i - 1)); i = i + 1; }
  return r;
}

"#;

// nt: 字符串的最小 C 运
const ANLE_STR_RT: &str = r#"long long anle_str_char_len(const char* s) {
  long long n = 0;
  const unsigned char* p = (const unsigned char*)(s ? s : "");
  while (*p) { if ((*p & 0xC0) != 0x80) { n = n + 1; } p = p + 1; }
  return n;
}
\
     long long anle_str_byte_len(const char* s) { return (long long)strlen(s ? s : ""); }
\
     long long anle_utf8_len(unsigned char c) {
  if (c < 0x80) return 1;
  if ((c >> 5) == 0x6) return 2;
  if ((c >> 4) == 0xE) return 3;
  if ((c >> 3) == 0x1E) return 4;
  return 1;
}
\
     void anle_str_die(const char* what, long long v, long long n) {
  fprintf(stderr, "运行错误：%s位置 %lld 越界——这个容器只有 %lld 个位置；取之前先用 len(容器) 查一下个数\
", what, v, n);
  anle_at(); exit(1);
}
\
     const char* anle_concat(const char* a, const char* b) {
  a = a ? a : ""; b = b ? b : "";
  size_t n = strlen(a) + strlen(b) + 1;
  char* p = (char*)anle_region_alloc(anle_region_cur, (long long)n);
  if (!p) { anle_die_nc(6, "内存不够——程序申请的空间超出可用内存", "缩小数据量，或换更省内存的写法"); }
  strcpy(p, a); strcat(p, b);
  return p;
}
\
     const char* anle_sub_str(const char* s, long long a, long long b) {
  s = s ? s : "";
  long long n = anle_str_char_len(s);
  if (a < 1) a = 1;
  if (b > n) b = n;
  if (b < a) return "";
  long long i = 0, cnt = 0, st = 0, sp = 0, k = 0;
  const unsigned char* p = (const unsigned char*)s;
  while (p[k]) {
    long long w = anle_utf8_len(p[k]);
    cnt = cnt + 1;
    if (cnt == a) st = k;
    if (cnt == b) { sp = k + w; break; }
    k = k + w;
  }
  long long len = sp - st;
  char* r = (char*)anle_region_alloc(anle_region_cur, (long long)(len + 1));
  if (!r) { anle_die_nc(6, "内存不够——程序申请的空间超出可用内存", "缩小数据量，或换更省内存的写法"); }
  long long j = 0; while (j < len) { r[j] = s[st + j]; j = j + 1; }
  r[len] = 0;
  return r;
}
\
     const char* anle_nth_char(const char* s, long long n) {
  s = s ? s : "";
  long long total = anle_str_char_len(s);
  if (n < 1 || n > total) return "";
  const unsigned char* p = (const unsigned char*)s;
  long long k = 0, c = 0;
  while (p[k]) {
    long long w = anle_utf8_len(p[k]);
    c = c + 1;
    if (c == n) {
      char* b = (char*)anle_region_alloc(anle_region_cur, (long long)(w + 1));
      if (!b) { anle_die_nc(6, "内存不够——程序申请的空间超出可用内存", "缩小数据量，或换更省内存的写法"); }
      long long j = 0; while (j < w) { b[j] = (char)p[k + j]; j = j + 1; }
      b[w] = 0;
      return b;
    }
    k = k + w;
  }
  return "";
}
\
     /* 串遍历游标：顺序取 O(1)/字符，由 IR 的 #str_iter_* 专用 */
static const char* anle_it_s = 0;
static long long anle_it_i = 0;
static long long anle_it_off = 0;

static void anle_str_iter_start(const char* s) {
  anle_it_s = s; anle_it_i = 0; anle_it_off = 0;
}

static const char* anle_str_iter_next(const char* s, long long n) {
  s = s ? s : "";
  long long k;
  if (s == anle_it_s && anle_it_i > 0 && n == anle_it_i + 1) {
    k = anle_it_off + anle_utf8_len((unsigned char)s[anle_it_off]);
  } else if (s == anle_it_s && n == anle_it_i) {
    k = anle_it_off;
  } else {
    k = 0; long long c = 0;
    while (s[k]) {
      c = c + 1;
      if (c == n) break;
      k = k + anle_utf8_len((unsigned char)s[k]);
    }
    if (c < n) return "";
  }
  if (!s[k]) return "";
  long long w = anle_utf8_len((unsigned char)s[k]);
  char* b = (char*)anle_region_alloc(anle_region_cur, (long long)(w + 1));
  if (!b) { anle_die_nc(6, "内存不够——程序申请的空间超出可用内存", "缩小数据量，或换更省内存的写法"); }
  long long j = 0; while (j < w) { b[j] = (char)s[k + j]; j = j + 1; }
  b[w] = 0;
  anle_it_s = s; anle_it_i = n; anle_it_off = k;
  return b;
}

     long long anle_char_code(const char* s) {
  const unsigned char* p = (const unsigned char*)(s ? s : "");
  if (!*p) return -1;
  if (*p < 0x80) return (long long)(*p);
  if ((*p & 0xE0) == 0xC0) return (long long)(((*p & 0x1F) << 6) | (p[1] & 0x3F));
  if ((*p & 0xF0) == 0xE0) return (long long)(((*p & 0x0F) << 12) | ((p[1] & 0x3F) << 6) | (p[2] & 0x3F));
  if ((*p & 0xF8) == 0xF0) return (long long)(((*p & 0x07) << 18) | ((p[1] & 0x3F) << 12) | ((p[2] & 0x3F) << 6) | (p[3] & 0x3F));
  return (long long)(*p);
}
\
     const char* anle_code_char(long long cp) {
  char* r = (char*)anle_region_alloc(anle_region_cur, 5);
  long long n = 0;
  if (!r) { anle_die_nc(6, "内存不够——程序申请的空间超出可用内存", "缩小数据量，或换更省内存的写法"); }
  if (cp < 0 || cp > 0x10FFFF || (cp >= 0xD800 && cp <= 0xDFFF)) { r[0] = 0; return r; }
  if (cp < 0x80) { r[0] = (char)cp; n = 1; }
  else if (cp < 0x800) { r[0] = (char)(0xC0 | (cp >> 6)); r[1] = (char)(0x80 | (cp & 0x3F)); n = 2; }
  else if (cp < 0x10000) { r[0] = (char)(0xE0 | (cp >> 12)); r[1] = (char)(0x80 | ((cp >> 6) & 0x3F)); r[2] = (char)(0x80 | (cp & 0x3F)); n = 3; }
  else { r[0] = (char)(0xF0 | (cp >> 18)); r[1] = (char)(0x80 | ((cp >> 12) & 0x3F)); r[2] = (char)(0x80 | ((cp >> 6) & 0x3F)); r[3] = (char)(0x80 | (cp & 0x3F)); n = 4; }
  r[n] = 0;
  return r;
}
\
     long long anle_str_eq(const char* a, const char* b) { if (!a || !b) return (a == b) ? 1 : 0; return (strcmp(a, b) == 0) ? 1 : 0; }
     long long anle_str_index(const char* s, const char* sub) {
  if (!s || !sub) return 0;
  const char* p = strstr(s, sub);
  if (!p) return 0;
  return (long long)(p - s) + 1;
}
\
     long long anle_int_of_str(const char* s) { return s ? atoll(s) : 0; }
\
     const char* anle_str_of_float(double d) {
  char* b = (char*)anle_region_alloc(anle_region_cur, 40);
  if (!b) { anle_die_nc(6, "内存不够——程序申请的空间超出可用内存", "缩小数据量，或换更省内存的写法"); }
  snprintf(b, 40, "%g", d);
  return b;
}
\
     double anle_float_of_str(const char* s) {
  if (!s || !*s) return 0;
  return strtod(s, 0);
}
     const char* anle_str_of_int(long long v) {
  char* b = (char*)anle_region_alloc(anle_region_cur, 32);
  if (!b) { anle_die_nc(6, "内存不够——程序申请的空间超出可用内存", "缩小数据量，或换更省内存的写法"); }
  snprintf(b, 32, "%lld", v);
  return b;
}

"#;

// nt: map 最小 C 模
// nt: 键槽longlong
const ANLE_MAP_RT: &str = r#"const char* anle_str_copy(const char* s);
\
     typedef struct { long long len; long long cap; long long esz; long long sgn; long long kty; long long* k; char* v; } AnleMap;
\
     AnleMap* anle_map_new(long long esz, long long sgn, long long kty) {
  AnleMap* m = (AnleMap*)anle_region_alloc(anle_region_cur, (long long)sizeof(AnleMap));
  m->len = 0; m->cap = 4; m->esz = esz; m->sgn = sgn; m->kty = kty;
  m->k = (long long*)anle_region_alloc(anle_region_cur, (long long)(sizeof(long long) * 4));
  m->v = (char*)anle_region_alloc(anle_region_cur, (long long)(esz * 4));
  if (!m || !m->k || !m->v) { anle_die_nc(6, "内存不够——程序申请的空间超出可用内存", "缩小数据量，或换更省内存的写法"); }
  return m;
}
\
     long long anle_map_len(AnleMap* m) { return m ? m->len : 0; }
\
     long long anle_map_find(AnleMap* m, long long k) {
  long long i = 0;
  while (i < (m ? m->len : 0)) {
    if (m->kty == 0) { if (m->k[i] && k && strcmp((const char*)(size_t)m->k[i], (const char*)(size_t)k) == 0) return i; }
    else { if (m->k[i] == k) return i; }
    i = i + 1;
  }
  return -1;
}
\
     void anle_map_die_key(AnleMap* m, long long k) {
  char b[192];
  if (m && m->kty == 0) snprintf(b, sizeof b, "键\"%s\"不存在", k ? (const char*)(size_t)k : "");
  else snprintf(b, sizeof b, "键%lld 不存在", k);
  anle_die_nc(5, b, "先用 has 问一声，要新增请用 put");
}
\
     long long anle_map_contains(AnleMap* m, long long k) { return anle_map_find(m, k) >= 0; }
\
     long long anle_map_at(AnleMap* m, long long k) {
  long long i = anle_map_find(m, k);
  ANLE_GUARD(if (i < 0) anle_map_die_key(m, k);)
  return anle_slot_get(m->v + i * m->esz, m->esz, m->sgn);
}
\
     void anle_map_set(AnleMap* m, long long k, long long v) {
  long long i = anle_map_find(m, k);
  ANLE_GUARD(if (i < 0) anle_map_die_key(m, k);)
  anle_slot_put(m->v + i * m->esz, m->esz, v);
}
\
     void anle_map_put(AnleMap* m, long long k, long long v) {
  long long i = anle_map_find(m, k);
  if (i >= 0) { anle_slot_put(m->v + i * m->esz, m->esz, v); return; }
  if (m->len >= m->cap) { long long ncap = m->cap * 2; long long* nk = (long long*)anle_region_alloc(anle_region_cur, (long long)(sizeof(long long) * ncap)); char* nv = (char*)anle_region_alloc(anle_region_cur, (long long)(m->esz * ncap)); if (m->k && m->len > 0) { memcpy(nk, m->k, sizeof(long long) * (size_t)m->len); memcpy(nv, m->v, (size_t)(m->esz * m->len)); } m->cap = ncap; m->k = nk; m->v = nv; }
  m->k[m->len] = (m->kty == 0) ? (long long)(size_t)anle_str_copy((const char*)(size_t)k) : k;
  anle_slot_put(m->v + m->len * m->esz, m->esz, v);
  m->len = m->len + 1;
}
\
     long long anle_map_remove(AnleMap* m, long long k) {
  long long i = anle_map_find(m, k);
  ANLE_GUARD(if (i < 0) anle_map_die_key(m, k);)
  long long v = anle_slot_get(m->v + i * m->esz, m->esz, m->sgn);
  while (i < m->len - 1) { m->k[i] = m->k[i + 1]; memcpy(m->v + i * m->esz, m->v + (i + 1) * m->esz, (size_t)m->esz); i = i + 1; }
  m->len = m->len - 1;
  return v;
}
\
     void anle_map_clear(AnleMap* m) { if (m) m->len = 0; }
\
     AnleMap* anle_map_lit(long long n, const long long* k, const long long* v, long long esz, long long sgn, long long kty) {
  AnleMap* m = anle_map_new(esz, sgn, kty);
  long long i = 0;
  while (i < n) { anle_map_put(m, k[i], v[i]); i = i + 1; }
  return m;
}
\
     AnleList* anle_map_keys(AnleMap* m) {
  AnleList* r = anle_list_new(8, m ? m->kty : 1);
  long long i = 0;
  while (i < (m ? m->len : 0)) { anle_list_push(r, (m->kty == 0) ? (long long)(size_t)anle_str_copy((const char*)(size_t)m->k[i]) : m->k[i]); i = i + 1; }
  return r;
}
\
     AnleList* anle_map_values(AnleMap* m) {
  AnleList* r = anle_list_new(m ? m->esz : 8, m ? m->sgn : 1);
  long long i = 0;
  while (i < (m ? m->len : 0)) { anle_list_push(r, anle_slot_get(m->v + i * m->esz, m->esz, m->sgn)); i = i + 1; }
  return r;
}
\
     AnleMap* anle_dc_map(AnleMap* m, long long str_val) {
  if (!m) return 0;
  AnleMap* n = anle_map_new(m->esz, m->sgn, m->kty);
  long long i = 0;
  while (i < m->len) { long long v = anle_slot_get(m->v + i * m->esz, m->esz, m->sgn); if (str_val) v = (long long)(size_t)anle_str_copy((const char*)(size_t)v); anle_map_put(n, m->k[i], v); i = i + 1; }
  return n;
}
\
     void anle_map_print_key(long long k, long long kty) {
  if (kty == 0) printf("\"%s\"", (const char*)(size_t)k); else printf("%lld", k);
}
\
     void anle_map_print(AnleMap* m) {
  printf("{");
  long long i = 0;
  while (i < anle_map_len(m)) { if (i > 0) printf(", "); anle_map_print_key(m->k[i], m->kty); printf(": %lld", anle_slot_get(m->v + i * m->esz, m->esz, m->sgn)); i = i + 1; }
  printf("}");
}
\
     void anle_map_print_f(AnleMap* m) {
  printf("{");
  long long i = 0;
  while (i < anle_map_len(m)) { if (i > 0) printf(", "); anle_map_print_key(m->k[i], m->kty); printf(": %g", anle_s2f(anle_slot_get(m->v + i * m->esz, m->esz, m->sgn))); i = i + 1; }
  printf("}");
}
\
     void anle_map_print_s(AnleMap* m) {
  printf("{");
  long long i = 0;
  while (i < anle_map_len(m)) { if (i > 0) printf(", "); anle_map_print_key(m->k[i], m->kty); printf(": \"%s\"", (const char*)(size_t)anle_slot_get(m->v + i * m->esz, m->esz, m->sgn)); i = i + 1; }
  printf("}");
}

"#;

// nt: 文件 IO 最小模板
const ANLE_FILE_RT: &str = r#"#ifdef _WIN32
__declspec(dllimport) int __stdcall MultiByteToWideChar(unsigned int, unsigned long, const char*, int, wchar_t*, int);
__declspec(dllimport) int __stdcall WideCharToMultiByte(unsigned int, unsigned long, const wchar_t*, int, char*, int, const char*, int*);
#endif
/* 路径是 UTF-8 字节；Windows 的 fopen 按本地 ANSI 代码页解 ⇒ 先转 UTF-16 再转 CP_ACP（中文系统即 GBK） */
static char* anle_path_local(const char* p) {
#ifdef _WIN32
  if (!p || !*p) return (char*)"";
  int n = MultiByteToWideChar(65001, 0, p, -1, 0, 0);
  if (n <= 0) return (char*)p;
  wchar_t* w = (wchar_t*)malloc((size_t)n * sizeof(wchar_t));
  if (!w) return (char*)p;
  MultiByteToWideChar(65001, 0, p, -1, w, n);
  int m = WideCharToMultiByte(0, 0, w, -1, 0, 0, 0, 0);
  if (m <= 0) { free(w); return (char*)p; }
  char* b = (char*)malloc((size_t)m);
  if (!b) { free(w); return (char*)p; }
  WideCharToMultiByte(0, 0, w, -1, b, m, 0, 0);
  free(w);
  return b;
#else
  return (char*)p;
#endif
}
static FILE* anle_fopen_rb(const char* p) {
  char* lp = anle_path_local(p ? p : "");
  FILE* f = fopen(lp, "rb");
#ifdef _WIN32
  if (lp != p) free(lp);
#endif
  return f;
}
static FILE* anle_fopen_wb(const char* p) {
  char* lp = anle_path_local(p ? p : "");
  FILE* f = fopen(lp, "wb");
#ifdef _WIN32
  if (lp != p) free(lp);
#endif
  return f;
}
const char* anle_read_file(const char* path) {
\
     FILE* fp = anle_fopen_rb(path ? path : "");
\
     if (!fp) { fprintf(stderr, "运行错误：读不到文件「%s」——文件不存在，或没有读权限\n", path ? path : ""); putc(10, stderr); exit(1); }
\
     fseek(fp, 0, SEEK_END); long long n = (long long)ftell(fp); fseek(fp, 0, SEEK_SET);
\
     if (n < 0) n = 0;
\
     char* b = (char*)anle_region_alloc(anle_region_cur, (long long)((size_t)n + 1));
\
     if (!b) { anle_die_nc(6, "内存不够——程序申请的空间超出可用内存", "缩小数据量，或换更省内存的写法"); }
\
     size_t got = fread(b, 1, (size_t)n, fp);
\
     b[got] = 0;
\
     fclose(fp);
\
     return b;
\
}
\
     long long anle_save_file(const char* path, const char* text) {
\
     FILE* fp = anle_fopen_wb(path ? path : "");
\
     if (!fp) { fprintf(stderr, "运行错误：写不进文件「%s」——目录不存在，或没有写权限\n", path ? path : ""); putc(10, stderr); exit(1); }
\
     if (text) fwrite(text, 1, strlen(text), fp);
\
     fclose(fp);
\
     return 1;
\
}

"#;

// nt: 命令行参数的最小 C
const ANLE_ARGS_RT: &str = r#"static int g_argc = 0;
\
     static char** g_argv = 0;
\
     AnleList* anle_args(void) {
\
     AnleList* r = anle_list_new(8, 0);
\
     long long i = 0;
\
     while (i < g_argc) { anle_list_push(r, (long long)(size_t)anle_str_copy(g_argv[i])); i = i + 1; }
\
     return r;
\
}
\
     void anle_args_set(int argc, char** argv) { g_argc = argc; g_argv = argv; }
\

"#;

// nt: 程序里是否用到 ma
fn uses_map(p: &Program) -> bool {
    p.functions.iter().any(|f| {
        f.locals.iter().any(|l| l.ty.0 == "map" || l.ty.0.starts_with("map("))
            || f.blocks.iter().any(|b| {
                b.stmts.iter().any(|s| {
                    matches!(s, Stmt::MapSet { .. })
                        || matches!(s, Stmt::Assign { rhs: Rvalue::MapLit { .. }, .. })
                        || matches!(s, Stmt::Assign { rhs: Rvalue::MapGet { .. }, .. })
                })
            })
    })
}

// nt: 程序里是否用到 li
// nt: queue / st
fn uses_list(p: &Program) -> bool {
    const LIST_RET_FNS: &[&str] = &["args", "keys", "vals", "slice"];
    p.functions.iter().any(|f| {
        f.locals.iter().any(|l| {
            let t = l.ty.0.as_str();
            t == "list"
                || t.starts_with("list(")
                || t.starts_with("que")
                || t.starts_with("stack")
        })
            || f.blocks.iter().any(|b| {
                b.stmts.iter().any(|s| match s {
                    Stmt::ListSet { .. } => true,
                    Stmt::Assign { rhs: Rvalue::Call { name, .. }, .. } => {
                        LIST_RET_FNS.contains(&name.as_str())
                    }
                    _ => false,
                })
            })
    })
}

// nt: 程序里是否用到字符串
fn uses_str(p: &Program) -> bool {
    // nt: ⚠️ 这张表判的是「
    // nt: 名字对应 codeg
    // nt: nth_char /
    // nt: slice 也在列
    const STR_FNS: &[&str] = &[
        "len", "len_utf8", "concat", "slice", "sub_str", "nth_char", "code", "char", "int", "str",
        "split", "join", "replace", "starts_with", "ends_with", "trim",
        //nt: 串遍历游标
        "#str_iter_start", "#str_iter_next",
    ];
    p.functions.iter().any(|f| {
        f.blocks.iter().any(|b| {
            b.stmts.iter().any(|s| match s {
                Stmt::Assign { rhs: Rvalue::Call { name, .. }, .. } => STR_FNS.contains(&name.as_str()),
                // nt: 字符串比较
                Stmt::Assign { rhs: Rvalue::BinOp { op: BinOp::Eq | BinOp::Ne, lhs, rhs }, .. } => {
                    is_str_val(f, lhs) && is_str_val(f, rhs)
                }
                Stmt::Assign { rhs: Rvalue::BinOp { op: BinOp::Add, lhs, rhs }, .. } => {
                    is_str_val(f, lhs) || is_str_val(f, rhs)
                }
                _ => false,
            })
        })
    })
}

// nt: 是否用到文件 IO
fn uses_file(p: &Program) -> bool {
    called_builtins(p)
        .iter()
        .any(|n| matches!(*n, "read_file" | "save_file"))
}

// nt: 程序里是否用到命令行
fn uses_args(p: &Program) -> bool {
    called_builtins(p).contains("args")
}

// nt: 收集程序里所有被调用
fn called_builtins(p: &Program) -> std::collections::BTreeSet<&str> {
    let mut s = std::collections::BTreeSet::new();
    for f in &p.functions {
        for b in &f.blocks {
            for st in &b.stmts {
                match st {
                    Stmt::Call { name, .. } => {
                        s.insert(name.as_str());
                    }
                    Stmt::Assign { rhs: Rvalue::Call { name, .. }, .. } => {
                        s.insert(name.as_str());
                    }
                    _ => {}
                }
            }
        }
    }
    s
}

// nt: C 局部变量名
fn vname(id: LocalId) -> String {
    //nt: 槽局部指向全局
    if let Some(g) = SLOT_MAP.with(|m| m.borrow().get(&id).cloned()) {
        return g;
    }
    format!("v{}", id)
}

// nt: 字段名的 C 形态
fn field_c_name(types: &TypeMap, owner: Option<&str>, field: &str) -> String {
    if let Some((_, fields)) = owner.and_then(|o| types.get(o)) {
        if let Some(i) = fields.iter().position(|(n, _)| n == field) {
            return format!("f{}", i);
        }
    }
    format!("f_{}", field)
}

fn field_ty(types: &TypeMap, owner: Option<&str>, field: &str) -> Option<String> {
    owner
        .and_then(|o| types.get(o))
        .and_then(|(_, fs)| fs.iter().find(|(n, _)| n == field).map(|(_, t)| t.0.clone()))
}

// nt: MMIO 族
fn mmio_ty(name: &str) -> Option<&'static str> {
    match name {
        "read_8" | "write_8" => Some("char"),
        "read_16" | "write_16" => Some("short"),
        "read_32" | "write_32" => Some("int"),
        "read_64" | "write_64" => Some("long long"),
        _ => None,
    }
}

// nt: 位置 → C 左值
fn place_c(f: &Function, types: &TypeMap, p: &Place) -> String {
    let mut s = vname(p.base);
    let mut cur: Option<String> = f.locals.get(p.base).map(|l| l.ty.0.clone());
    let mut is_ptr = cur.as_deref().is_some_and(|t| t.ends_with('*'));
    for pr in &p.proj {
        match pr {
            Proj::Field(n) => {
                let owner = cur.as_deref().map(|t| t.strip_suffix('*').unwrap_or(t));
                s.push_str(if is_ptr { "->" } else { "." });
                s.push_str(&field_c_name(types, owner, n));
                cur = field_ty(types, owner, n);
                is_ptr = cur.as_deref().is_some_and(|t| t.ends_with('*'));
            }
            Proj::Index(i) => s.push_str(&format!("[{}]", i)),
            Proj::Deref => {
                // nt: 解引用
                s = format!("(*{})", s);
                cur = cur.as_deref().and_then(|t| t.strip_suffix('*')).map(|t| t.to_string());
                is_ptr = false;
            }
        }
    }
    s
}

// nt: 位置的有效类型
// nt: 与 place 共用
fn place_ty_of(f: &Function, types: &TypeMap, p: &Place) -> String {
    let mut t = f.locals.get(p.base).map(|l| l.ty.0.clone()).unwrap_or_default();
    for pr in &p.proj {
        match pr {
            Proj::Deref => {
                if let Some(s) = t.strip_suffix('*') {
                    t = s.to_string();
                }
            }
            Proj::Field(n) => {
                let owner = t.strip_suffix('*').unwrap_or(&t).to_string();
                if let Some(ft) = field_ty(types, Some(&owner), n) {
                    t = ft;
                }
            }
            Proj::Index(_) => {
                if let Some(e) = elem_ty_of(&t) {
                    t = e;
                }
            }
        }
    }
    t
}

fn c_escape(s: &str) -> String {
    let mut o = String::new();
    for c in s.chars() {
        match c {
            '\n' => o.push_str("\\n"),
            '\t' => o.push_str("\\t"),
            '\\' => o.push_str("\\\\"),
            '"' => o.push_str("\\\""),
            _ => o.push(c),
        }
    }
    o
}

// nt: 拆函数类型
fn fn_sig_c(s: &str) -> (String, Vec<String>) {
    if let Some(rest) = s.strip_prefix("fn(") {
        let mut 深 = 1i32;
        for (k, c) in rest.char_indices() {
            match c {
                '(' => 深 += 1,
                ')' => {
                    深 -= 1;
                    if 深 == 0 {
                        let ret = rest[k + 1..].trim();
                        if ret.is_empty() {
                            break;
                        }
                        let 段 = rest[..k].trim();
                        let ps = if 段.is_empty() {
                            Vec::new()
                        } else {
                            split_ty_list(段)
                        };
                        return (ret.to_string(), ps);
                    }
                }
                _ => {}
            }
        }
    }
    ("long long".to_string(), Vec::new())
}

fn split_ty_list(s: &str) -> Vec<String> {
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

fn value_c(v: &Value) -> String {
    match v {
        Value::Local(id) => vname(*id),
        // nt: 整数一律 long
        Value::Int(i) => format!("{}LL", i),
        Value::Float(x) => format!("{:?}", x),
        Value::Str(s) => format!("\"{}\"", c_escape(s)),
        Value::Bool(b) => (if *b { "1" } else { "0" }).to_string(),
        Value::Null => "0".to_string(),
    }
}

// nt: 算术是否走护栏函数
fn guarded(op: BinOp) -> Option<&'static str> {
    match op {
        BinOp::Add => Some("anle_sadd"),
        BinOp::Sub => Some("anle_ssub"),
        BinOp::Mul => Some("anle_smul"),
        BinOp::Div => Some("anle_div"),
        BinOp::Mod => Some("anle_mod"),
        _ => None,
    }
}

// nt: 容器种类
fn container_kind(ty: &str) -> &'static str {
    if ty.starts_with("que") { "que" } else if ty.starts_with("stack") { "stack" } else { "list" }
}

// nt: 值若是容器局部 ⇒
fn container_val_kind(f: &Function, v: &Value) -> Option<&'static str> {
    let ty = match v {
        Value::Local(id) => f.locals.get(*id).map(|l| l.ty.0.as_str())?,
        _ => return None,
    };
    if ty.starts_with("list(str") {
        Some("list(str)")
    } else if ty == "list" || ty.starts_with("list(") {
        if matches!(container_elem_ty(ty).as_deref(), Some("float")) {
            Some("list(float)")
        } else {
            Some("list")
        }
    } else if ty.starts_with("que") {
        if matches!(container_elem_ty(ty).as_deref(), Some("str")) {
            Some("que(str)")
        } else if matches!(container_elem_ty(ty).as_deref(), Some("float")) {
            Some("que(float)")
        } else {
            Some("que")
        }
    } else if ty.starts_with("stack") {
        if matches!(container_elem_ty(ty).as_deref(), Some("str")) {
            Some("stack(str)")
        } else if matches!(container_elem_ty(ty).as_deref(), Some("float")) {
            Some("stack(float)")
        } else {
            Some("stack")
        }
    } else if ty == "map" || ty.starts_with("map(") {
        if matches!(container_elem_ty(ty).as_deref(), Some("str")) {
            Some("map(str)")
        } else if matches!(container_elem_ty(ty).as_deref(), Some("float")) {
            Some("map(float)")
        } else {
            Some("map")
        }
    } else {
        None
    }
}

// nt: 单个标量实参的 pr
fn print_one_c(f: &Function, a: &Value) -> (String, String) {
    let is_bool = match a {
        Value::Bool(_) => true,
        Value::Local(id) => f.locals.get(*id).map(|l| l.ty.0 == "bool").unwrap_or(false),
        _ => false,
    };
    if is_bool {
        return ("%s".into(), format!("anle_bool_str({})", value_c(a)));
    }
    match a {
        Value::Str(_) => ("%s".into(), value_c(a)),
        Value::Float(_) => ("%g".into(), value_c(a)),
        Value::Local(id) => match f.locals.get(*id).map(|l| l.ty.0.as_str()) {
            Some("str") => ("%s".into(), value_c(a)),
            Some("float") => ("%g".into(), value_c(a)),
            // nt: 定长族
            Some(t)
                if matches!(
                    t,
                    "int_8" | "uint_8" | "int_16" | "uint_16" | "int_32" | "uint_32" | "int_64"
                        | "uint_64"
                ) =>
            {
                ("%lld".into(), format!("(long long)({})", value_c(a)))
            }
            _ => ("%lld".into(), value_c(a)),
        },
        _ => ("%lld".into(), value_c(a)),
    }
}

// nt: list
fn elem_ty_of(ty: &str) -> Option<String> {
    ty.strip_prefix("list(")
        .and_then(|s| s.strip_suffix(')'))
        .map(|s| s.to_string())
}

// nt: 容器的元素类型
fn container_elem_ty(ty: &str) -> Option<String> {
    let (head, inner) = ty.split_once('(')?;
    let tail = inner.strip_suffix(')')?;
    match head {
        "list" | "que" | "stack" => Some(tail.trim().to_string()),
        "map" => {
            // nt: map
            let mut depth = 0i32;
            for (i, c) in tail.char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    ',' if depth == 0 => return Some(tail[i + 1..].trim().to_string()),
                    _ => {}
                }
            }
            None
        }
        _ => None,
    }
}

// nt: 元素槽的
fn elem_esz_sgn(t: &str, types: &TypeMap) -> (String, i64) {
    //nt: 聚合元素
    if let Some((c, _)) = types.get(t) {
        return (format!("sizeof({})", c), 0);
    }
    let (n, s) = match t {
        "int_8" => (1, 1),
        "uint_8" => (1, 0),
        "int_16" => (2, 1),
        "uint_16" => (2, 0),
        "int_32" => (4, 1),
        "uint_32" => (4, 0),
        "int_64" => (8, 1),
        "uint_64" => (8, 0),
        _ => (8, 1),
    };
    (n.to_string(), s)
}

//nt: 是否聚合
fn is_agg_ty(t: &str, types: &TypeMap) -> bool {
    types.contains_key(t)
}

//nt: 值是否聚合
fn is_agg_val(f: &Function, types: &TypeMap, v: &Value) -> bool {
    match v {
        Value::Local(id) => f
            .locals
            .get(*id)
            .map(|l| is_agg_ty(l.ty.0.trim_end_matches('*'), types))
            .unwrap_or(false),
        _ => false,
    }
}

// nt: 把手型值
fn is_ptr_val(f: &Function, v: &Value) -> bool {
    match v {
        Value::Local(id) => f.locals.get(*id).map(|l| l.ty.0.ends_with('*')).unwrap_or(false),
        _ => false,
    }
}

// nt: 字符串比较
// nt: 值是否是把手
fn is_handle_val(f: &Function, v: &Value) -> bool {
    match v {
        Value::Local(id) => f
            .locals
            .get(*id)
            .map(|l| l.ty.0.ends_with('*'))
            .unwrap_or(false),
        _ => false,
    }
}

fn is_str_val(f: &Function, v: &Value) -> bool {
    match v {
        Value::Str(_) => true,
        Value::Local(id) => f.locals.get(*id).map(|l| l.ty.0 == "str").unwrap_or(false),
        _ => false,
    }
}

// nt: 键型str或int
fn map_kty(ty: &str) -> i64 {
    if let Some(rest) = ty.strip_prefix("map(") {
        let k = rest.split(',').next().unwrap_or("").trim();
        if k == "int" {
            return 1;
        }
    }
    0
}

// nt: str键转指针整数
fn key_c(f: &Function, v: &Value) -> String {
    if is_str_val(f, v) {
        format!("(long long)(size_t)({})", value_c(v))
    } else {
        value_c(v)
    }
}

/// nt: 元素是否 float
fn container_elem_is_float(f: &Function, v: &Value) -> bool {
    match v {
        Value::Local(id) => f
            .locals
            .get(*id)
            .and_then(|l| container_elem_ty(&l.ty.0))
            .map(|e| e == "float")
            .unwrap_or(false),
        _ => false,
    }
}

//nt: 元素是否聚合
fn container_elem_is_agg(f: &Function, v: &Value, types: &TypeMap) -> bool {
    match v {
        Value::Local(id) => f
            .locals
            .get(*id)
            .and_then(|l| container_elem_ty(&l.ty.0))
            .map(|e| is_agg_ty(&e, types))
            .unwrap_or(false),
        _ => false,
    }
}

//nt: 聚合取元素助手名
fn 容器元素助手(f: &Function, v: &Value, types: &TypeMap) -> String {
    match v {
        Value::Local(id) => f
            .locals
            .get(*id)
            .and_then(|l| container_elem_ty(&l.ty.0))
            .and_then(|e| types.get(&e).map(|(c, _)| c.replace("anle_st_", "anle_at_st_")))
            .unwrap_or_default(),
        _ => String::new(),
    }
}

// nt: 值是否是 float
fn is_float_val(f: &Function, v: &Value) -> bool {
    match v {
        Value::Float(_) => true,
        Value::Local(id) => f.locals.get(*id).map(|l| l.ty.0 == "float").unwrap_or(false),
        _ => false,
    }
}

// nt: 值是否是把手
fn value_is_handle(f: &Function, v: &Value) -> bool {
    match v {
        Value::Local(id) => f
            .locals
            .get(*id)
            .map(|l| l.ty.0.ends_with('*'))
            .unwrap_or(false),
        _ => false,
    }
}

fn binop_c(
    f: &Function,
    op: BinOp,
    lhs: &Value,
    rhs: &Value,
    used: &mut BTreeSet<&'static str>,
) -> String {
    // nt: 字符串的 == /
    if matches!(op, BinOp::Eq | BinOp::Ne) && is_str_val(f, lhs) && is_str_val(f, rhs) {
        used.insert("anle_str_eq");
        let c = format!("anle_str_eq({}, {})", value_c(lhs), value_c(rhs));
        return if matches!(op, BinOp::Ne) { format!("(!{})", c) } else { c };
    }
// nt: 字符串拼接
    if matches!(op, BinOp::Add) && (is_str_val(f, lhs) || is_str_val(f, rhs)) {
        return format!("anle_concat({}, {})", value_c(lhs), value_c(rhs));
    }
    let a = value_c(lhs);
    let b = value_c(rhs);
    // nt: float 参与算术
    if is_float_val(f, lhs) || is_float_val(f, rhs) {
        if let Some(sym) = match op {
            BinOp::Add => Some("+"),
            BinOp::Sub => Some("-"),
            BinOp::Mul => Some("*"),
            BinOp::Div => Some("/"),
            _ => None,
        } {
            return format!("(({}) {} ({}))", a, sym, b);
        }
    }
    let core = match guarded(op) {
        Some(fn_name) => {
            used.insert(fn_name);
            format!("{}({}, {})", fn_name, a, b)
        }
        None => {
            let sym = match op {
                BinOp::Eq => "==",
                BinOp::Ne => "!=",
                BinOp::Lt => "<",
                BinOp::Le => "<=",
                BinOp::Gt => ">",
                BinOp::Ge => ">=",
                BinOp::And => "&&",
                BinOp::Or => "||",
                BinOp::BitAnd => "&",
                BinOp::BitOr => "|",
                BinOp::BitXor => "^",
                BinOp::Shl => "<<",
                BinOp::Shr => ">>",
                _ => unreachable!(),
            };
            format!("(({}) {} ({}))", a, sym, b)
        }
    };
    // nt: 定长操作数 ⇒ 结果
    if is_wrap_op(op) {
        if let Some(t) = fixed_cty(f, lhs).or_else(|| fixed_cty(f, rhs)) {
            return format!("(({})({}))", t, core);
        }
    }
    core
}

// nt: 需要按定长宽度截回结
fn is_wrap_op(op: BinOp) -> bool {
    matches!(
        op,
        BinOp::Add
            | BinOp::Sub
            | BinOp::Mul
            | BinOp::Div
            | BinOp::Mod
            | BinOp::BitAnd
            | BinOp::BitOr
            | BinOp::BitXor
            | BinOp::Shl
            | BinOp::Shr
    )
}

// nt: 值若是定长族局部 ⇒
fn fixed_cty(f: &Function, v: &Value) -> Option<&'static str> {
    let ty = match v {
        Value::Local(id) => f.locals.get(*id).map(|l| l.ty.0.as_str())?,
        _ => return None,
    };
    match ty {
        "int_8" => Some("signed char"),
        "uint_8" => Some("unsigned char"),
        "int_16" => Some("short"),
        "uint_16" => Some("unsigned short"),
        "int_32" => Some("int"),
        "uint_32" => Some("unsigned int"),
        "int_64" => Some("long long"),
        "uint_64" => Some("unsigned long long"),
        _ => None,
    }
}

fn stmt_c(
    f: &Function,
    s: &Stmt,
    out: &mut String,
    used: &mut BTreeSet<&'static str>,
    names: &BTreeMap<String, String>,
    types: &TypeMap,
) -> Result<(), CodegenError> {
    match s {
        // nt: 分配只是存储类标注；
        Stmt::Assign { rhs: Rvalue::Alloc { .. }, .. } => {}
// nt: 裸地址写
        Stmt::Call { name, args }
            if mmio_ty(name).is_some() && name.starts_with("write_") => {
            match (mmio_ty(name), args.first(), args.get(1)) {
                (Some(t), Some(a), Some(v)) => {
                    out.push_str(&format!(
                        "  (*(volatile {}*)({}) = ({})({}));\n",
                        t,
                        value_c(a),
                        t,
                        value_c(v)
                    ));
                    return Ok(());
                }
                _ => return Err(CodegenError { message: format!("「{}」要地址与值两个参数", name) , code: "E401".into() , line: 0 }),
            }
        }
        // nt: 显式提前回收
        Stmt::KillRegion { block } => {
            out.push_str(&format!("  anle_region_kill(\"{}\");\n", c_escape(block)));
            return Ok(());
        }
        // nt: 内联汇编
        Stmt::Asm(form) => {
            let outs: Vec<String> = form
                .outs
                .iter()
                .map(|(约束, id, 读写)| {
                    let 名 = f
                        .locals
                        .get(*id)
                        .and_then(|l| l.name.clone())
                        .unwrap_or_else(|| format!("o{}", id));
                    format!(
                        "[{}]\"{}{}\"({})",
                        名,
                        if *读写 { "+" } else { "=" },
                        约束,
                        vname(*id)
                    )
                })
                .collect();
            let ins: Vec<String> = form
                .ins
                .iter()
                .enumerate()
                .map(|(k, (约束, v))| format!("[i{}]\"{}\"({})", k, 约束, value_c(v)))
                .collect();
            let 破坏: Vec<String> = form.clobbers.iter().map(|c| format!("\"{}\"", c)).collect();
            out.push_str(&format!(
                "  __asm__ volatile (\"{}\" : {} : {} : {});\n",
                c_escape(&form.tmpl),
                outs.join(", "),
                ins.join(", "),
                破坏.join(", ")
            ));
            return Ok(());
        }
        // nt: 无返回值的调用语句
        Stmt::Call { name, args } => {
            //nt: 串遍历游标置起点
            if name == "#str_iter_start" {
                if let Some(a) = args.first() {
                    out.push_str(&format!("  anle_str_iter_start({});\n", value_c(a)));
                }
                return Ok(());
            }
            if name == "print" {
                // nt: print 整个 l
                if let (Some(Value::Local(id)), None) = (args.first(), args.get(1)) {
                    let ty = f.locals.get(*id).map(|l| l.ty.0.clone()).unwrap_or_default();
                    if ty.starts_with("list(str") {
                        out.push_str(&format!("  anle_list_print_s({});\n  printf(\"\\n\");\n", value_c(&args[0])));
                        return Ok(());
                    }
                    // nt: 定长族
                    if matches!(ty.as_str(), "int_8" | "uint_8" | "int_16" | "uint_16"
                        | "int_32" | "uint_32" | "int_64" | "uint_64")
                    {
                        out.push_str(&format!("  printf(\"%lld\", (long long)({}))", value_c(&args[0])));
                        out.push_str(";\n");
                        return Ok(());
                    }
                    if ty.starts_with("list(float") {
                        out.push_str(&format!("  anle_list_print_f({});\n  printf(\"\\n\");\n", value_c(&args[0])));
                        return Ok(());
                    }
                    if ty == "list" || ty.starts_with("list(") {
                        out.push_str(&format!("  anle_list_print({});\n  printf(\"\\n\");\n", value_c(&args[0])));
                        return Ok(());
                    }
                    // nt: 队列/栈 print
                    if ty.starts_with("que") {
                        let fname = match container_elem_ty(&ty).as_deref() {
                            Some("str") => "anle_list_print_tag_s",
                            Some("float") => "anle_list_print_tag_f",
                            _ => "anle_list_print_tag",
                        };
                        out.push_str(&format!(
                            "  {}({}, \"que\");\n  printf(\"\\n\");\n",
                            fname,
                            value_c(&args[0])
                        ));
                        return Ok(());
                    }
                    if ty.starts_with("stack") {
                        let fname = match container_elem_ty(&ty).as_deref() {
                            Some("str") => "anle_list_print_tag_s",
                            Some("float") => "anle_list_print_tag_f",
                            _ => "anle_list_print_tag",
                        };
                        out.push_str(&format!(
                            "  {}({}, \"stack\");\n  printf(\"\\n\");\n",
                            fname,
                            value_c(&args[0])
                        ));
                        return Ok(());
                    }
                    if ty == "map" || ty.starts_with("map(") {
                        let fname = match container_elem_ty(&ty).as_deref() {
                            Some("str") => "anle_map_print_s",
                            Some("float") => "anle_map_print_f",
                            _ => "anle_map_print",
                        };
                        out.push_str(&format!("  {}({});\n  printf(\"\\n\");\n", fname, value_c(&args[0])));
                        return Ok(());
                    }
                }
                // nt: 多参里含容器
                // nt: 故仅当它排在末位
                if args.iter().any(|a| container_val_kind(f, a).is_some()) {
                    for (i, a) in args.iter().enumerate() {
                        if i > 0 {
                            out.push_str("  printf(\" \");\n");
                        }
                        match container_val_kind(f, a) {
                            Some("list(str)") => {
                                out.push_str(&format!("  anle_list_print_s({});\n", value_c(a)))
                            }
                            Some("list(float)") => {
                                out.push_str(&format!("  anle_list_print_f({});\n", value_c(a)))
                            }
                            Some("list") => {
                                out.push_str(&format!("  anle_list_print({});\n", value_c(a)))
                            }
                            Some("que(str)") => out.push_str(&format!(
                                "  anle_list_print_tag_s({}, \"que\");\n",
                                value_c(a)
                            )),
                            Some("que(float)") => out.push_str(&format!(
                                "  anle_list_print_tag_f({}, \"que\");\n",
                                value_c(a)
                            )),
                            Some("que") => out.push_str(&format!(
                                "  anle_list_print_tag({}, \"que\");\n",
                                value_c(a)
                            )),
                            Some("stack(str)") => out.push_str(&format!(
                                "  anle_list_print_tag_s({}, \"stack\");\n",
                                value_c(a)
                            )),
                            Some("stack(float)") => out.push_str(&format!(
                                "  anle_list_print_tag_f({}, \"stack\");\n",
                                value_c(a)
                            )),
                            Some("stack") => out.push_str(&format!(
                                "  anle_list_print_tag({}, \"stack\");\n",
                                value_c(a)
                            )),
                            Some("map(str)") => {
                                out.push_str(&format!("  anle_map_print_s({});\n", value_c(a)))
                            }
                            Some("map(float)") => {
                                out.push_str(&format!("  anle_map_print_f({});\n", value_c(a)))
                            }
                            Some("map") => {
                                out.push_str(&format!("  anle_map_print({});\n", value_c(a)))
                            }
                            _ => {
                                let (fmt, expr) = print_one_c(f, a);
                                out.push_str(&format!("  printf(\"{}\", {});\n", fmt, expr));
                            }
                        }
                    }
// nt: 容器 helper
                    out.push_str("  printf(\"\\n\");\n");
                    return Ok(());
                }
// nt: print 不收积类
                for a in args {
                    if let Value::Local(id) = a {
                        if let Some(l) = f.locals.get(*id) {
                            if types.contains_key(l.ty.0.trim_end_matches('*')) {
                                return Err(CodegenError {
                                    message: "print 不收积类型——先取字段（结构可能成环）".into(),
                                    code: "E401".into(),
                                    line: 0,
                                });
                            }
                        }
                    }
                }
                // nt: 逐参定格式
                let mut fmts: Vec<String> = Vec::new();
                let mut as_: Vec<String> = Vec::new();
                for a in args {
                    let is_bool = match a {
                        Value::Bool(_) => true,
                        Value::Local(id) => {
                            f.locals.get(*id).map(|l| l.ty.0 == "bool").unwrap_or(false)
                        }
                        _ => false,
                    };
                    if is_bool {
                        fmts.push("%s".into());
                        as_.push(format!("anle_bool_str({})", value_c(a)));
                        continue;
                    }
                    match a {
                        Value::Str(_) => {
                            fmts.push("%s".into());
                            as_.push(value_c(a));
                        }
                        Value::Float(_) => {
                            fmts.push("%g".into());
                            as_.push(value_c(a));
                        }
                        Value::Local(id) => match f.locals.get(*id).map(|l| l.ty.0.as_str()) {
                            Some("str") => {
                                fmts.push("%s".into());
                                as_.push(value_c(a));
                            }
                            Some("float") => {
                                fmts.push("%g".into());
                                as_.push(value_c(a));
                            }
                            // nt: 定长族
                            Some(t)
                                if matches!(
                                    t,
                                    "int_8" | "uint_8" | "int_16" | "uint_16" | "int_32"
                                        | "uint_32" | "int_64" | "uint_64"
                                ) =>
                            {
                                fmts.push("%lld".into());
                                as_.push(format!("(long long)({})", value_c(a)));
                            }
                            _ => {
                                fmts.push("%lld".into());
                                as_.push(value_c(a));
                            }
                        },
                        _ => {
                            fmts.push("%lld".into());
                            as_.push(value_c(a));
                        }
                    }
                }
                out.push_str(&format!(
                    "  printf(\"{}\\n\", {});\n",
                    fmts.join(" "),
                    as_.join(", ")
                ));
            } else if name == "push" {
                // nt: push
                if args.len() != 2 {
                    return Err(CodegenError { message: "push 要 2 个参数（列表, 值）".into() , code: "E401".into() , line: 0 });
                }
                if is_str_val(f, &args[1]) {
                    out.push_str(&format!(
                        "  anle_list_push({}, (long long)(size_t)anle_str_copy({}));\n",
                        value_c(&args[0]),
                        value_c(&args[1])
                    ));
                } else if is_float_val(f, &args[1]) {
                    out.push_str(&format!(
                        "  anle_list_push({}, anle_f2s({}));\n",
                        value_c(&args[0]),
                        value_c(&args[1])
                    ));
                } else if is_handle_val(f, &args[1]) {
                    // nt: 把手
                    //nt: fn自有对象复印重绑
                    let esc = match &args[1] {
                        Value::Local(id) => f.locals.get(*id).filter(|l| l.fn_owned).and_then(|l| {
                            let b = l.ty.0.trim_end_matches('*').to_string();
                            types.get(&b).map(|(ct, _)| (*id, vname(*id), ct.clone()))
                        }),
                        _ => None,
                    };
                    if let Some((lid, vn, ct)) = &esc {
                        let 已逃 = ESCAPED.with(|s| s.borrow().contains(lid));
                        if !已逃 {
                            out.push_str(&format!(
                                "  {vn} = ({ct}*)anle_copy_to_parent({vn}, (long long)sizeof({ct}));\n",
                                vn = vn,
                                ct = ct
                            ));
                            ESCAPED.with(|s| s.borrow_mut().insert(*lid));
                        }
                    }
                    out.push_str(&format!(
                        "  anle_list_push({}, (long long)(size_t){});\n",
                        value_c(&args[0]),
                        value_c(&args[1])
                    ));
                } else if is_agg_val(f, types, &args[1]) {
                    //nt: 聚合按地址
                    out.push_str(&format!(
                        "  anle_list_push_from({}, (const char*)&{});\n",
                        value_c(&args[0]),
                        value_c(&args[1])
                    ));
                } else {
                    out.push_str(&format!(
                        "  anle_list_push({}, {});\n",
                        value_c(&args[0]),
                        value_c(&args[1])
                    ));
                }
            } else if name == "exit" {
                // nt: exit
                if args.len() != 1 {
                    return Err(CodegenError { message: "exit 要 1 个参数（码）".into() , code: "E401".into() , line: 0 });
                }
                out.push_str(&format!("  exit((int)({}));\n", value_c(&args[0])));
            } else if name == "save_file" {
                // nt: save_file
                if args.len() != 2 {
                    return Err(CodegenError { message: "save_file 要 2 个参数（路径, 内容）".into() , code: "E401".into() , line: 0 });
                }
                let as_: Vec<String> = args.iter().map(value_c).collect();
                out.push_str(&format!("  anle_save_file({}, {});\n", as_[0], as_[1]));
            } else if name == "put" {
                // nt: put
                if args.len() != 3 {
                    return Err(CodegenError { message: "put 要 3 个参数（map, 键, 值）".into() , code: "E401".into() , line: 0 });
                }
                let mut as_: Vec<String> = args.iter().map(value_c).collect();
                // nt: str键转指针整数
                if args.len() > 1 {
                    as_[1] = key_c(f, &args[1]);
                }
                // nt: map 的值槽是 8
                if let Some(v) = args.get(2) {
                    // nt: 值槽是 8 字节整数
                    let need_box = match v {
                        Value::Str(_) => true,
                        Value::Local(id) => f
                            .locals
                            .get(*id)
                            .map(|l| {
                                let t = l.ty.0.as_str();
                                t == "str"
                                    || t.ends_with('*')
                                    || matches!(
                                        t.split('(').next().unwrap_or("").trim(),
                                        "list" | "que" | "stack" | "map"
                                    )
                            })
                            .unwrap_or(false),
                        _ => false,
                    };
                    if is_str_val(f, v) {
                        // nt: 复印 str
                        as_[2] = format!("(long long)(size_t)anle_str_copy({})", value_c(v));
                    } else if is_float_val(f, v) {
                        as_[2] = format!("anle_f2s({})", value_c(v));
                    } else if need_box {
                        as_[2] = format!("(long long)(size_t)({})", value_c(v));
                    }
                }
                out.push_str(&format!("  anle_map_put({}, {}, {});\n", as_[0], as_[1], as_[2]));
            } else if name == "remove" && matches!(args.first(), Some(Value::Local(id)) if f.locals.get(*id).map(|l| l.ty.0 == "map" || l.ty.0.starts_with("map(")).unwrap_or(false)) {
                // nt: remove
                if args.len() != 2 {
                    return Err(CodegenError { message: "remove 要 2 个参数（map, 键）".into() , code: "E401".into() , line: 0 });
                }
                out.push_str(&format!(
                    "  anle_map_remove({}, {});\n",
                    value_c(&args[0]),
                    key_c(f, &args[1])
                ));
            } else if name == "clear"
                && matches!(args.first(), Some(Value::Local(id))
                    if f.locals.get(*id).map(|l| l.ty.0 == "map" || l.ty.0.starts_with("map(")).unwrap_or(false))
            {
                out.push_str(&format!("  anle_map_clear({});\n", value_c(&args[0])));
            } else if name == "pop" {
                // nt: pop 共用
                let first_ty = match args.first() {
                    Some(Value::Local(id)) => {
                        f.locals.get(*id).map(|l| l.ty.0.clone()).unwrap_or_default()
                    }
                    _ => String::new(),
                };
                if first_ty.starts_with("que") {
                    out.push_str(&format!(
                        "  anle_list_remove({}, 1);\n",
                        value_c(&args[0])
                    ));
                } else {
                    out.push_str(&format!("  anle_list_pop({});\n", value_c(&args[0])));
                }
            } else if name == "insert" || name == "remove" || name == "clear" || name == "sort" {
                // nt: 列表增删改排
                let first_ty = match args.first() {
                    Some(Value::Local(id)) => {
                        f.locals.get(*id).map(|l| l.ty.0.clone()).unwrap_or_default()
                    }
                    _ => String::new(),
                };
                let kind = container_kind(&first_ty);
                if kind != "list" {
                    return Err(CodegenError {
                        message: "队列/栈只进只出——insert/remove(按位置)/sort 只对列表用".into(),
                        code: "E401".into(),
                        line: 0,
                    });
                }
                let c = match name.as_str() {
                    "run" => {
                        used.insert("anle_run");
                        "anle_run"
                    }
                    "insert" => "anle_list_insert",
                    "remove" => "anle_list_remove",
                    "clear" => "anle_list_clear",
                    _ => {
                        // nt: sort
                        let is_str_elem = matches!(args.first(), Some(Value::Local(id))
                            if f.locals.get(*id).map(|l| l.ty.0.starts_with("list(str")).unwrap_or(false));
                        if is_str_elem {
                            used.insert("anle_list_sort_s");
                            "anle_list_sort_s"
                        } else {
                            "anle_list_sort"
                        }
                    }
                };
                let mut as_: Vec<String> = args.iter().map(value_c).collect();
                if name == "insert" && args.len() == 3 && is_float_val(f, &args[2]) {
                    as_[2] = format!("anle_f2s({})", as_[2]);
                }
                if name == "insert" && args.len() == 3 && is_agg_val(f, types, &args[2]) {
                    //nt: 聚合按地址
                    as_[2] = format!("(const char*)&{}", as_[2]);
                    out.push_str(&format!("  anle_list_insert_from({});\n", as_.join(", ")));
                    return Ok(());
                }
                out.push_str(&format!("  {}({});\n", c, as_.join(", ")));
            } else if name == "sle_ms" {
                // nt: sleep
                if args.len() != 1 {
                    return Err(CodegenError { message: "sleep 要 1 个参数（毫秒）".into() , code: "E401".into() , line: 0 });
                }
                used.insert("anle_sleep");
                out.push_str(&format!("  anle_sleep({});\n", value_c(&args[0])));
            } else if let Some(callee) = names.get(name) {
                // nt: 调用方 fn 的调用
                let as_: Vec<String> = args.iter().map(value_c).collect();
                out.push_str(&format!("  {}({});\n", callee, as_.join(", ")));
            } else {
                return Err(CodegenError { message: format!("暂未支持的内置调用：{}", name) , code: "E401".into() , line: 0 });
            }
        }
        Stmt::Assign { lhs, rhs } => {
            // nt: 容器元素槽尺寸按
            let dest_ty = place_ty_of(f, types, lhs);
            let (elem_esz, elem_sgn) = container_elem_ty(&dest_ty)
                .map(|e| elem_esz_sgn(&e, types))
                .unwrap_or(("8".to_string(), 1));
            let r = match rhs {
                Rvalue::Use(p) => place_c(f, types, p),
                Rvalue::Ref(p) => format!("(&{})", place_c(f, types, p)),
                // nt: 和类型构造 ⇒ 复合
                Rvalue::VariantLit { ty, tag, args, tys } => {
                    let c = types.get(ty).map(|(c, _)| c.clone()).unwrap_or_else(|| ty.clone());
                    let mut parts = vec![format!(".tag = {}LL", tag)];
                    for (i, a) in args.iter().enumerate() {
                        // nt: payload 槽统
                        let v = value_c(a);
                        let v = match tys.get(i).map(|t| t.0.as_str()) {
                            Some("str") => format!("(long long)(size_t)({})", v),
                            // nt: 指针字段
                            // nt: 实参已是把手
                            Some(t) if t.ends_with('*') => match a {
                                Value::Local(id)
                                    if !f.locals.get(*id).map(|l| l.ty.0.ends_with('*')).unwrap_or(false) =>
                                {
                                    format!("(long long)(size_t)(&{})", v)
                                }
                                _ => format!("(long long)(size_t)({})", v),
                            },
                            _ => v,
                        };
                        parts.push(format!(".p{} = {}", i, v));
                    }
                    format!("(({}){{ {} }})", c, parts.join(", "))
                }
                Rvalue::VariantTag(v) => {
                    // nt: 对象是把手
                    let base = if value_is_handle(f, v) {
                        format!("(*{})", value_c(v))
                    } else {
                        value_c(v)
                    };
                    format!("(({}).tag)", base)
                }
                Rvalue::VariantGet { value, idx, ty } => {
                    let base = if value_is_handle(f, value) {
                        format!("(*{})", value_c(value))
                    } else {
                        value_c(value)
                    };
                    let raw = format!("(({}).p{})", base, idx);
                    // nt: 拆箱
                    if ty.0 == "str" {
                        format!("((const char*)(size_t)({}))", raw)
                    } else if ty.0.ends_with('*') {
                        format!("(({})(size_t)({}))", c_ty_in(ty, types), raw)
                    } else {
                        raw
                    }
                }
                Rvalue::Const(v) => value_c(v),
                Rvalue::FnRef { name } => {
                    let callee = names.get(name).cloned().unwrap_or_else(|| name.clone());
                    format!("((long long)(size_t){})", callee)
                }
                Rvalue::CallIndirect { callee, args } => {
                    let as_: Vec<String> = args.iter().map(value_c).collect();
                    let fty = match callee {
                        Value::Local(id) => f
                            .locals
                            .get(*id)
                            .map(|l| l.ty.0.clone())
                            .unwrap_or_default(),
                        _ => String::new(),
                    };
                    let (ret, ps) = fn_sig_c(&fty);
                    let ps_c: Vec<String> = ps.iter().map(|t| c_ty_in(&Ty(t.clone()), types)).collect();
                    let ps_s = if ps_c.is_empty() { "void".to_string() } else { ps_c.join(", ") };
                    format!(
                        "(({} (*)({}))(size_t){})({})",
                        c_ty_in(&Ty(ret), types),
                        ps_s,
                        value_c(callee),
                        as_.join(", ")
                    )
                }
                Rvalue::BinOp { op, lhs, rhs } => binop_c(f, *op, lhs, rhs, used),
// nt: 裸地址读
                Rvalue::Call { name, args }
                    if mmio_ty(name).is_some() && name.starts_with("read_") => {
                    match (mmio_ty(name), args.first()) {
                        (Some(t), Some(a)) => {
                            format!("((long long)(*(volatile {}*)({})))", t, value_c(a))
                        }
                        _ => return Err(CodegenError { message: format!("「{}」要一个地址参数", name) , code: "E401".into() , line: 0 }),
                    }
                }
                Rvalue::Call { name, args } => {
                    let as_: Vec<String> = args.iter().map(value_c).collect();
                    let arg = |i: usize| as_.get(i).cloned().unwrap_or_else(|| "0".to_string());
                    match name.as_str() {
                        // nt: len
                        "len" => {
                            // nt: 列表/队列/栈共用同
                            let is_list = matches!(args.first(), Some(Value::Local(id))
                                if f.locals.get(*id).map(|l| {
                                    let t = l.ty.0.as_str();
                                    t == "list" || t == "que" || t == "stack"
                                        || t.starts_with("list(") || t.starts_with("que(") || t.starts_with("stack(")
                                }).unwrap_or(false));
                            let is_map = matches!(args.first(), Some(Value::Local(id))
                                if f.locals.get(*id).map(|l| l.ty.0 == "map" || l.ty.0.starts_with("map(")).unwrap_or(false));
                            if is_list {
                                format!("anle_list_len({})", arg(0))
                            } else if is_map {
                                format!("anle_map_len({})", arg(0))
                            } else {
                                format!("anle_str_char_len({})", arg(0))
                            }
                        }
                        "empty" => {
                            let is_map = matches!(args.first(), Some(Value::Local(id))
                                if f.locals.get(*id).map(|l| l.ty.0 == "map" || l.ty.0.starts_with("map(")).unwrap_or(false));
                            if is_map {
                                format!("(anle_map_len({}) == 0)", arg(0))
                            } else {
                                format!("(anle_list_len({}) == 0)", arg(0))
                            }
                        }
                        "len_utf8" => format!("anle_str_byte_len({})", arg(0)),
                        "str_index" => {
                            //nt: 串内找子串
                            format!(
                                "(strstr({}, {}) ? (long long)(strstr({}, {}) - {}) + 1 : 0)",
                                arg(0), arg(1), arg(0), arg(1), arg(0)
                            )
                        }
                        "has" | "index" => {
                            let list_ty = match args.first() {
                                Some(Value::Local(id)) => {
                                    f.locals.get(*id).map(|l| l.ty.0.clone()).unwrap_or_default()
                                }
                                _ => String::new(),
                            };
                            if list_ty == "map" || list_ty.starts_with("map(") {
                                if name == "has" {
                                    format!(
                                        "anle_map_contains({}, {})",
                                        arg(0),
                                        key_c(f, &args[1])
                                    )
                                } else {
                                    // nt: map 的 inde
                                    format!(
                                        "anle_map_at({}, {})",
                                        arg(0),
                                        key_c(f, &args[1])
                                    )
                                }
                            } else if list_ty.starts_with("list(str") {
                                if name == "has" {
                                    format!("anle_list_contains_s({}, {})", arg(0), arg(1))
                                } else {
                                    format!("anle_list_index_s({}, {})", arg(0), arg(1))
                                }
                            } else if name == "has" {
                                let b = if is_ptr_val(f, &args[1]) {
                                    format!("(long long)(size_t){}", arg(1))
                                } else {
                                    arg(1)
                                };
                                format!("anle_list_contains({}, {})", arg(0), b)
                            } else {
                                let b = if is_ptr_val(f, &args[1]) {
                                    format!("(long long)(size_t){}", arg(1))
                                } else {
                                    arg(1)
                                };
                                format!("anle_list_index({}, {})", arg(0), b)
                            }
                        }
                        // nt: 切片
                        // nt: 列表/队列/栈基按元
                        // nt: 读长度 ⇒ 申请巨大
                        "slice" => {
                            let is_str = matches!(args.first(), Some(Value::Local(id))
                                if f.locals.get(*id).map(|l| l.ty.0 == "str").unwrap_or(false));
                            if is_str {
                                format!("anle_sub_str({}, {}, {})", arg(0), arg(1), arg(2))
                            } else {
                                format!("anle_list_slice({}, {}, {})", arg(0), arg(1), arg(2))
                            }
                        }
                        // nt: take 的复印
                        "#str_copy" => format!("anle_str_copy({})", arg(0)),
                        "#dc_list" => format!("anle_dc_list({}, {})", arg(0), arg(1)),
                        "#dc_map" => format!("anle_dc_map({}, {})", arg(0), arg(1)),
                        //nt: 结构体深拷
                        n if n.starts_with("#dcdeep_") => {
                            let 型 = n.trim_start_matches("#dcdeep_");
                            let c = types.get(型).map(|t| t.0.clone()).unwrap_or_default();
                            let 助手 = c.replace("anle_st_", "anle_dcdeep_st_");
                            format!("{}(&{})", 助手, arg(0))
                        }
                        "concat" => format!("anle_concat({}, {})", arg(0), arg(1)),
                        "sub_str" => format!("anle_sub_str({}, {}, {})", arg(0), arg(1), arg(2)),
                        "nth_char" => format!("anle_nth_char({}, {})", arg(0), arg(1)),
                        //nt: 串顺序取字符
                        "#str_iter_next" => format!("anle_str_iter_next({}, {})", arg(0), arg(1)),
                        "code" => format!("anle_char_code({})", arg(0)),
                        "char" => format!("anle_code_char({})", arg(0)),
// nt: 类型名当函数
                        // nt: queue/stac
                        "que" | "stack" => {
                            if args.is_empty() {
                                format!("anle_list_new({}, {})", elem_esz, elem_sgn)
                            } else {
                                arg(0)
                            }
                        }
                        "args" => "anle_args()".to_string(),
                        "read_file" => format!("anle_read_file({})", arg(0)),
                        // nt: pop
                        "pop" => {
                            let first_ty = match args.first() {
                                Some(Value::Local(id)) => {
                                    f.locals.get(*id).map(|l| l.ty.0.clone()).unwrap_or_default()
                                }
                                _ => String::new(),
                            };
                            let raw = if first_ty.starts_with("que") {
                                format!("anle_list_remove({}, 1)", arg(0))
                            } else {
                                format!("anle_list_pop({})", arg(0))
                            };
                            // nt: str 元素 ⇒ 把
                            // nt: 注意用 contai
                            if matches!(container_elem_ty(&first_ty).unwrap_or_default().as_str(), "str") {
                                format!("((const char*)(size_t){})", raw)
                            } else if matches!(container_elem_ty(&first_ty).unwrap_or_default().as_str(), "float") {
                                format!("anle_s2f({})", raw)
                            } else {
                                raw
                            }
                        }
                        "keys" => format!("anle_map_keys({})", arg(0)),
                        "vals" => format!("anle_map_values({})", arg(0)),
                        "int" => {
                            //nt: float截断
                            let is_f = matches!(args.first(), Some(Value::Float(_)))
                                || matches!(args.first(), Some(Value::Local(id))
                                    if f.locals.get(*id).map(|l| l.ty.0 == "float").unwrap_or(false))
                                || matches!(args.first(), Some(Value::Str(_)));
                            if is_f {
                                format!("((long long)({}))", arg(0))
                            } else {
                                format!("anle_int_of_str({})", arg(0))
                            }
                        }
                        "str" => {
                            // nt: str
                            let is_float = matches!(args.first(), Some(Value::Float(_)))
                                || matches!(args.first(), Some(Value::Local(id))
                                    if f.locals.get(*id).map(|l| l.ty.0 == "float").unwrap_or(false));
                            if is_float {
                                format!("anle_str_of_float({})", arg(0))
                            } else {
                                format!("anle_str_of_int({})", arg(0))
                            }
                        }
                        "float" => {
                            //nt: str解析int提升
                            let is_s = matches!(args.first(), Some(Value::Str(_)))
                                || matches!(args.first(), Some(Value::Local(id))
                                    if f.locals.get(*id).map(|l| l.ty.0 == "str").unwrap_or(false));
                            if is_s {
                                format!("anle_float_of_str({})", arg(0))
                            } else {
                                format!("((double)({}))", arg(0))
                            }
                        }
                        "bool" => format!("(({}) != 0)", arg(0)),
                        // nt: 定长族转换
                        "int_8" | "uint_8" | "int_16" | "uint_16" | "int_32" | "uint_32"
                        | "int_64" | "uint_64" => {
                            format!("(({})({}))", c_ty_in(&Ty(name.clone()), types), arg(0))
                        }
                        // nt: 时间与随机 / FF
                        "rnd" => {
                            used.insert("anle_random");
                            format!("anle_random({})", arg(0))
                        }
                        // nt: remove
                        "remove" => {
                            let first_ty = match args.first() {
                                Some(Value::Local(id)) => f.locals.get(*id).map(|l| l.ty.0.clone()).unwrap_or_default(),
                                _ => String::new(),
                            };
                            //nt: map按键删除
                            if first_ty == "map" || first_ty.starts_with("map(") {
                                used.insert("anle_map_remove");
                                format!("anle_map_remove({}, {})", arg(0), key_c(f, &args.get(1).unwrap_or(&Value::Int(0))))
                            } else {
                            used.insert("anle_list_remove");
                            let a1 = args.get(1).map(value_c).unwrap_or_else(|| "1".into());
                            let raw = format!("anle_list_remove({}, {})", arg(0), a1);
                            if matches!(container_elem_ty(&first_ty).unwrap_or_default().as_str(), "str") {
                                format!("((const char*)(size_t){})", raw)
                            } else if matches!(container_elem_ty(&first_ty).unwrap_or_default().as_str(), "float") {
                                format!("anle_s2f({})", raw)
                            } else {
                                raw
                            }
                            }
                        }
// nt: 运行子程序
                        "run" => {
                            used.insert("anle_run");
                            let a1 = args.get(1).map(value_c).unwrap_or_else(|| "0".into());
                            let a2 = args.get(2).map(value_c).unwrap_or_else(|| "\"\"".into());
                            let a3 = args.get(3).map(value_c).unwrap_or_else(|| "\"\"".into());
                            format!("anle_run({}, {}, {}, {})", arg(0), a1, a2, a3)
                        }
                        "str_to_c" => {
                            used.insert("anle_to_c_str");
                            format!("((long long)(size_t)anle_to_c_str({}))", arg(0))
                        }
                        "str_from_c" => {
                            used.insert("anle_from_c_str");
                            format!("anle_from_c_str({})", arg(0))
                        }
                        // nt: 文件写
                        "save_file" => {
                            used.insert("anle_save_file");
                            format!("anle_save_file({}, {})", arg(0), arg(1))
                        }
                        _ => {
                            let callee = names.get(name).cloned().unwrap_or_else(|| name.clone());
                            format!("{}({})", callee, as_.join(", "))
                        }
                    }
                }
                Rvalue::Alloc { .. } => unreachable!(),
                Rvalue::ListLit { elems } => {
                    let es: Vec<String> = elems.iter().map(value_c).collect();
                    if es.is_empty() {
                        format!("anle_list_new({}, {})", elem_esz, elem_sgn)
                    } else if elems.iter().any(|e| is_str_val(f, e)) {
                        // nt: str 元素 ⇒ 逐
                        format!(
                            "anle_list_lit_s({}, (const char* const[]){{{}}})",
                            es.len(),
                            es.join(", ")
                        )
                    } else if elems.iter().any(|e| is_float_val(f, e)) {
                        // nt: 按元素判 float
                        let es2: Vec<String> = elems
                            .iter()
                            .map(|e| {
                                if is_float_val(f, e) {
                                    format!("anle_f2s({})", value_c(e))
                                } else {
                                    value_c(e)
                                }
                            })
                            .collect();
                        format!(
                            "anle_list_lit({}, (long long[]){{{}}}, {}, {})",
                            es.len(),
                            es2.join(", "),
                            elem_esz,
                            elem_sgn
                        )
                    } else if elems.iter().any(|e| is_ptr_val(f, e)) {
                        // nt: 把手元素 ⇒ 指针按
                        let es2: Vec<String> = elems
                            .iter()
                            .map(|e| format!("(long long)(size_t){}", value_c(e)))
                            .collect();
                        format!(
                            "anle_list_lit({}, (long long[]){{{}}}, {}, {})",
                            es.len(),
                            es2.join(", "),
                            elem_esz,
                            elem_sgn
                        )
                    } else if elems.iter().any(|e| is_agg_val(f, types, e)) {
                        //nt: 聚合字面量
                        let (c, _) = container_elem_ty(&dest_ty)
                            .and_then(|e| types.get(&e).map(|t| (t.0.clone(), ())))
                            .unwrap_or((String::new(), ()));
                        format!(
                            "anle_list_lit_from({}, (const char*)({}[]){{{}}}, sizeof({}))",
                            es.len(),
                            c,
                            es.join(", "),
                            c
                        )
                    } else {
                        format!(
                            "anle_list_lit({}, (long long[]){{{}}}, {}, {})",
                            es.len(),
                            es.join(", "),
                            elem_esz,
                            elem_sgn
                        )
                    }
                }
                Rvalue::MapLit { pairs } => {
                     // nt: 键槽随键型
                    let kty = map_kty(&dest_ty);
                    let ks: Vec<String> = pairs
                        .iter()
                        .map(|(k, _)| match k {
                            anle_ir::MapKey::Int(n) => format!("{}LL", n),
                            anle_ir::MapKey::Str(s) => format!("(long long)(size_t)\"{}\"", s),
                        })
                        .collect();
                    let vs: Vec<String> = pairs
                        .iter()
                        .map(|(_, v)| {
                            if is_float_val(f, v) {
                                format!("anle_f2s({})", value_c(v))
                            } else if is_str_val(f, v) {
                                format!("(long long)(size_t)anle_str_copy({})", value_c(v))
                            } else {
                                value_c(v)
                            }
                        })
                        .collect();
                    format!(
                        "anle_map_lit({}, (const long long[]){{{}}}, (long long[]){{{}}}, {}, {}, {})",
                        pairs.len(),
                        ks.join(", "),
                        vs.join(", "),
                        elem_esz,
                        elem_sgn,
                        kty
                    )
                }
                Rvalue::MapGet { map, key } => {
                    // nt: map 的值槽是 8
                    // nt: 值类型取 map 值型
                    let vt = match map {
                        Value::Local(id) => f
                            .locals
                            .get(*id)
                            .and_then(|l| l.ty.0.strip_prefix("map("))
                            .and_then(|x| x.strip_suffix(')'))
                            .and_then(|inner| {
                                let mut d = 0i32;
                                for (i, c) in inner.char_indices() {
                                    match c {
                                        '(' => d += 1,
                                        ')' => d -= 1,
                                        ',' if d == 0 => {
                                            return Some(inner[i + 1..].trim().to_string())
                                        }
                                        _ => {}
                                    }
                                }
                                None
                            })
                            .unwrap_or_default(),
                        _ => String::new(),
                    };
                    let raw = format!("anle_map_at({}, {})", value_c(map), key_c(f, key));
                    // nt: IR 侧已把结果类型
                    if vt == "str" {
                        format!("((const char*)(size_t){})", raw)
                    } else if vt.ends_with('*') {
                        format!("((void*)(size_t){})", raw)
                    } else if matches!(vt.split('(').next().unwrap_or("").trim(), "list" | "que" | "stack" | "map") {
                        format!("((AnleList*)(size_t){})", raw)
                    } else if vt == "float" {
                        format!("anle_s2f({})", raw)
                    } else {
                        raw
                    }
                }
                Rvalue::ListGet { list, index } => {
                    // nt: str 元素 ⇒ 把
                    let elem_str = matches!(list, Value::Local(id)
                        if f.locals.get(*id).map(|l| l.ty.0.starts_with("list(str")).unwrap_or(false));
                    let elem_handle = matches!(list, Value::Local(id)
                        if f.locals.get(*id).map(|l| {
                            let e = elem_ty_of(&l.ty.0).unwrap_or_default();
                            e.ends_with('*')
                        }).unwrap_or(false));
                    if elem_str {
                        format!("((const char*)(size_t)anle_list_at({}, {}))", value_c(list), value_c(index))
                    } else if elem_handle {
                        format!("((void*)(size_t)anle_list_at({}, {}))", value_c(list), value_c(index))
                    } else if container_elem_is_float(f, list) {
                        format!("anle_s2f(anle_list_at({}, {}))", value_c(list), value_c(index))
                    } else if container_elem_is_agg(f, list, types) {
                        //nt: 聚合走助手
                        let 助手 = 容器元素助手(f, list, types);
                        format!("{}({}, {})", 助手, value_c(list), value_c(index))
                    } else {
                        format!("anle_list_at({}, {})", value_c(list), value_c(index))
                    }
                }
            };
            // nt: 积类型
            // nt: 目标类型按**投影链
            let base_ty = dest_ty.trim_end_matches('*').to_string();
            let is_handle = dest_ty.ends_with('*');
            let is_struct_val = matches!(rhs, Rvalue::Use(_) | Rvalue::Const(Value::Local(_)));
// nt: 把手 ⇒ 把手
            if is_handle && is_struct_val {
                let src_is_handle = match rhs {
                    Rvalue::Use(p) => f.locals.get(p.base).map(|l| l.ty.0.ends_with('*')).unwrap_or(false),
                    Rvalue::Const(Value::Local(id)) => {
                        f.locals.get(*id).map(|l| l.ty.0.ends_with('*')).unwrap_or(false)
                    }
                    _ => false,
                };
                if src_is_handle {
                    out.push_str(&format!("  {} = {};\n", place_c(f, types, lhs), r));
                    return Ok(());
                }
            }
            if types.contains_key(&base_ty) && is_struct_val {
                let (ct, _) = types.get(&base_ty).unwrap();
                let dc = ct.replace("anle_st_", "anle_dc_");
                let lhs_c = place_c(f, types, lhs);
                if is_handle {
                    let ptr_ct = c_ty_in(&Ty(dest_ty.clone()), types); // anle_st_0*
                    out.push_str(&format!(
                        "  {lhs} = ({ptr})anle_region_alloc(anle_region_cur, (long long)sizeof({ct}));\n  *{lhs} = {dc}(&{r});\n",
                        lhs = lhs_c,
                        ptr = ptr_ct,
                        ct = ct,
                        dc = dc,
                        r = r
                    ));
                } else {
                    out.push_str(&format!("  {} = {}(&{});\n", lhs_c, dc, r));
                }
                return Ok(());
            }
            //nt: 槽赋容器复印
            //nt: fn内容器活得过fn
            let 槽赋容器 = matches!(rhs, Rvalue::ListLit { .. })
                && matches!(lhs, Place { base, proj } if proj.is_empty()
                    && f.locals.get(*base).map(|l| l.is_param && l.ty.0.ends_with('*')).unwrap_or(false));
            let r = if 槽赋容器 {
                format!("anle_list_dc_parent({})", r)
            } else {
                r
            };
            out.push_str(&format!("  {} = {};\n", place_c(f, types, lhs), r));
        }
        // nt: 生命周期 / 访问期
        Stmt::Live(_) | Stmt::Dead(_) | Stmt::AccessBegin(_) | Stmt::AccessEnd(_) | Stmt::Release(_) | Stmt::Destroy(_) => {}
        // nt: map 键写
        Stmt::MapSet { map, key, value } => {
            let slot = if is_str_val(f, value) {
                format!("(long long)(size_t)anle_str_copy({})", value_c(value))
            } else if is_float_val(f, value) {
                format!("anle_f2s({})", value_c(value))
            } else {
                value_c(value)
            };
            out.push_str(&format!(
                "  anle_map_set({}, {}, {});\n",
                value_c(map),
                key_c(f, key),
                slot
            ))
        }
        // nt: 下标写
        Stmt::ListSet { list, index, value } => {
            if is_str_val(f, value) {
                out.push_str(&format!(
                    "  anle_list_setat({}, {}, (long long)(size_t)anle_str_copy({}));\n",
                    value_c(list),
                    value_c(index),
                    value_c(value)
                ));
            } else if is_float_val(f, value) {
                out.push_str(&format!(
                    "  anle_list_setat({}, {}, anle_f2s({}));\n",
                    value_c(list),
                    value_c(index),
                    value_c(value)
                ));
            } else if is_agg_val(f, types, value) {
                //nt: 聚合按地址
                out.push_str(&format!(
                    "  anle_list_setat_from({}, {}, (const char*)&{});\n",
                    value_c(list),
                    value_c(index),
                    value_c(value)
                ));
            } else {
                out.push_str(&format!(
                    "  anle_list_setat({}, {}, {});\n",
                    value_c(list),
                    value_c(index),
                    value_c(value)
                ));
            }
        }
        Stmt::Copy { from, to } => out.push_str(&format!(
            "  {} = {};\n",
            place_c(f, types, to),
            place_c(f, types, from)
        )),
        Stmt::Assert { cond, msg, fix } => out.push_str(&format!(
            "  if (!({})) {{ anle_die({}, anle_line, \"{}\", \"{}\"); }}\n",
            value_c(cond),
            if msg.contains("空引用") { "1" } else { "7" },
            c_escape(msg),
            c_escape(fix)
        )),
    }
    Ok(())
}

// nt: 圈头判定
// nt: 需要块的全局视野 ⇒
thread_local! {
    static LOOP_HEADS: std::cell::RefCell<std::collections::BTreeSet<usize>> =
        std::cell::RefCell::new(std::collections::BTreeSet::new());
}

fn is_loop_head(b: &Block) -> bool {
    LOOP_HEADS.with(|h| h.borrow().contains(&b.id))
}

// nt: 预扫函数内所有块，标
fn mark_loop_heads(blocks: &[Block]) {
    let mut heads = std::collections::BTreeSet::new();
    for b in blocks {
        let ts: &[usize] = match &b.exit {
            Exit::Branch { targets, .. } => targets,
            _ => &[],
        };
        for &t in ts {
            if t < b.id {
                heads.insert(t);
            }
        }
    }
    LOOP_HEADS.with(|h| *h.borrow_mut() = heads);
}

// nt: 链根集
thread_local! {
    static ROOT_FNS: std::cell::RefCell<std::collections::BTreeSet<String>> =
        std::cell::RefCell::new(std::collections::BTreeSet::new());
}

// nt: 跨块槽全局化
thread_local! {
    //nt: 槽局部到全局名
    static SLOT_MAP: std::cell::RefCell<std::collections::BTreeMap<usize, String>> =
        std::cell::RefCell::new(std::collections::BTreeMap::new());
    //nt: 已复印逃逸的局部
    static ESCAPED: std::cell::RefCell<std::collections::BTreeSet<usize>> =
        std::cell::RefCell::new(std::collections::BTreeSet::new());
}

//nt: 槽名修饰
fn slot_global_name(key: &str, idx: usize) -> String {
    let _ = key;
    format!("anle_slot_{}", idx)
}

//nt: 槽局部判定
fn is_slot_local(l: &Local) -> bool {
    l.name.as_deref().map(|n| n.contains('#')).unwrap_or(false)
}

// nt: 块函数出口代码
// nt: 必须在**每个 re
fn exit_restore_code(f: &Function) -> String {
    let is_root = ROOT_FNS.with(|r| r.borrow().contains(&f.name));
    // nt: d kill
    if f.kills_own && f.name != "main" {
        return "  anle_region_cur = __r->parent; anle_region_free(__r);\n".to_string();
    }
    if is_root && f.name != "main" {
        "  anle_region_cur = __r->parent; anle_region_free(__r);\n".to_string()
    } else {
        "  anle_region_cur = __r->parent;\n".to_string()
    }
}

// nt: 保守判定
// nt: 那意味着本块持有跨块
fn loop_body_has_call_assign(f: &Function, head_id: usize, cur_id: usize) -> bool {
    for b in &f.blocks {
        if b.id >= head_id && b.id <= cur_id {
            for (i, s) in b.stmts.iter().enumerate() {
                if let Stmt::Assign { lhs, rhs: Rvalue::Call { name, .. } } = s {
                    //nt: 复印语句不建段
                    if matches!(name.as_str(), "#str_copy" | "#dc_list" | "#dc_map") {
                        continue;
                    }
                    //nt: 调用落临时则看下一句
                    let 是临时 = f.locals.get(lhs.base).map(|l| l.name.is_none()).unwrap_or(true);
                    let (dst, 位) = if 是临时 {
                        match b.stmts.get(i + 1) {
                            Some(Stmt::Assign { lhs: d, rhs }) => {
                                let src = match rhs {
                                    Rvalue::Const(Value::Local(u)) => *u,
                                    Rvalue::Use(p) => p.base,
                                    _ => continue,
                                };
                                if src != lhs.base {
                                    continue;
                                }
                                (d.base, i + 2)
                            }
                            _ => continue,
                        }
                    } else {
                        (lhs.base, i + 1)
                    };
                    let Some(l) = f.locals.get(dst) else { continue };
                    //nt: 未标跨圈无需护
                    if !l.loop_escape {
                        continue;
                    }
                    //nt: 已复印逃逸值
                    if matches!(
                        b.stmts.get(位),
                        Some(Stmt::Assign { rhs: Rvalue::Call { name, .. }, .. })
                            if matches!(name.as_str(), "#str_copy" | "#dc_list" | "#dc_map")
                                || name.starts_with("#dcdeep_")
                    ) {
                        continue;
                    }
                    return true;
                }
            }
        }
    }
    false
}

fn block_c(
    f: &Function,
    b: &Block,
    out: &mut String,
    used: &mut BTreeSet<&'static str>,
    is_void: bool,
    names: &BTreeMap<String, String>,
    types: &TypeMap,
) -> Result<(), CodegenError> {
    out.push_str(&format!("bb{}:\n", b.id));
    // nt: 圈头取标记
    if is_loop_head(b) {
        out.push_str(&format!("  __m{} = anle_region_scope_mark();\n", b.id));
    }
    for (i, s) in b.stmts.iter().enumerate() {
        // nt: 语句级位置
        if let Some(sp) = b.spans.get(i) {
            if sp.line > 0 {
                if DEBUG_LINES.with(|c| c.get()) {
                    let f = SRC_FILE.with(|c| c.borrow().clone());
                    out.push_str(&format!("#line {} \"{}\"\n", sp.line, c_escape(&f)));
                }
                //nt: 当前源码行
                let key = if f.file.is_empty() {
                    SRC_FILE.with(|c| c.borrow().clone())
                } else {
                    f.file.clone()
                };
                let 行文本 = SRC_LINES
                    .with(|m| m.borrow().get(&key).and_then(|ls| ls.get(sp.line - 1).cloned()))
                    .unwrap_or_default();
                if 行文本.is_empty() {
                    out.push_str("  anle_src = 0;\n");
                } else {
                    out.push_str(&format!("  anle_src = \"{}\";\n", c_escape(&行文本)));
                }
                out.push_str(&format!(
                    "  anle_line = {}; anle_col = {}; anle_len = {};\n",
                    sp.line, sp.col, sp.len
                ));
            }
        }
        stmt_c(f, s, out, used, names, types)?;
    }
    match &b.exit {
        Exit::Return(Some(v)) => {
            //nt: fn自有对象复印重绑
            //nt: 复印目标 = 调用方地皮
            let esc = match v {
                Value::Local(id) => f.locals.get(*id).filter(|l| l.fn_owned).and_then(|l| {
                    let b = l.ty.0.trim_end_matches('*').to_string();
                    types.get(&b).map(|(ct, _)| (*id, vname(*id), ct.clone()))
                }),
                _ => None,
            };
            if let Some((lid, vn, ct)) = &esc {
                let 已逃 = ESCAPED.with(|s| s.borrow().contains(lid));
                if !已逃 {
                    out.push_str(&format!(
                        "  {vn} = ({ct}*)anle_copy_to_parent({vn}, (long long)sizeof({ct}));\n",
                        vn = vn,
                        ct = ct
                    ));
                    ESCAPED.with(|s| s.borrow_mut().insert(*lid));
                }
            }
            out.push_str(&exit_restore_code(f));
            out.push_str(&format!("  return {};\n", value_c(v)))
        }
        // nt: 无返回值的块/函数
        Exit::Return(None) => {
            out.push_str(&exit_restore_code(f));
            out.push_str(if is_void { "  return;\n" } else { "  return 0;\n" })
        }
        Exit::Unreachable => out.push_str("  abort();\n"),
        Exit::Branch { cond, targets } => match targets.len() {
            0 => return Err(CodegenError { message: "分支没有目标块".into() , code: "E401".into() , line: 0 }),
            1 => {
                let rel = if targets[0] < b.id && !loop_body_has_call_assign(f, targets[0], b.id) {
                    format!("  anle_region_scope_release(__m{});\n", targets[0])
                } else {
                    String::new()
                };
                out.push_str(&format!("{}  goto bb{};\n", rel, targets[0]))
            }
            2 => {
                // nt: 回边才回收
                let rel_of = |t: usize| -> String {
                    if t < b.id && !loop_body_has_call_assign(f, t, b.id) {
                        format!("anle_region_scope_release(__m{}); ", t)
                    } else {
                        String::new()
                    }
                };
                let (t0_back, t1_back) = (targets[0] < b.id, targets[1] < b.id);
                let cond = value_c(cond);
                out.push_str(&match (t0_back, t1_back) {
                    (true, false) => format!(
                        "  if ({}) {{ {}goto bb{}; }} else goto bb{};\n",
                        cond, rel_of(targets[0]), targets[0], targets[1]
                    ),
                    (false, true) => format!(
                        "  if ({}) goto bb{}; else {{ {}goto bb{}; }}\n",
                        cond, targets[0], rel_of(targets[1]), targets[1]
                    ),
                    (true, true) => format!(
                        "  if ({}) {{ {}goto bb{}; }} else {{ {}goto bb{}; }}\n",
                        cond,
                        rel_of(targets[0]),
                        targets[0],
                        rel_of(targets[1]),
                        targets[1]
                    ),
                    _ => format!(
                        "  if ({}) goto bb{}; else goto bb{};\n",
                        cond, targets[0], targets[1]
                    ),
                })
            }
            _ => {
                out.push_str(&format!("  switch ({}) {{\n", value_c(cond)));
                for (i, t) in targets.iter().enumerate() {
                    out.push_str(&format!("    case {}: goto bb{};\n", i, t));
                }
                out.push_str("  }\n");
            }
        },
    }
    Ok(())
}

fn support_section(used: &BTreeSet<&'static str>) -> String {
    let mut s = String::new();
    let emit_overflow = |name: &str, builtin: &str| {
        format!(
            "static long long {}(long long a, long long b) {{\n  long long r;\n  if ({}(a, b, &r)) {{ anle_die_nc(3, \"整数运算溢出——加减乘结果超出了 int 的范围\", \"改用 float，或把计算拆小步\"); }}\n  return r;\n}}\n",
            name, builtin
        )
    };
    if used.contains("anle_sadd") {
        s.push_str(&emit_overflow("anle_sadd", "__builtin_add_overflow"));
        s.push('\n');
    }
    if used.contains("anle_ssub") {
        s.push_str(&emit_overflow("anle_ssub", "__builtin_sub_overflow"));
        s.push('\n');
    }
    if used.contains("anle_smul") {
        s.push_str(&emit_overflow("anle_smul", "__builtin_mul_overflow"));
        s.push('\n');
    }
    if used.contains("anle_div") || used.contains("anle_mod") {
        s.push_str(
            "long long anle_div(long long a, long long b) {\n  if (b == 0) { anle_die_nc(2, \"除以零——除数不能是 0\", \"先用 if 判一下（if 除数 != 0:）再除\"); }\n  return a / b;\n}\n",
        );
        s.push('\n');
    }
    if used.contains("anle_mod") {
        s.push_str(
            "long long anle_mod(long long a, long long b) {\n  if (b == 0) { anle_die_nc(2, \"除以零——除数不能是 0\", \"先用 if 判一下（if 除数 != 0:）再除\"); }\n  return a % b;\n}\n\n",
        );
    }
    // nt: 时间与随机
    if used.contains("anle_sleep") {
        s.push_str("\n#ifdef _WIN32\n#include <windows.h>\n#else\n#include <unistd.h>\n#endif\nvoid anle_sleep(long long ms) {\n#ifdef _WIN32\n  Sleep((unsigned long)ms);\n#else\n  usleep((unsigned int)(ms * 1000));\n#endif\n}\n");
    }
    if used.contains("anle_random") {
        s.push_str("\n#include <time.h>\nlong long anle_random(long long n) {\n  static int seeded = 0;\n  if (!seeded) { srand((unsigned)time(0)); seeded = 1; }\n  if (n <= 0) n = 1;\n  return (long long)(((unsigned long long)rand()) % (unsigned long long)n);\n}\n");
    }
// nt: 运行子程序
    // nt: stdout / s

    s
}

//nt: FFI 串转换
fn ffi_str_section(used: &BTreeSet<&'static str>) -> String {
    if !(used.contains("anle_to_c_str") || used.contains("anle_from_c_str")) {
        return String::new();
    }
    "const char* anle_to_c_str(const char* s) {\n  size_t n = strlen(s) + 1;\n  char* p = (char*)anle_region_alloc(anle_region_cur, (long long)n);\n  memcpy(p, s, n);\n  return p;\n}\nconst char* anle_from_c_str(long long addr) {\n  const char* s = (const char*)(size_t)addr;\n  size_t n = strlen(s) + 1;\n  char* p = (char*)anle_region_alloc(anle_region_cur, (long long)n);\n  memcpy(p, s, n);\n  return p;\n}\n".to_string()
}

// nt: 运行子程序运行时
const ANLE_RUN_RT: &str = r#"
#ifdef _WIN32
#include <windows.h>
long long anle_run(const char* prog, AnleList* args, const char* out_file, const char* err_file) {
  if (!prog || !prog[0]) return -404;
  wchar_t wcmd[16384]; wchar_t wtmp[4096];
  // nt: 命令行形式
  if (MultiByteToWideChar(CP_UTF8, 0, prog, -1, wtmp, 4096) <= 0) return -10086;
  _snwprintf(wcmd, 16384, L"\"%s\"", wtmp);
  long long i = 0;
  while (args && i < args->len) {
    const char* a = (const char*)(size_t)anle_slot_get(anle_list_slot(args, i), args->esz, args->sgn);
    if (a && a[0] && MultiByteToWideChar(CP_UTF8, 0, a, -1, wtmp, 4096) > 0) {
      wcscat_s(wcmd, 16384, L" "); if (strchr(a, 32)) wcscat_s(wcmd, 16384, (const wchar_t[]){0x22, 0});
      wcscat_s(wcmd, 16384, wtmp); if (strchr(a, 32)) wcscat_s(wcmd, 16384, (const wchar_t[]){0x22, 0});
    }
    i = i + 1;
  }
  HANDLE ho = INVALID_HANDLE_VALUE, he = INVALID_HANDLE_VALUE;
  SECURITY_ATTRIBUTES sa; sa.nLength = sizeof sa; sa.lpSecurityDescriptor = NULL; sa.bInheritHandle = TRUE;

  STARTUPINFOW si; PROCESS_INFORMATION pi;
  ZeroMemory(&si, sizeof si); si.cb = sizeof si; ZeroMemory(&pi, sizeof pi);
  if (out_file && out_file[0]) {
    if (MultiByteToWideChar(CP_UTF8, 0, out_file, -1, wtmp, 4096) <= 0) return -10086;
    ho = CreateFileW(wtmp, GENERIC_WRITE, 0, &sa, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
    if (ho == INVALID_HANDLE_VALUE) return -10086;
  }
  if (err_file && err_file[0]) {
    if (MultiByteToWideChar(CP_UTF8, 0, err_file, -1, wtmp, 4096) <= 0) { if (ho != INVALID_HANDLE_VALUE) CloseHandle(ho); return -10086; }
    he = CreateFileW(wtmp, GENERIC_WRITE, 0, &sa, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
    if (he == INVALID_HANDLE_VALUE) { if (ho != INVALID_HANDLE_VALUE) CloseHandle(ho); return -10086; }
  }
  if (ho != INVALID_HANDLE_VALUE || he != INVALID_HANDLE_VALUE) {
    si.dwFlags |= STARTF_USESTDHANDLES;
    si.hStdInput = GetStdHandle(STD_INPUT_HANDLE);
    si.hStdOutput = (ho != INVALID_HANDLE_VALUE) ? ho : GetStdHandle(STD_OUTPUT_HANDLE);
    si.hStdError = (he != INVALID_HANDLE_VALUE) ? he : GetStdHandle(STD_ERROR_HANDLE);
  }
  BOOL ok = CreateProcessW(NULL, wcmd, NULL, NULL, TRUE, 0, NULL, NULL, &si, &pi);
  if (!ok) {
    DWORD e = GetLastError();
    if (ho != INVALID_HANDLE_VALUE) CloseHandle(ho);
    if (he != INVALID_HANDLE_VALUE) CloseHandle(he);
    return (e == ERROR_FILE_NOT_FOUND || e == ERROR_PATH_NOT_FOUND) ? -404 : -10086;
  }
  WaitForSingleObject(pi.hProcess, INFINITE);
  DWORD rc = 0; GetExitCodeProcess(pi.hProcess, &rc);
  CloseHandle(pi.hProcess); CloseHandle(pi.hThread);
  if (ho != INVALID_HANDLE_VALUE) CloseHandle(ho);
  if (he != INVALID_HANDLE_VALUE) CloseHandle(he);
  if (rc >= 0xC0000000u) return -4;
  return (long long)rc;
}
#else
#include <unistd.h>
#include <sys/wait.h>
#include <fcntl.h>
#include <errno.h>
long long anle_run(const char* prog, AnleList* args, const char* out_file, const char* err_file) {
  if (!prog || !prog[0]) return -404;
  long long n = args ? args->len : 0;
  char** av = (char**)malloc(sizeof(char*) * (size_t)(n + 2));
  if (!av) return -10086;
  av[0] = (char*)prog;
  long long i = 0;
  while (i < n) {
    const char* a = (const char*)(size_t)anle_slot_get(anle_list_slot(args, i), args->esz, args->sgn);
    av[i + 1] = (char*)(a ? a : "");
    i = i + 1;
  }
  av[n + 1] = 0;
  int errp[2];
  if (pipe(errp) != 0) { free(av); return -10086; }
  fcntl(errp[0], F_SETFD, FD_CLOEXEC);
  fcntl(errp[1], F_SETFD, FD_CLOEXEC);
  pid_t pid = fork();
  if (pid < 0) { close(errp[0]); close(errp[1]); free(av); return -10086; }
  if (pid == 0) {
    close(errp[0]);
    if (out_file && out_file[0]) {
      int fd = open(out_file, O_WRONLY | O_CREAT | O_TRUNC, 0644);
      if (fd < 0) { int e = errno; ssize_t w = write(errp[1], &e, sizeof e); (void)w; _exit(127); }
      dup2(fd, 1);
      close(fd);
    }
    if (err_file && err_file[0]) {
      int fd = open(err_file, O_WRONLY | O_CREAT | O_TRUNC, 0644);
      if (fd < 0) { int e = errno; ssize_t w = write(errp[1], &e, sizeof e); (void)w; _exit(127); }
      dup2(fd, 2);
      close(fd);
    }
    execvp(prog, av);
    { int e = errno; ssize_t w = write(errp[1], &e, sizeof e); (void)w; }
    _exit(127);
  }
  close(errp[1]);
  int e = 0;
  ssize_t got = read(errp[0], &e, sizeof e);
  close(errp[0]);
  free(av);
  int st = 0;
  while (waitpid(pid, &st, 0) < 0 && errno == EINTR) {}
  if (got == (ssize_t)sizeof e) return (e == ENOENT) ? -404 : -10086;
  if (WIFEXITED(st)) return (long long)WEXITSTATUS(st);
  return -4;
}
#endif
"#;


// nt: 调用的内置名 → 它
fn mark_runtime(name: &str, used: &mut BTreeSet<&'static str>) {
    match name {
        "rnd" => {
            used.insert("anle_random");
        }
// nt: 运行子程序
        "run" => {
            used.insert("anle_run");
        }
        "sle_ms" => {
            used.insert("anle_sleep");
        }
        "str_to_c" => {
            used.insert("anle_to_c_str");
        }
        "str_from_c" => {
            used.insert("anle_from_c_str");
        }
        _ => {}
    }
}

// nt: lower

// nt: 运行期护栏统一出口
const ANLE_ARENA_RT: &str = r#"/* ===== 确定性内存管理：段树 + 段池（2026-10-17 新做，不抄旧实现）=====

 * 段级池：按段分配与回收，段与段互不干扰

 * 要点：每块实例一段（树）；段内 bump 分配 O(1)；段池按 64KB 统一档 O(1) 借还。

 * 分配策略：不做字节级 best-fit freelist（O(n) 热路径），

 * 复用走**段级**池；不设静默上限（池满/池空明确报错）。

 * 现阶段：只落实现，暂不接线（所有分配仍走 libc malloc）。

 */

#define ANLE_SEG_BLK 65536u

#define ANLE_SEG_POOL_MAX 65536

#define ANLE_SEG_ERR_SZ 7



typedef struct AnleSegBlock {

    char* base; size_t cap; struct AnleSegBlock* next;

} AnleSegBlock;



typedef struct AnleRegion {

    struct AnleRegion* parent;

    struct AnleRegion* kids;

    struct AnleRegion* sib;

    AnleSegBlock* blocks;

    AnleSegBlock* cur;

    size_t used;

    int alive; const char* blk;   /* 产生本段的块名（kill 按名找活跃实例段用） */

} AnleRegion;



static AnleSegBlock* anle_seg_pool = NULL;

long long anle_seg_pool_n = 0;

long long anle_seg_live = 0;
static AnleRegion* anle_region_cur = NULL;

/* 段结构池：空调用不再为结构体占一整块 */
#define ANLE_REG_SLAB 256
static char* anle_reg_slab = NULL;
static long long anle_reg_left = 0;
static AnleRegion* anle_reg_free = NULL;

static AnleRegion* anle_region_node(void) {
    AnleRegion* r;
    if (anle_reg_free) { r = anle_reg_free; anle_reg_free = r->sib; return r; }
    if (anle_reg_left <= 0) {
        anle_reg_slab = (char*)malloc(sizeof(AnleRegion) * ANLE_REG_SLAB);
        if (!anle_reg_slab) return NULL;
        anle_reg_left = ANLE_REG_SLAB;
    }
    r = (AnleRegion*)(anle_reg_slab + (ANLE_REG_SLAB - anle_reg_left) * sizeof(AnleRegion));
    anle_reg_left--;
    return r;
}



static AnleSegBlock* anle_seg_take(void) {

    AnleSegBlock* b;

    if (anle_seg_pool) {

        b = anle_seg_pool; anle_seg_pool = b->next; anle_seg_pool_n--; b->next = NULL;

        return b;

    }

    b = (AnleSegBlock*)malloc(sizeof(AnleSegBlock));

    if (!b) return NULL;

    b->base = (char*)malloc(ANLE_SEG_BLK);

    if (!b->base) { free(b); return NULL; }

    b->cap = ANLE_SEG_BLK; b->next = NULL;

    return b;

}



void anle_seg_give(AnleSegBlock* b) {

    if (!b) return;

    /* oversized 块回池会占着大内存 ⇒ 直接还给 libc */
    if (b->cap != ANLE_SEG_BLK) { free(b->base); free(b); return; }

    if (anle_seg_pool_n >= ANLE_SEG_POOL_MAX) { free(b->base); free(b); return; }

    b->next = anle_seg_pool; anle_seg_pool = b; anle_seg_pool_n++;

}



static AnleRegion* anle_region_open(AnleRegion* parent, const char* blkname) {

    AnleRegion* r = anle_region_node();

    if (!r) { anle_die_nc(ANLE_SEG_ERR_SZ, "内存不够——段池分配失败", "缩小数据量"); return NULL; }

    r->parent = parent; r->kids = NULL; r->sib = NULL;

    r->blocks = NULL; r->cur = NULL; r->used = 0;   /* 首次分配才取段 */

    r->alive = 1; r->blk = blkname;

    if (parent) { r->sib = parent->kids; parent->kids = r; }

    anle_seg_live++;

    return r;

}



static void* anle_region_alloc(AnleRegion* r, long long size) {

    size_t need = (size <= 0) ? 8u : (size_t)((size + 7) & ~((long long)7));

    char* p;
    if (!r) return malloc(need);   /* 未开段（函数外/初始化）⇒ libc 兜底 */
    if (!r->cur || r->used + need > r->cur->cap) {

        AnleSegBlock* b;

        /* 大于整段的请求单独开块，不塞进 64KB 段 */
        if (need > ANLE_SEG_BLK) {
            b = (AnleSegBlock*)malloc(sizeof(AnleSegBlock));
            if (!b) return NULL;
            b->base = (char*)malloc(need);
            if (!b->base) { free(b); return NULL; }
            b->cap = need; b->next = NULL;
        } else {
            b = anle_seg_take();
        }

        if (!b) return NULL;

        if (r->cur) r->cur->next = b; else r->blocks = b;

        r->cur = b; r->used = 0;

    }

    p = r->cur->base + r->used;

    r->used += need;

    return p;

}

/* 逃逸复印：整块拷进父段（本圈随后可照样回收） */
static void* anle_copy_to_parent(const void* src, long long n) {
    AnleRegion* t = anle_region_cur ? anle_region_cur->parent : 0;
    if (!t) t = anle_region_cur;
    void* d = anle_region_alloc(t, n);
    if (src && d) memcpy(d, src, (size_t)n);
    return d;
}




/* 圈级回收（ 十一-3）：循环体每圈调用产生的子段，圈尾批量还池（子树随之）。
 * 用法：圈头 anle_region_scope_mark() 取"当前段的子段链头"；回到圈头前
 * anle_region_scope_release(mark) 把"该标记之后新增的子段"整棵还池。
 * 这是**编译器隐式机制**，与调用方 kill 无关。 */
void anle_region_free(AnleRegion* r);   /* 前向声明（scope_release 要用） */

static AnleRegion* anle_region_scope_mark(void) {
    return anle_region_cur ? anle_region_cur->kids : NULL;
}

/* 显式提前回收（ 十一-2）：`kill #:块名` —— 释放该块的**活跃实例段**（子树随之）。
 * 从当前段向上找到链根，再从链根递归下探，回收所有 blk 名匹配的活跃段。
 * 仅 unsafe 块内、仅顺序语句位置（由 check 保证）。 */
void anle_region_kill(const char* name) {
    AnleRegion* root = anle_region_cur;
    AnleRegion* k;
    if (!root || !name) return;
    while (root->parent) root = root->parent;   /* 找到链根 */
    /* 下探一层：链根自己也算候选（但 kill 目标须非当前块，由 check 保证） */
    k = root->kids;
    while (k) {
        AnleRegion* nx = k->sib;
        if (k->alive && k->blk && strcmp(k->blk, name) == 0) {
            AnleRegion* np = k->parent;
            anle_region_free(k);                 /* 子树随之还池 */
            k = np ? np->kids : NULL;            /* 链头可能已变，重取 */
            continue;
        }
        k = nx;
    }
}

void anle_region_scope_release(AnleRegion* mark) {
    AnleRegion* k;
    if (!anle_region_cur) return;
    k = anle_region_cur->kids;
    while (k && k != mark) {
        AnleRegion* nx = k->sib;
        anle_region_free(k);          /* 子树随之还池 */
        k = nx;
    }
}

void anle_region_free(AnleRegion* r) {

    AnleSegBlock* b;

    AnleRegion* k;

    if (!r || !r->alive) return;

    k = r->kids; while (k) { AnleRegion* nx = k->sib; anle_region_free(k); k = nx; }

    b = r->blocks;

    r->blocks = NULL; r->cur = NULL; r->used = 0; r->alive = 0;

    anle_seg_live--;

    if (r->parent) {

        AnleRegion** pp = &r->parent->kids;

        while (*pp && *pp != r) pp = &(*pp)->sib;

        if (*pp) *pp = r->sib;

    }

    while (b) { AnleSegBlock* n = b->next; anle_seg_give(b); b = n; }

    r->sib = anle_reg_free; anle_reg_free = r;   /* 结构体复用 */

}

"#;

const ANLE_DIE_RT: &str = r#"void anle_die(int code, long long line, const char* msg, const char* fix) {
  const char* srcf = anle_cur_file ? anle_cur_file : ANLE_FILE;
  fprintf(stderr, "[R%02d] 位于 %s", code, srcf);
  if (line > 0) {
    long long n = (long long)(sizeof(ANLE_SRC) / sizeof(ANLE_SRC[0])) - 1;
    if (anle_col > 0) {
      fprintf(stderr, " 第 %lld 行第 %lld 列发生错误", line, anle_col);
    } else {
      fprintf(stderr, " 第 %lld 行发生错误", line);
    }
    if (anle_src) {
      fprintf(stderr, "，源码为：%s", anle_src);
    } else if (anle_cur_file == 0 && line >= 1 && line <= n) {
      fprintf(stderr, "，源码为：%s", ANLE_SRC[line - 1]);
    }
  } else {
    fprintf(stderr, "发生错误");
  }
  fprintf(stderr, "，错误为：%s。改法：%s。\n", msg, fix);
  exit(1);
}
void anle_die_nc(int code, const char* msg, const char* fix) { anle_die(code, anle_line, msg, fix); }
"#;

// nt: 供增量编译
pub fn split_parts(full: &str) -> (String, Vec<(usize, String)>) {
    const BEG: &str = "/*@FN_BEGIN:";
    const END: &str = "/*@FN_END@*/";
    let mut units: Vec<(usize, String)> = Vec::new();
    let first = match full.find(BEG) {
        Some(p) => p,
        None => return (full.to_string(), units),
    };
    let prelude = full[..first].to_string();
    let mut pos = first;
    while let Some(rel) = full[pos..].find(BEG) {
        let bpos = pos + rel;
        let after = &full[bpos + BEG.len()..];
        let close = match after.find("@*/") { Some(c) => c, None => break };
        let idx: usize = after[..close].trim().parse().unwrap_or(0);
        let body_start = bpos + BEG.len() + close + 3;
        let epos = match full[body_start..].find(END) { Some(e) => e, None => break };
        units.push((idx, full[body_start..body_start + epos].to_string()));
        pos = body_start + epos + END.len();
    }
    (prelude, units)
}

// nt: 导出完整 C 运行时
pub fn runtime_c() -> String {
    runtime_c_for("", "")
}

/// nt: 运行时对象
pub fn runtime_c_for(file: &str, src_text: &str) -> String {
    let mut used: BTreeSet<&'static str> = BTreeSet::new();
    for n in [
        "anle_sadd", "anle_ssub", "anle_smul", "anle_div", "anle_mod", "anle_run",
        "anle_list_at", "anle_list_push", "anle_map_at", "anle_str_eq", "anle_concat",
        "anle_str_index",
        "anle_read_file", "anle_save_file", "anle_args", "anle_sleep", "anle_random",
        "anle_to_c_str", "anle_from_c_str",
    ] {
        used.insert(n);
    }
    let mut s = String::new();
    s.push_str("#include <stdio.h>\n#include <stdlib.h>\n#include <string.h>\n\n");
    //nt: 护栏开关宏
    s.push_str("#ifdef ANLE_NOGUARD\n#define ANLE_GUARD(x)\n#else\n#define ANLE_GUARD(x) x\n#endif\n\n");
    s.push_str("long long anle_line = 0; long long anle_col = 0; long long anle_len = 0; const char* anle_cur_file = 0; const char* anle_src = 0;\nvoid anle_at(void) { fprintf(stderr, \"  （第 %lld 行）\\n\", anle_line); }\nconst char* anle_bool_str(long long b) { return b ? \"re\" : \"fa\"; }\n\n");
    {
        let esc = |x: &str| x.replace('\\', "\\\\").replace('"', "\\\"");
        s.push_str(&format!("static const char* ANLE_FILE = \"{}\";\n", esc(file)));
        s.push_str("static const char* ANLE_SRC[] = {\n");
        for ln in src_text.lines() {
            s.push_str(&format!("  \"{}\",\n", esc(ln)));
        }
        s.push_str("  0\n};\n");
    }
    s.push_str(ANLE_DIE_RT);
    s.push_str(&support_section(&used));
    s.push_str(ANLE_ARENA_RT);
    s.push_str(&ffi_str_section(&used));
    s.push_str(ANLE_LIST_RT);
    s.push_str(ANLE_RUN_RT);
    s.push_str(ANLE_STR_RT);
    s.push_str(ANLE_MAP_RT);
    s.push_str(ANLE_FILE_RT);
    s.push_str(ANLE_ARGS_RT);
    s.push_str("const char* anle_str_copy(const char* s) {\n  if (!s) return 0;\n  size_t n = strlen(s) + 1;\n  char* p = (char*)anle_region_alloc(anle_region_cur, (long long)n);\n  if (!p) { anle_die_nc(6, \"内存不够\", \"缩小数据量\"); }\n  memcpy(p, s, n);\n  return p;\n}\n\n");
    s.replace("static long long anle_sadd(", "long long anle_sadd(")
        .replace("static long long anle_ssub(", "long long anle_ssub(")
        .replace("static long long anle_smul(", "long long anle_smul(")
        .replace("static AnleRegion* anle_region_cur", "AnleRegion* anle_region_cur")
        .replace("static AnleRegion* anle_region_open", "AnleRegion* anle_region_open")
        .replace("static void* anle_region_alloc", "void* anle_region_alloc")
        .replace("static AnleRegion* anle_region_scope_mark", "AnleRegion* anle_region_scope_mark")
        .replace("static void* anle_copy_to_parent(", "void* anle_copy_to_parent(")
        .replace("static void anle_str_iter_start(", "void anle_str_iter_start(")
        .replace("static const char* anle_str_iter_next(", "const char* anle_str_iter_next(")
}

pub fn lower_to_c(
    p: &Program,
    file: &str,
    src_text: &str,
    debug: bool,
) -> Result<String, CodegenError> {
    DEBUG_LINES.with(|c| c.set(debug));
    SRC_FILE.with(|c| *c.borrow_mut() = file.to_string());
    //nt: 各文件源码行
    let mut 源表: BTreeMap<String, Vec<String>> = BTreeMap::new();
    源表.insert(file.to_string(), src_text.lines().map(|s| s.to_string()).collect());
    for f in &p.functions {
        if !f.file.is_empty() && !源表.contains_key(&f.file) {
            if let Ok(t) = std::fs::read_to_string(&f.file) {
                源表.insert(f.file.clone(), t.lines().map(|s| s.to_string()).collect());
            }
        }
    }
    SRC_LINES.with(|c| *c.borrow_mut() = 源表);
    let mut used: BTreeSet<&'static str> = BTreeSet::new();

    // nt: 先过一遍
    for f in &p.functions {
        for b in &f.blocks {
            for s in &b.stmts {
                match s {
                    Stmt::Assign { rhs: Rvalue::BinOp { op, .. }, .. } => {
                        if let Some(n) = guarded(*op) {
                            used.insert(n);
                        }
                    }
                    Stmt::Assign { rhs: Rvalue::Call { name, .. }, .. } => mark_runtime(name, &mut used),
                    Stmt::Call { name, .. } => mark_runtime(name, &mut used),
                    _ => {}
                }
            }
        }
    }

    // nt: 积类型表
    let mut types: TypeMap = BTreeMap::new();
    for (i, st) in p.types.iter().enumerate() {
        types.insert(st.name.clone(), (format!("anle_st_{}", i), st.fields.clone()));
    }

    let mut out = String::new();
    out.push_str("#include <stdio.h>\n#include <stdlib.h>\n#include <string.h>\n\n");
    //nt: 护栏开关宏
    out.push_str("#ifdef ANLE_NOGUARD\n#define ANLE_GUARD(x)\n#else\n#define ANLE_GUARD(x) x\n#endif\n\n");
    // nt: 真假值按源码字面量的
    out.push_str("long long anle_line = 0; long long anle_col = 0; long long anle_len = 0; const char* anle_cur_file = 0; const char* anle_src = 0;\nvoid anle_at(void) { fprintf(stderr, \"  （第 %lld 行）\\n\", anle_line); }\nconst char* anle_bool_str(long long b) { return b ? \"re\" : \"fa\"; }\n\n");
    // nt: 源码位置
    {
        let esc = |x: &str| x.replace('\\', "\\\\").replace('"', "\\\"");
        out.push_str(&format!("static const char* ANLE_FILE = \"{}\";\n", esc(file)));
        out.push_str("static const char* ANLE_SRC[] = {\n");
        for ln in src_text.lines() {
            out.push_str(&format!("  \"{}\",\n", esc(ln)));
        }
        out.push_str("  0\n};\n");
        out.push_str(ANLE_DIE_RT);
    }
    out.push_str(&support_section(&used));
    // nt: 内存管理基础设施
    out.push_str(ANLE_ARENA_RT);
    out.push_str(&ffi_str_section(&used));

    // nt: list 运行时
    if uses_list(p) || uses_map(p) {
        out.push_str(ANLE_LIST_RT);
    }
    // nt: 运行子程序
    if used.contains("anle_run") {
        out.push_str(ANLE_RUN_RT);
    }
    // nt: str 运行时
    if uses_str(p) {
        out.push_str(ANLE_STR_RT);
    }
    // nt: map 运行时
    if uses_map(p) {
        out.push_str(ANLE_MAP_RT);
    }
    // nt: 文件 IO 运行时
    if uses_file(p) {
        out.push_str(ANLE_FILE_RT);
    }
    // nt: 命令行参数
    if uses_args(p) {
        out.push_str(ANLE_ARGS_RT);
    }

    // nt: str 的「一层复印
    out.push_str("const char* anle_str_copy(const char* s) {\n  if (!s) return 0;\n  size_t n = strlen(s) + 1;\n  char* p = (char*)anle_region_alloc(anle_region_cur, (long long)n);\n  if (!p) { anle_die_nc(6, \"内存不够——程序申请的空间超出可用内存\", \"缩小数据量，或换更省内存的写法\"); }\n  memcpy(p, s, n);\n  return p;\n}\n\n");

    // nt: 积类型 ⇒ C ty
    for st in &p.types {
        let (c, _) = &types[&st.name];
        let fs: Vec<String> = if !st.variants.is_empty() {
            // nt: 和类型
            let maxn = st.variants.iter().map(|v| v.fields.len()).max().unwrap_or(0);
            let mut v = vec!["  long long tag;".to_string()];
            for i in 0..maxn {
                v.push(format!("  long long p{};", i));
            }
            v
        } else {
            st.fields
                .iter()
                .enumerate()
                .map(|(i, (_, t))| format!("  {} f{};", c_ty_in(t, &types), i))
                .collect()
        };
        out.push_str(&format!(
            "struct {c};\ntypedef struct {c} {c};\nstruct {c} {{\n{}\n}};\n",
            fs.join("\n"),
            c = c
        ));
    }
    out.push('\n');

// nt: 「一层复印」助手
    for (i, st) in p.types.iter().enumerate() {
        let (c, fields) = &types[&st.name];
        let mut body = String::new();
        body.push_str(&format!("  {} r;\n", c));
        if !st.variants.is_empty() {
            // nt: 和类型
            body.push_str("  r = *s;\n");
            body.push_str("  return r;\n");
            out.push_str(&format!(
                "static {} anle_dc_{}(const {}* s) {{\n{}}}\n\n",
                c, i, c, body
            ));
            continue;
        }
        for (k, (_, t)) in fields.iter().enumerate() {
            let fty = t.0.as_str();
            let base = fty.trim_end_matches('*');
            if base == "str" {
                body.push_str(&format!("  r.f{} = s->f{} ? anle_str_copy(s->f{}) : 0;\n", k, k, k));
            } else if fty.ends_with('*') {
                body.push_str(&format!("  r.f{} = s->f{};\n", k, k)); // 把手：只抄指针（共享）
            } else if let Some((cc, _)) = types.get(base) {
                let idx = cc.trim_start_matches("anle_st_");
                body.push_str(&format!("  r.f{} = anle_dc_{}(&s->f{});\n", k, idx, k));
            } else {
                body.push_str(&format!("  r.f{} = s->f{};\n", k, k));
            }
        }
        body.push_str("  return r;\n");
        out.push_str(&format!(
            "static {} anle_dc_{}(const {}* s) {{\n{}}}\n\n",
            c, i, c, body
        ));
    }

    // nt: 聚合取元素助手
    if uses_list(p) || uses_map(p) {
        for (i, st) in p.types.iter().enumerate() {
        let (c, _) = &types[&st.name];
        out.push_str(&format!(
            "static {c} anle_at_st_{i}(AnleList* l, long long i) {{\n  {c} r;\n  anle_list_copy_at(l, i, (char*)&r);\n  return r;\n}}\n\n",
            c = c,
            i = i
        ));
        }
    }

    // nt: 结构体深拷助手
    let mut 深拷表: Vec<(usize, String, String)> = Vec::new();
    for (i, st) in p.types.iter().enumerate() {
        if !st.variants.is_empty() {
            continue;
        }
        let (c, 字段表) = &types[&st.name];
        let mut 体 = String::new();
        for (fi, (_, ty)) in 字段表.iter().enumerate() {
            let t = ty.0.trim().to_string();
            let 行 = if t == "str" {
                format!("  d.f{} = anle_str_copy(s->f{});\n", fi, fi)
            } else if let Some(e) = ["list(", "que(", "stack("].iter().find_map(|q| {
                t.strip_prefix(q).and_then(|x| x.strip_suffix(')')).map(|x| x.trim().to_string())
            }) {
                let 旗 = if e == "str" { 1 } else { 0 };
                format!("  d.f{} = anle_dc_list(s->f{}, {});\n", fi, fi, 旗)
            } else if t.starts_with("map(") {
                let v = t
                    .trim_start_matches("map(")
                    .trim_end_matches(')')
                    .rsplit(',')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let 旗 = if v == "str" { 1 } else { 0 };
                format!("  d.f{} = anle_dc_map(s->f{}, {});\n", fi, fi, 旗)
            } else if let Some((c2, _)) = types.get(&t) {
                let 助 = c2.replace("anle_st_", "anle_dcdeep_st_");
                format!("  d.f{} = {}(&s->f{});\n", fi, 助, fi)
            } else {
                format!("  d.f{} = s->f{};\n", fi, fi)
            };
            体.push_str(&行);
        }
        深拷表.push((i, c.clone(), 体));
    }
    if !深拷表.is_empty() {
        for (i, c, _) in 深拷表.iter() {
            out.push_str(&format!("static {c} anle_dcdeep_st_{i}(const {c}* s);\n"));
        }
        out.push('\n');
        for (i, c, 体) in 深拷表.iter() {
            out.push_str(&format!(
                "static {c} anle_dcdeep_st_{i}(const {c}* s) {{\n  {c} d;\n{体}  return d;\n}}\n\n"
            ));
        }
    }

    // nt: 前向声明
    for (fi, f) in p.functions.iter().enumerate() {
        if f.name == "main" {
            continue;
        }
        let params: Vec<String> = f
            .locals
            .iter()
            .enumerate()
            .filter(|(_, l)| l.is_param)
            .map(|(i, l)| format!("{} {}", c_ty_in(&l.ty, &types), vname(i)))
            .collect();
        let ret = match &f.ret {
            Some(t) => c_ty_in(t, &types),
            None => "void".to_string(),
        };
        out.push_str(&format!(
            "{} f{}({});\n",
            ret,
            fi,
            if params.is_empty() { "void".to_string() } else { params.join(", ") }
        ));
    }
    out.push('\n');

    // nt: 外部函数声明
    for e in &p.externs {
        let ps: Vec<String> = e
            .params
            .iter()
            .enumerate()
            .map(|(i, t)| format!("{} a{}", c_ty_in(t, &types), i))
            .collect();
        out.push_str(&format!(
            "extern {} {}({});\n",
            c_ty_in(&e.ret, &types),
            e.name,
            if ps.is_empty() { "void".to_string() } else { ps.join(", ") }
        ));
    }
    out.push('\n');

    let has_main = p.functions.iter().any(|f| f.name == "main");

    // nt: 源函数名 → C 函
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    for (fi, f) in p.functions.iter().enumerate() {
        let c = if f.name == "main" { "main".to_string() } else { format!("f{}", fi) };
        names.insert(f.name.clone(), c);
    }
    // nt: 外部函数
    for e in &p.externs {
        names.insert(e.name.clone(), e.name.clone());
    }

    // nt: 地皮树接线
    let mut called_early: BTreeSet<String> = BTreeSet::new();
    for f in &p.functions {
        for b in &f.blocks {
            for st in &b.stmts {
                match st {
                    Stmt::Call { name, .. } => { called_early.insert(name.clone()); }
                    Stmt::Assign { rhs: Rvalue::Call { name, .. }, .. } => { called_early.insert(name.clone()); }
                    _ => {}
                }
            }
        }
    }

    //nt: 跨块槽收集为全局
    //nt: 同键共用一份存储
    let mut slot_keys: std::collections::BTreeMap<String, Ty> = std::collections::BTreeMap::new();
    for f in &p.functions {
        for l in &f.locals {
            if let Some(n) = &l.name {
                if n.contains('#') {
                    slot_keys.entry(n.clone()).or_insert_with(|| l.ty.clone());
                }
            }
        }
    }
    let slot_idx: std::collections::BTreeMap<String, usize> = slot_keys
        .keys()
        .enumerate()
        .map(|(i, k)| (k.clone(), i))
        .collect();
    if !slot_keys.is_empty() {
        out.push_str("/*nt: 跨块槽存储 定义块一份 */\n");
        for (k, ty) in &slot_keys {
            let i = slot_idx[k];
            out.push_str(&format!(
                "static {} {} = 0; /*nt: {} */\n",
                c_ty_in(ty, &types),
                slot_global_name(k, i),
                k.trim_start_matches('|')
            ));
        }
    }

    for (fi, f) in p.functions.iter().enumerate() {
        out.push_str(&format!("/*@FN_BEGIN:{}@*/\n", fi));
        let is_main = f.name == "main";
        //nt: 槽局部映射到全局
        //nt: 先于签名，防串函数
        ESCAPED.with(|s| s.borrow_mut().clear());
        SLOT_MAP.with(|m| {
            let mut m = m.borrow_mut();
            m.clear();
            for (i, l) in f.locals.iter().enumerate() {
                if l.is_param {
                    continue;
                }
                if let Some(n) = &l.name {
                    if let Some(idx) = slot_idx.get(n) {
                        m.insert(i, slot_global_name(n, *idx));
                    }
                }
            }
        });
        // nt: 函数名同样 mang
        let c_name = if is_main {
            "main".to_string()
        } else {
            format!("f{}", fi)
        };
        // nt: naked
        if f.is_naked {
            out.push_str(&format!(
                "__attribute__((naked)) void {}(void) {{  /* {} */\n",
                c_name, f.name
            ));
            out.push_str("  __asm__ volatile (\n");
            let n = f.asm_lines.len();
            for (i, line) in f.asm_lines.iter().enumerate() {
                let sep = if i + 1 < n { "\\n\\t" } else { "" };
                out.push_str(&format!("    \"{}{}\"\n", c_escape(line), sep));
            }
            out.push_str("  );\n}\n\n");
            out.push_str("/*@FN_END@*/\n");
            continue;
        }
        // nt: 无返回类型
        let (ret_ty, is_void) = if is_main {
            ("int".to_string(), false)
        } else {
            match &f.ret {
                Some(t) => (c_ty_in(t, &types).to_string(), false),
                None => ("void".to_string(), true),
            }
        };
        let params: Vec<String> = f
            .locals
            .iter()
            .enumerate()
            .filter(|(_, l)| l.is_param)
            .map(|(i, l)| format!("{} {}", c_ty_in(&l.ty, &types), vname(i)))
            .collect();
        out.push_str(&format!(
            "{} {}({}) {{  /* {} */\n",
            ret_ty,
            c_name,
            if params.is_empty() { "void".to_string() } else { params.join(", ") },
            f.name
        ));
        // nt: 块入口
        // nt: 段上记的块名 = 源
        let blk_src_name = f.name.rsplit('|').next().unwrap_or(&f.name);
        //nt: 当前源文件
        let 文件名 = f.file.rsplit(['/', '\\']).next().unwrap_or("");
        if 文件名.is_empty() {
            out.push_str("  anle_cur_file = 0;\n");
        } else {
            out.push_str(&format!("  anle_cur_file = \"{}\";\n", c_escape(文件名)));
        }
        out.push_str(&format!("  AnleRegion* __r = anle_region_open(anle_region_cur, \"{}\"); anle_region_cur = __r;\n", c_escape(blk_src_name)));
        // nt: 每圈头一标记
        mark_loop_heads(&f.blocks);
        let heads: Vec<usize> = LOOP_HEADS.with(|h| h.borrow().iter().cloned().collect());
        for h in &heads {
            out.push_str(&format!("  AnleRegion* __m{} = NULL;\n", h));
        }
        // nt: 局部声明
        for (i, l) in f.locals.iter().enumerate() {
            if !l.is_param && !is_slot_local(l) {
                out.push_str(&format!("  {} {};\n", c_ty_in(&l.ty, &types), vname(i)));
            }
        }
        ROOT_FNS.with(|r| {
            let mut m = r.borrow_mut();
            m.clear();
            for g in &p.functions {
                if g.is_block && !g.is_sle && !called_early.contains(&g.name) {
                    m.insert(g.name.clone());
                }
            }
        });
        for b in &f.blocks {
            block_c(f, b, &mut out, &mut used, is_void, &names, &types)?;
        }
        out.push_str("}\n\n");
        out.push_str("/*@FN_END@*/\n");
    }

    // nt: 入口
    if !has_main && !p.functions.is_empty() {
// nt: 链式执行
        // nt: 其余块 = 链根，按
        // nt: 「绝不静默不跑」。
        let mut called: BTreeSet<String> = BTreeSet::new();
        for f in &p.functions {
            for b in &f.blocks {
                for st in &b.stmts {
                    match st {
                        // nt: 无返回值调用语句
                        Stmt::Call { name, .. } => {
                            called.insert(name.clone());
                        }
                        // nt: 带返回值的调用
                        Stmt::Assign { rhs: Rvalue::Call { name, .. }, .. } => {
                            called.insert(name.clone());
                        }
                        _ => {}
                    }
                }
            }
        }
        let roots: Vec<usize> = p
            .functions
            .iter()
            .enumerate()
            .filter(|(_, f)| f.is_block && !f.is_sle && !called.contains(&f.name))
            .map(|(i, _)| i)
            .collect();
        // nt: 调用图
        // nt: 索引建两份
        // nt: 旧口径与主模块块使用
        let mut block_idx: BTreeMap<&str, usize> = BTreeMap::new();
        for (i, f) in p.functions.iter().enumerate() {
            if f.is_block {
                block_idx.insert(f.name.as_str(), i);
            }
        }
        // nt: 裸名备用
        let mut by_short: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for (i, f) in p.functions.iter().enumerate() {
            if f.is_block {
                let short = f.name.rsplit('|').next().unwrap_or(&f.name);
                by_short.entry(short).or_default().push(i);
            }
        }
        for (short, v) in by_short {
            if v.len() == 1 {
                block_idx.entry(short).or_insert(v[0]);
            }
        }
        let mut edges: Vec<Vec<usize>> = vec![Vec::new(); p.functions.len()];
        for (i, f) in p.functions.iter().enumerate() {
            if !f.is_block {
                continue;
            }
            for b in &f.blocks {
                for st in &b.stmts {
                    let callee = match st {
                        Stmt::Call { name, .. } => Some(name.as_str()),
                        Stmt::Assign { rhs: Rvalue::Call { name, .. }, .. } => Some(name.as_str()),
                        _ => None,
                    };
                    if let Some(j) = callee.and_then(|n| block_idx.get(n)) {
                        edges[i].push(*j);
                    }
                }
            }
        }
        let mut reached = vec![false; p.functions.len()];
        let mut queue: VecDeque<usize> = VecDeque::new();
        for &r in &roots {
            reached[r] = true;
            queue.push_back(r);
        }
        while let Some(i) = queue.pop_front() {
            for &j in &edges[i] {
                if !reached[j] {
                    reached[j] = true;
                    queue.push_back(j);
                }
            }
        }
        // nt: 死块 = 块函数、非
        let dead: Vec<&str> = p
            .functions
            .iter()
            .enumerate()
            .filter(|(i, f)| f.is_block && !f.is_sle && !reached[*i])
            .map(|(_, f)| f.name.as_str())
            .collect();
        if !dead.is_empty() {
            return Err(CodegenError {
                message: format!(
                    "块「{}」永远执行不到——它们只被互相调用的块引用，成了没有起点的环。\
                     每条链都要有一个没被引用的起点（链根）",
                    dead.join("」「")
                ),
                code: "E401".into(),
                line: 0,
            });
        }
        // nt: 每个链根
        let mut prologue = String::new();
        let mut calls = String::new();
        let mut k = 0usize;
        out.push_str("/*@FN_BEGIN:999999@*/\n");
        for i in &roots {
            let f = &p.functions[*i];
            let mut args: Vec<String> = Vec::new();
            for l in f.locals.iter().filter(|l| l.is_param) {
                if l.ty.0.ends_with('*') {
                    let inner = l.ty.0.trim_end_matches('*');
                    prologue.push_str(&format!(
                        "  {} __slot{} = {};\n",
                        c_ty_in(&Ty(inner.to_string()), &types),
                        k,
                        if inner == "str" { "\"\"" } else { "0" }
                    ));
                    args.push(format!("&__slot{}", k));
                    k += 1;
                }
            }
            calls.push_str(&format!("  f{}({});\n", i, args.join(", ")));
        }
        // nt: 用到 args 时才
        if uses_args(p) {
            out.push_str(&format!(
                "int main(int argc, char** argv) {{\n  g_argc = argc;\n  g_argv = argv;\n{}{}  return 0;\n}}\n",
                prologue, calls
            ));
        } else {
            out.push_str(&format!(
                "int main(void) {{\n{}{}  return 0;\n}}\n",
                prologue, calls
            ));
        }
    }
    out.push_str("/*@FN_END@*/\n");
    Ok(out)
}