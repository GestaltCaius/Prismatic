# 0010. Small binary, strict lints

Status: accepted

## Decision
The release profile favours size over speed: `opt-level = "z"`, fat LTO, one codegen unit, symbols stripped and
`panic = "abort"`. The work is network and disk bound, and this took the macOS binary from 9.0 MB to 5.5 MB.
A panic ends the program instead of unwinding.

The code builds with warnings as errors, clippy's `pedantic` and `nursery` groups on, `unwrap` denied, `dbg!`,
`todo!` and `unimplemented!` denied, and `unsafe` forbidden. Formatting is checked in CI. Clippy and the tests run
on every platform, since folder handling and cookie reading differ per OS.

## Forbidden
- `unsafe` code.
- `unwrap` outside tests.
- Catching panics to keep a request alive. A bug should stop loudly, not limp on.

## Revisit when
Speed matters (a CPU-bound feature), or a panic in one web request taking the whole server down proves a real problem.

## Cost if wrong
A panic anywhere, including one web request, stops the whole program and the open page stops answering.
