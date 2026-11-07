# Security policy

## Reporting a vulnerability

Please do not open a public issue. Use GitHub's private vulnerability reporting:

<https://github.com/yuxin-lin52/Anle/security/advisories/new>

Include the version (`anlec --version`), the smallest program that triggers the problem, and what you expected instead.

## In scope

Anle compiles source to native code, so the compiler and its generated runtime are the surface of interest:

- a program that compiles but behaves differently from its source;
- malformed input that makes the compiler emit invalid code or crash;
- memory-safety problems in the generated runtime.

## Out of scope

- Programs rejected with a diagnostic. That is a bug report, not a vulnerability.
- Problems inside LLVM, clang, or the C toolchain. Report those upstream.

## Versions

Only the latest release is supported. The language is not frozen, and older releases are not patched.
