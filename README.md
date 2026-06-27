# Emela Compiler

Emela is an experimental functional language intended to compile to native code and WebAssembly.
This repository contains the early compiler implementation for the minimal core language.

The current compiler supports:

- top-level `fn` definitions
- `main` and `main!` executable entry points
- block expressions and immutable local bindings
- `I32`, `Bool`, and `Unit`
- function calls
- primitive method calls such as `x.add(y)`
- operators backed by primitive trait-style methods: `+`, `-`, `*`, `==`, `<`
- `match` expressions over integer, boolean, unit, and wildcard patterns
- effect markers with `!`
- platform capability declarations with `#[requires(...)]`
- target capability checking
- native assembly generation for `aarch64-apple-darwin` and `x86_64-unknown-linux-gnu`

The language specification lives in the separate `emela-lang/specification` repository.

## Requirements

Development requires:

- Rust toolchain with Cargo, edition 2021 compatible; currently tested with `rustc 1.84.1`
- `rustfmt`, normally installed with the Rust toolchain
- Apple arm64 macOS or x86_64 Linux for native executable builds
- A system C compiler available as `cc` for assembling and linking generated native assembly when building executables

The native backend can emit assembly with `--emit-asm` without invoking `cc`.
Building an executable invokes the host `cc`, so native executable builds require a matching host for the selected target.

The compiler currently has no third-party Rust crate dependencies.

## Supported Targets

The compiler recognizes these target triples:

| Target | Capability checking | Code generation |
| --- | --- | --- |
| `aarch64-apple-darwin` | Yes | Native arm64 assembly |
| `x86_64-unknown-linux-gnu` | Yes | Native x86_64 System V assembly |
| `wasm32-unknown-unknown` | Yes | Not implemented |
| `wasm32-wasi` | Yes | Not implemented |

Target capability sets currently follow SPEC-0003:

- `aarch64-apple-darwin`: `Stdout`, `Stdin`, `Stderr`, `FileRead`, `FileWrite`, `Clock`, `Random`, `Env`, `Process`, `Network`
- `x86_64-unknown-linux-gnu`: `Stdout`, `Stdin`, `Stderr`, `FileRead`, `FileWrite`, `Clock`, `Random`, `Env`, `Process`, `Network`
- `wasm32-unknown-unknown`: no platform capabilities
- `wasm32-wasi`: `Stdout`, `Stdin`, `Stderr`, `FileRead`, `FileWrite`, `Clock`, `Random`, `Env`

If `--target` is omitted, the compiler uses the host target. At the moment, automatic host target detection accepts Apple arm64 macOS and x86_64 Linux.

## Common Commands

Format the code:

```sh
cargo fmt
```

Type-check and run tests:

```sh
cargo check
cargo test
```

Check an Emela source file without building:

```sh
cargo run -- --check examples/maximal.emel
```

Check against a specific platform target:

```sh
cargo run -- --target wasm32-wasi --check examples/maximal.emel
```

Emit native assembly:

```sh
cargo run -- --emit-asm /tmp/emela-maximal.s --check examples/maximal.emel
```

Build a native executable on a matching host:

```sh
cargo run -- --target aarch64-apple-darwin examples/maximal.emel -o /tmp/emela-maximal
```

Emit x86_64 Linux assembly from any supported development host:

```sh
cargo run -- --target x86_64-unknown-linux-gnu --emit-asm /tmp/emela-maximal-x86_64.s --check examples/maximal.emel
```

Run it and inspect the process exit code:

```sh
/tmp/emela-maximal
echo $?
```

`examples/add.emel` and `examples/maximal.emel` currently exit with code `42`.

## Examples

Minimal program:

```emela
fn main() {
}
```

Integer computation:

```emela
fn add(x, y) -> I32 {
  x + y
}

fn main() -> I32 {
  add(20, 22)
}
```

Effectful entry point with a platform capability:

```emela
#[requires(Stdout)]
fn tick!() -> Unit {
  ()
}

fn main!() -> I32 {
  tick!()
  42
}
```

## Current Limitations

- Native executable building uses the host `cc`; cross-target native builds are not implemented.
- WebAssembly targets are capability-checked only; WASM code generation is not implemented.
- The native backend supports the current core language subset only.
- Runtime implementations for real I/O capabilities are not connected yet.
- User-defined traits, trait declarations, and impl declarations are not implemented.
- Effect handlers and error values are not implemented.
