//! WebAssembly backend (Tier 1).
//!
//! Lowers the Emela IR to a WASI module that runs on WAMR (`iwasm`) or any
//! other WASI preview1 runtime. The module is authored as WAT text and then
//! assembled to a binary with the pure-Rust [`wat`] crate, so the compiler
//! needs no external wasm tools.
//!
//! ## Representation
//! - `Int` / `Bool` / `Unit` -> `i32` (`Unit` is the constant `0`)
//! - `Float` -> `f64`
//! - `String` / `Array` / function values -> `i32` pointers into linear memory
//!
//! ## First-class functions
//! Every Emela function (top level and `fn` lambda) is closure-converted to a
//! wasm function taking an environment pointer as its first parameter, and is
//! placed in a function table. A function *value* is a pointer to a closure
//! `[table_index: i32, capture0, capture1, ...]`. A direct call to a known
//! top-level function uses `call`; calling any other function value loads the
//! table index from the closure and uses `call_indirect`.
//!
//! ## Execution
//! `_start` calls `main`. If `main` returns `Int`, that value is the process
//! exit code; otherwise the result is dropped and the exit code is `0`.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use emela_codegen::{
    Artifact, ArtifactKind, Backend, BackendError, BackendOptions, BinaryOp, EmitMode,
    FunctionType, IrArm, IrCapture, IrExpr, IrFunction, IrParam, IrPattern, IrProgram,
    QuestionMode, Result, Tier, Type,
};

/// The WASI/WAMR WebAssembly backend.
pub struct WasmBackend;

impl Backend for WasmBackend {
    fn name(&self) -> &str {
        "wasm-wasi"
    }

    fn tier(&self) -> Tier {
        Tier::Tier1
    }

    fn compile(&self, ir: &IrProgram, options: &BackendOptions) -> Result<Artifact> {
        let wat = emit_module(ir)?;
        match options.mode {
            EmitMode::Text => Ok(Artifact::text(ArtifactKind::WasmText, wat)),
            EmitMode::Default => {
                // Assemble the WAT to a binary, then type-validate the module so
                // a malformed module fails here rather than at run time.
                let bytes = wat::parse_str(&wat).map_err(|err| {
                    BackendError::with(
                        "internal error: generated WAT failed to assemble".to_string(),
                        vec![err.to_string()],
                    )
                })?;
                wasmparser::validate(&bytes).map_err(|err| {
                    BackendError::with(
                        "internal error: generated wasm failed to validate".to_string(),
                        vec![err.to_string()],
                    )
                })?;
                Ok(Artifact {
                    kind: ArtifactKind::WasmBinary,
                    bytes,
                })
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Value representation
// ---------------------------------------------------------------------------

/// The numeric WebAssembly type a value is represented with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum WasmTy {
    I32,
    F64,
}

impl WasmTy {
    fn of(ty: &Type) -> WasmTy {
        match ty {
            Type::Float => WasmTy::F64,
            _ => WasmTy::I32,
        }
    }

    fn keyword(self) -> &'static str {
        match self {
            WasmTy::I32 => "i32",
            WasmTy::F64 => "f64",
        }
    }

    fn size(self) -> u32 {
        match self {
            WasmTy::I32 => 4,
            WasmTy::F64 => 8,
        }
    }

    fn load(self) -> &'static str {
        match self {
            WasmTy::I32 => "i32.load",
            WasmTy::F64 => "f64.load",
        }
    }

    fn store(self) -> &'static str {
        match self {
            WasmTy::I32 => "i32.store",
            WasmTy::F64 => "f64.store",
        }
    }
}

/// A wasm function signature, with the leading environment pointer included.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct WasmSig {
    params: Vec<WasmTy>,
    result: WasmTy,
}

impl WasmSig {
    fn from_parts(params: impl Iterator<Item = WasmTy>, ret: WasmTy) -> WasmSig {
        let mut all = vec![WasmTy::I32]; // environment pointer
        all.extend(params);
        WasmSig {
            params: all,
            result: ret,
        }
    }

    fn of_fn(params: &[IrParam], ret: &Type, throws: &Option<Type>) -> WasmSig {
        let result = result_wasm_ty(ret, throws.is_some());
        WasmSig::from_parts(params.iter().map(|p| WasmTy::of(&p.ty)), result)
    }

    fn of_type(ty: &FunctionType) -> WasmSig {
        let result = result_wasm_ty(&ty.ret, ty.throws.is_some());
        WasmSig::from_parts(ty.params.iter().map(WasmTy::of), result)
    }
}

/// The wasm result type of a function: a throwing function returns a Result
/// pointer (`i32`, `[ok:i32][value]`); otherwise its plain value type.
fn result_wasm_ty(ret: &Type, throwing: bool) -> WasmTy {
    if throwing {
        WasmTy::I32
    } else {
        WasmTy::of(ret)
    }
}

/// Whether a call's callee is a throwing function, so its result is a Result
/// pointer that the caller must unwrap (spec 0011).
fn is_throwing(callee: &IrExpr) -> bool {
    matches!(callee.ty(), Type::Function(ft) if ft.throws.is_some())
}

const STRING_BASE: u32 = 16;
const MEMORY_PAGES: u32 = 16;

fn align(value: u32, to: u32) -> u32 {
    (value + to - 1) & !(to - 1)
}

// ---------------------------------------------------------------------------
// Static string data
// ---------------------------------------------------------------------------

/// Interned string literals laid out in linear memory as `[len: i32][utf8]`.
struct StringTable {
    offsets: HashMap<String, u32>,
    segments: Vec<(u32, Vec<u8>)>,
    heap_start: u32,
}

fn collect_strings(ir: &IrProgram) -> StringTable {
    let mut order = Vec::new();
    let mut seen = HashSet::new();
    for function in &ir.functions {
        walk(&function.body, &mut |expr| {
            if let IrExpr::String(value) = expr {
                if seen.insert(value.clone()) {
                    order.push(value.clone());
                }
            }
        });
    }
    let mut offsets = HashMap::new();
    let mut segments = Vec::new();
    let mut cursor = STRING_BASE;
    for literal in order {
        let offset = align(cursor, 4);
        let mut bytes = (literal.len() as u32).to_le_bytes().to_vec();
        bytes.extend_from_slice(literal.as_bytes());
        cursor = offset + bytes.len() as u32;
        offsets.insert(literal, offset);
        segments.push((offset, bytes));
    }
    StringTable {
        offsets,
        segments,
        heap_start: align(cursor, 8),
    }
}

