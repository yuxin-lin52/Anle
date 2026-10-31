# Contributing to Anle

Thanks for your interest. Anle is early: the syntax is not frozen and the language design is still being settled, which shapes what kinds of contributions are useful right now.

中文讨论同样欢迎 — feel free to write in Chinese.

## What is welcome

- Bug reports, with a minimal reproduction.
- Documentation fixes: typos, wrong examples, unclear wording.
- Tooling around the language, such as editor support or build helpers. Open an issue first so the shape can be agreed on.

## What is not accepted

Language and syntax proposals. The syntax is not frozen, so accepting a proposal would commit the project to an interface that may later be removed.

## Before you open a pull request

Open an issue describing the change. Pure fixes — a typo, an example that does not run — can go straight to a pull request. Anything that changes compiler behavior needs agreement first, otherwise the work may be turned down after it is written.

## Building

See the Building section of the README:

```
cd compiler
cargo build --release --no-default-features --features backend
```

Run the demo before and after a change:

```
./target/release/anlec build demos/en/physics-engine/physics-engine
```

## Commit sign-off

Every commit carries a `Signed-off-by` line. `git commit -s` adds it:

```
git commit -s -m "your message"
```

The line states that you wrote the change, or have the right to submit it, and that you license it under Apache-2.0, matching the license the project is distributed under.

## Use of AI assistants

AI assistance is allowed, with disclosure:

- Using an assistant to answer questions, analyze, review, or refine is fine, and needs no disclosure.
- A change whose code was **created** by a model is accepted when it is non-critical, well tested, and you can explain every line. Say so in the pull request.
- Do not post model-generated issue text, review comments, or documentation as your own words.
- Your contributions are your responsibility. Blame cannot be placed on a model.

## Conduct

Be civil. Disagreement about design is expected and welcome; personal attacks are not.
