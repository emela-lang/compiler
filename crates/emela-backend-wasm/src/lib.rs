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
    FunctionType, IrCapture, IrExpr, IrFunction, IrParam, IrProgram, Result, Tier, Type,
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

    fn of_fn(params: &[IrParam], ret: &Type) -> WasmSig {
        WasmSig::from_parts(params.iter().map(|p| WasmTy::of(&p.ty)), WasmTy::of(ret))
    }

    fn of_type(ty: &FunctionType) -> WasmSig {
        WasmSig::from_parts(ty.params.iter().map(WasmTy::of), WasmTy::of(&ty.ret))
    }
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
        IrExpr::Fn { body, .. } => walk(body, visit),
        IrExpr::Binary { left, right, .. } => {
            walk(left, visit);
            walk(right, visit);
        }
        _ => {}
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
        sigs.add(WasmSig::of_fn(&function.params, &function.ret));
    }
    for lambda in &table.lambdas {
        if let IrExpr::Fn { params, ret, .. } = lambda {
            sigs.add(WasmSig::of_fn(params, ret));
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
    module.push_str(&functions);
    module.push_str(&emit_start(main));
    module.push_str("  (export \"main\" (func $f_main))\n");
    module.push_str("  (export \"_start\" (func $_start))\n");
    module.push_str(")\n");
    Ok(module)
}

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
        captures,
        body,
        ..
    } = lambda
    else {
        unreachable!("lambda table only holds Fn nodes");
    };
    emit_fn_like(&name, params, ret, captures, body, ctx)
}

fn emit_fn_like(
    wasm_name: &str,
    params: &[IrParam],
    ret: &Type,
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

    let mut signature = format!("  (func {wasm_name} (param $p0 i32)");
    for (index, param) in params.iter().enumerate() {
        let _ = write!(
            signature,
            " (param $p{} {})",
            index + 1,
            WasmTy::of(&param.ty).keyword()
        );
    }
    let _ = writeln!(signature, " (result {})", WasmTy::of(ret).keyword());

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
    ctx: &'a Ctx<'a>,
}

impl<'a> FnEmitter<'a> {
    fn new(ctx: &'a Ctx<'a>) -> Self {
        Self {
            code: String::new(),
            locals: Vec::new(),
            scope: Vec::new(),
            counter: 0,
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
            IrExpr::Call { callee, args, .. } => self.emit_call(callee, args)?,
            IrExpr::String(value) => {
                let offset = *self.ctx.strings.offsets.get(value).ok_or_else(|| {
                    BackendError::new("internal error: string literal was not interned")
                })?;
                self.line(&format!("i32.const {offset}"));
            }
            IrExpr::Array { elem_ty, elems } => self.emit_array(elem_ty, elems)?,
            IrExpr::FunctionRef { name, .. } => self.emit_function_ref(name)?,
            IrExpr::Fn { captures, .. } => self.emit_closure(expr, captures)?,
        }
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
        (WasmTy::I32, BinaryOp::Eq) => "i32.eq",
        (WasmTy::I32, BinaryOp::Lt) => "i32.lt_s",
        (WasmTy::F64, BinaryOp::Add) => "f64.add",
        (WasmTy::F64, BinaryOp::Sub) => "f64.sub",
        (WasmTy::F64, BinaryOp::Mul) => "f64.mul",
        (WasmTy::F64, BinaryOp::Eq) => "f64.eq",
        (WasmTy::F64, BinaryOp::Lt) => "f64.lt",
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
            effects: EffectRow::default(),
        }
    }

    fn main_returning(ret: Type, body: IrExpr) -> IrProgram {
        IrProgram {
            functions: vec![IrFunction {
                name: "main".into(),
                params: vec![],
                ret,
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
            effects: EffectRow::default(),
            body: lambda,
        };
        let main = IrFunction {
            name: "main".into(),
            params: vec![],
            ret: Type::Int,
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
