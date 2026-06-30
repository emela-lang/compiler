use std::collections::{HashMap, HashSet};

use crate::ast::{
    BinaryOp, Block, BlockItem, EffectRow, Expr, FieldBinding, Function, FunctionType, MatchArm,
    Pattern, Program, Type,
};
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
    pub(crate) throws: Option<Type>,
    pub(crate) effects: EffectRow,
}

#[derive(Debug, Clone)]
struct FunctionSig {
    params: Vec<Type>,
    ret: Type,
    throws: Option<Type>,
    effects: EffectRow,
}

impl FunctionSig {
    fn ty(&self) -> Type {
        Type::Function(FunctionType {
            params: self.params.clone(),
            ret: Box::new(self.ret.clone()),
            throws: self.throws.clone().map(Box::new),
            effects: self.effects.clone(),
        })
    }
}

/// A declared enum's variants, in declaration order (spec 0005).
#[derive(Debug, Clone)]
struct EnumInfo {
    variants: Vec<VariantInfo>,
}

#[derive(Debug, Clone)]
struct VariantInfo {
    name: String,
    fields: Vec<Type>,
}

#[derive(Debug, Clone)]
struct ExprInfo {
    ty: Type,
    effects: EffectRow,
    /// The error type this expression may put on the throws channel (spec
    /// 0011), still unresolved at this point. `None` means non-throwing.
    throws: Option<Type>,
    span: Span,
}

/// The enclosing function's error/return contract, threaded into `?`/`throw`.
struct FnCtx<'a> {
    throws: &'a Option<Type>,
    ret: &'a Type,
}

pub(crate) fn check(program: &Program) -> Result<TypedProgram> {
    let mut checker = Checker {
        functions: HashMap::new(),
        enums: HashMap::new(),
    };
    checker.register_enums(program)?;
    checker.register_functions(program)?;
    checker.register_externs(program)?;
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
                throws: function.throws.clone(),
                effects: function.effects.clone(),
            })
            .collect(),
    })
}

struct Checker {
    functions: HashMap<String, FunctionSig>,
    enums: HashMap<String, EnumInfo>,
}

impl Checker {
    fn register_enums(&mut self, program: &Program) -> Result<()> {
        for decl in &program.enums {
            if self.enums.contains_key(&decl.name) {
                return Err(Error::diagnostic(Diagnostic::new("Duplicate enum").label(
                    decl.name_span.clone(),
                    format!("enum `{}` is already defined", decl.name),
                )));
            }
            let mut variants = Vec::new();
            let mut seen = HashSet::new();
            for variant in &decl.variants {
                if !seen.insert(variant.name.clone()) {
                    return Err(Error::diagnostic(
                        Diagnostic::new("Duplicate variant").label(
                            variant.name_span.clone(),
                            format!("variant `{}` is already defined", variant.name),
                        ),
                    ));
                }
                variants.push(VariantInfo {
                    name: variant.name.clone(),
                    fields: variant.fields.clone(),
                });
            }
            self.enums.insert(decl.name.clone(), EnumInfo { variants });
        }
        // Payload types may reference other enums; validate now that all are in.
        for decl in &program.enums {
            for variant in &decl.variants {
                for field in &variant.fields {
                    self.validate_type(field, &variant.name_span)?;
                }
            }
        }
        Ok(())
    }

    /// Rejects a type that names an enum that was never declared (spec 0005).
    fn validate_type(&self, ty: &Type, span: &Span) -> Result<()> {
        match ty {
            Type::Enum(name) if !self.enums.contains_key(name) => {
                Err(Error::diagnostic(Diagnostic::new("Unknown type").label(
                    span.clone(),
                    format!("`{name}` is not a declared enum or built-in type"),
                )))
            }
            Type::Array(inner) | Type::Option(inner) => self.validate_type(inner, span),
            Type::Function(function) => {
                for param in &function.params {
                    self.validate_type(param, span)?;
                }
                self.validate_type(&function.ret, span)?;
                if let Some(throws) = &function.throws {
                    self.validate_type(throws, span)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

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
                self.validate_type(&param.ty, &param.name_span)?;
                if !names.insert(param.name.clone()) {
                    return Err(Error::diagnostic(
                        Diagnostic::new("Duplicate parameter").label(
                            param.name_span.clone(),
                            format!("parameter `{}` is already defined", param.name),
                        ),
                    ));
                }
            }
            self.validate_type(&function.ret, &function.name_span)?;
            if let Some(throws) = &function.throws {
                self.validate_type(throws, &function.name_span)?;
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
                    throws: function.throws.clone(),
                    effects: function.effects.clone(),
                },
            );
        }
        Ok(())
    }

