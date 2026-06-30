//! Lowering: the typed AST -> the `emela-codegen` IR.
//!
//! The IR is fully typed, so every node records the type that the type checker
//! already computed. Lambdas additionally record their captured variables, in
//! a stable order, for closure-converting backends. Calls to `extern fn`
//! platform functions (spec 0013) become `IrExpr::Platform` nodes. Enums,
//! `match`, and the error-handling forms (spec 0005/0011) lower to the IR's
//! `EnumValue`/`Match`/`Throw`/`Try`/`Question`/`Panic` nodes.

use std::collections::{HashMap, HashSet};

use emela_codegen::{
    BinaryOp, FunctionType, IrArm, IrCapture, IrExpr, IrFunction, IrParam, IrPattern, IrProgram,
    QuestionMode, Type,
};

use crate::ast::{Block, BlockItem, Expr, FieldBinding, MatchArm, Pattern, Program};
use crate::typecheck::TypedProgram;

type Scope = HashMap<String, Type>;

/// A platform function in scope: its canonical name and return type.
struct ExternInfo {
    canonical: String,
    ret: Type,
}

/// One variant of a declared enum, with its tag (declaration order) and fields.
struct VariantDef {
    name: String,
    tag: u32,
    fields: Vec<Type>,
}

struct Lowerer {
    function_types: HashMap<String, FunctionType>,
    externs: HashMap<String, ExternInfo>,
    enums: HashMap<String, Vec<VariantDef>>,
}

pub(crate) fn lower(program: &Program, typed: &TypedProgram) -> IrProgram {
    let function_types: HashMap<String, FunctionType> = typed
        .functions
        .iter()
        .map(|function| {
            (
                function.name.clone(),
                FunctionType {
                    params: function.params.clone(),
                    ret: Box::new(function.ret.clone()),
                    throws: function.throws.clone().map(Box::new),
                    effects: function.effects.clone(),
                },
            )
        })
        .collect();
    let externs: HashMap<String, ExternInfo> = program
        .externs
        .iter()
        .map(|declaration| {
            (
                declaration.name.clone(),
                ExternInfo {
                    canonical: declaration.canonical(),
                    ret: declaration.ret.clone(),
                },
            )
        })
        .collect();
    let enums = program
        .enums
        .iter()
        .map(|decl| {
            let variants = decl
                .variants
                .iter()
                .enumerate()
                .map(|(tag, variant)| VariantDef {
                    name: variant.name.clone(),
                    tag: tag as u32,
                    fields: variant.fields.clone(),
                })
                .collect();
            (decl.name.clone(), variants)
        })
        .collect();
    let lowerer = Lowerer {
        function_types,
        externs,
        enums,
    };

    let functions = program
        .functions
        .iter()
        .zip(typed.functions.iter())
        .map(|(function, typed)| {
            let mut scope: Scope = function
                .params
                .iter()
                .map(|param| (param.name.clone(), param.ty.clone()))
                .collect();
            IrFunction {
                name: function.name.clone(),
                params: function
                    .params
                    .iter()
                    .map(|param| IrParam {
                        name: param.name.clone(),
                        ty: param.ty.clone(),
                    })
                    .collect(),
                ret: typed.ret.clone(),
                throws: typed.throws.clone(),
                effects: typed.effects.clone(),
                body: lowerer.lower_block(&function.body.items, &mut scope).0,
            }
        })
        .collect();
    IrProgram { functions }
}

impl Lowerer {
    fn lower_block(&self, items: &[BlockItem], scope: &mut Scope) -> (IrExpr, Type) {
        match items.split_first() {
            None => (IrExpr::Unit, Type::Unit),
            Some((BlockItem::Expr(expr), [])) => self.lower_expr(expr, scope),
            Some((BlockItem::Expr(_), rest)) => self.lower_block(rest, scope),
            Some((
                BlockItem::Let {
                    name, ty, value, ..
                },
                rest,
            )) => {
                let expected_elem = match (value, ty) {
                    (Expr::Array(_, _), Some(Type::Array(element))) => Some(element.as_ref()),
                    _ => None,
                };
                let (value, inferred) = match value {
                    Expr::Array(elements, _) => self.lower_array(elements, scope, expected_elem),
                    _ => self.lower_expr(value, scope),
                };
                let value_ty = ty.clone().unwrap_or(inferred);
                scope.insert(name.clone(), value_ty.clone());
                let (next, next_ty) = self.lower_block(rest, scope);
                (
                    IrExpr::Let {
                        name: name.clone(),
                        value_ty,
                        value: Box::new(value),
                        next: Box::new(next),
                    },
                    next_ty,
                )
            }
        }
    }

