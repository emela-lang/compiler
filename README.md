# Emela

Emela is an experimental functional language intended to compile to native code
and WebAssembly. This repository contains the early Emela CLI and compiler for
the minimal core language. The current build type-checks the core language and
lowers it to a typed IR, which pluggable backends turn into **WebAssembly**
(Tier 1) or **JavaScript** (Tier 2).

The language specification lives in the separate `emela-lang/specification`
repository. This README documents what the compiler in this repository actually
implements today, which is a small subset of the full language.

## Workspace layout

The compiler is a Cargo workspace. The IR-to-target boundary is published as a
small core crate so backends can be added without depending on the whole
compiler:

| Crate | Role |
| --- | --- |
| `emela-codegen` | Public core API: the IR, the type-system types it uses, the `Backend` trait, `Tier`, `Artifact`, the registry, and the external-plugin protocol. |
| `emela-backend-wasm` | WebAssembly (WASI / WAMR) backend — Tier 1. |
| `emela-backend-js` | JavaScript backend — Tier 2. |
| `emela` | Frontend (lexer, parser, type checker, imports, lowering) and the CLI. |

A backend is anything implementing `emela_codegen::Backend`. It can be in-process
(a Rust crate depending only on `emela-codegen`) or an external process driven by
the JSON IR protocol (see [Backends](#backends)).

## What the compiler supports

- top-level `fn` definitions
- a `main` entry point (no parameters)
- block expressions and immutable `let` bindings, with optional type annotations
- primitive types `Unit`, `Bool`, `Int`, `Float`, and `String`
- `Array<T>` literals, including nested arrays
- function types such as `(Int) -> Int` and `(Int, Int) -> Int uses { ... }`
- first-class functions: function values, `fn` lambda expressions, closures, and
  higher-order functions
- generic functions with type parameters such as `fn identity<T>(x: T) -> T`;
  type arguments are inferred from the call and each instantiation is
  monomorphized to a concrete specialization (spec 0014)
- numeric arithmetic `+`, `-`, `*`, `/` on matching `Int` or `Float` operands,
  and `%` on `Int` (integer division truncates toward zero; division by zero traps)
- comparisons `==` and `<` on matching numeric operands, producing `Bool`
- `if cond { ... } else { ... }` as an expression (the `else` branch is required)
- a `Char` type with `'x'` literals, `String` concatenation `++`, and the pure
  conversions `Char.from_code(Int)` / `String.from_char(Char)`
- `extern fn` platform functions whose side effects are resolved by the
  selected backend's runtime (see [Standard library and platform functions](#standard-library-and-platform-functions))
- effect rows declared with `uses { ... }`, checked so a body's effects are a
  subset of the function's declared effects
- `module`, `pub`, and `import` for splitting code across files and source
  packages
- line comments starting with `--`
- `enum` declarations and exhaustive `match` expressions, with pattern guards
- error handling with the `throws E` clause, `throw`, the `?` propagation
  operator, `try` / `catch`, and `panic`; `Option<T>` represents an absent value.
  Recoverable failure uses the throws channel — there is no built-in `Result`
- WebAssembly and JavaScript code generation, plus a textual IR dump
- in-process and external-process pluggable backends

A user-declared `enum` name is a usable type, with variant constructors such as
`Color.Red` and exhaustive `match`. The type names `Record` and `Function` are
accepted in signatures, but have no literal or constructor syntax yet, so they
cannot appear in runnable code.

## Not yet implemented

To set expectations, the following are **not** part of this build:

- no `struct`, `trait`, or `impl` declarations
- generics are limited to functions: no explicit type arguments (they are always
  inferred), no generic data-type declarations, no generic function values, and
  no effect-row or error-row polymorphism (spec 0014)
- no boolean operators (`&&` / `||`); branch with `if` or `match`
- WebAssembly `String.from_char` currently encodes ASCII (1-byte) only
- no native (machine-code) backend
- platform functions are still the minimal `Unit`-returning set and do not yet
  declare `throws`
- no dead-code elimination: importing a module pulls in all of its functions, so
  a backend must provide every platform function any imported wrapper references
- no project manifest or dependency fetching

## Requirements

- Rust toolchain with Cargo, edition 2024 (Rust 1.85 or newer; tested with `rustc 1.96.0`)
- `rustfmt`, normally installed with the Rust toolchain
- Node.js to run generated JavaScript
- A WASI runtime such as WAMR (`iwasm`) or `wasmtime` to run generated wasm

The compiler assembles WAT to wasm with the pure-Rust `wat` crate and validates
it with `wasmparser`, so no external wasm tools are needed to *build*; a runtime
is only needed to *run* the output.

## Build and test

```sh
cargo build
cargo fmt
cargo test
```

Run the compiler through Cargo with `cargo run --bin emela -- <args>`, or use the
installed `emela` binary directly.

## CLI usage

```text
emela check [--backend NAME] [--package DIR] FILE
emela build [--backend NAME|PATH] [--emit default|text] [--package DIR] [-o FILE] FILE
emela ir            [--package DIR] [-o FILE] FILE
emela backends
emela --version
```

- `check` type-checks a program without producing output.
- `build` lowers to IR and runs the selected backend. Without `-o`/`--output` it
  prints text artifacts to stdout; a binary artifact (wasm) requires `-o`.
- `ir` prints the lowered intermediate representation as text.
- `backends` lists the built-in backends and their tiers.
- `--backend NAME` selects a built-in backend (default `js-node`). `NAME` may
  also be a path to a `backend.json` descriptor that declares an external
  `command` (see [Backends](#backends)).
- `--emit text` asks a backend for a textual artifact when it has one (WAT for
  the wasm backend); the default is the binary/source artifact.
- `--package DIR` adds a source package root (see [Packages](#packages)).

Build and run an example as JavaScript (Tier 2):

```sh
cargo run --bin emela -- build --backend js-node examples/add.emel | node
# prints 42
```

Build and run an example as WebAssembly (Tier 1):

```sh
cargo run --bin emela -- build --backend wasm-wasi -o /tmp/add.wasm examples/add.emel
wasmtime /tmp/add.wasm; echo $?    # exit code 42  (iwasm works too)
```

`main`'s `Int` result becomes the process exit code; any other result type exits
`0`. Inspect the generated WAT with `--emit text`, or the IR with `emela ir`.

## Examples

All files under `examples/` type-check and build with this compiler. Each
standalone example below is run with:

```sh
cargo run --bin emela -- build --backend js-node examples/<file>.emel | node
```

| File | Demonstrates | Output |
| --- | --- | --- |
| `minimal.emel` | the smallest valid program | _(none; returns `Unit`)_ |
| `add.emel` | functions, typed parameters, calls | `42` |
| `string.emel` | `String` values and `let` bindings | `Hello, Emela!` |
| `function_values.emel` | function values, higher-order functions, closures | `63` |
| `generics.emel` | generic functions, inferred type arguments, monomorphization | `42` |
| `effects.emel` | `uses { ... }` effect rows and propagation | _(none; returns `Unit`)_ |
| `maximal.emel` | the largest subset that compiles, combined | `44` |
| `imports/main.emel` | `module` / `pub` / `import` across files | `37` |

`imports/main.emel` imports from the sibling module `imports/geometry.emel`. The
module file has no `main`, so it is consumed via `import` rather than checked on
its own.

`examples/hello.emel` performs real I/O through the bundled `examples/stdlib`
package, so it is built with a package root:

```sh
cargo run --bin emela -- build --backend js-node --package examples/stdlib examples/hello.emel | node
# prints: Hello, Emela!
```

`examples/print_int.emel` converts an integer to text with the pure
`std.int.to_string` (built on `if`, `/`/`%`, and `Char`/`++`) and prints it:

```sh
cargo run --bin emela -- build --backend js-node --package examples/stdlib examples/print_int.emel | node
# prints: 42
```

The same examples build to WebAssembly; the numeric ones produce the same value
as their exit code (`add`→42, `function_values`→63, `maximal`→44,
`imports/main`→37), and the others exit `0`.

## Language tour

Minimal program:

```emela
fn main() -> Unit {
}
```

Functions and calls:

```emela
fn add(x: Int, y: Int) -> Int {
  x + y
}

fn main() -> Int {
  add(20, 22)
}
```

`let` bindings and blocks (blocks are expressions; the last expression is the
value):

```emela
fn main() -> Int {
  let base: Int = 20
  let computed = {
    let stepped = base + 1
    stepped * 2
  }
  computed
}
```

Function values and closures:

```emela
fn apply(f: (Int) -> Int, x: Int) -> Int {
  f(x)
}

fn make_adder(n: Int) -> (Int) -> Int {
  fn (x: Int) -> Int {
    x + n
  }
}

fn main() -> Int {
  let add10 = make_adder(10)
  apply(add10, 32)
}
```

Generic functions (type arguments are inferred from the call):

```emela
fn identity<T>(x: T) -> T {
  x
}

fn apply_to<A, B>(f: (A) -> B, x: A) -> B {
  f(x)
}

fn main() -> Int {
  let start: Int = identity(41)
  apply_to(fn (n: Int) -> Int { n + 1 }, start)
}
```

Effects:

```emela
fn log_line() -> Unit uses { Stdout } {
  ()
}

fn main() -> Unit uses { Stdout } {
  let printed: Unit = log_line()
  ()
}
```

## Backends

`emela backends` lists what is available:

```text
wasm-wasi   Tier 1
js-node     Tier 2
```

Tiers mirror Rust's target tiers and are metadata, not a gate: Tier 1 is built
and run in CI, Tier 2 is built and smoke-tested. Building with a non-Tier-1
backend prints a one-line note. Built-in backends are feature-gated on the
`emela` crate (`backend-wasm` and `backend-js`, both on by default). Name aliases
`wasm`, `js`, and `js-bun` resolve to the canonical names above.

### Adding a backend (in-process)

Depend on `emela-codegen` and implement the trait:

```rust
use emela_codegen::{Artifact, ArtifactKind, Backend, BackendOptions, IrProgram, Result, Tier};

struct MyBackend;
impl Backend for MyBackend {
    fn name(&self) -> &str { "my-backend" }
    fn tier(&self) -> Tier { Tier::Tier3 }
    fn compile(&self, ir: &IrProgram, _opts: &BackendOptions) -> Result<Artifact> {
        Ok(Artifact { kind: ArtifactKind::Other("custom".into()), bytes: render(ir) })
    }
}
```

### Adding a backend (external process)

Point `--backend` at a `backend.json` that declares a `command`:

```json
{ "name": "my-backend", "backend": "custom", "abi_version": 1,
  "command": ["my-emela-backend"], "tier": "Tier3" }
```

The compiler writes a JSON `PluginRequest` (the IR plus `target`/`runtime`/`mode`)
to the process's stdin and reads a `PluginResponse` from stdout:

```json
{ "status": "ok", "kind": "WasmBinary", "bytes": [0, 97, 115, 109] }
```

or, on failure, `{ "status": "error", "diagnostics": ["message"] }`. The request
and response types live in `emela-codegen` so Rust plugins can reuse them, and
the JSON shape is the contract for plugins written in any language.

## Standard library and platform functions

Side effects are not implemented by the compiler. Every capability effect (such
as `io`) originates from a **platform function**, a language-defined operation
declared with `extern fn` and resolved at run time by the selected backend. This
is specified in `emela-lang/specification` spec 0013.

A standard-library module wraps the platform functions; application code never
names a backend, so the same source runs on any backend that provides the
platform functions it uses. `examples/stdlib/src/io.emel`:

```emela
module io

extern fn write_stdout(s: String) -> Unit uses { io }

pub fn print(s: String) -> Unit uses { io } {
  write_stdout(s)
}
```

- `extern fn` has no body; its signature must match an entry of the platform
  interface (or compilation fails). It declares the capability it produces.
- Each backend implements the subset of platform functions it supports — the JS
  backend bundles a default runtime object (`__rt`), the wasm backend emits
  WASI-backed glue. Building a program that uses a platform function the selected
  backend does not provide is a compile error.
- Because effects only enter through platform functions and the compiler emits no
  bodies for them, the runtime is guaranteed to be what resolves every effect.

This repository ships a small `examples/stdlib` for trying things out; the real
standard library is developed in the separate `emela-lang/stdlib` repository and
consumed with `--package path/to/stdlib`.

## Packages

`--package DIR` adds a source package root. `DIR` must contain
`emela-package.json`:

```json
{
  "name": "math",
  "source": "src"
}
```

With that package, `import math.ops.add_one` loads `DIR/src/ops.emel` and imports
the public function `add_one`. The module file must declare a matching
`module ops`, and only `pub` functions can be imported.

Imports that do not name a package are resolved relative to the importing file.
For example, `import geometry.square` loads `geometry.emel` from the same
directory, which must declare `module geometry`.

## Install

Dogfooding builds are published from `main` as timestamped prereleases. They are
intended for quickly trying the current compiler state, not for stable production
use.

Install the latest dogfooding build:

```sh
curl -fsSL https://raw.githubusercontent.com/emela-lang/emela/main/install.sh | sh
```

By default this installs `emela` into `$HOME/.emela/bin`. Set `EMELA_INSTALL_DIR`
to choose another directory, and `EMELA_VERSION` to install a specific release
tag.

Check the installed version:

```sh
emela --version
```
