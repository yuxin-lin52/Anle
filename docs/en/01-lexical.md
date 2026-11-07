# 01 Lexical structure

Status: implemented.

## Keywords

`add call fn val type fix give take outs if else for while in max match case unsafe pub sle ABI hide kill asm and or not null re fa int float bool str int_8 int_16 int_32 int_64 uint_8 uint_16 uint_32 uint_64 list map que stack`

## Symbols

`= : ; ~ , . ( ) [ ] { } + - * / % < > ? & | ^ ! == != <= >= << >>` and the markers `#:` (block), `##:` (module), `###:` (package).

## Comments

Whole-line `//` only — column 0, indentation allowed, no mid-line comments. The compiler ignores the rest of the line.

## Literals

- Integers: decimal; `0x` hex, `0b` binary, `0o` octal, with `_` digit separators (`0b1111_0000`).
- Floats: decimal point and scientific notation (`1e6`, `1.5e-3`).
- Strings: normal `"…"` with `\n \t \\ \"` escapes — unknown escapes are compile errors; verbatim `@"…"` where backslashes are literal and `""` escapes a quote. Strings cannot span lines.
- Booleans: `re` (true) / `fa` (false).

## Files

UTF-8 without BOM, LF line endings. One `.an` file is one complete program (or one module).