/// Renders raw bytes as a WAT data string using only `\HH` hex escapes.
fn wat_bytes(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 4);
    for byte in bytes {
        let _ = write!(out, "\\{byte:02x}");
    }
    out
}

/// Visits every sub-expression, parents before children.
fn walk<'a>(expr: &'a IrExpr, visit: &mut impl FnMut(&'a IrExpr)) {
    visit(expr);
    match expr {
        IrExpr::Array { elems, .. } => elems.iter().for_each(|e| walk(e, visit)),
        IrExpr::Let { value, next, .. } => {
            walk(value, visit);
            walk(next, visit);
        }
        IrExpr::Call { callee, args, .. } => {
            walk(callee, visit);
            args.iter().for_each(|a| walk(a, visit));
        }
        IrExpr::Platform { args, .. } => args.iter().for_each(|a| walk(a, visit)),
        IrExpr::If {
            cond, then, els, ..
        } => {
            walk(cond, visit);
            walk(then, visit);
            walk(els, visit);
        }
        IrExpr::Fn { body, .. } => walk(body, visit),
        IrExpr::Binary { left, right, .. } => {
            walk(left, visit);
            walk(right, visit);
        }
        IrExpr::EnumValue { payload, .. } => payload.iter().for_each(|e| walk(e, visit)),
        IrExpr::Match {
            scrutinee, arms, ..
        } => {
            walk(scrutinee, visit);
            walk_arms(arms, visit);
        }
        IrExpr::Try { body, arms, .. } => {
            walk(body, visit);
            walk_arms(arms, visit);
        }
        IrExpr::Throw { value } | IrExpr::Question { value, .. } => walk(value, visit),
        IrExpr::Panic { message } => walk(message, visit),
        IrExpr::CharFromCode(value) | IrExpr::StringFromChar(value) => walk(value, visit),
        IrExpr::Concat { left, right } => {
            walk(left, visit);
            walk(right, visit);
        }
        _ => {}
    }
}

fn walk_arms<'a>(arms: &'a [IrArm], visit: &mut impl FnMut(&'a IrExpr)) {
    for arm in arms {
        if let Some(guard) = &arm.guard {
            walk(guard, visit);
        }
        walk(&arm.body, visit);
    }
}

// ---------------------------------------------------------------------------
// Function table (closure conversion)
// ---------------------------------------------------------------------------

/// Maps every callable to its index in the wasm function table.
struct FnTable<'a> {
    /// Top-level function name -> table index (0..n_top).
    toplevel: HashMap<&'a str, u32>,
    /// `fn` lambda nodes, in collection order; table index = `n_top + i`.
    lambdas: Vec<&'a IrExpr>,
    /// Identity map from a lambda node to its table index.
    lambda_index: HashMap<*const IrExpr, u32>,
    n_top: u32,
}

impl<'a> FnTable<'a> {
    fn build(ir: &'a IrProgram) -> FnTable<'a> {
        let n_top = ir.functions.len() as u32;
        let mut toplevel = HashMap::new();
        for (index, function) in ir.functions.iter().enumerate() {
            toplevel.insert(function.name.as_str(), index as u32);
        }
        let mut lambdas = Vec::new();
        let mut lambda_index = HashMap::new();
        for function in &ir.functions {
            walk(&function.body, &mut |expr| {
                if let IrExpr::Fn { .. } = expr {
                    let index = n_top + lambdas.len() as u32;
                    lambdas.push(expr);
                    lambda_index.insert(expr as *const IrExpr, index);
                }
            });
        }
        FnTable {
            toplevel,
            lambdas,
            lambda_index,
            n_top,
        }
    }

    fn is_direct<'b>(&self, callee: &'b IrExpr) -> Option<&'b str> {
        match callee {
            IrExpr::FunctionRef { name, .. } if self.toplevel.contains_key(name.as_str()) => {
                Some(name)
            }
            _ => None,
        }
    }
}

/// Interns the distinct wasm signatures used as `call_indirect` types.
#[derive(Default)]
struct SigTable {
    list: Vec<WasmSig>,
    index: HashMap<WasmSig, usize>,
}

impl SigTable {
    fn add(&mut self, sig: WasmSig) {
        if !self.index.contains_key(&sig) {
            self.index.insert(sig.clone(), self.list.len());
            self.list.push(sig);
        }
    }

    fn index_of(&self, sig: &WasmSig) -> Option<usize> {
        self.index.get(sig).copied()
    }
}

fn build_sigs(ir: &IrProgram, table: &FnTable) -> SigTable {
    let mut sigs = SigTable::default();
    // Every table entry can be the target of an indirect call.
    for function in &ir.functions {
        sigs.add(WasmSig::of_fn(
            &function.params,
            &function.ret,
            &function.throws,
        ));
    }
    for lambda in &table.lambdas {
        if let IrExpr::Fn {
            params,
            ret,
            throws,
            ..
        } = lambda
        {
            sigs.add(WasmSig::of_fn(params, ret, throws));
        }
    }
    // And every indirect call site needs its signature declared.
    for function in &ir.functions {
        walk(&function.body, &mut |expr| {
            if let IrExpr::Call { callee, .. } = expr {
                if table.is_direct(callee).is_none() {
                    if let Type::Function(ft) = callee.ty() {
                        sigs.add(WasmSig::of_type(&ft));
                    }
                }
            }
        });
    }
    sigs
}

fn capture_layout(captures: &[IrCapture]) -> Vec<(String, u32, WasmTy)> {
    let mut out = Vec::new();
    let mut offset = 4; // after the table-index header
    for capture in captures {
        let ty = WasmTy::of(&capture.ty);
        out.push((capture.name.clone(), offset, ty));
        offset += ty.size();
    }
    out
}

fn closure_size(captures: &[IrCapture]) -> u32 {
    4 + captures
        .iter()
        .map(|c| WasmTy::of(&c.ty).size())
        .sum::<u32>()
}

// ---------------------------------------------------------------------------
// Module assembly
// ---------------------------------------------------------------------------

