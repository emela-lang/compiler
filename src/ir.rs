use std::collections::HashMap;

use crate::ast::{BinaryOp, EffectRow, Expr, Program, Type};
use crate::typecheck::TypedProgram;

#[derive(Debug, Clone)]
pub(crate) struct IrProgram {
    pub(crate) functions: Vec<IrFunction>,
}

#[derive(Debug, Clone)]
pub(crate) struct IrFunction {
    pub(crate) name: String,
    pub(crate) params: Vec<String>,
    pub(crate) ret: Type,
    pub(crate) effects: EffectRow,
    pub(crate) body: IrExpr,
}

#[derive(Debug, Clone)]
pub(crate) enum IrExpr {
    Int(i32),
    Float(f64),
    Bool(bool),
    String(String),
    Array(Vec<IrExpr>),
    Unit,
    Var(String),
    Let {
        name: String,
        value: Box<IrExpr>,
        next: Box<IrExpr>,
    },
    Call {
        name: String,
        args: Vec<IrExpr>,
    },
    Binary {
        op: BinaryOp,
        ty: Type,
        left: Box<IrExpr>,
        right: Box<IrExpr>,
    },
}

pub(crate) fn lower(program: &Program, typed: &TypedProgram) -> IrProgram {
    let signatures: HashMap<_, _> = typed
        .functions
        .iter()
        .map(|function| (function.name.clone(), function.ret.clone()))
        .collect();
    let functions = program
        .functions
        .iter()
        .zip(typed.functions.iter())
        .map(|(function, typed)| IrFunction {
            name: function.name.clone(),
            params: function
                .params
                .iter()
                .map(|param| param.name.clone())
                .collect(),
            ret: typed.ret.clone(),
            effects: typed.effects.clone(),
            body: lower_function_body(function, &signatures),
        })
        .collect();
    IrProgram { functions }
}

pub(crate) fn emit_text(program: &IrProgram) -> String {
    let mut out = String::new();
    for function in &program.functions {
        out.push_str("fn ");
        out.push_str(&function.name);
        out.push('(');
        out.push_str(&function.params.join(", "));
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
        IrExpr::Let { name, value, next } => {
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

fn inline_expr(expr: &IrExpr) -> String {
    match expr {
        IrExpr::Int(value) => value.to_string(),
        IrExpr::Float(value) => value.to_string(),
        IrExpr::Bool(value) => value.to_string(),
        IrExpr::String(value) => format!("{value:?}"),
        IrExpr::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(inline_expr)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        IrExpr::Unit => "()".to_string(),
        IrExpr::Var(name) => format!("%{name}"),
        IrExpr::Let { .. } => {
            let mut out = String::from("{\n");
            emit_expr_text(expr, 1, &mut out);
            out.push('}');
            out
        }
        IrExpr::Call { name, args } => format!(
            "call @{}({})",
            name,
            args.iter().map(inline_expr).collect::<Vec<_>>().join(", ")
        ),
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
    }
}

fn ir_op(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "add",
        BinaryOp::Sub => "sub",
        BinaryOp::Mul => "mul",
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
        Type::Array(element) => format!("Array<{}>", type_name(element)),
        Type::Record => "Record".to_string(),
        Type::Enum => "Enum".to_string(),
        Type::Function => "Function".to_string(),
    }
}

fn lower_function_body(
    function: &crate::ast::Function,
    signatures: &HashMap<String, Type>,
) -> IrExpr {
    let mut scope = function
        .params
        .iter()
        .map(|param| (param.name.clone(), param.ty.clone()))
        .collect();
    lower_block(&function.body.items, &mut scope, signatures).0
}

fn lower_block(
    items: &[crate::ast::BlockItem],
    scope: &mut HashMap<String, Type>,
    signatures: &HashMap<String, Type>,
) -> (IrExpr, Type) {
    match items.split_first() {
        None => (IrExpr::Unit, Type::Unit),
        Some((crate::ast::BlockItem::Expr(expr), [])) => lower_expr(expr, scope, signatures),
        Some((crate::ast::BlockItem::Expr(_), rest)) => lower_block(rest, scope, signatures),
        Some((
            crate::ast::BlockItem::Let {
                name, ty, value, ..
            },
            rest,
        )) => {
            let (value, value_ty) = lower_expr(value, scope, signatures);
            scope.insert(name.clone(), ty.clone().unwrap_or(value_ty));
            let (next, next_ty) = lower_block(rest, scope, signatures);
            (
                IrExpr::Let {
                    name: name.clone(),
                    value: Box::new(value),
                    next: Box::new(next),
                },
                next_ty,
            )
        }
    }
}

fn lower_expr(
    expr: &Expr,
    scope: &mut HashMap<String, Type>,
    signatures: &HashMap<String, Type>,
) -> (IrExpr, Type) {
    match expr {
        Expr::Int(value, _) => (IrExpr::Int(*value), Type::Int),
        Expr::Float(value, _) => (IrExpr::Float(*value), Type::Float),
        Expr::Bool(value, _) => (IrExpr::Bool(*value), Type::Bool),
        Expr::String(value, _) => (IrExpr::String(value.clone()), Type::String),
        Expr::Array(values, _) => {
            let lowered = values
                .iter()
                .map(|value| lower_expr(value, scope, signatures))
                .collect::<Vec<_>>();
            let ty = lowered
                .first()
                .map(|(_, ty)| Type::Array(Box::new(ty.clone())))
                .unwrap_or_else(|| Type::Array(Box::new(Type::Unit)));
            (
                IrExpr::Array(lowered.into_iter().map(|(expr, _)| expr).collect()),
                ty,
            )
        }
        Expr::Unit(_) => (IrExpr::Unit, Type::Unit),
        Expr::Var(name, _) => (
            IrExpr::Var(name.clone()),
            scope.get(name).cloned().unwrap_or(Type::Unit),
        ),
        Expr::Call { name, args, .. } => (
            IrExpr::Call {
                name: name.clone(),
                args: args
                    .iter()
                    .map(|arg| lower_expr(arg, scope, signatures).0)
                    .collect(),
            },
            signatures.get(name).cloned().unwrap_or(Type::Unit),
        ),
        Expr::Binary {
            op, left, right, ..
        } => {
            let (left, left_ty) = lower_expr(left, scope, signatures);
            let (right, _) = lower_expr(right, scope, signatures);
            let result_ty = match op {
                BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul => left_ty.clone(),
                BinaryOp::Eq | BinaryOp::Lt => Type::Bool,
            };
            (
                IrExpr::Binary {
                    op: *op,
                    ty: left_ty,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                result_ty,
            )
        }
        Expr::Block(block) => lower_block(&block.items, &mut scope.clone(), signatures),
    }
}
