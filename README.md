# Emela

Emela is an experimental functional language intended to compile to native code and WebAssembly.
This repository contains the early Emela CLI and compiler implementation for the minimal core language.

The current CLI supports:

- top-level `fn` definitions
- `main` and `main!` executable entry points
- block expressions and immutable local bindings
- `I32`, `Bool`, and `Unit`
- required type annotations on function parameters, function returns, and local bindings
- single-field `struct` declarations and field access
- `enum` declarations with zero or one payload value per variant
- `Result`-style enums with `match` over variant patterns
- function calls
- generic function declarations and inferred generic calls
- function type annotations and function values for type-checking
- forward pipeline calls with `|>`
- primitive method calls such as `x.add(y)`
- operators backed by primitive trait-style methods: `+`, `-`, `*`, `==`, `<`
- `match` expressions over integer, boolean, unit, and wildcard patterns
- effect markers with `!`
- top-level `import` declarations for compiler-known external functions
- platform capability declarations with `#[requires(...)]`
- platform capability checking from the selected backend
- native assembly generation for `aarch64-apple-darwin` and `x86_64-unknown-linux-gnu`
- JavaScript generation for the current core subset
- external process backend plugins using versioned JSON IR
- library checking mode for compilation units without `main` / `main!`

The language specification lives in the separate `emela-lang/specification` repository.

## Install

Dogfooding builds are published from `main` as timestamped prereleases.
They are intended for quickly trying the current compiler state, not for stable production use.

Install the latest dogfooding build:

```sh
curl -fsSL https://raw.githubusercontent.com/emela-lang/emela/main/install.sh | sh
```

By default this installs `emela` into `$HOME/.emela/bin`.
Set `EMELA_INSTALL_DIR` to choose another directory.

Install a specific timestamp release:

```sh
curl -fsSL https://raw.githubusercontent.com/emela-lang/emela/main/install.sh \
  | EMELA_VERSION=v0.0.20260628120000 sh
```

Check the installed version:

```sh
emela --version
emela check --backend js-node examples/minimal.emel
```

## Requirements

Development requires:

- Rust toolchain with Cargo, edition 2021 compatible; currently tested with `rustc 1.84.1`
- `rustfmt`, normally installed with the Rust toolchain
- Apple arm64 macOS or x86_64 Linux for native executable builds
- A system C compiler available as `cc` for assembling and linking generated native assembly when building executables

The native backend can emit assembly with a native backend profile and `--artifact PATH` without invoking `cc`.
Building an executable invokes the host `cc`, so native executable builds require a matching host for the selected target.

The compiler uses `serde` and `serde_json` for backend manifests and plugin IR.

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

`--backend PROFILE|PATH` selects a backend profile. Built-in profiles include
`native-aarch64-apple-darwin`, `native-x86_64-unknown-linux-gnu`, `js-node`, and
`js-bun`. `PATH` points to an external backend manifest. `--backend` is required
for `emela build` and optional for `emela check`; short aliases such as `native`
and `js` are not supported.

Built-in backend descriptors live under `backends/`.
They document the same platform extern and capability surface used by the
in-process implementations. Each descriptor is a backend profile that combines
a backend kind with a runtime or target, such as `js-node`, `js-bun`, or
`native-aarch64-apple-darwin`.

External backend manifests are JSON:

```json
{
  "name": "example-backend",
  "backend": "js",
  "abi_version": 1,
  "command": ["example-emela-backend"],
  "runtime": "node",
  "capabilities": ["Stdout", "Stdin"],
  "externs": [
    {
      "path": ["platform", "io"],
      "name": "_write_stdout_utf8!",
      "params": ["String"],
      "return": "Result<Unit, PlatformError>",
      "effectful": true,
      "capabilities": ["Stdout"],
      "bindings": {
        "js": {
          "symbol": "__emela_write_stdout_utf8"
        }
      }
    }
  ]
}
```

The compiler sends a versioned JSON request to the backend process on stdin. The
request contains the checked program IR, typed function signatures, target,
runtime, compilation mode, and resolved imports. Profiles that do not define a
target send `null` for target. The backend returns JSON on stdout:

```json
{
  "artifact": "backend output"
}
```

or diagnostics:

```json
{
  "diagnostics": ["message"]
}
```

`--package DIR` on `emela check` or `emela build` adds a source package root. `DIR` must contain
`emela-package.json`:

```json
{
  "name": "math",
  "source": "src"
}
```

`import math.ops.add_one` loads `DIR/src/ops.emel` and imports `add_one`.

Package `std` is special only because the compiler has a bundled fallback. If a
package named `std` is supplied with `--package ../stdlib`, that package is used
instead of the bundled stdlib. Imports such as `import std.io.write_stdout_utf8!`
load Emela source from the selected `std` package or the bundled stdlib. stdlib
wrappers then call `platform.*` imports supplied by the selected backend.
Only the requested stdlib API and its dependencies are expanded, so a backend
does not need to implement unused stdlib platform imports.

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
cargo run --bin emela -- check --backend js-node examples/maximal.emel
```

Check with an external source package:

```sh
cargo run --bin emela -- check --backend js-node --package ../stdlib examples/std-print.emel
```

Check a library source file without requiring `main` / `main!`:

```sh
cargo run --bin emela -- check --backend js-node --library ../stdlib/std/io.emel
```

Check against a native backend profile:

```sh
cargo run --bin emela -- check --backend native-aarch64-apple-darwin examples/maximal.emel
```

Emit native assembly:

```sh
cargo run --bin emela -- build --backend native-aarch64-apple-darwin --artifact /tmp/emela-maximal.s examples/maximal.emel
```

Build a native executable on a matching host:

```sh
cargo run --bin emela -- build --backend native-aarch64-apple-darwin --output /tmp/emela-maximal examples/maximal.emel
```

Emit x86_64 Linux assembly from any supported development host:

```sh
cargo run --bin emela -- build --backend native-x86_64-unknown-linux-gnu --artifact /tmp/emela-maximal-x86_64.s examples/maximal.emel
```

Emit JavaScript:

```sh
cargo run --bin emela -- build --backend js-node --artifact /tmp/emela.js examples/maximal.emel
```

Use the stdlib from user code:

```emela
import std.io.write_stdout_utf8!

fn main!() -> Result<Unit, PlatformError> {
  "hello\n" |> write_stdout_utf8!()
}
```

```sh
cargo run --bin emela -- build --backend js-node --artifact /tmp/emela.js examples/std-print.emel
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
fn main() -> Unit {
}
```

Integer computation:

```emela
fn add(x: I32, y: I32) -> I32 {
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
- Function values are type-checked, but native lowering is not implemented yet.
- Runtime implementations for real I/O capabilities are not connected yet.
- Imported external functions are type-checked and capability-checked against the selected backend.
- JavaScript external lowering requires a `bindings.js.symbol` entry for each imported external function.
- Library mode can check stdlib source files, and user programs can import `std.*` modules from the bundled stdlib or an explicit `std` package.
- User-defined traits, trait declarations, and impl declarations are not implemented.
- Effect handlers and error values are not implemented.
- Structs and enums are currently limited to the first draft subset: one field per struct, at most one payload per variant, and no generics.