fn emit_module(ir: &IrProgram) -> Result<String> {
    let main = ir
        .functions
        .iter()
        .find(|function| function.name == "main")
        .ok_or_else(|| BackendError::new("wasm backend requires a `main` function"))?;

    let used_platform = used_platform_fns(ir);
    for name in &used_platform {
        if platform_glue(name).is_none() {
            return Err(BackendError::new(format!(
                "backend `wasm-wasi` does not provide platform function `{name}`"
            )));
        }
    }

    let strings = collect_strings(ir);
    let table = FnTable::build(ir);
    let sigs = build_sigs(ir, &table);
    let ctx = Ctx {
        table: &table,
        sigs: &sigs,
        strings: &strings,
    };

    let mut functions = String::new();
    for function in &ir.functions {
        functions.push_str(&emit_function(function, &ctx)?);
    }
    for lambda in &table.lambdas {
        functions.push_str(&emit_lambda(lambda, &ctx)?);
    }

    let mut module = String::new();
    module.push_str(";; Generated by the Emela wasm backend.\n");
    module.push_str("(module\n");
    module.push_str(
        "  (import \"wasi_snapshot_preview1\" \"proc_exit\" (func $proc_exit (param i32)))\n",
    );
    if !used_platform.is_empty() {
        module.push_str(
            "  (import \"wasi_snapshot_preview1\" \"fd_write\" (func $wasi_fd_write (param i32 i32 i32 i32) (result i32)))\n",
        );
    }
    let _ = writeln!(
        module,
        "  (memory (export \"memory\") {MEMORY_PAGES})\n  (global $heap (mut i32) (i32.const {}))",
        strings.heap_start
    );
    for (offset, bytes) in &strings.segments {
        let _ = writeln!(
            module,
            "  (data (i32.const {offset}) \"{}\")",
            wat_bytes(bytes)
        );
    }
    module.push_str(&emit_types(&sigs));
    module.push_str(&emit_table(&table));
    module.push_str(ALLOC);
    for name in &used_platform {
        module.push_str(platform_glue(name).expect("checked above"));
    }
    module.push_str(&functions);
    module.push_str(&emit_start(main));
    module.push_str("  (export \"main\" (func $f_main))\n");
    module.push_str("  (export \"_start\" (func $_start))\n");
    module.push_str(")\n");
    Ok(module)
}

/// The wasm function id for a platform function's runtime glue.
fn platform_wasm_name(canonical: &str) -> String {
    let mut out = String::from("$plat_");
    for ch in canonical.chars() {
        out.push(if ch.is_ascii_alphanumeric() { ch } else { '_' });
    }
    out
}

/// The platform functions the program references, in first-occurrence order.
fn used_platform_fns(ir: &IrProgram) -> Vec<String> {
    let mut order = Vec::new();
    let mut seen = HashSet::new();
    for function in &ir.functions {
        walk(&function.body, &mut |expr| {
            if let IrExpr::Platform { name, .. } = expr {
                if seen.insert(name.clone()) {
                    order.push(name.clone());
                }
            }
        });
    }
    order
}

/// WASI-backed glue for a platform function, or `None` if not provided.
///
/// `write_*` take a string pointer to `[len: i32][utf8]`. The iovec/nwritten
/// scratch lives in the null-guard region `[0, 16)`.
fn platform_glue(canonical: &str) -> Option<&'static str> {
    match canonical {
        "io.write_stdout" => Some(WRITE_STDOUT_GLUE),
        "io.write_stderr" => Some(WRITE_STDERR_GLUE),
        _ => None,
    }
}

const WRITE_STDOUT_GLUE: &str = "  (func $plat_io_write_stdout (param $s i32) (result i32)\n    i32.const 0\n    local.get $s\n    i32.const 4\n    i32.add\n    i32.store\n    i32.const 4\n    local.get $s\n    i32.load\n    i32.store\n    i32.const 1\n    i32.const 0\n    i32.const 1\n    i32.const 8\n    call $wasi_fd_write\n    drop\n    i32.const 0)\n";

const WRITE_STDERR_GLUE: &str = "  (func $plat_io_write_stderr (param $s i32) (result i32)\n    i32.const 0\n    local.get $s\n    i32.const 4\n    i32.add\n    i32.store\n    i32.const 4\n    local.get $s\n    i32.load\n    i32.store\n    i32.const 2\n    i32.const 0\n    i32.const 1\n    i32.const 8\n    call $wasi_fd_write\n    drop\n    i32.const 0)\n";

fn emit_types(sigs: &SigTable) -> String {
    let mut out = String::new();
    for (index, sig) in sigs.list.iter().enumerate() {
        out.push_str(&format!("  (type $sig_{index} (func"));
        for param in &sig.params {
            let _ = write!(out, " (param {})", param.keyword());
        }
        let _ = writeln!(out, " (result {})))", sig.result.keyword());
    }
    out
}

fn emit_table(table: &FnTable) -> String {
    let total = table.n_top + table.lambdas.len() as u32;
    if total == 0 {
        return String::new();
    }
    let mut entries: Vec<String> = vec![String::new(); total as usize];
    for (name, index) in &table.toplevel {
        entries[*index as usize] = format!("$f_{name}");
    }
    for (offset, _) in table.lambdas.iter().enumerate() {
        entries[table.n_top as usize + offset] = format!("$lambda_{offset}");
    }
    let mut out = String::new();
    let _ = writeln!(out, "  (table {total} funcref)");
    let _ = writeln!(out, "  (elem (i32.const 0) {})", entries.join(" "));
    out
}

/// A bump allocator over linear memory. No free; 8-byte aligned.
const ALLOC: &str = "  (func $alloc (param $n i32) (result i32)\n    (local $p i32)\n    global.get $heap\n    i32.const 7\n    i32.add\n    i32.const -8\n    i32.and\n    local.set $p\n    local.get $p\n    local.get $n\n    i32.add\n    global.set $heap\n    local.get $p)\n";

fn emit_start(main: &IrFunction) -> String {
    let mut out = String::new();
    out.push_str("  (func $_start\n");
    out.push_str("    i32.const 0\n");
    out.push_str("    call $f_main\n");
    if main.ret == Type::Int {
        // The Int result is the exit code.
        out.push_str("    call $proc_exit)\n");
    } else {
        // Drop any other result and exit 0.
        out.push_str("    drop\n    i32.const 0\n    call $proc_exit)\n");
    }
    out
}

fn emit_function(function: &IrFunction, ctx: &Ctx) -> Result<String> {
    emit_fn_like(
        &format!("$f_{}", function.name),
        &function.params,
        &function.ret,
        &function.throws,
        &[],
        &function.body,
        ctx,
    )
}

fn emit_lambda(lambda: &IrExpr, ctx: &Ctx) -> Result<String> {
    let index = ctx.table.lambda_index[&(lambda as *const IrExpr)];
    let name = format!("$lambda_{}", index - ctx.table.n_top);
    let IrExpr::Fn {
        params,
        ret,
        throws,
        captures,
        body,
        ..
    } = lambda
    else {
        unreachable!("lambda table only holds Fn nodes");
    };
    emit_fn_like(&name, params, ret, throws, captures, body, ctx)
}