    /// Validates each `extern fn` against the platform interface (spec 0013) and
    /// registers it as a callable signature so wrappers can call it.
    fn register_externs(&mut self, program: &Program) -> Result<()> {
        for declaration in &program.externs {
            if self.functions.contains_key(&declaration.name) {
                return Err(Error::diagnostic(
                    Diagnostic::new("Duplicate function").label(
                        declaration.name_span.clone(),
                        format!("`{}` is already defined", declaration.name),
                    ),
                ));
            }
            let canonical = declaration.canonical();
            let Some(entry) = emela_codegen::platform_lookup(&canonical) else {
                return Err(Error::diagnostic(
                    Diagnostic::new("Unknown platform function")
                        .label(
                            declaration.name_span.clone(),
                            format!("`{canonical}` is not a platform function"),
                        )
                        .help("Platform functions are defined by spec 0013."),
                ));
            };
            let params: Vec<Type> = declaration
                .params
                .iter()
                .map(|param| param.ty.clone())
                .collect();
            if params != entry.params || declaration.ret != entry.ret {
                return Err(Error::diagnostic(
                    Diagnostic::new("Platform signature mismatch").label(
                        declaration.name_span.clone(),
                        format!("`{canonical}` does not match the platform interface"),
                    ),
                ));
            }
            let expected = EffectRow::sorted(vec![entry.capability.clone()]);
            if declaration.effects != expected {
                return Err(Error::diagnostic(
                    Diagnostic::new("Platform effect mismatch").label(
                        declaration.name_span.clone(),
                        format!(
                            "`{canonical}` must declare `uses {{ {} }}`",
                            entry.capability
                        ),
                    ),
                ));
            }
            self.functions.insert(
                declaration.name.clone(),
                FunctionSig {
                    params,
                    ret: declaration.ret.clone(),
                    throws: declaration.throws.clone(),
                    effects: declaration.effects.clone(),
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
        // The entrypoint's throws channel must be `Never`, i.e. non-throwing
        // (spec 0011). `throws Never` is normalized to `None` by the parser, so
        // any remaining declared error type is rejected here.
        if main.throws.is_some() {
            return Err(Error::diagnostic(
                Diagnostic::new("Invalid entrypoint")
                    .label(main.name_span.clone(), "`main`'s `throws` must be `Never`")
                    .help(
                        "Omit `throws` (or write `throws Never`); handle errors with `try`/`catch` or `panic`.",
                    ),
            ));
        }
        Ok(())
    }

    fn check_function(&self, function: &Function) -> Result<()> {
        let mut scope = HashMap::new();
        for param in &function.params {
            scope.insert(param.name.clone(), param.ty.clone());
        }
        let ctx = FnCtx {
            throws: &function.throws,
            ret: &function.ret,
        };
        let body = self.check_block(&function.body, &mut scope, &ctx, false)?;
        expect_assignable(&body.ty, &function.ret, body.span.clone())?;
        if !body.effects.is_subset_of(&function.effects) {
            return Err(Error::diagnostic(
                Diagnostic::new("Unhandled effects")
                    .label(
                        body.span.clone(),
                        format!(
                            "function `{}` declares uses {:?}, but body uses {:?}",
                            function.name, function.effects.effects, body.effects.effects
                        ),
                    )
                    .help("Add the missing effect names to `uses { ... }`."),
            ));
        }
        self.check_throws_subset(&body.throws, &function.throws, &function.name, body.span)?;
        Ok(())
    }

    /// The body may only put on the throws channel what the function declares.
    fn check_throws_subset(
        &self,
        body: &Option<Type>,
        declared: &Option<Type>,
        name: &str,
        span: Span,
    ) -> Result<()> {
        match (body, declared) {
            (None, _) => Ok(()),
            (Some(actual), Some(expected)) if types_compatible(actual, expected) => Ok(()),
            (Some(actual), Some(expected)) => Err(Error::diagnostic(
                Diagnostic::new("Throws type mismatch").label(
                    span,
                    format!(
                        "`{name}` declares `throws {expected:?}`, but the body throws `{actual:?}`"
                    ),
                ),
            )),
            (Some(actual), None) => Err(Error::diagnostic(
                Diagnostic::new("Unhandled error")
                    .label(
                        span,
                        format!("`{name}` may throw `{actual:?}`, but declares no `throws`"),
                    )
                    .help("Add a `throws E` clause, or handle the error with `try`/`catch`."),
            )),
        }
    }

    fn check_block(
        &self,
        block: &Block,
        outer_scope: &mut HashMap<String, Type>,
        ctx: &FnCtx,
        allow_throw: bool,
    ) -> Result<ExprInfo> {
        let mut scope = outer_scope.clone();
        let mut effects = EffectRow::default();
        let mut throws: Option<Type> = None;
        let mut last = ExprInfo {
            ty: Type::Unit,
            effects: EffectRow::default(),
            throws: None,
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
                        (Expr::Array(elements, span), Some(Type::Array(element))) => self
                            .check_array(
                                elements,
                                span,
                                &mut scope,
                                ctx,
                                Some(element),
                                allow_throw,
                            )?,
                        _ => self.check_expr(value, &mut scope, ctx, allow_throw)?,
                    };
                    let binding_ty = if let Some(annotation) = ty {
                        expect_assignable(&info.ty, annotation, info.span.clone())?;
                        annotation.clone()
                    } else {
                        info.ty
                    };
                    effects.union(&info.effects);
                    throws = merge_throws(throws, info.throws, info.span)?;
                    scope.insert(name.clone(), binding_ty);
                    last = ExprInfo {
                        ty: Type::Unit,
                        effects: EffectRow::default(),
                        throws: None,
                        span: name_span.clone(),
                    };
                }
                BlockItem::Expr(expr) => {
                    last = self.check_expr(expr, &mut scope, ctx, allow_throw)?;
                    effects.union(&last.effects);
                    throws = merge_throws(throws, last.throws.clone(), last.span.clone())?;
                }
            }
        }
        last.effects = effects;
        last.throws = throws;
        Ok(last)
    }

