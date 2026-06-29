use std::collections::{HashMap, HashSet};

use crate::ast::{BinaryOp, Block, BlockItem, EffectRow, Expr, Function, Program, Type};
use crate::error::{Diagnostic, Error, Result, Span};

#[derive(Debug, Clone)]
pub(crate) struct TypedProgram {
    pub(crate) functions: Vec<TypedFunction>,
}

#[derive(Debug, Clone)]
pub(crate) struct TypedFunction {
    pub(crate) name: String,
    pub(crate) params: Vec<Type>,
    pub(crate) ret: Type,
    pub(crate) effects: EffectRow,
}

#[derive(Debug, Clone)]
struct FunctionSig {
    params: Vec<Type>,
    ret: Type,
    effects: EffectRow,
}

#[derive(Debug, Clone)]
struct ExprInfo {
    ty: Type,
    effects: EffectRow,
    span: Span,
}

pub(crate) fn check(program: &Program) -> Result<TypedProgram> {
    let mut checker = Checker {
        functions: HashMap::new(),
    };
    checker.register_functions(program)?;
    checker.check_main(program)?;
    for function in &program.functions {
        checker.check_function(function)?;
    }
    Ok(TypedProgram {
        functions: program
            .functions
            .iter()
            .map(|function| TypedFunction {
                name: function.name.clone(),
                params: function
                    .params
                    .iter()
                    .map(|param| param.ty.clone())
                    .collect(),
                ret: function.ret.clone(),
                effects: function.effects.clone(),
            })
            .collect(),
    })
}

impl TypedProgram {
    pub(crate) fn function_count(&self) -> usize {
        self.functions.len()
    }