fn emit_fn_like(
    wasm_name: &str,
    params: &[IrParam],
    ret: &Type,
    throws: &Option<Type>,
    captures: &[IrCapture],
    body: &IrExpr,
    ctx: &Ctx,
) -> Result<String> {
    let mut emitter = FnEmitter::new(ctx);
    for (index, param) in params.iter().enumerate() {
        // Param 0 is the closure environment; user params follow.
        emitter.bind(param.name.clone(), Slot::Local(format!("$p{}", index + 1)));
    }
    for (name, offset, ty) in capture_layout(captures) {
        emitter.bind(name, Slot::Capture(offset, ty));
    }
    emitter.emit(body)?;
    if throws.is_some() {
        // The body left its success value on the stack; wrap it as `Ok`.
        emitter.wrap_ok(ret);
    }

    let mut signature = format!("  (func {wasm_name} (param $p0 i32)");
    for (index, param) in params.iter().enumerate() {
        let _ = write!(
            signature,
            " (param $p{} {})",
            index + 1,
            WasmTy::of(&param.ty).keyword()
        );
    }
    let _ = writeln!(
        signature,
        " (result {})",
        result_wasm_ty(ret, throws.is_some()).keyword()
    );

    let mut out = signature;
    for (id, ty) in &emitter.locals {
        let _ = writeln!(out, "    (local {} {})", id, ty.keyword());
    }
    out.push_str(&emitter.code);
    out.push_str(")\n");
    Ok(out)
}

// ---------------------------------------------------------------------------
// Expression emission
// ---------------------------------------------------------------------------

struct Ctx<'a> {
    table: &'a FnTable<'a>,
    sigs: &'a SigTable,
    strings: &'a StringTable,
}

/// Where a bound name lives at run time.
enum Slot {
    /// A wasm local (a parameter or a `let` binding).
    Local(String),
    /// A captured variable, at `offset` bytes into the environment pointer.
    Capture(u32, WasmTy),
}

struct FnEmitter<'a> {
    code: String,
    locals: Vec<(String, WasmTy)>,
    scope: Vec<(String, Slot)>,
    counter: usize,
    /// Labels of the enclosing `try` blocks' catch handlers. A `throw` or
    /// throwing call branches to the innermost one, or returns `Err` if empty.
    catch_stack: Vec<String>,
    ctx: &'a Ctx<'a>,
}

impl<'a> FnEmitter<'a> {
    fn new(ctx: &'a Ctx<'a>) -> Self {
        Self {
            code: String::new(),
            locals: Vec::new(),
            scope: Vec::new(),
            counter: 0,
            catch_stack: Vec::new(),
            ctx,
        }
    }

    fn bind(&mut self, name: String, slot: Slot) {
        self.scope.push((name, slot));
    }

    fn slot(&self, name: &str) -> Option<&Slot> {
        self.scope
            .iter()
            .rev()
            .find(|(bound, _)| bound == name)
            .map(|(_, slot)| slot)
    }

    fn fresh_local(&mut self, ty: WasmTy) -> String {
        let id = format!("$v{}", self.counter);
        self.counter += 1;
        self.locals.push((id.clone(), ty));
        id
    }

    fn line(&mut self, instruction: &str) {
        self.code.push_str("    ");
        self.code.push_str(instruction);
        self.code.push('\n');
    }

    fn emit(&mut self, expr: &IrExpr) -> Result<()> {
        match expr {
            IrExpr::Int(value) => self.line(&format!("i32.const {value}")),
            IrExpr::Bool(value) => self.line(&format!("i32.const {}", i32::from(*value))),
            IrExpr::Unit => self.line("i32.const 0"),
            IrExpr::Float(value) => self.line(&format!("f64.const {value}")),
            IrExpr::Var { name, .. } => self.emit_var(name)?,
            IrExpr::Binary {
                op,
                ty,
                left,
                right,
                ..
            } => {
                self.emit(left)?;
                self.emit(right)?;
                self.line(binary_op(*op, ty));
            }
            IrExpr::Let {
                name,
                value_ty,
                value,
                next,
            } => {
                self.emit(value)?;
                let id = self.fresh_local(WasmTy::of(value_ty));
                self.line(&format!("local.set {id}"));
                self.bind(name.clone(), Slot::Local(id));
                self.emit(next)?;
            }
            IrExpr::Call { callee, args, ret } => {
                self.emit_call(callee, args)?;
                if is_throwing(callee) {
                    // A throwing call yields a Result pointer; unwrap it.
                    self.unwrap_result(ret)?;
                }
            }
            IrExpr::Platform { name, args, .. } => {
                for arg in args {
                    self.emit(arg)?;
                }
                self.line(&format!("call {}", platform_wasm_name(name)));
            }
            IrExpr::String(value) => {
                let offset = *self.ctx.strings.offsets.get(value).ok_or_else(|| {
                    BackendError::new("internal error: string literal was not interned")
                })?;
                self.line(&format!("i32.const {offset}"));
            }
            // A `Char` is its codepoint as i32 (spec 0017); `from_code` is the
            // identity on that representation.
            IrExpr::Char(code) => self.line(&format!("i32.const {code}")),
            IrExpr::CharFromCode(value) => self.emit(value)?,
            IrExpr::StringFromChar(value) => self.emit_string_from_char(value)?,
            IrExpr::Concat { left, right } => self.emit_concat(left, right)?,
            IrExpr::Array { elem_ty, elems } => self.emit_array(elem_ty, elems)?,
            IrExpr::FunctionRef { name, .. } => self.emit_function_ref(name)?,
            IrExpr::Fn { captures, .. } => self.emit_closure(expr, captures)?,
            IrExpr::EnumValue { tag, payload, .. } => self.emit_enum_value(*tag, payload)?,
            IrExpr::Match {
                scrutinee,
                arms,
                ty,
            } => self.emit_match(scrutinee, arms, ty)?,
            IrExpr::If {
                cond,
                then,
                els,
                ty,
            } => {
                self.emit(cond)?;
                self.line(&format!("if (result {})", WasmTy::of(ty).keyword()));
                self.emit(then)?;
                self.line("else");
                self.emit(els)?;
                self.line("end");
            }
            IrExpr::Panic { .. } => self.line("unreachable"),
            IrExpr::Throw { value } => {
                self.emit(value)?;
                self.raise_error();
            }
            IrExpr::Try { body, arms, ty } => self.emit_try(body, arms, ty)?,
            IrExpr::Question { value, mode, ty } => self.emit_question(value, *mode, ty)?,
        }
        Ok(())
    }