    fn check_expr(
        &self,
        expr: &Expr,
        scope: &mut HashMap<String, Type>,
        ctx: &FnCtx,
        allow_throw: bool,
    ) -> Result<ExprInfo> {
        match expr {
            Expr::Int(_, span) => Ok(self.info(Type::Int, span.clone())),
            Expr::Float(_, span) => Ok(self.info(Type::Float, span.clone())),
            Expr::Bool(_, span) => Ok(self.info(Type::Bool, span.clone())),
            Expr::String(_, span) => Ok(self.info(Type::String, span.clone())),
            Expr::Char(_, span) => Ok(self.info(Type::Char, span.clone())),
            Expr::Array(elements, span) => {
                self.check_array(elements, span, scope, ctx, None, allow_throw)
            }
            Expr::Unit(span) => Ok(self.info(Type::Unit, span.clone())),
            Expr::Var(name, span) => {
                if let Some(ty) = scope.get(name) {
                    Ok(self.info(ty.clone(), span.clone()))
                } else if name == "None" {
                    Ok(self.info(Type::Option(Box::new(Type::Never)), span.clone()))
                } else if let Some(sig) = self.functions.get(name) {
                    Ok(self.info(sig.ty(), span.clone()))
                } else {
                    Err(Error::diagnostic(Diagnostic::new("Unknown name").label(
                        span.clone(),
                        format!("`{name}` is not defined in this scope"),
                    )))
                }
            }
            Expr::Call { callee, args, span } => {
                self.check_call(callee, args, span, scope, ctx, allow_throw)
            }
            Expr::Fn {
                params,
                ret,
                throws,
                effects,
                body,
                span,
            } => {
                let mut names = HashSet::new();
                let mut fn_scope = scope.clone();
                for param in params {
                    self.validate_type(&param.ty, &param.name_span)?;
                    if !names.insert(param.name.clone()) {
                        return Err(Error::diagnostic(
                            Diagnostic::new("Duplicate parameter").label(
                                param.name_span.clone(),
                                format!("parameter `{}` is already defined", param.name),
                            ),
                        ));
                    }
                    fn_scope.insert(param.name.clone(), param.ty.clone());
                }
                let inner_ctx = FnCtx { throws, ret };
                let body_info = self.check_block(body, &mut fn_scope, &inner_ctx, false)?;
                expect_assignable(&body_info.ty, ret, body_info.span.clone())?;
                if !body_info.effects.is_subset_of(effects) {
                    return Err(Error::diagnostic(
                        Diagnostic::new("Unhandled effects")
                            .label(
                                body_info.span.clone(),
                                format!(
                                    "function literal declares uses {:?}, but body uses {:?}",
                                    effects.effects, body_info.effects.effects
                                ),
                            )
                            .help("Add the missing effect names to `uses { ... }`."),
                    ));
                }
                self.check_throws_subset(
                    &body_info.throws,
                    throws,
                    "function literal",
                    body_info.span,
                )?;
                Ok(ExprInfo {
                    ty: Type::Function(FunctionType {
                        params: params.iter().map(|param| param.ty.clone()).collect(),
                        ret: Box::new(ret.clone()),
                        throws: throws.clone().map(Box::new),
                        effects: effects.clone(),
                    }),
                    effects: EffectRow::default(),
                    throws: None,
                    span: span.clone(),
                })
            }
            Expr::Binary {
                op,
                left,
                right,
                span,
            } => {
                let left = self.check_expr(left, scope, ctx, allow_throw)?;
                let right = self.check_expr(right, scope, ctx, allow_throw)?;
                let mut effects = left.effects.clone();
                effects.union(&right.effects);
                let throws = merge_throws(left.throws.clone(), right.throws.clone(), span.clone())?;
                let ty = match op {
                    BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div => {
                        expect_numeric_pair(&left, &right)?
                    }
                    BinaryOp::Rem => match (&left.ty, &right.ty) {
                        (Type::Int, Type::Int) => Type::Int,
                        _ => {
                            return Err(Error::diagnostic(Diagnostic::new("Type mismatch").label(
                                right.span.clone(),
                                format!(
                                    "`%` requires `Int` operands, but found `{:?}` and `{:?}`",
                                    left.ty, right.ty
                                ),
                            )));
                        }
                    },
                    BinaryOp::Concat => {
                        expect_assignable(&left.ty, &Type::String, left.span.clone())?;
                        expect_assignable(&right.ty, &Type::String, right.span.clone())?;
                        Type::String
                    }
                    BinaryOp::Eq | BinaryOp::Lt => {
                        expect_comparable_numeric_pair(&left, &right)?;
                        Type::Bool
                    }
                };
                Ok(ExprInfo {
                    ty,
                    effects,
                    throws,
                    span: span.clone(),
                })
            }
            Expr::Block(block) => self.check_block(block, scope, ctx, allow_throw),
            Expr::Throw { value, span } => {
                let val = self.check_expr(value, scope, ctx, allow_throw)?;
                Ok(ExprInfo {
                    ty: Type::Never,
                    effects: val.effects,
                    throws: Some(val.ty),
                    span: span.clone(),
                })
            }
            Expr::Panic { message, span } => {
                let message = self.check_expr(message, scope, ctx, allow_throw)?;
                expect_assignable(&message.ty, &Type::String, message.span.clone())?;
                Ok(ExprInfo {
                    ty: Type::Never,
                    effects: message.effects,
                    throws: None,
                    span: span.clone(),
                })
            }
            Expr::Question { value, span } => {
                let inner = self.check_expr(value, scope, ctx, true)?;
                if let Some(error) = inner.throws.clone() {
                    // Throws propagation: `?` forwards the error to the
                    // enclosing function's throws channel (spec 0011).
                    match ctx.throws {
                        Some(declared) if types_compatible(&error, declared) => {}
                        Some(declared) => {
                            return Err(Error::diagnostic(
                                Diagnostic::new("Throws type mismatch").label(
                                    span.clone(),
                                    format!(
                                        "`?` propagates `{error:?}`, but the function declares `throws {declared:?}`"
                                    ),
                                ),
                            ));
                        }
                        None => {
                            return Err(Error::diagnostic(
                                Diagnostic::new("Cannot propagate error").label(
                                    span.clone(),
                                    "`?` requires the enclosing function to declare `throws`",
                                ),
                            ));
                        }
                    }
                    Ok(ExprInfo {
                        ty: inner.ty,
                        effects: inner.effects,
                        throws: Some(error),
                        span: span.clone(),
                    })
                } else if let Type::Option(inner_ty) = &inner.ty {
                    // Option propagation: `?` forwards `None` to the function's
                    // `Option<_>` return (spec 0011).
                    if !matches!(ctx.ret, Type::Option(_)) {
                        return Err(Error::diagnostic(
                            Diagnostic::new("Cannot propagate None").label(
                                span.clone(),
                                "`?` on an Option requires the function to return `Option<_>`",
                            ),
                        ));
                    }
                    Ok(ExprInfo {
                        ty: (**inner_ty).clone(),
                        effects: inner.effects,
                        throws: None,
                        span: span.clone(),
                    })
                } else {
                    Err(Error::diagnostic(Diagnostic::new("Invalid `?`").label(
                        span.clone(),
                        "`?` applies to a throwing call or an `Option` value",
                    )))
                }
            }
            Expr::Variant {
                enum_name,
                variant,
                args,
                span,
            } => {
                // Built-in pure conversions (spec 0017), spelled `Char.from_code`
                // and `String.from_char`.
                if let Some(builtin) = self.check_char_builtin(
                    enum_name,
                    variant,
                    args,
                    span,
                    scope,
                    ctx,
                    allow_throw,
                )? {
                    return Ok(builtin);
                }
                self.check_variant(enum_name, variant, args, span, scope, ctx, allow_throw)
            }
            Expr::Match {
                scrutinee,
                arms,
                span,
            } => self.check_match(scrutinee, arms, span, scope, ctx, allow_throw),
            Expr::Try { body, arms, span } => self.check_try(body, arms, span, scope, ctx),
            Expr::If {
                cond,
                then,
                els,
                span,
            } => {
                let cond_info = self.check_expr(cond, scope, ctx, allow_throw)?;
                expect_assignable(&cond_info.ty, &Type::Bool, cond_info.span.clone())?;
                let then_info = self.check_block(then, scope, ctx, allow_throw)?;
                let els_info = self.check_block(els, scope, ctx, allow_throw)?;
                let ty = unify_arm(Some(then_info.ty), els_info.ty, els_info.span.clone())?;
                let mut effects = cond_info.effects;
                effects.union(&then_info.effects);
                effects.union(&els_info.effects);
                let throws = merge_throws(cond_info.throws, then_info.throws, span.clone())?;
                let throws = merge_throws(throws, els_info.throws, span.clone())?;
                Ok(ExprInfo {
                    ty,
                    effects,
                    throws,
                    span: span.clone(),
                })
            }
        }
    }

