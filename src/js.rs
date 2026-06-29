use crate::ast::{BinaryOp, Type};
use crate::ir::{IrExpr, IrProgram};

pub(crate) fn emit(program: &IrProgram) -> String {
    let mut out = String::new();
    out.push_str("\"use strict\";\n\n");
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
                .map(|name| js_name(name))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        out.push_str("  return ");
        out.push_str(&emit_expr(&function.body));
        out.push_str(";\n}\n\n");
    }
    let main_ret = program
        .functions
        .iter()
        .find(|function| function.name == "main")
        .map(|function| &function.ret);
    out.push_str("const __emela_result = main();\n");
    if !matches!(main_ret, Some(Type::Unit)) {
        out.push_str("if (__emela_result !== undefined) console.log(__emela_result);\n");
    }
    out
}

fn emit_expr(expr: &IrExpr) -> String {
    match expr {
        IrExpr::Int(value) => value.to_string(),
        IrExpr::Float(value) => value.to_string(),
        IrExpr::Bool(value) => value.to_string(),
        IrExpr::String(value) => format!("{value:?}"),
        IrExpr::Array(values) => format!(
            "[{}]",
            values.iter().map(emit_expr).collect::<Vec<_>>().join(", ")
        ),
        IrExpr::Unit => "undefined".to_string(),
        IrExpr::Var(name) => js_name(name),
        IrExpr::Let { name, value, next } => format!(
            "(() => {{ const {} = {}; return {}; }})()",
            js_name(name),
            emit_expr(value),
            emit_expr(next)
        ),
        IrExpr::Call { name, args } => format!(
            "{}({})",
            js_name(name),
            args.iter().map(emit_expr).collect::<Vec<_>>().join(", ")
        ),
        IrExpr::Binary {
            op, left, right, ..
        } => {
            let op = match op {
                BinaryOp::Add => "+",
                BinaryOp::Sub => "-",
                BinaryOp::Mul => "*",
                BinaryOp::Eq => "===",
                BinaryOp::Lt => "<",
            };
            format!("({} {} {})", emit_expr(left), op, emit_expr(right))
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