    pub(crate) fn signature_summary(&self) -> String {
        self.functions
            .iter()
            .map(|function| {
                format!(
                    "{}({}) -> {:?}",
                    function.name,
                    function
                        .params
                        .iter()
                        .map(|param| format!("{param:?}"))
                        .collect::<Vec<_>>()
                        .join(", "),
                    function.ret
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

struct Checker {
    functions: HashMap<String, FunctionSig>,
}

impl Checker {
    fn register_functions(&mut self, program: &Program) -> Result<()> {
        for function in &program.functions {
            if self.functions.contains_key(&function.name) {
                return Err(Error::diagnostic(
                    Diagnostic::new("Duplicate function").label(
                        function.name_span.clone(),
                        format!("function `{}` is already defined", function.name),
                    ),
                ));
            }
            let mut names = HashSet::new();
            for param in &function.params {
                if !names.insert(param.name.clone()) {
                    return Err(Error::diagnostic(
                        Diagnostic::new("Duplicate parameter").label(
                            param.name_span.clone(),
                            format!("parameter `{}` is already defined", param.name),
                        ),
                    ));
                }
            }
            self.functions.insert(
                function.name.clone(),
                FunctionSig {
                    params: function
                        .params
                        .iter()
                        .map(|param| param.ty.clone())
                        .collect(),
                    ret: function.ret.clone(),
                    effects: function.effects.clone(),
                },
            );
        }
        Ok(())
    }

    fn check_main(&self, program: &Program) -> Result<()> {
        let Some(main) = program
            .functions
            .iter()
            .find(|function| function.name == "main")
        else {
            let span = program
                .functions
                .first()
                .map(|function| function.name_span.clone())
                .ok_or_else(|| Error::new("program has no functions"))?;
            return Err(Error::diagnostic(
                Diagnostic::new("Missing entrypoint")
                    .label(span, "expected a top-level `main` function"),
            ));
        };
        if !main.params.is_empty() {
            return Err(Error::diagnostic(
                Diagnostic::new("Invalid entrypoint")
                    .label(main.name_span.clone(), "`main` must not take parameters"),
            ));
        }
        Ok(())
    }

    fn check_function(&self, function: &Function) -> Result<()> {
        let mut scope = HashMap::new();
        for param in &function.params {
            scope.insert(param.name.clone(), param.ty.clone());
        }
        let body = self.check_block(&function.body, &mut scope)?;
        expect_type(&body.ty, &function.ret, body.span.clone())?;
        if !body.effects.is_subset_of(&function.effects) {
            return Err(Error::diagnostic(
                Diagnostic::new("Unhandled effects")
                    .label(
                        body.span,
                        format!(
                            "function `{}` declares uses {:?}, but body uses {:?}",
                            function.name, function.effects.effects, body.effects.effects
                        ),
                    )
                    .help("Add the missing effect names to `uses { ... }`."),
            ));
        }
        Ok(())
    }

    fn check_block(
        &self,
        block: &Block,
        outer_scope: &mut HashMap<String, Type>,
    ) -> Result<ExprInfo> {
        let mut scope = outer_scope.clone();
        let mut effects = EffectRow::default();
        let mut last = ExprInfo {
            ty: Type::Unit,
            effects: EffectRow::default(),
            span: block.span.clone(),
        };
        for item in &block.items {
            match item {
                BlockItem::Let {
                    name,
                    name_span,
                    ty,
                    value,
                } => {
                    if scope.contains_key(name) {
                        return Err(Error::diagnostic(
                            Diagnostic::new("Duplicate binding")
                                .label(name_span.clone(), format!("`{name}` is already bound")),
                        ));
                    }
                    let info = match (value, ty) {
                        (Expr::Array(elements, span), Some(Type::Array(element))) => {
                            self.check_array(elements, span, &mut scope, Some(element))?
                        }
                        _ => self.check_expr(value, &mut scope)?,
                    };
                    let binding_ty = if let Some(annotation) = ty {
                        expect_type(&info.ty, annotation, info.span.clone())?;
                        annotation.clone()
                    } else {
                        info.ty
                    };
                    effects.union(&info.effects);
                    scope.insert(name.clone(), binding_ty);
                    last = ExprInfo {
                        ty: Type::Unit,
                        effects: EffectRow::default(),
                        span: name_span.clone(),
                    };
                }
                BlockItem::Expr(expr) => {
                    last = self.check_expr(expr, &mut scope)?;
                    effects.union(&last.effects);
                }
            }
        }
        last.effects = effects;
        Ok(last)
    }

    fn check_expr(&self, expr: &Expr, scope: &mut HashMap<String, Type>) -> Result<ExprInfo> {
        match expr {
            Expr::Int(_, span) => Ok(info(Type::Int, span.clone())),
            Expr::Float(_, span) => Ok(info(Type::Float, span.clone())),
            Expr::Bool(_, span) => Ok(info(Type::Bool, span.clone())),
            Expr::String(_, span) => Ok(info(Type::String, span.clone())),
            Expr::Array(elements, span) => self.check_array(elements, span, scope, None),
            Expr::Unit(span) => Ok(info(Type::Unit, span.clone())),
            Expr::Var(name, span) => scope
                .get(name)
                .cloned()
                .map(|ty| info(ty, span.clone()))
                .ok_or_else(|| {
                    Error::diagnostic(Diagnostic::new("Unknown name").label(
                        span.clone(),
                        format!("`{name}` is not defined in this scope"),
                    ))
                }),
            Expr::Call { name, args, span } => {
                let sig = self.functions.get(name).ok_or_else(|| {
                    Error::diagnostic(
                        Diagnostic::new("Unknown function")
                            .label(span.clone(), format!("function `{name}` is not defined")),
                    )
                })?;
                if args.len() != sig.params.len() {
                    return Err(Error::diagnostic(
                        Diagnostic::new("Wrong number of arguments").label(
                            span.clone(),
                            format!(
                                "`{name}` expects {} argument(s), got {}",
                                sig.params.len(),
                                args.len()
                            ),
                        ),
                    ));
                }
                let mut effects = sig.effects.clone();
                for (arg, expected) in args.iter().zip(sig.params.iter()) {
                    let actual = self.check_expr(arg, scope)?;
                    expect_type(&actual.ty, expected, actual.span.clone())?;
                    effects.union(&actual.effects);
                }
                Ok(ExprInfo {
                    ty: sig.ret.clone(),
                    effects,
                    span: span.clone(),
                })
            }
            Expr::Binary {
                op,
                left,
                right,
                span,
            } => {
                let left = self.check_expr(left, scope)?;
                let right = self.check_expr(right, scope)?;
                let mut effects = left.effects.clone();
                effects.union(&right.effects);
                match op {
                    BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul => {
                        let ty = expect_numeric_pair(&left, &right)?;
                        Ok(ExprInfo {
                            ty,
                            effects,
                            span: span.clone(),
                        })
                    }
                    BinaryOp::Eq | BinaryOp::Lt => {
                        expect_comparable_numeric_pair(&left, &right)?;
                        Ok(ExprInfo {
                            ty: Type::Bool,
                            effects,
                            span: span.clone(),
                        })
                    }
                }
            }
            Expr::Block(block) => self.check_block(block, scope),
        }
    }

    fn check_array(
        &self,
        elements: &[Expr],
        span: &Span,
        scope: &mut HashMap<String, Type>,
        expected_element: Option<&Type>,
    ) -> Result<ExprInfo> {
        let mut effects = EffectRow::default();
        let mut element_ty = expected_element.cloned();
        for element in elements {
            let actual = self.check_expr(element, scope)?;
            effects.union(&actual.effects);
            match &element_ty {
                Some(expected) => expect_type(&actual.ty, expected, actual.span.clone())?,
                None => element_ty = Some(actual.ty),
            }
        }
        let Some(element_ty) = element_ty else {
            return Err(Error::diagnostic(
                Diagnostic::new("Cannot infer array type")
                    .label(span.clone(), "empty array needs an `Array<T>` annotation"),
            ));
        };
        Ok(ExprInfo {
            ty: Type::Array(Box::new(element_ty)),
            effects,
            span: span.clone(),
        })
    }
}

fn info(ty: Type, span: Span) -> ExprInfo {
    ExprInfo {
        ty,
        effects: EffectRow::default(),
        span,
    }
}

fn expect_numeric_pair(left: &ExprInfo, right: &ExprInfo) -> Result<Type> {
    match (&left.ty, &right.ty) {
        (Type::Int, Type::Int) => Ok(Type::Int),
        (Type::Float, Type::Float) => Ok(Type::Float),
        _ => Err(Error::diagnostic(Diagnostic::new("Type mismatch").label(
            right.span.clone(),
            format!(
                "expected operands with matching numeric types, but found `{:?}` and `{:?}`",
                left.ty, right.ty
            ),
        ))),
    }
}

fn expect_comparable_numeric_pair(left: &ExprInfo, right: &ExprInfo) -> Result<()> {
    match (&left.ty, &right.ty) {
        (Type::Int, Type::Int) | (Type::Float, Type::Float) => Ok(()),
        _ => Err(Error::diagnostic(Diagnostic::new("Type mismatch").label(
            right.span.clone(),
            format!(
                "expected operands with matching numeric types, but found `{:?}` and `{:?}`",
                left.ty, right.ty
            ),
        ))),
    }
}

fn expect_type(actual: &Type, expected: &Type, span: Span) -> Result<()> {
    if actual == expected {
        return Ok(());
    }
    Err(Error::diagnostic(Diagnostic::new("Type mismatch").label(
        span,
        format!("expected `{expected:?}`, but found `{actual:?}`"),
    )))
}