    fn check_call(
        &self,
        callee: &Expr,
        args: &[Expr],
        span: &Span,
        scope: &mut HashMap<String, Type>,
        ctx: &FnCtx,
        allow_throw: bool,
    ) -> Result<ExprInfo> {
        // Built-in Option constructor `Some(x)`.
        if let Expr::Var(name, _) = callee {
            if name == "Some" && !scope.contains_key(name) && !self.functions.contains_key(name) {
                if args.len() != 1 {
                    return Err(Error::diagnostic(
                        Diagnostic::new("Wrong number of arguments").label(
                            span.clone(),
                            format!("`Some` takes 1 argument, got {}", args.len()),
                        ),
                    ));
                }
                let arg = self.check_expr(&args[0], scope, ctx, allow_throw)?;
                return Ok(ExprInfo {
                    ty: Type::Option(Box::new(arg.ty)),
                    effects: arg.effects,
                    throws: arg.throws,
                    span: span.clone(),
                });
            }
        }
        let callee_info = self.check_expr(callee, scope, ctx, allow_throw)?;
        let Type::Function(sig) = &callee_info.ty else {
            return Err(Error::diagnostic(
                Diagnostic::new("Cannot call value").label(
                    callee_info.span.clone(),
                    format!(
                        "expected a function value, but found `{:?}`",
                        callee_info.ty
                    ),
                ),
            ));
        };
        if args.len() != sig.params.len() {
            return Err(Error::diagnostic(
                Diagnostic::new("Wrong number of arguments").label(
                    span.clone(),
                    format!(
                        "function expects {} argument(s), got {}",
                        sig.params.len(),
                        args.len()
                    ),
                ),
            ));
        }
        let mut effects = callee_info.effects.clone();
        effects.union(&sig.effects);
        let mut throws = callee_info.throws.clone();
        for (arg, expected) in args.iter().zip(sig.params.iter()) {
            let actual = self.check_expr(arg, scope, ctx, allow_throw)?;
            expect_assignable(&actual.ty, expected, actual.span.clone())?;
            effects.union(&actual.effects);
            throws = merge_throws(throws, actual.throws, actual.span)?;
        }
        // A throwing call must use `?` or sit inside a `try` block (spec 0011).
        if let Some(call_error) = &sig.throws {
            if !allow_throw {
                return Err(Error::diagnostic(
                    Diagnostic::new("Unhandled throwing call")
                        .label(span.clone(), "this call may throw")
                        .help("Use `?` to propagate the error, or wrap it in `try`/`catch`."),
                ));
            }
            throws = merge_throws(throws, Some((**call_error).clone()), span.clone())?;
        }
        Ok(ExprInfo {
            ty: (*sig.ret).clone(),
            effects,
            throws,
            span: span.clone(),
        })
    }