    /// `String.from_char` (spec 0017): a one-byte (ASCII) `[len=1][byte]` string.
    /// Multi-byte UTF-8 encoding is a follow-up.
    fn emit_string_from_char(&mut self, value: &IrExpr) -> Result<()> {
        let code = self.fresh_local(WasmTy::I32);
        let out = self.fresh_local(WasmTy::I32);
        self.emit(value)?;
        self.line(&format!("local.set {code}"));
        self.line("i32.const 5");
        self.line("call $alloc");
        self.line(&format!("local.set {out}"));
        self.line(&format!("local.get {out}"));
        self.line("i32.const 1");
        self.line("i32.store");
        self.line(&format!("local.get {out}"));
        self.line("i32.const 4");
        self.line("i32.add");
        self.line(&format!("local.get {code}"));
        self.line("i32.store8");
        self.line(&format!("local.get {out}"));
        Ok(())
    }

    /// `a ++ b` (spec 0017): allocate `[len_a+len_b][bytes_a bytes_b]` and copy.
    fn emit_concat(&mut self, left: &IrExpr, right: &IrExpr) -> Result<()> {
        let a = self.fresh_local(WasmTy::I32);
        let b = self.fresh_local(WasmTy::I32);
        let len_a = self.fresh_local(WasmTy::I32);
        let len_b = self.fresh_local(WasmTy::I32);
        let out = self.fresh_local(WasmTy::I32);
        self.emit(left)?;
        self.line(&format!("local.set {a}"));
        self.emit(right)?;
        self.line(&format!("local.set {b}"));
        self.line(&format!("local.get {a}"));
        self.line("i32.load");
        self.line(&format!("local.set {len_a}"));
        self.line(&format!("local.get {b}"));
        self.line("i32.load");
        self.line(&format!("local.set {len_b}"));
        // out = alloc(4 + len_a + len_b)
        self.line("i32.const 4");
        self.line(&format!("local.get {len_a}"));
        self.line("i32.add");
        self.line(&format!("local.get {len_b}"));
        self.line("i32.add");
        self.line("call $alloc");
        self.line(&format!("local.set {out}"));
        // store the combined length
        self.line(&format!("local.get {out}"));
        self.line(&format!("local.get {len_a}"));
        self.line(&format!("local.get {len_b}"));
        self.line("i32.add");
        self.line("i32.store");
        // memory.copy(dest = out+4, src = a+4, n = len_a)
        self.line(&format!("local.get {out}"));
        self.line("i32.const 4");
        self.line("i32.add");
        self.line(&format!("local.get {a}"));
        self.line("i32.const 4");
        self.line("i32.add");
        self.line(&format!("local.get {len_a}"));
        self.line("memory.copy");
        // memory.copy(dest = out+4+len_a, src = b+4, n = len_b)
        self.line(&format!("local.get {out}"));
        self.line("i32.const 4");
        self.line("i32.add");
        self.line(&format!("local.get {len_a}"));
        self.line("i32.add");
        self.line(&format!("local.get {b}"));
        self.line("i32.const 4");
        self.line("i32.add");
        self.line(&format!("local.get {len_b}"));
        self.line("memory.copy");
        self.line(&format!("local.get {out}"));
        Ok(())
    }

    /// Allocates `[tag:i32][field*8bytes]` and leaves the pointer. Each payload
    /// field gets a fixed 8-byte slot so binding offsets need no type info.
    fn emit_enum_value(&mut self, tag: u32, payload: &[IrExpr]) -> Result<()> {
        let size = 8 + payload.len() as u32 * 8;
        let ptr = self.fresh_local(WasmTy::I32);
        self.line(&format!("i32.const {size}"));
        self.line("call $alloc");
        self.line(&format!("local.set {ptr}"));
        self.line(&format!("local.get {ptr}"));
        self.line(&format!("i32.const {tag}"));
        self.line("i32.store");
        for (index, field) in payload.iter().enumerate() {
            let slot = WasmTy::of(&field.ty());
            let offset = 8 + index as u32 * 8;
            self.line(&format!("local.get {ptr}"));
            self.line(&format!("i32.const {offset}"));
            self.line("i32.add");
            self.emit(field)?;
            self.line(slot.store());
        }
        self.line(&format!("local.get {ptr}"));
        Ok(())
    }

    /// Lowers a `match` to a tag dispatch that yields the matched arm's value.
    fn emit_match(&mut self, scrutinee: &IrExpr, arms: &[IrArm], result_ty: &Type) -> Result<()> {
        let subject = self.fresh_local(WasmTy::I32);
        self.emit(scrutinee)?;
        self.line(&format!("local.set {subject}"));
        let label = format!("$match_{}", self.counter);
        self.counter += 1;
        self.line(&format!(
            "(block {label} (result {})",
            WasmTy::of(result_ty).keyword()
        ));
        for arm in arms {
            self.emit_arm(&subject, arm, &label)?;
        }
        self.line("unreachable");
        self.line(")");
        Ok(())
    }

    fn emit_arm(&mut self, subject: &str, arm: &IrArm, done: &str) -> Result<()> {
        let mark = self.scope.len();
        match &arm.pattern {
            IrPattern::Variant { tag, bindings, .. } => {
                self.line(&format!("local.get {subject}"));
                self.line("i32.load");
                self.line(&format!("i32.const {tag}"));
                self.line("i32.eq");
                self.line("(if");
                self.line("(then");
                self.bind_payload(subject, bindings);
                self.emit_arm_body(arm, done)?;
                self.line("))");
            }
            IrPattern::Wildcard { binding } => {
                if let Some((name, ty)) = binding {
                    let local = self.fresh_local(WasmTy::of(ty));
                    self.line(&format!("local.get {subject}"));
                    self.line(&format!("local.set {local}"));
                    self.bind(name.clone(), Slot::Local(local));
                }
                self.emit_arm_body(arm, done)?;
            }
        }
        self.scope.truncate(mark);
        Ok(())
    }

    fn bind_payload(&mut self, subject: &str, bindings: &[Option<(String, Type)>]) {
        for (index, binding) in bindings.iter().enumerate() {
            if let Some((name, ty)) = binding {
                let slot = WasmTy::of(ty);
                let local = self.fresh_local(slot);
                let offset = 8 + index as u32 * 8;
                self.line(&format!("local.get {subject}"));
                self.line(&format!("i32.const {offset}"));
                self.line("i32.add");
                self.line(slot.load());
                self.line(&format!("local.set {local}"));
                self.bind(name.clone(), Slot::Local(local));
            }
        }
    }

