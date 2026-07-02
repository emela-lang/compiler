//! A textual dump of the IR, used by `emela ir`.

use crate::ir::{IrArm, IrExpr, IrParam, IrPattern, IrProgram};
use crate::types::{BinaryOp, Type};

pub fn emit_text(program: &IrProgram) -> String {
    let mut out = String::new();
    for function in &program.functions {
        out.push_str("fn ");
        out.push_str(&function.name);
        out.push('(');
        out.push_str(&param_names(&function.params));
        out.push_str(") -> ");
        out.push_str(&type_name(&function.ret));
        out.push_str(" uses {");
        out.push_str(&function.effects.effects.join(", "));
        out.push_str("} {\n");
        emit_expr_text(&function.body, 1, &mut out);
        out.push_str("}\n\n");
    }
    out
}

fn emit_expr_text(expr: &IrExpr, indent: usize, out: &mut String) {
    let pad = "  ".repeat(indent);
    match expr {
        IrExpr::Let {
            name, value, next, ..
        } => {
            out.push_str(&pad);
            out.push_str("let ");
            out.push_str(name);
            out.push_str(" = ");
            out.push_str(&inline_expr(value));
            out.push('\n');
            emit_expr_text(next, indent, out);
        }
        other => {
            out.push_str(&pad);
            out.push_str("return ");
            out.push_str(&inline_expr(other));
            out.push('\n');
        }
    }
}

fn param_names(params: &[IrParam]) -> String {
    params
        .iter()
        .map(|param| param.name.clone())
        .collect::<Vec<_>>()
        .join(", ")
}

fn inline_expr(expr: &IrExpr) -> String {
    match expr {
        IrExpr::Int(value) => value.to_string(),
        IrExpr::Float(value) => value.to_string(),
        IrExpr::Bool(value) => value.to_string(),
        IrExpr::String(value) => format!("{value:?}"),
        IrExpr::Char(value) => format!("char {value}"),
        IrExpr::CharFromCode(value) => format!("char_from_code {}", inline_expr(value)),
        IrExpr::StringFromChar(value) => format!("string_from_char {}", inline_expr(value)),
        IrExpr::Concat { left, right } => {
            format!("concat {}, {}", inline_expr(left), inline_expr(right))
        }
        IrExpr::Array { elems, .. } => format!(
            "[{}]",
            elems.iter().map(inline_expr).collect::<Vec<_>>().join(", ")
        ),
        IrExpr::Unit => "()".to_string(),
        IrExpr::Var { name, .. } => format!("%{name}"),
        IrExpr::FunctionRef { name, .. } => format!("@{name}"),
        IrExpr::Let { .. } => {
            let mut out = String::from("{\n");
            emit_expr_text(expr, 1, &mut out);
            out.push('}');
            out
        }
        IrExpr::Call { callee, args, .. } => format!(
            "call {}({})",
            inline_callee(callee),
            args.iter().map(inline_expr).collect::<Vec<_>>().join(", ")
        ),
        IrExpr::Platform { name, args, .. } => format!(
            "platform {}({})",
            name,
            args.iter().map(inline_expr).collect::<Vec<_>>().join(", ")
        ),
        IrExpr::Fn { params, body, .. } => {
            format!("fn ({}) {{ {} }}", param_names(params), inline_expr(body))
        }
        IrExpr::Binary {
            op,
            ty,
            left,
            right,
        } => format!(
            "{}.{} {}, {}",
            ir_op(*op),
            ir_type_suffix(ty),
            inline_expr(left),
            inline_expr(right)
        ),
        IrExpr::EnumValue {
            variant, payload, ..
        } => {
            if payload.is_empty() {
                variant.clone()
            } else {
                format!(
                    "{variant}({})",
                    payload
                        .iter()
                        .map(inline_expr)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        }
        IrExpr::Match {
            scrutinee, arms, ..
        } => format!(
            "match {} {{ {} }}",
            inline_expr(scrutinee),
            arms.iter().map(inline_arm).collect::<Vec<_>>().join(" ")
        ),
        IrExpr::If {
            cond, then, els, ..
        } => format!(
            "if {} {{ {} }} else {{ {} }}",
            inline_expr(cond),
            inline_expr(then),
            inline_expr(els)
        ),
        IrExpr::Throw { value } => format!("throw {}", inline_expr(value)),
        IrExpr::Try { body, arms, .. } => format!(
            "try {{ {} }} catch {{ {} }}",
            inline_expr(body),
            arms.iter().map(inline_arm).collect::<Vec<_>>().join(" ")
        ),
        IrExpr::Question { value, .. } => format!("{}?", inline_expr(value)),
        IrExpr::Panic { message } => format!("panic {}", inline_expr(message)),
    }
}

fn inline_arm(arm: &IrArm) -> String {
    let pattern = match &arm.pattern {
        IrPattern::Variant {
            variant, bindings, ..
        } => {
            if bindings.is_empty() {
                variant.clone()
            } else {
                let names = bindings
                    .iter()
                    .map(|binding| binding.as_ref().map_or("_", |(name, _)| name.as_str()))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{variant}({names})")
            }
        }
        IrPattern::Wildcard { binding } => binding
            .as_ref()
            .map_or_else(|| "_".to_string(), |(name, _)| name.clone()),
    };
    let guard = arm
        .guard
        .as_ref()
        .map_or_else(String::new, |guard| format!(" if {}", inline_expr(guard)));
    format!("{pattern}{guard} -> {}", inline_expr(&arm.body))
}

fn ir_op(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "add",
        BinaryOp::Sub => "sub",
        BinaryOp::Mul => "mul",
        BinaryOp::Div => "div",
        BinaryOp::Rem => "rem",
        BinaryOp::Concat => "concat",
        BinaryOp::Eq => "eq",
        BinaryOp::Lt => "lt",
    }
}

fn ir_type_suffix(ty: &Type) -> &'static str {
    match ty {
        Type::Float => "f64",
        _ => "i32",
    }
}

fn type_name(ty: &Type) -> String {
    match ty {
        Type::Unit => "Unit".to_string(),
        Type::Bool => "Bool".to_string(),
        Type::Int => "Int".to_string(),
        Type::Float => "Float".to_string(),
        Type::String => "String".to_string(),
        Type::Char => "Char".to_string(),
        Type::Array(element) => format!("Array<{}>", type_name(element)),
        Type::Record => "Record".to_string(),
        Type::Enum(name) => name.clone(),
        Type::Option(inner) => format!("Option<{}>", type_name(inner)),
        Type::Never => "Never".to_string(),
        Type::Function(function) => format!(
            "({}) -> {} uses {{{}}}",
            function
                .params
                .iter()
                .map(type_name)
                .collect::<Vec<_>>()
                .join(", "),
            type_name(&function.ret),
            function.effects.effects.join(", ")
        ),
        Type::OpaqueFunction => "Function".to_string(),
        // A type parameter (spec 0014). Monomorphization removes it before
        // lowering, so seeing one in the IR dump would be a bug; render the name.
        Type::Var(name) => name.clone(),
    }
}

fn inline_callee(expr: &IrExpr) -> String {
    match expr {
        IrExpr::FunctionRef { name, .. } => format!("@{name}"),
        other => inline_expr(other),
    }
}