    /// Type-checks the built-in pure conversions `Char.from_code(Int) -> Char`
    /// and `String.from_char(Char) -> String` (spec 0017). Returns `None` when
    /// the call is not one of them.
    #[allow(clippy::too_many_arguments)]
    fn check_char_builtin(
        &self,
        enum_name: &Option<String>,
        variant: &str,
        args: &[Expr],
        span: &Span,
        scope: &mut HashMap<String, Type>,
        ctx: &FnCtx,
        allow_throw: bool,
    ) -> Result<Option<ExprInfo>> {
        let Some(name) = enum_name else {
            return Ok(None);
        };
        let (arg_ty, ret_ty) = match (name.as_str(), variant) {
            ("Char", "from_code") => (Type::Int, Type::Char),
            ("String", "from_char") => (Type::Char, Type::String),
            _ => return Ok(None),
        };
        if args.len() != 1 {
            return Err(Error::diagnostic(
                Diagnostic::new("Wrong number of arguments").label(
                    span.clone(),
                    format!("`{name}.{variant}` takes 1 argument, got {}", args.len()),
                ),
            ));
        }
        let arg = self.check_expr(&args[0], scope, ctx, allow_throw)?;
        expect_assignable(&arg.ty, &arg_ty, arg.span.clone())?;
        Ok(Some(ExprInfo {
            ty: ret_ty,
            effects: arg.effects,
            throws: arg.throws,
            span: span.clone(),
        }))
    }

