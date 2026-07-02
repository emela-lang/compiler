//! JavaScript backend (Tier 2).
//!
//! Emits a `"use strict"` module of plain functions, relying on JavaScript's
//! lexical scoping for closures, and logs `main()`'s result unless it is Unit.
//!
//! Platform functions (spec 0013) are resolved by a bundled default runtime
//! object `__rt`; only the platform functions the program uses are emitted.

use std::collections::HashSet;

use emela_codegen::{
    Artifact, ArtifactKind, Backend, BackendError, BackendOptions, BinaryOp, IrArm, IrExpr,
    IrPattern, IrProgram, QuestionMode, Result, Tier, Type,
};

/// The Node.js-flavored JavaScript backend.
pub struct JsBackend;

impl Backend for JsBackend {
    fn name(&self) -> &str {
        "js-node"
    }

    fn tier(&self) -> Tier {
        Tier::Tier2
    }

    fn compile(&self, ir: &IrProgram, _options: &BackendOptions) -> Result<Artifact> {
        let used = used_platform_fns(ir);
        for name in &used {
            if runtime_impl(name).is_none() {
                return Err(BackendError::new(format!(
                    "backend `js-node` does not provide platform function `{name}`"
                )));
            }
        }
        Ok(Artifact::text(ArtifactKind::JsSource, emit(ir, &used)))
    }
}

/// The platform functions this backend provides, with their JS implementations.
fn runtime_impl(name: &str) -> Option<&'static str> {
    match name {
        "io.write_stdout" => Some("(s) => process.stdout.write(s)"),
        "io.write_stderr" => Some("(s) => process.stderr.write(s)"),
        "clock.monotonic_seconds" => Some("() => Math.floor(Date.now() / 1000)"),
        _ => None,
    }
}

/// The platform functions the program references, in first-occurrence order.
fn used_platform_fns(program: &IrProgram) -> Vec<String> {
    let mut order = Vec::new();
    let mut seen = HashSet::new();
    for function in &program.functions {
        collect_platform(&function.body, &mut order, &mut seen);
    }
    order
}

fn collect_platform(expr: &IrExpr, order: &mut Vec<String>, seen: &mut HashSet<String>) {
    match expr {
        IrExpr::Platform { name, args, .. } => {
            if seen.insert(name.clone()) {
                order.push(name.clone());
            }
            args.iter().for_each(|a| collect_platform(a, order, seen));
        }
        IrExpr::Array { elems, .. } => elems.iter().for_each(|e| collect_platform(e, order, seen)),
        IrExpr::Let { value, next, .. } => {
            collect_platform(value, order, seen);
            collect_platform(next, order, seen);
        }
        IrExpr::Call { callee, args, .. } => {
            collect_platform(callee, order, seen);
            args.iter().for_each(|a| collect_platform(a, order, seen));
        }
        IrExpr::Fn { body, .. } => collect_platform(body, order, seen),
        IrExpr::Binary { left, right, .. } => {
            collect_platform(left, order, seen);
            collect_platform(right, order, seen);
        }
        IrExpr::EnumValue { payload, .. } => {
            payload
                .iter()
                .for_each(|e| collect_platform(e, order, seen));
        }
        IrExpr::Match {
            scrutinee, arms, ..
        } => {
            collect_platform(scrutinee, order, seen);
            collect_arms(arms, order, seen);
        }
        IrExpr::Try { body, arms, .. } => {
            collect_platform(body, order, seen);
            collect_arms(arms, order, seen);
        }
        IrExpr::Throw { value } | IrExpr::Question { value, .. } => {
            collect_platform(value, order, seen);
        }
        IrExpr::Panic { message } => collect_platform(message, order, seen),
        IrExpr::If {
            cond, then, els, ..
        } => {
            collect_platform(cond, order, seen);
            collect_platform(then, order, seen);
            collect_platform(els, order, seen);
        }
        IrExpr::CharFromCode(value) | IrExpr::StringFromChar(value) => {
            collect_platform(value, order, seen);
        }
        IrExpr::Concat { left, right } => {
            collect_platform(left, order, seen);
            collect_platform(right, order, seen);
        }
        _ => {}
    }
}

fn collect_arms(arms: &[IrArm], order: &mut Vec<String>, seen: &mut HashSet<String>) {
    for arm in arms {
        if let Some(guard) = &arm.guard {
            collect_platform(guard, order, seen);
        }
        collect_platform(&arm.body, order, seen);
    }
}