    fn emit_arm_body(&mut self, arm: &IrArm, done: &str) -> Result<()> {
        match &arm.guard {
            Some(guard) => {
                self.emit(guard)?;
                self.line("(if");
                self.line("(then");
                self.emit(&arm.body)?;
                self.line(&format!("br {done}"));
                self.line("))");
            }
            None => {
                self.emit(&arm.body)?;
                self.line(&format!("br {done}"));
            }
        }
        Ok(())
    }

    /// Wraps the success value on the stack into a `[ok:1][value]` Result
    /// pointer (spec 0011's IR lowering note).
    fn wrap_ok(&mut self, ret: &Type) {
        let value = self.fresh_local(WasmTy::of(ret));
        self.line(&format!("local.set {value}"));
        let res = self.fresh_local(WasmTy::I32);
        self.line("i32.const 16");
        self.line("call $alloc");
        self.line(&format!("local.set {res}"));
        self.line(&format!("local.get {res}"));
        self.line("i32.const 1");
        self.line("i32.store");
        self.line(&format!("local.get {res}"));
        self.line("i32.const 8");
        self.line("i32.add");
        self.line(&format!("local.get {value}"));
        self.line(WasmTy::of(ret).store());
        self.line(&format!("local.get {res}"));
    }

    /// Raises the error value on the stack: branch to the nearest enclosing
    /// `catch`, or build and return an `Err` Result from a throwing function.
    fn raise_error(&mut self) {
        if let Some(label) = self.catch_stack.last() {
            let label = label.clone();
            self.line(&format!("br {label}"));
        } else {
            let err = self.fresh_local(WasmTy::I32);
            self.line(&format!("local.set {err}"));
            let res = self.fresh_local(WasmTy::I32);
            self.line("i32.const 16");
            self.line("call $alloc");
            self.line(&format!("local.set {res}"));
            self.line(&format!("local.get {res}"));
            self.line("i32.const 0");
            self.line("i32.store");
            self.line(&format!("local.get {res}"));
            self.line("i32.const 8");
            self.line("i32.add");
            self.line(&format!("local.get {err}"));
            self.line("i32.store");
            self.line(&format!("local.get {res}"));
            self.line("return");
        }
    }

    /// Unwraps a Result pointer on the stack: on `Err`, raise the error; on
    /// `Ok`, leave the success value of type `success_ty`.
    fn unwrap_result(&mut self, success_ty: &Type) -> Result<()> {
        let r = self.fresh_local(WasmTy::I32);
        self.line(&format!("local.set {r}"));
        self.line(&format!("local.get {r}"));
        self.line("i32.load");
        self.line("i32.eqz");
        self.line("(if");
        self.line("(then");
        self.line(&format!("local.get {r}"));
        self.line("i32.const 8");
        self.line("i32.add");
        self.line("i32.load");
        self.raise_error();
        self.line("))");
        self.line(&format!("local.get {r}"));
        self.line("i32.const 8");
        self.line("i32.add");
        self.line(WasmTy::of(success_ty).load());
        Ok(())
    }

    fn emit_question(&mut self, value: &IrExpr, mode: QuestionMode, ty: &Type) -> Result<()> {
        match mode {
            // The inner throwing call already unwraps, so `?` yields its value.
            QuestionMode::Throws => self.emit(value)?,
            QuestionMode::Option => {
                self.emit(value)?;
                let opt = self.fresh_local(WasmTy::I32);
                self.line(&format!("local.set {opt}"));
                self.line(&format!("local.get {opt}"));
                self.line("i32.load");
                self.line("i32.const 1");
                self.line("i32.eq");
                self.line("(if");
                self.line("(then");
                // Propagate `None`: return `Option::None` from this function.
                self.emit_enum_value(1, &[])?;
                self.line("return");
                self.line("))");
                self.line(&format!("local.get {opt}"));
                self.line("i32.const 8");
                self.line("i32.add");
                self.line(WasmTy::of(ty).load());
            }
        }
        Ok(())
    }

    fn emit_try(&mut self, body: &IrExpr, arms: &[IrArm], ty: &Type) -> Result<()> {
        let id = self.counter;
        self.counter += 1;
        let try_label = format!("$try_{id}");
        let catch_label = format!("$catch_{id}");
        self.line(&format!(
            "(block {try_label} (result {})",
            WasmTy::of(ty).keyword()
        ));
        // The catch block yields the error pointer that a throwing call/`throw`
        // branches to it with.
        self.line(&format!("(block {catch_label} (result i32)"));
        self.catch_stack.push(catch_label);
        self.emit(body)?;
        self.catch_stack.pop();
        self.line(&format!("br {try_label}"));
        self.line(")");
        let err = self.fresh_local(WasmTy::I32);
        self.line(&format!("local.set {err}"));
        for arm in arms {
            self.emit_arm(&err, arm, &try_label)?;
        }
        self.line("unreachable");
        self.line(")");
        Ok(())
    }

    fn emit_var(&mut self, name: &str) -> Result<()> {
        match self.slot(name) {
            Some(Slot::Local(id)) => {
                let id = id.clone();
                self.line(&format!("local.get {id}"));
            }
            Some(Slot::Capture(offset, ty)) => {
                let (offset, load) = (*offset, ty.load());
                self.line("local.get $p0");
                self.line(&format!("i32.const {offset}"));
                self.line("i32.add");
                self.line(load);
            }
            None => {
                return Err(BackendError::new(format!(
                    "unbound variable `{name}` in wasm"
                )));
            }
        }
        Ok(())
    }

    /// Allocates `[len: i32][elem...]` and returns the pointer.
    fn emit_array(&mut self, elem_ty: &Type, elems: &[IrExpr]) -> Result<()> {
        let elem = WasmTy::of(elem_ty);
        let total = 4 + elem.size() * elems.len() as u32;
        let arr = self.fresh_local(WasmTy::I32);
        self.line(&format!("i32.const {total}"));
        self.line("call $alloc");
        self.line(&format!("local.set {arr}"));
        self.line(&format!("local.get {arr}"));
        self.line(&format!("i32.const {}", elems.len()));
        self.line("i32.store");
        for (index, value) in elems.iter().enumerate() {
            let offset = 4 + elem.size() * index as u32;
            self.line(&format!("local.get {arr}"));
            self.line(&format!("i32.const {offset}"));
            self.line("i32.add");
            self.emit(value)?;
            self.line(elem.store());
        }
        self.line(&format!("local.get {arr}"));
        Ok(())
    }