    fn check_variant(
        &self,
        enum_name: &Option<String>,
        variant: &str,
        args: &[Expr],
        span: &Span,
        scope: &mut HashMap<String, Type>,
        ctx: &FnCtx,
        allow_throw: bool,
    ) -> Result<ExprInfo> {
        let Some(name) = enum_name else {
            return Err(Error::diagnostic(
                Diagnostic::new("Ambiguous variant").label(
                    span.clone(),
                    "qualify the variant with its enum name, e.g. `Enum.Variant`",
                ),
            ));
        };
        let Some(info) = self.enums.get(name) else {
            return Err(Error::diagnostic(
                Diagnostic::new("Unknown enum")
                    .label(span.clone(), format!("`{name}` is not a declared enum")),
            ));
        };
        let Some(vinfo) = info.variants.iter().find(|v| v.name == variant) else {
            return Err(Error::diagnostic(Diagnostic::new("Unknown variant").label(
                span.clone(),
                format!("`{name}` has no variant `{variant}`"),
            )));
        };
        if args.len() != vinfo.fields.len() {
            return Err(Error::diagnostic(
                Diagnostic::new("Wrong number of fields").label(
                    span.clone(),
                    format!(
                        "`{name}.{variant}` takes {} field(s), got {}",
                        vinfo.fields.len(),
                        args.len()
                    ),
                ),
            ));
        }
        let mut effects = EffectRow::default();
        let mut throws = None;
        for (arg, field_ty) in args.iter().zip(vinfo.fields.iter()) {
            let actual = self.check_expr(arg, scope, ctx, allow_throw)?;
            expect_assignable(&actual.ty, field_ty, actual.span.clone())?;
            effects.union(&actual.effects);
            throws = merge_throws(throws, actual.throws, actual.span)?;
        }
        Ok(ExprInfo {
            ty: Type::Enum(name.clone()),
            effects,
            throws,
            span: span.clone(),
        })
    }

    fn check_match(
        &self,
        scrutinee: &Expr,
        arms: &[MatchArm],
        span: &Span,
        scope: &mut HashMap<String, Type>,
        ctx: &FnCtx,
        allow_throw: bool,
    ) -> Result<ExprInfo> {
        let scrut = self.check_expr(scrutinee, scope, ctx, allow_throw)?;
        let variants = self.scrutinee_variants(&scrut.ty, scrutinee.span())?;
        let mut effects = scrut.effects;
        let mut throws = scrut.throws;
        let result = self.check_arms(
            arms,
            &scrut.ty,
            &variants,
            span,
            scope,
            ctx,
            allow_throw,
            &mut effects,
            &mut throws,
        )?;
        Ok(ExprInfo {
            ty: result,
            effects,
            throws,
            span: span.clone(),
        })
    }