fn emit(program: &IrProgram, used_platform: &[String]) -> String {
    let mut out = String::new();
    out.push_str("\"use strict\";\n\n");
    // Error-handling runtime (spec 0011): a thrown error carries its value, a
    // propagated `None` is its own signal, and a panic is distinct so `catch`
    // never swallows it.
    out.push_str("class EmelaError { constructor(value) { this.value = value; } }\n");
    out.push_str("class EmelaNone {}\n");
    out.push_str("class EmelaPanic { constructor(message) { this.message = message; } }\n\n");
    if !used_platform.is_empty() {
        // Bundled default runtime: the backend supplies the platform bodies.
        out.push_str("const __rt = {\n");
        for name in used_platform {
            if let Some(body) = runtime_impl(name) {
                out.push_str(&format!("  {name:?}: {body},\n"));
            }
        }
        out.push_str("};\n\n");
    }
    for function in &program.functions {
        if !function.effects.effects.is_empty() {
            out.push_str(&format!(
                "// uses {{{}}}\n",
                function.effects.effects.join(", ")
            ));
        }
        out.push_str(&format!(
            "function {}({}) {{\n",
            js_name(&function.name),
            function
                .params
                .iter()
                .map(|param| js_name(&param.name))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        if matches!(function.ret, Type::Option(_)) {
            // A function returning Option catches a propagated `None` (`?`).
            out.push_str("  try { return ");
            out.push_str(&emit_expr(&function.body));
            out.push_str("; } catch (__e) { if (__e instanceof EmelaNone) return { tag: 1, values: [] }; throw __e; }\n}\n\n");
        } else {
            out.push_str("  return ");
            out.push_str(&emit_expr(&function.body));
            out.push_str(";\n}\n\n");
        }
    }
    let main_ret = program
        .functions
        .iter()
        .find(|function| function.name == "main")
        .map(|function| &function.ret);
    out.push_str("try {\n");
    out.push_str("  const __emela_result = main();\n");
    if !matches!(main_ret, Some(Type::Unit)) {
        out.push_str("  if (__emela_result !== undefined) console.log(__emela_result);\n");
    }
    out.push_str("} catch (__e) {\n");
    out.push_str(
        "  if (__e instanceof EmelaPanic) { console.error(\"panic: \" + __e.message); process.exit(1); }\n",
    );
    out.push_str("  throw __e;\n");
    out.push_str("}\n");
    out
}

fn emit_expr(expr: &IrExpr) -> String {
    match expr {
        IrExpr::Int(value) => value.to_string(),
        IrExpr::Float(value) => value.to_string(),
        IrExpr::Bool(value) => value.to_string(),
        IrExpr::String(value) => format!("{value:?}"),
        // A `Char` is its Unicode scalar value as a number (spec 0017).
        IrExpr::Char(value) => value.to_string(),
        IrExpr::CharFromCode(value) => emit_expr(value),
        IrExpr::StringFromChar(value) => format!("String.fromCodePoint({})", emit_expr(value)),
        IrExpr::Concat { left, right } => format!("({} + {})", emit_expr(left), emit_expr(right)),
        IrExpr::Array { elems, .. } => format!(
            "[{}]",
            elems.iter().map(emit_expr).collect::<Vec<_>>().join(", ")
        ),
        IrExpr::Unit => "undefined".to_string(),
        IrExpr::Var { name, .. } => js_name(name),
        IrExpr::FunctionRef { name, .. } => js_name(name),
        IrExpr::Let {
            name, value, next, ..
        } => format!(
            "(() => {{ const {} = {}; return {}; }})()",
            js_name(name),
            emit_expr(value),
            emit_expr(next)
        ),
        IrExpr::Call { callee, args, .. } => format!(
            "{}({})",
            emit_expr(callee),
            args.iter().map(emit_expr).collect::<Vec<_>>().join(", ")
        ),
        IrExpr::Platform { name, args, .. } => format!(
            "__rt[{name:?}]({})",
            args.iter().map(emit_expr).collect::<Vec<_>>().join(", ")
        ),
        IrExpr::Fn {
            params, body, ret, ..
        } => {
            let params = params
                .iter()
                .map(|param| js_name(&param.name))
                .collect::<Vec<_>>()
                .join(", ");
            if matches!(ret, Type::Option(_)) {
                format!(
                    "function({params}) {{ try {{ return {}; }} catch (__e) {{ if (__e instanceof EmelaNone) return {{ tag: 1, values: [] }}; throw __e; }} }}",
                    emit_expr(body)
                )
            } else {
                format!("function({params}) {{ return {}; }}", emit_expr(body))
            }
        }
        IrExpr::Binary {
            op,
            ty,
            left,
            right,
        } => {
            let a = emit_expr(left);
            let b = emit_expr(right);
            match op {
                BinaryOp::Add => format!("({a} + {b})"),
                BinaryOp::Sub => format!("({a} - {b})"),
                BinaryOp::Mul => format!("({a} * {b})"),
                // Integer division truncates toward zero (spec 0016).
                BinaryOp::Div if *ty == Type::Int => format!("(({a} / {b}) | 0)"),
                BinaryOp::Div => format!("({a} / {b})"),
                BinaryOp::Rem => format!("({a} % {b})"),
                // `++` lowers to `IrExpr::Concat`, never to a Binary.
                BinaryOp::Concat => unreachable!("concat lowers to IrExpr::Concat"),
                BinaryOp::Eq => format!("({a} === {b})"),
                BinaryOp::Lt => format!("({a} < {b})"),
            }
        }
        IrExpr::EnumValue { tag, payload, .. } => format!(
            "{{ tag: {tag}, values: [{}] }}",
            payload.iter().map(emit_expr).collect::<Vec<_>>().join(", ")
        ),
        IrExpr::Match {
            scrutinee, arms, ..
        } => emit_match(scrutinee, arms),
        IrExpr::If {
            cond, then, els, ..
        } => format!(
            "({} ? ({}) : ({}))",
            emit_expr(cond),
            emit_expr(then),
            emit_expr(els)
        ),
        IrExpr::Throw { value } => {
            format!(
                "(() => {{ throw new EmelaError({}); }})()",
                emit_expr(value)
            )
        }
        IrExpr::Try { body, arms, .. } => emit_try(body, arms),
        IrExpr::Question { value, mode, .. } => match mode {
            // A thrown error propagates as a native exception, so `?` is a
            // no-op on the value channel.
            QuestionMode::Throws => emit_expr(value),
            QuestionMode::Option => format!(
                "(() => {{ const __o = {}; if (__o.tag === 1) throw new EmelaNone(); return __o.values[0]; }})()",
                emit_expr(value)
            ),
        },
        IrExpr::Panic { message } => {
            format!(
                "(() => {{ throw new EmelaPanic({}); }})()",
                emit_expr(message)
            )
        }
    }
}

/// Lowers a `match` to an IIFE that dispatches on the enum/Option tag.
fn emit_match(scrutinee: &IrExpr, arms: &[IrArm]) -> String {
    let mut out = format!("(() => {{ const __m = {}; ", emit_expr(scrutinee));
    for arm in arms {
        out.push_str(&emit_arm("__m", arm));
    }
    out.push_str("throw new Error(\"non-exhaustive match\"); })()");
    out
}

/// Lowers a `try`/`catch` to an IIFE; thrown `EmelaError`s route to the arms
/// while panics (and other exceptions) propagate.
fn emit_try(body: &IrExpr, arms: &[IrArm]) -> String {
    let mut out = format!(
        "(() => {{ try {{ return {}; }} catch (__e) {{ if (!(__e instanceof EmelaError)) throw __e; const __err = __e.value; ",
        emit_expr(body)
    );
    for arm in arms {
        out.push_str(&emit_arm("__err", arm));
    }
    out.push_str("throw __e; } })()");
    out
}

/// Emits one `match`/`catch` arm testing `subject`.
fn emit_arm(subject: &str, arm: &IrArm) -> String {
    let body = emit_expr(&arm.body);
    let guard = arm
        .guard
        .as_ref()
        .map(|guard| format!("if ({}) ", emit_expr(guard)))
        .unwrap_or_default();
    match &arm.pattern {
        IrPattern::Variant { tag, bindings, .. } => {
            let mut binds = String::new();
            for (index, binding) in bindings.iter().enumerate() {
                if let Some((name, _)) = binding {
                    binds.push_str(&format!(
                        "const {} = {subject}.values[{index}]; ",
                        js_name(name)
                    ));
                }
            }
            format!("if ({subject}.tag === {tag}) {{ {binds}{guard}return {body}; }} ")
        }
        IrPattern::Wildcard { binding } => {
            let bind = binding
                .as_ref()
                .map(|(name, _)| format!("const {} = {subject}; ", js_name(name)))
                .unwrap_or_default();
            format!("{{ {bind}{guard}return {body}; }} ")
        }
    }
}

fn js_name(name: &str) -> String {
    if name == "main" {
        return "main".to_string();
    }
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use emela_codegen::{EffectRow, IrFunction};

    fn main_with(body: IrExpr) -> IrProgram {
        IrProgram {
            functions: vec![IrFunction {
                name: "main".into(),
                params: vec![],
                ret: Type::Unit,
                throws: None,
                effects: EffectRow::sorted(vec!["io".into()]),
                body,
            }],
        }
    }

    fn platform_call(name: &str) -> IrExpr {
        IrExpr::Platform {
            name: name.into(),
            args: vec![IrExpr::String("hi".into())],
            ret: Type::Unit,
        }
    }

    #[test]
    fn bundles_runtime_for_used_platform_fns() {
        let artifact = JsBackend
            .compile(
                &main_with(platform_call("io.write_stdout")),
                &BackendOptions::default(),
            )
            .expect("compile");
        let js = String::from_utf8(artifact.bytes).unwrap();
        assert!(js.contains("const __rt = {"), "{js}");
        assert!(
            js.contains("\"io.write_stdout\": (s) => process.stdout.write(s)"),
            "{js}"
        );
        assert!(js.contains("__rt[\"io.write_stdout\"](\"hi\")"), "{js}");
        // An unused platform function is not bundled.
        assert!(!js.contains("write_stderr"), "{js}");
    }

    #[test]
    fn rejects_unprovided_platform_fn() {
        let err = JsBackend
            .compile(
                &main_with(platform_call("fs.read")),
                &BackendOptions::default(),
            )
            .unwrap_err();
        assert!(err.to_string().contains("does not provide"), "{err}");
        assert!(err.to_string().contains("fs.read"), "{err}");
    }
}