    /// A bare top-level function used as a value: a closure with no captures.
    fn emit_function_ref(&mut self, name: &str) -> Result<()> {
        let index = *self.ctx.table.toplevel.get(name).ok_or_else(|| {
            BackendError::new(format!("function `{name}` is not in the wasm table"))
        })?;
        let closure = self.fresh_local(WasmTy::I32);
        self.line("i32.const 4");
        self.line("call $alloc");
        self.line(&format!("local.set {closure}"));
        self.line(&format!("local.get {closure}"));
        self.line(&format!("i32.const {index}"));
        self.line("i32.store");
        self.line(&format!("local.get {closure}"));
        Ok(())
    }

    /// A `fn` lambda used as a value: allocate `[table_index, captures...]`.
    fn emit_closure(&mut self, node: &IrExpr, captures: &[IrCapture]) -> Result<()> {
        let index = *self
            .ctx
            .table
            .lambda_index
            .get(&(node as *const IrExpr))
            .ok_or_else(|| BackendError::new("internal error: lambda not in table"))?;
        let total = closure_size(captures);
        let closure = self.fresh_local(WasmTy::I32);
        self.line(&format!("i32.const {total}"));
        self.line("call $alloc");
        self.line(&format!("local.set {closure}"));
        self.line(&format!("local.get {closure}"));
        self.line(&format!("i32.const {index}"));
        self.line("i32.store");
        for (name, offset, ty) in capture_layout(captures) {
            self.line(&format!("local.get {closure}"));
            self.line(&format!("i32.const {offset}"));
            self.line("i32.add");
            // The capture's value comes from the *enclosing* scope.
            self.emit_var(&name)?;
            self.line(ty.store());
        }
        self.line(&format!("local.get {closure}"));
        Ok(())
    }

    fn emit_call(&mut self, callee: &IrExpr, args: &[IrExpr]) -> Result<()> {
        if let Some(name) = self.ctx.table.is_direct(callee) {
            // Direct call to a known top-level function: env is unused (0).
            let name = name.to_string();
            self.line("i32.const 0");
            for arg in args {
                self.emit(arg)?;
            }
            self.line(&format!("call $f_{name}"));
            return Ok(());
        }

        // Indirect call through a closure value.
        let Type::Function(ft) = callee.ty() else {
            return Err(BackendError::new(
                "wasm backend cannot call a non-function value",
            ));
        };
        let sig = WasmSig::of_type(&ft);
        let sig_index = self.ctx.sigs.index_of(&sig).ok_or_else(|| {
            BackendError::new("internal error: indirect call signature was not declared")
        })?;

        self.emit(callee)?;
        let closure = self.fresh_local(WasmTy::I32);
        self.line(&format!("local.set {closure}"));
        self.line(&format!("local.get {closure}")); // environment = closure pointer
        for arg in args {
            self.emit(arg)?;
        }
        self.line(&format!("local.get {closure}"));
        self.line("i32.load"); // table index
        self.line(&format!("call_indirect (type $sig_{sig_index})"));
        Ok(())
    }
}