    fn check_try(
        &self,
        body: &Block,
        arms: &[MatchArm],
        span: &Span,
        scope: &mut HashMap<String, Type>,
        ctx: &FnCtx,
    ) -> Result<ExprInfo> {
        // The body runs with throwing calls allowed; thrown errors go to catch.
        let body_info = self.check_block(body, &mut scope.clone(), ctx, true)?;
        let caught = body_info.throws.clone();
        let error_ty = caught.clone().unwrap_or(Type::Never);
        let variants = match &caught {
            Some(ty) => self
                .scrutinee_variants(ty, span.clone())
                .unwrap_or_default(),
            None => Vec::new(),
        };
        let mut effects = body_info.effects;
        // The catch arms resolve the error channel; only a re-`throw` re-raises.
        let mut throws = None;
        let mut result = Some(body_info.ty);
        for arm in arms {
            let mut arm_scope = scope.clone();
            self.bind_pattern(&arm.pattern, &error_ty, &variants, &mut arm_scope)?;
            if let Some(guard) = &arm.guard {
                let g = self.check_expr(guard, &mut arm_scope, ctx, false)?;
                expect_assignable(&g.ty, &Type::Bool, g.span.clone())?;
                effects.union(&g.effects);
                throws = merge_throws(throws, g.throws, g.span)?;
            }
            let arm_body = self.check_expr(&arm.body, &mut arm_scope, ctx, false)?;
            result = Some(unify_arm(result, arm_body.ty, arm_body.span.clone())?);
            effects.union(&arm_body.effects);
            throws = merge_throws(throws, arm_body.throws, arm_body.span)?;
        }
        self.check_exhaustive(arms, &variants, &caught, span)?;
        Ok(ExprInfo {
            ty: result.unwrap_or(Type::Unit),
            effects,
            throws,
            span: span.clone(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn check_arms(
        &self,
        arms: &[MatchArm],
        scrut_ty: &Type,
        variants: &[VariantInfo],
        span: &Span,
        scope: &mut HashMap<String, Type>,
        ctx: &FnCtx,
        allow_throw: bool,
        effects: &mut EffectRow,
        throws: &mut Option<Type>,
    ) -> Result<Type> {
        let mut result: Option<Type> = None;
        for arm in arms {
            let mut arm_scope = scope.clone();
            self.bind_pattern(&arm.pattern, scrut_ty, variants, &mut arm_scope)?;
            if let Some(guard) = &arm.guard {
                let g = self.check_expr(guard, &mut arm_scope, ctx, false)?;
                expect_assignable(&g.ty, &Type::Bool, g.span.clone())?;
                effects.union(&g.effects);
                *throws = merge_throws(throws.clone(), g.throws, g.span)?;
            }
            let arm_body = self.check_expr(&arm.body, &mut arm_scope, ctx, allow_throw)?;
            result = Some(unify_arm(result, arm_body.ty, arm_body.span.clone())?);
            effects.union(&arm_body.effects);
            *throws = merge_throws(throws.clone(), arm_body.throws, arm_body.span)?;
        }
        self.check_exhaustive(arms, variants, &Some(scrut_ty.clone()), span)?;
        Ok(result.unwrap_or(Type::Unit))
    }

    /// The variants a value of `ty` can take when matched (spec 0005).
    fn scrutinee_variants(&self, ty: &Type, span: Span) -> Result<Vec<VariantInfo>> {
        match ty {
            Type::Enum(name) => self
                .enums
                .get(name)
                .map(|info| info.variants.clone())
                .ok_or_else(|| {
                    Error::diagnostic(
                        Diagnostic::new("Unknown enum")
                            .label(span, format!("`{name}` is not a declared enum")),
                    )
                }),
            Type::Option(inner) => Ok(vec![
                VariantInfo {
                    name: "Some".to_string(),
                    fields: vec![(**inner).clone()],
                },
                VariantInfo {
                    name: "None".to_string(),
                    fields: vec![],
                },
            ]),
            _ => Err(Error::diagnostic(Diagnostic::new("Cannot match").label(
                span,
                format!("`match` needs an enum or `Option`, but found `{ty:?}`"),
            ))),
        }
    }

    fn bind_pattern(
        &self,
        pattern: &Pattern,
        scrut_ty: &Type,
        variants: &[VariantInfo],
        scope: &mut HashMap<String, Type>,
    ) -> Result<()> {
        match pattern {
            Pattern::Wildcard(_) => Ok(()),
            Pattern::Binding { name, .. } => {
                scope.insert(name.clone(), scrut_ty.clone());
                Ok(())
            }
            Pattern::Variant {
                variant,
                fields,
                span,
                ..
            } => {
                let Some(vinfo) = variants.iter().find(|v| &v.name == variant) else {
                    return Err(Error::diagnostic(Diagnostic::new("Unknown variant").label(
                        span.clone(),
                        format!("`{variant}` is not a variant of the matched type"),
                    )));
                };
                if fields.len() != vinfo.fields.len() {
                    return Err(Error::diagnostic(
                        Diagnostic::new("Wrong number of fields").label(
                            span.clone(),
                            format!(
                                "`{variant}` has {} field(s), but the pattern binds {}",
                                vinfo.fields.len(),
                                fields.len()
                            ),
                        ),
                    ));
                }
                for (binding, field_ty) in fields.iter().zip(vinfo.fields.iter()) {
                    if let FieldBinding::Name(name) = binding {
                        scope.insert(name.clone(), field_ty.clone());
                    }
                }
                Ok(())
            }
        }
    }

    /// Enforces that the arms cover every variant (spec 0005). Guarded arms do
    /// not count toward coverage because the guard may fail.
    fn check_exhaustive(
        &self,
        arms: &[MatchArm],
        variants: &[VariantInfo],
        scrutinee: &Option<Type>,
        span: &Span,
    ) -> Result<()> {
        let mut covered = HashSet::new();
        let mut catch_all = false;
        for arm in arms {
            if arm.guard.is_some() {
                continue;
            }
            match &arm.pattern {
                Pattern::Wildcard(_) | Pattern::Binding { .. } => catch_all = true,
                Pattern::Variant { variant, .. } => {
                    covered.insert(variant.clone());
                }
            }
        }
        if catch_all {
            return Ok(());
        }
        // A `try` whose body never throws (`scrutinee == None`) has no variants
        // to cover, so an unreachable catch arm is allowed without a wildcard.
        if scrutinee.is_none() {
            return Ok(());
        }
        let missing: Vec<&str> = variants
            .iter()
            .filter(|v| !covered.contains(&v.name))
            .map(|v| v.name.as_str())
            .collect();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(Error::diagnostic(
                Diagnostic::new("Non-exhaustive match")
                    .label(
                        span.clone(),
                        format!("missing case(s): {}", missing.join(", ")),
                    )
                    .help("Add the missing arms, or a wildcard `_ -> ...` arm."),
            ))
        }
    }

    fn check_array(
        &self,
        elements: &[Expr],
        span: &Span,
        scope: &mut HashMap<String, Type>,
        ctx: &FnCtx,
        expected_element: Option<&Type>,
        allow_throw: bool,
    ) -> Result<ExprInfo> {
        let mut effects = EffectRow::default();
        let mut throws = None;
        let mut element_ty = expected_element.cloned();
        for element in elements {
            let actual = self.check_expr(element, scope, ctx, allow_throw)?;
            effects.union(&actual.effects);
            throws = merge_throws(throws, actual.throws, actual.span.clone())?;
            match &element_ty {
                Some(expected) => expect_assignable(&actual.ty, expected, actual.span.clone())?,
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
            throws,
            span: span.clone(),
        })
    }

    fn info(&self, ty: Type, span: Span) -> ExprInfo {
        ExprInfo {
            ty,
            effects: EffectRow::default(),
            throws: None,
            span,
        }
    }
}

/// Combines the error a subexpression may throw with that of its siblings. The
/// throws channel carries a single error type (spec 0011), so two different
/// error types in the same expression are a type error.
fn merge_throws(current: Option<Type>, next: Option<Type>, span: Span) -> Result<Option<Type>> {
    match (current, next) {
        (None, other) | (other, None) => Ok(other),
        (Some(a), Some(b)) if types_compatible(&a, &b) => Ok(Some(a)),
        (Some(b), Some(a)) if types_compatible(&a, &b) => Ok(Some(b)),
        (Some(a), Some(b)) => Err(Error::diagnostic(
            Diagnostic::new("Conflicting error types").label(
                span,
                format!(
                    "this expression mixes errors `{a:?}` and `{b:?}`; use a single error enum"
                ),
            ),
        )),
    }
}

/// Unifies one `match`/`catch` arm body type with the running result type.
fn unify_arm(current: Option<Type>, ty: Type, span: Span) -> Result<Type> {
    match current {
        None => Ok(ty),
        Some(existing) => {
            if types_compatible(&ty, &existing) {
                Ok(existing)
            } else if types_compatible(&existing, &ty) {
                Ok(ty)
            } else {
                Err(Error::diagnostic(
                    Diagnostic::new("Arm type mismatch").label(
                        span,
                        format!("this arm yields `{ty:?}`, but earlier arms yield `{existing:?}`"),
                    ),
                ))
            }
        }
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

fn expect_assignable(actual: &Type, expected: &Type, span: Span) -> Result<()> {
    if types_compatible(actual, expected) {
        return Ok(());
    }
    Err(Error::diagnostic(Diagnostic::new("Type mismatch").label(
        span,
        format!("expected `{expected:?}`, but found `{actual:?}`"),
    )))
}

/// Whether a value of type `actual` is acceptable where `expected` is wanted.
/// `Never` (from `throw`/`panic`) is assignable to anything; `Option<Never>`
/// (from a bare `None`) is assignable to any `Option<T>` (spec 0011).
fn types_compatible(actual: &Type, expected: &Type) -> bool {
    if actual == expected {
        return true;
    }
    match (actual, expected) {
        (Type::Never, _) => true,
        (Type::Option(a), Type::Option(e)) => types_compatible(a, e),
        (Type::Array(a), Type::Array(e)) => types_compatible(a, e),
        _ => false,
    }
}