    fn lower_array(
        &self,
        elements: &[Expr],
        scope: &mut Scope,
        expected_elem: Option<&Type>,
    ) -> (IrExpr, Type) {
        let lowered = elements
            .iter()
            .map(|element| self.lower_expr(element, scope))
            .collect::<Vec<_>>();
        let elem_ty = lowered
            .first()
            .map(|(_, ty)| ty.clone())
            .or_else(|| expected_elem.cloned())
            .unwrap_or(Type::Unit);
        (
            IrExpr::Array {
                elem_ty: elem_ty.clone(),
                elems: lowered.into_iter().map(|(expr, _)| expr).collect(),
            },
            Type::Array(Box::new(elem_ty)),
        )
    }

    fn lower_expr(&self, expr: &Expr, scope: &mut Scope) -> (IrExpr, Type) {
        match expr {
            Expr::Int(value, _) => (IrExpr::Int(*value), Type::Int),
            Expr::Float(value, _) => (IrExpr::Float(*value), Type::Float),
            Expr::Bool(value, _) => (IrExpr::Bool(*value), Type::Bool),
            Expr::String(value, _) => (IrExpr::String(value.clone()), Type::String),
            Expr::Char(value, _) => (IrExpr::Char(*value as u32), Type::Char),
            Expr::Array(elements, _) => self.lower_array(elements, scope, None),
            Expr::Unit(_) => (IrExpr::Unit, Type::Unit),
            Expr::Var(name, _) => {
                if let Some(ty) = scope.get(name) {
                    (
                        IrExpr::Var {
                            name: name.clone(),
                            ty: ty.clone(),
                        },
                        ty.clone(),
                    )
                } else if name == "None" {
                    let ty = Type::Option(Box::new(Type::Never));
                    (
                        IrExpr::EnumValue {
                            ty: ty.clone(),
                            variant: "None".to_string(),
                            tag: 1,
                            payload: Vec::new(),
                        },
                        ty,
                    )
                } else if let Some(sig) = self.function_types.get(name) {
                    (
                        IrExpr::FunctionRef {
                            name: name.clone(),
                            sig: sig.clone(),
                        },
                        Type::Function(sig.clone()),
                    )
                } else {
                    (
                        IrExpr::Var {
                            name: name.clone(),
                            ty: Type::Unit,
                        },
                        Type::Unit,
                    )
                }
            }
            Expr::Call { callee, args, .. } => self.lower_call(callee, args, scope),
            Expr::Fn {
                params,
                ret,
                throws,
                effects,
                body,
                ..
            } => {
                let captures = self.lambda_captures(params, body, scope);
                let mut fn_scope = scope.clone();
                for param in params {
                    fn_scope.insert(param.name.clone(), param.ty.clone());
                }
                let (body, _) = self.lower_block(&body.items, &mut fn_scope);
                let ir_params: Vec<IrParam> = params
                    .iter()
                    .map(|param| IrParam {
                        name: param.name.clone(),
                        ty: param.ty.clone(),
                    })
                    .collect();
                let signature = FunctionType {
                    params: ir_params.iter().map(|param| param.ty.clone()).collect(),
                    ret: Box::new(ret.clone()),
                    throws: throws.clone().map(Box::new),
                    effects: effects.clone(),
                };
                (
                    IrExpr::Fn {
                        params: ir_params,
                        ret: ret.clone(),
                        throws: throws.clone(),
                        effects: effects.clone(),
                        captures,
                        body: Box::new(body),
                    },
                    Type::Function(signature),
                )
            }
            Expr::Binary {
                op, left, right, ..
            } => {
                let (left, left_ty) = self.lower_expr(left, scope);
                let (right, _) = self.lower_expr(right, scope);
                if matches!(op, BinaryOp::Concat) {
                    return (
                        IrExpr::Concat {
                            left: Box::new(left),
                            right: Box::new(right),
                        },
                        Type::String,
                    );
                }
                let result_ty = match op {
                    BinaryOp::Add
                    | BinaryOp::Sub
                    | BinaryOp::Mul
                    | BinaryOp::Div
                    | BinaryOp::Rem => left_ty.clone(),
                    BinaryOp::Concat => Type::String,
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
            Expr::Block(block) => self.lower_block(&block.items, &mut scope.clone()),
            Expr::If {
                cond, then, els, ..
            } => {
                let (cond_ir, _) = self.lower_expr(cond, scope);
                let (then_ir, then_ty) = self.lower_block(&then.items, &mut scope.clone());
                let (els_ir, els_ty) = self.lower_block(&els.items, &mut scope.clone());
                let ty = pick_ty([then_ty, els_ty].into_iter());
                (
                    IrExpr::If {
                        cond: Box::new(cond_ir),
                        then: Box::new(then_ir),
                        els: Box::new(els_ir),
                        ty: ty.clone(),
                    },
                    ty,
                )
            }
            Expr::Throw { value, .. } => {
                let (value, _) = self.lower_expr(value, scope);
                (
                    IrExpr::Throw {
                        value: Box::new(value),
                    },
                    Type::Never,
                )
            }
            Expr::Panic { message, .. } => {
                let (message, _) = self.lower_expr(message, scope);
                (
                    IrExpr::Panic {
                        message: Box::new(message),
                    },
                    Type::Never,
                )
            }
            Expr::Question { value, .. } => {
                let (value, value_ty) = self.lower_expr(value, scope);
                if is_throwing(&value) {
                    (
                        IrExpr::Question {
                            value: Box::new(value),
                            mode: QuestionMode::Throws,
                            ty: value_ty.clone(),
                        },
                        value_ty,
                    )
                } else if let Type::Option(inner) = &value_ty {
                    let ty = (**inner).clone();
                    (
                        IrExpr::Question {
                            value: Box::new(value),
                            mode: QuestionMode::Option,
                            ty: ty.clone(),
                        },
                        ty,
                    )
                } else {
                    (
                        IrExpr::Question {
                            value: Box::new(value),
                            mode: QuestionMode::Throws,
                            ty: value_ty.clone(),
                        },
                        value_ty,
                    )
                }
            }
            Expr::Variant {
                enum_name,
                variant,
                args,
                ..
            } => {
                // Built-in pure conversions (spec 0017).
                if enum_name.as_deref() == Some("Char") && variant == "from_code" {
                    let (arg, _) = self.lower_expr(&args[0], scope);
                    return (IrExpr::CharFromCode(Box::new(arg)), Type::Char);
                }
                if enum_name.as_deref() == Some("String") && variant == "from_char" {
                    let (arg, _) = self.lower_expr(&args[0], scope);
                    return (IrExpr::StringFromChar(Box::new(arg)), Type::String);
                }
                let name = enum_name.clone().unwrap_or_default();
                let tag = self
                    .enums
                    .get(&name)
                    .and_then(|variants| variants.iter().find(|v| v.name == *variant))
                    .map_or(0, |v| v.tag);
                let payload = args
                    .iter()
                    .map(|arg| self.lower_expr(arg, scope).0)
                    .collect();
                let ty = Type::Enum(name);
                (
                    IrExpr::EnumValue {
                        ty: ty.clone(),
                        variant: variant.clone(),
                        tag,
                        payload,
                    },
                    ty,
                )
            }
            Expr::Match {
                scrutinee, arms, ..
            } => {
                let (scrutinee_ir, scrutinee_ty) = self.lower_expr(scrutinee, scope);
                let variants = self.variants_of(&scrutinee_ty);
                let ir_arms: Vec<IrArm> = arms
                    .iter()
                    .map(|arm| self.lower_arm(arm, &scrutinee_ty, &variants, scope))
                    .collect();
                let ty = pick_ty(ir_arms.iter().map(|arm| arm.body.ty()));
                (
                    IrExpr::Match {
                        scrutinee: Box::new(scrutinee_ir),
                        arms: ir_arms,
                        ty: ty.clone(),
                    },
                    ty,
                )
            }
            Expr::Try { body, arms, .. } => {
                let (body_ir, body_ty) = self.lower_block(&body.items, &mut scope.clone());
                let error_ty = body_error_ty(&body_ir).unwrap_or(Type::Never);
                let variants = self.variants_of(&error_ty);
                let ir_arms: Vec<IrArm> = arms
                    .iter()
                    .map(|arm| self.lower_arm(arm, &error_ty, &variants, scope))
                    .collect();
                let ty = pick_ty(
                    std::iter::once(body_ty).chain(ir_arms.iter().map(|arm| arm.body.ty())),
                );
                (
                    IrExpr::Try {
                        body: Box::new(body_ir),
                        arms: ir_arms,
                        ty: ty.clone(),
                    },
                    ty,
                )
            }
        }
    }

    fn lower_call(&self, callee: &Expr, args: &[Expr], scope: &mut Scope) -> (IrExpr, Type) {
        if let Expr::Var(name, _) = callee {
            // Built-in Option constructor `Some(x)`.
            if name == "Some"
                && !scope.contains_key(name)
                && !self.function_types.contains_key(name)
            {
                let (arg_ir, arg_ty) = self.lower_expr(&args[0], scope);
                let ty = Type::Option(Box::new(arg_ty));
                return (
                    IrExpr::EnumValue {
                        ty: ty.clone(),
                        variant: "Some".to_string(),
                        tag: 0,
                        payload: vec![arg_ir],
                    },
                    ty,
                );
            }
            // A call to a platform function (extern) lowers to a Platform node.
            if let Some(info) = self.externs.get(name) {
                let ret = info.ret.clone();
                let args = args
                    .iter()
                    .map(|arg| self.lower_expr(arg, scope).0)
                    .collect();
                return (
                    IrExpr::Platform {
                        name: info.canonical.clone(),
                        args,
                        ret: ret.clone(),
                    },
                    ret,
                );
            }
        }
        let (callee, callee_ty) = self.lower_expr(callee, scope);
        let ret = match callee_ty {
            Type::Function(function) => (*function.ret).clone(),
            _ => Type::Unit,
        };
        (
            IrExpr::Call {
                callee: Box::new(callee),
                args: args
                    .iter()
                    .map(|arg| self.lower_expr(arg, scope).0)
                    .collect(),
                ret: ret.clone(),
            },
            ret,
        )
    }

    fn lower_arm(
        &self,
        arm: &MatchArm,
        scrutinee_ty: &Type,
        variants: &[VariantDef],
        scope: &Scope,
    ) -> IrArm {
        let mut arm_scope = scope.clone();
        let pattern = self.lower_pattern(&arm.pattern, scrutinee_ty, variants, &mut arm_scope);
        let guard = arm
            .guard
            .as_ref()
            .map(|guard| self.lower_expr(guard, &mut arm_scope).0);
        let body = self.lower_expr(&arm.body, &mut arm_scope).0;
        IrArm {
            pattern,
            guard,
            body,
        }
    }

    fn lower_pattern(
        &self,
        pattern: &Pattern,
        scrutinee_ty: &Type,
        variants: &[VariantDef],
        scope: &mut Scope,
    ) -> IrPattern {
        match pattern {
            Pattern::Wildcard(_) => IrPattern::Wildcard { binding: None },
            Pattern::Binding { name, .. } => {
                scope.insert(name.clone(), scrutinee_ty.clone());
                IrPattern::Wildcard {
                    binding: Some((name.clone(), scrutinee_ty.clone())),
                }
            }
            Pattern::Variant {
                enum_name,
                variant,
                fields,
                ..
            } => {
                // A qualified pattern names its enum directly; otherwise use the
                // scrutinee's variants.
                let owned;
                let resolved: &[VariantDef] = match enum_name {
                    Some(name) => {
                        owned = self.variants_of(&Type::Enum(name.clone()));
                        &owned
                    }
                    None => variants,
                };
                let info = resolved.iter().find(|v| v.name == *variant);
                let tag = info.map_or(0, |v| v.tag);
                let field_tys = info.map(|v| v.fields.clone()).unwrap_or_default();
                let bindings = fields
                    .iter()
                    .enumerate()
                    .map(|(index, binding)| match binding {
                        FieldBinding::Name(name) => {
                            let ty = field_tys.get(index).cloned().unwrap_or(Type::Unit);
                            scope.insert(name.clone(), ty.clone());
                            Some((name.clone(), ty))
                        }
                        FieldBinding::Ignore => None,
                    })
                    .collect();
                IrPattern::Variant {
                    variant: variant.clone(),
                    tag,
                    bindings,
                }
            }
        }
    }

    /// The variants a matched value of `ty` can take (spec 0005). `Option<T>`
    /// is the built-in `Some(T)`/`None` enum with tags 0 and 1.
    fn variants_of(&self, ty: &Type) -> Vec<VariantDef> {
        match ty {
            Type::Enum(name) => self
                .enums
                .get(name)
                .map(|variants| {
                    variants
                        .iter()
                        .map(|v| VariantDef {
                            name: v.name.clone(),
                            tag: v.tag,
                            fields: v.fields.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            Type::Option(inner) => vec![
                VariantDef {
                    name: "Some".to_string(),
                    tag: 0,
                    fields: vec![(**inner).clone()],
                },
                VariantDef {
                    name: "None".to_string(),
                    tag: 1,
                    fields: vec![],
                },
            ],
            _ => Vec::new(),
        }
    }

    /// The variables a lambda captures from its enclosing runtime scope, in
    /// first-occurrence order. Top-level functions and platform functions are
    /// not in `scope`, so they are never captured.
    fn lambda_captures(
        &self,
        params: &[crate::ast::Param],
        body: &Block,
        scope: &Scope,
    ) -> Vec<IrCapture> {
        let bound: HashSet<String> = params.iter().map(|param| param.name.clone()).collect();
        let mut free = Vec::new();
        free_vars_block(&body.items, &bound, &mut free);
        free.into_iter()
            .filter_map(|name| {
                scope.get(&name).map(|ty| IrCapture {
                    name,
                    ty: ty.clone(),
                })
            })
            .collect()
    }
}

/// Whether the lowered expression is a call to a throwing function — the cue
/// that `?` propagates an error rather than a `None` (spec 0011).
fn is_throwing(ir: &IrExpr) -> bool {
    match ir {
        IrExpr::Call { callee, .. } => {
            matches!(callee.ty(), Type::Function(function) if function.throws.is_some())
        }
        _ => false,
    }
}

/// The error type a `try` body can raise, found from the first `throw` or
/// throwing call in it. The type checker guarantees a single error type, so the
/// first one found is representative — it is used to resolve `catch` arm tags.
fn body_error_ty(ir: &IrExpr) -> Option<Type> {
    match ir {
        IrExpr::Throw { value } => Some(value.ty()),
        IrExpr::Call { callee, .. } => match callee.ty() {
            Type::Function(function) => function.throws.map(|throws| *throws),
            _ => None,
        },
        IrExpr::Question { value, .. } => body_error_ty(value),
        IrExpr::Let { value, next, .. } => body_error_ty(value).or_else(|| body_error_ty(next)),
        IrExpr::Binary { left, right, .. } => body_error_ty(left).or_else(|| body_error_ty(right)),
        IrExpr::Match {
            scrutinee, arms, ..
        } => body_error_ty(scrutinee)
            .or_else(|| arms.iter().find_map(|arm| body_error_ty(&arm.body))),
        IrExpr::Array { elems, .. } => elems.iter().find_map(body_error_ty),
        IrExpr::EnumValue { payload, .. } => payload.iter().find_map(body_error_ty),
        _ => None,
    }
}

/// Picks the representative type of a set of arm bodies, preferring a concrete
/// type over `Never` (which a `panic`/`throw`-only arm yields).
fn pick_ty(types: impl Iterator<Item = Type>) -> Type {
    let mut result = Type::Never;
    for ty in types {
        if !matches!(ty, Type::Never) {
            return ty;
        }
        result = ty;
    }
    result
}

fn free_vars_block(items: &[BlockItem], bound: &HashSet<String>, out: &mut Vec<String>) {
    let mut bound = bound.clone();
    for item in items {
        match item {
            BlockItem::Let { name, value, .. } => {
                free_vars_expr(value, &bound, out);
                bound.insert(name.clone());
            }
            BlockItem::Expr(expr) => free_vars_expr(expr, &bound, out),
        }
    }
}

fn free_vars_expr(expr: &Expr, bound: &HashSet<String>, out: &mut Vec<String>) {
    match expr {
        Expr::Var(name, _) => {
            if !bound.contains(name) && !out.contains(name) {
                out.push(name.clone());
            }
        }
        Expr::Array(elements, _) => {
            for element in elements {
                free_vars_expr(element, bound, out);
            }
        }
        Expr::Call { callee, args, .. } => {
            free_vars_expr(callee, bound, out);
            for arg in args {
                free_vars_expr(arg, bound, out);
            }
        }
        Expr::Binary { left, right, .. } => {
            free_vars_expr(left, bound, out);
            free_vars_expr(right, bound, out);
        }
        Expr::Fn { params, body, .. } => {
            let mut inner = bound.clone();
            for param in params {
                inner.insert(param.name.clone());
            }
            free_vars_block(&body.items, &inner, out);
        }
        Expr::Block(block) => free_vars_block(&block.items, bound, out),
        Expr::If {
            cond, then, els, ..
        } => {
            free_vars_expr(cond, bound, out);
            free_vars_block(&then.items, bound, out);
            free_vars_block(&els.items, bound, out);
        }
        Expr::Throw { value, .. } | Expr::Question { value, .. } => {
            free_vars_expr(value, bound, out)
        }
        Expr::Panic { message, .. } => free_vars_expr(message, bound, out),
        Expr::Variant { args, .. } => {
            for arg in args {
                free_vars_expr(arg, bound, out);
            }
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            free_vars_expr(scrutinee, bound, out);
            for arm in arms {
                free_vars_arm(arm, bound, out);
            }
        }
        Expr::Try { body, arms, .. } => {
            free_vars_block(&body.items, bound, out);
            for arm in arms {
                free_vars_arm(arm, bound, out);
            }
        }
        Expr::Int(_, _)
        | Expr::Float(_, _)
        | Expr::Bool(_, _)
        | Expr::String(_, _)
        | Expr::Char(_, _)
        | Expr::Unit(_) => {}
    }
}

fn free_vars_arm(arm: &MatchArm, bound: &HashSet<String>, out: &mut Vec<String>) {
    let mut inner = bound.clone();
    pattern_bindings(&arm.pattern, &mut inner);
    if let Some(guard) = &arm.guard {
        free_vars_expr(guard, &inner, out);
    }
    free_vars_expr(&arm.body, &inner, out);
}

fn pattern_bindings(pattern: &Pattern, bound: &mut HashSet<String>) {
    match pattern {
        Pattern::Wildcard(_) => {}
        Pattern::Binding { name, .. } => {
            bound.insert(name.clone());
        }
        Pattern::Variant { fields, .. } => {
            for field in fields {
                if let FieldBinding::Name(name) = field {
                    bound.insert(name.clone());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_program;
    use crate::typecheck;

    fn lower_source(source: &str) -> IrProgram {
        let program = parse_program("test", source).expect("parse");
        let typed = typecheck::check(&program).expect("typecheck");
        lower(&program, &typed)
    }

    fn main_body(ir: &IrProgram) -> &IrExpr {
        &ir.functions
            .iter()
            .find(|function| function.name == "main")
            .expect("main")
            .body
    }

    // Walk to the first `Fn` literal in an expression tree.
    fn first_lambda(expr: &IrExpr) -> Option<&IrExpr> {
        match expr {
            IrExpr::Fn { .. } => Some(expr),
            IrExpr::Let { value, next, .. } => first_lambda(value).or_else(|| first_lambda(next)),
            IrExpr::Call { callee, args, .. } => {
                first_lambda(callee).or_else(|| args.iter().find_map(first_lambda))
            }
            IrExpr::Binary { left, right, .. } => {
                first_lambda(left).or_else(|| first_lambda(right))
            }
            IrExpr::Array { elems, .. } => elems.iter().find_map(first_lambda),
            _ => None,
        }
    }

    #[test]
    fn lambda_captures_enclosing_binding() {
        let ir = lower_source(
            "fn make_adder(n: Int) -> (Int) -> Int {\n  fn (x: Int) -> Int { x + n }\n}\nfn main() -> Int { let a = make_adder(1) a(41) }\n",
        );
        let adder = ir
            .functions
            .iter()
            .find(|function| function.name == "make_adder")
            .expect("make_adder");
        let lambda = first_lambda(&adder.body).expect("lambda");
        let IrExpr::Fn { captures, .. } = lambda else {
            panic!("expected Fn");
        };
        assert_eq!(captures.len(), 1);
        assert_eq!(captures[0].name, "n");
        assert_eq!(captures[0].ty, Type::Int);
    }

    #[test]
    fn top_level_functions_are_not_captured() {
        let ir = lower_source(
            "fn helper(x: Int) -> Int { x }\nfn main() -> Int {\n  let k = 2\n  let f = fn (x: Int) -> Int { helper(x) + k }\n  f(40)\n}\n",
        );
        let lambda = first_lambda(main_body(&ir)).expect("lambda");
        let IrExpr::Fn { captures, .. } = lambda else {
            panic!("expected Fn");
        };
        let names: Vec<&str> = captures.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["k"]);
    }
}