fn binary_op(op: BinaryOp, operand_ty: &Type) -> &'static str {
    match (WasmTy::of(operand_ty), op) {
        (WasmTy::I32, BinaryOp::Add) => "i32.add",
        (WasmTy::I32, BinaryOp::Sub) => "i32.sub",
        (WasmTy::I32, BinaryOp::Mul) => "i32.mul",
        (WasmTy::I32, BinaryOp::Div) => "i32.div_s",
        (WasmTy::I32, BinaryOp::Rem) => "i32.rem_s",
        (WasmTy::I32, BinaryOp::Eq) => "i32.eq",
        (WasmTy::I32, BinaryOp::Lt) => "i32.lt_s",
        (WasmTy::F64, BinaryOp::Add) => "f64.add",
        (WasmTy::F64, BinaryOp::Sub) => "f64.sub",
        (WasmTy::F64, BinaryOp::Mul) => "f64.mul",
        (WasmTy::F64, BinaryOp::Div) => "f64.div",
        (WasmTy::F64, BinaryOp::Eq) => "f64.eq",
        (WasmTy::F64, BinaryOp::Lt) => "f64.lt",
        // `%` is Int-only (spec 0016); the type checker rejects Float `%`.
        (WasmTy::F64, BinaryOp::Rem) => unreachable!("Float % rejected by type checker"),
        // `++` lowers to `IrExpr::Concat`, never to a Binary (spec 0017).
        (_, BinaryOp::Concat) => unreachable!("concat lowers to IrExpr::Concat"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emela_codegen::{EffectRow, IrFunction, IrParam};

    fn int_fn(params: Vec<Type>) -> FunctionType {
        FunctionType {
            params,
            ret: Box::new(Type::Int),
            throws: None,
            effects: EffectRow::default(),
        }
    }

    fn main_returning(ret: Type, body: IrExpr) -> IrProgram {
        IrProgram {
            functions: vec![IrFunction {
                name: "main".into(),
                params: vec![],
                ret,
                throws: None,
                effects: EffectRow::default(),
                body,
            }],
        }
    }

    fn compile_binary(program: &IrProgram) -> Vec<u8> {
        WasmBackend
            .compile(program, &BackendOptions::default())
            .expect("compile")
            .bytes
    }

    fn compile_wat(program: &IrProgram) -> String {
        let artifact = WasmBackend
            .compile(
                program,
                &BackendOptions {
                    mode: EmitMode::Text,
                    ..Default::default()
                },
            )
            .expect("compile");
        String::from_utf8(artifact.bytes).unwrap()
    }

    // `fn add(x, y) -> Int { x + y }` and `fn main() -> Int { add(20, 22) }`.
    fn add_program() -> IrProgram {
        let add = IrFunction {
            name: "add".into(),
            params: vec![
                IrParam {
                    name: "x".into(),
                    ty: Type::Int,
                },
                IrParam {
                    name: "y".into(),
                    ty: Type::Int,
                },
            ],
            ret: Type::Int,
            throws: None,
            effects: EffectRow::default(),
            body: IrExpr::Binary {
                op: BinaryOp::Add,
                ty: Type::Int,
                left: Box::new(IrExpr::Var {
                    name: "x".into(),
                    ty: Type::Int,
                }),
                right: Box::new(IrExpr::Var {
                    name: "y".into(),
                    ty: Type::Int,
                }),
            },
        };
        let main = IrFunction {
            name: "main".into(),
            params: vec![],
            ret: Type::Int,
            throws: None,
            effects: EffectRow::default(),
            body: IrExpr::Call {
                callee: Box::new(IrExpr::FunctionRef {
                    name: "add".into(),
                    sig: int_fn(vec![Type::Int, Type::Int]),
                }),
                args: vec![IrExpr::Int(20), IrExpr::Int(22)],
                ret: Type::Int,
            },
        };
        IrProgram {
            functions: vec![add, main],
        }
    }

    #[test]
    fn compiles_to_a_valid_wasm_binary() {
        let bytes = compile_binary(&add_program());
        assert_eq!(&bytes[0..4], b"\0asm");
        assert_eq!(&bytes[4..8], &[1, 0, 0, 0]);
    }

    #[test]
    fn emits_expected_wat() {
        let wat = compile_wat(&add_program());
        assert!(wat.contains("call $f_add"), "{wat}");
        assert!(wat.contains("(export \"_start\" (func $_start))"), "{wat}");
        assert!(wat.contains("call $proc_exit"), "{wat}");
    }

    #[test]
    fn string_literal_becomes_a_data_segment() {
        let program = main_returning(Type::String, IrExpr::String("Hello, Emela!".into()));
        let wat = compile_wat(&program);
        assert!(wat.contains("(data (i32.const 16) \"\\0d"), "{wat}");
        let _ = compile_binary(&program);
    }

    #[test]
    fn array_allocates_and_stores_elements() {
        let program = main_returning(
            Type::Array(Box::new(Type::Int)),
            IrExpr::Array {
                elem_ty: Type::Int,
                elems: vec![IrExpr::Int(10), IrExpr::Int(20), IrExpr::Int(30)],
            },
        );
        let wat = compile_wat(&program);
        assert!(wat.contains("call $alloc"), "{wat}");
        assert!(wat.contains("i32.store"), "{wat}");
        let _ = compile_binary(&program);
    }

    #[test]
    fn float_array_uses_f64_stores() {
        let program = main_returning(
            Type::Array(Box::new(Type::Float)),
            IrExpr::Array {
                elem_ty: Type::Float,
                elems: vec![IrExpr::Float(1.5), IrExpr::Float(2.5)],
            },
        );
        let wat = compile_wat(&program);
        assert!(wat.contains("f64.store"), "{wat}");
        let _ = compile_binary(&program);
    }

    // `fn make_adder(n) { fn(x) { x + n } }` and a `main` that calls the result.
    fn closure_program() -> IrProgram {
        let lambda = IrExpr::Fn {
            params: vec![IrParam {
                name: "x".into(),
                ty: Type::Int,
            }],
            ret: Type::Int,
            throws: None,
            effects: EffectRow::default(),
            captures: vec![IrCapture {
                name: "n".into(),
                ty: Type::Int,
            }],
            body: Box::new(IrExpr::Binary {
                op: BinaryOp::Add,
                ty: Type::Int,
                left: Box::new(IrExpr::Var {
                    name: "x".into(),
                    ty: Type::Int,
                }),
                right: Box::new(IrExpr::Var {
                    name: "n".into(),
                    ty: Type::Int,
                }),
            }),
        };
        let make_adder = IrFunction {
            name: "make_adder".into(),
            params: vec![IrParam {
                name: "n".into(),
                ty: Type::Int,
            }],
            ret: Type::Function(int_fn(vec![Type::Int])),
            throws: None,
            effects: EffectRow::default(),
            body: lambda,
        };
        let main = IrFunction {
            name: "main".into(),
            params: vec![],
            ret: Type::Int,
            throws: None,
            effects: EffectRow::default(),
            body: IrExpr::Let {
                name: "add10".into(),
                value_ty: Type::Function(int_fn(vec![Type::Int])),
                value: Box::new(IrExpr::Call {
                    callee: Box::new(IrExpr::FunctionRef {
                        name: "make_adder".into(),
                        sig: FunctionType {
                            params: vec![Type::Int],
                            ret: Box::new(Type::Function(int_fn(vec![Type::Int]))),
                            throws: None,
                            effects: EffectRow::default(),
                        },
                    }),
                    args: vec![IrExpr::Int(10)],
                    ret: Type::Function(int_fn(vec![Type::Int])),
                }),
                next: Box::new(IrExpr::Call {
                    callee: Box::new(IrExpr::Var {
                        name: "add10".into(),
                        ty: Type::Function(int_fn(vec![Type::Int])),
                    }),
                    args: vec![IrExpr::Int(32)],
                    ret: Type::Int,
                }),
            },
        };
        IrProgram {
            functions: vec![make_adder, main],
        }
    }

    #[test]
    fn closures_use_table_and_indirect_calls() {
        let wat = compile_wat(&closure_program());
        assert!(wat.contains("(table "), "{wat}");
        assert!(wat.contains("(elem "), "{wat}");
        assert!(wat.contains("call_indirect (type $sig_"), "{wat}");
        assert!(wat.contains("$lambda_0"), "{wat}");
        // The captured `n` is loaded from the environment pointer.
        assert!(wat.contains("local.get $p0"), "{wat}");
        let _ = compile_binary(&closure_program());
    }
}

#[cfg(test)]
mod platform_tests {
    use super::*;
    use emela_codegen::{EffectRow, IrFunction};

    fn main_platform(name: &str) -> IrProgram {
        IrProgram {
            functions: vec![IrFunction {
                name: "main".into(),
                params: vec![],
                ret: Type::Unit,
                throws: None,
                effects: EffectRow::sorted(vec!["io".into()]),
                body: IrExpr::Platform {
                    name: name.into(),
                    args: vec![IrExpr::String("hi".into())],
                    ret: Type::Unit,
                },
            }],
        }
    }

    #[test]
    fn emits_wasi_glue_and_validates() {
        let program = main_platform("io.write_stdout");
        let wat = String::from_utf8(
            WasmBackend
                .compile(
                    &program,
                    &BackendOptions {
                        mode: EmitMode::Text,
                        ..Default::default()
                    },
                )
                .expect("compile")
                .bytes,
        )
        .unwrap();
        assert!(
            wat.contains("(import \"wasi_snapshot_preview1\" \"fd_write\""),
            "{wat}"
        );
        assert!(wat.contains("(func $plat_io_write_stdout"), "{wat}");
        assert!(wat.contains("call $plat_io_write_stdout"), "{wat}");
        // Default mode assembles and validates the binary.
        let bytes = WasmBackend
            .compile(&program, &BackendOptions::default())
            .expect("compile")
            .bytes;
        assert_eq!(&bytes[0..4], b"\0asm");
    }

    #[test]
    fn rejects_unprovided_platform_fn() {
        let err = WasmBackend
            .compile(&main_platform("fs.read"), &BackendOptions::default())
            .unwrap_err();
        assert!(err.to_string().contains("does not provide"), "{err}");
    }
}
