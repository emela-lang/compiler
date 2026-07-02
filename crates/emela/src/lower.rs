//! Lowering: the typed AST -> the `emela-codegen` IR.
//!
//! The IR is fully typed, so every node records the type that the type checker
//! already computed. Lambdas additionally record their captured variables, in
//! a stable order, for closure-converting backends. Calls to `extern fn`
//! platform functions (spec 0013) become `IrExpr::Platform` nodes. Enums,
//! `match`, and the error-handling forms (spec 0005/0011) lower to the IR's
//! `EnumValue`/`Match`/`Throw`/`Try`/`Question`/`Panic` nodes.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use emela_codegen::{
    BinaryOp, FunctionType, IrArm, IrCapture, IrExpr, IrFunction, IrParam, IrPattern, IrProgram,
    QuestionMode, Type,
};

use crate::ast::{Block, BlockItem, Expr, FieldBinding, Function, MatchArm, Pattern, Program};
use crate::resolve::{FnTable, Resolved};
use crate::typecheck::{TypedProgram, subst_type};

type Scope = HashMap<String, Type>;

/// A pending monomorphization (spec 0014): a generic function specialized at a
/// concrete set of type arguments, identified by its mangled name.
struct MonoRequest {
    /// The mangled name of the specialized function, e.g. `identity__Int`.
    mangled: String,
    /// Index (in `Program::functions`) of the generic function being specialized.
    template_index: usize,
    /// The concrete binding for each of the generic function's type parameters.
    subst: HashMap<String, Type>,
}

#[derive(Default)]
struct MonoState {
    queue: Vec<MonoRequest>,
    /// Mangled names already requested, so each specialization is emitted once.
    requested: HashSet<String>,
}

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

struct Lowerer<'a> {
    /// Suffix-resolution table over all top-level functions (spec 0018), built
    /// identically to the type checker's so the two passes resolve a path to the
    /// same function — and to the same backend emit name.
    table: FnTable,
    /// All top-level functions, indexed by `FnEntry::index`. Used to build a
    /// resolved call's signature and to fetch generic templates to specialize.
    functions: &'a [Function],
    externs: HashMap<String, ExternInfo>,
    enums: HashMap<String, Vec<VariantDef>>,
    /// Monomorphization worklist, filled while lowering call sites.
    mono: RefCell<MonoState>,
    /// The type-parameter substitution for the specialization currently being
    /// lowered. Empty while lowering an ordinary (non-generic) function, where
    /// `apply` is the identity.
    subst: RefCell<HashMap<String, Type>>,
}

pub(crate) fn lower(program: &Program, typed: &TypedProgram) -> IrProgram {
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
        table: FnTable::build(program),
        functions: &program.functions,
        externs,
        enums,
        mono: RefCell::new(MonoState::default()),
        subst: RefCell::new(HashMap::new()),
    };

    // Lower the ordinary functions (no substitution); calls to generics enqueue
    // specializations into the worklist. The type checker's signatures equal the
    // AST's, so the ret/throws/effects come straight from it.
    let mut functions: Vec<IrFunction> = program
        .functions
        .iter()
        .zip(typed.functions.iter())
        .enumerate()
        .filter(|(_, (function, _))| function.type_params.is_empty())
        .map(|(index, (function, typed))| {
            let mut scope: Scope = function
                .params
                .iter()
                .zip(typed.params.iter())
                .map(|(param, ty)| (param.name.clone(), ty.clone()))
                .collect();
            IrFunction {
                // Unique bare names are kept; colliding imports use a mangled
                // full path so same-named functions coexist (spec 0018).
                name: lowerer.table.emit_name(index).to_string(),
                params: function
                    .params
                    .iter()
                    .zip(typed.params.iter())
                    .map(|(param, ty)| IrParam {
                        name: param.name.clone(),
                        ty: ty.clone(),
                    })
                    .collect(),
                ret: typed.ret.clone(),
                throws: typed.throws.clone(),
                effects: typed.effects.clone(),
                body: lowerer.lower_block(&function.body.items, &mut scope).0,
            }
        })
        .collect();

    // Drain the monomorphization worklist. Each specialization may itself call
    // other generics, enqueueing more, so loop until the queue is empty.
    while let Some(request) = lowerer.next_request() {
        let template = &lowerer.functions[request.template_index];
        *lowerer.subst.borrow_mut() = request.subst;
        let specialized = lowerer.lower_named_function(template, request.mangled);
        lowerer.subst.borrow_mut().clear();
        functions.push(specialized);
    }

    IrProgram { functions }
}

impl<'a> Lowerer<'a> {
    /// Lowers a function body to an `IrFunction` under the current substitution
    /// (`apply`). For an ordinary function the substitution is empty; for a
    /// specialization it maps the type parameters to concrete types.
    fn lower_named_function(&self, function: &Function, name: String) -> IrFunction {
        let mut scope: Scope = function
            .params
            .iter()
            .map(|param| (param.name.clone(), self.apply(&param.ty)))
            .collect();
        IrFunction {
            name,
            params: function
                .params
                .iter()
                .map(|param| IrParam {
                    name: param.name.clone(),
                    ty: self.apply(&param.ty),
                })
                .collect(),
            ret: self.apply(&function.ret),
            throws: function.throws.as_ref().map(|throws| self.apply(throws)),
            effects: function.effects.clone(),
            body: self.lower_block(&function.body.items, &mut scope).0,
        }
    }

    /// Applies the current specialization substitution to a type. Identity when
    /// no specialization is in progress (the common, non-generic case).
    fn apply(&self, ty: &Type) -> Type {
        let subst = self.subst.borrow();
        if subst.is_empty() {
            ty.clone()
        } else {
            subst_type(ty, &subst)
        }
    }

    /// Records a specialization to emit, deduplicating by mangled name.
    fn request_specialization(
        &self,
        mangled: &str,
        template_index: usize,
        subst: HashMap<String, Type>,
    ) {
        let mut mono = self.mono.borrow_mut();
        if mono.requested.insert(mangled.to_string()) {
            mono.queue.push(MonoRequest {
                mangled: mangled.to_string(),
                template_index,
                subst,
            });
        }
    }

    /// The signature of a non-generic top-level function, by its index in
    /// `Program::functions`. Used to build a `FunctionRef` for a resolved call.
    fn fn_type(&self, index: usize) -> FunctionType {
        let function = &self.functions[index];
        FunctionType {
            params: function.params.iter().map(|p| p.ty.clone()).collect(),
            ret: Box::new(function.ret.clone()),
            throws: function.throws.clone().map(Box::new),
            effects: function.effects.clone(),
        }
    }

    fn next_request(&self) -> Option<MonoRequest> {
        self.mono.borrow_mut().queue.pop()
    }

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
                // The annotation may mention this function's type parameters in
                // a specialization, so resolve it under the substitution.
                let annotated = ty.as_ref().map(|ty| self.apply(ty));
                let expected_elem = match (value, &annotated) {
                    (Expr::Array(_, _), Some(Type::Array(element))) => Some(element.as_ref()),
                    _ => None,
                };
                let (value, inferred) = match value {
                    Expr::Array(elements, _) => self.lower_array(elements, scope, expected_elem),
                    _ => self.lower_expr(value, scope),
                };
                let value_ty = annotated.unwrap_or(inferred);
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
                } else if let Resolved::One(entry) = self.table.resolve(std::slice::from_ref(name))
                    && !entry.is_generic
                {
                    let sig = self.fn_type(entry.index);
                    (
                        IrExpr::FunctionRef {
                            name: entry.emit_name.clone(),
                            sig: sig.clone(),
                        },
                        Type::Function(sig),
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
            Expr::Path { segments, .. } => {
                // A bare path with no `(...)`: a no-payload enum variant, or a
                // (qualified) function used as a value (spec 0018). Built-in
                // conversions always carry args and are handled in `lower_call`.
                if let [enum_name, variant] = segments.as_slice()
                    && let Some(variants) = self.enums.get(enum_name)
                    && let Some(def) = variants.iter().find(|v| v.name == *variant)
                {
                    let ty = Type::Enum(enum_name.clone());
                    return (
                        IrExpr::EnumValue {
                            ty: ty.clone(),
                            variant: variant.clone(),
                            tag: def.tag,
                            payload: Vec::new(),
                        },
                        ty,
                    );
                }
                if let Resolved::One(entry) = self.table.resolve(segments)
                    && !entry.is_generic
                {
                    let sig = self.fn_type(entry.index);
                    return (
                        IrExpr::FunctionRef {
                            name: entry.emit_name.clone(),
                            sig: sig.clone(),
                        },
                        Type::Function(sig),
                    );
                }
                (IrExpr::Unit, Type::Unit)
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
                && !self.externs.contains_key(name)
                && matches!(
                    self.table.resolve(std::slice::from_ref(name)),
                    Resolved::None
                )
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
            // A call to a generic function (spec 0014).
            if !scope.contains_key(name)
                && let Resolved::One(entry) = self.table.resolve(std::slice::from_ref(name))
                && entry.is_generic
            {
                return self.lower_generic_call(entry.index, args, scope);
            }
        }
        // A qualified call target (spec 0018): a built-in conversion, an enum
        // variant, or a (possibly generic) qualified function. A non-generic
        // qualified function falls through to the general path, where
        // `lower_expr` on the path yields its `FunctionRef`.
        if let Expr::Path { segments, .. } = callee {
            if let [head, tail] = segments.as_slice() {
                // Built-in pure conversions (spec 0017).
                if head.as_str() == "Char" && tail.as_str() == "from_code" {
                    let (arg, _) = self.lower_expr(&args[0], scope);
                    return (IrExpr::CharFromCode(Box::new(arg)), Type::Char);
                }
                if head.as_str() == "String" && tail.as_str() == "from_char" {
                    let (arg, _) = self.lower_expr(&args[0], scope);
                    return (IrExpr::StringFromChar(Box::new(arg)), Type::String);
                }
                // An enum variant constructor with a payload, e.g. `Color.Red(x)`.
                if let Some(variants) = self.enums.get(head)
                    && let Some(def) = variants.iter().find(|v| v.name == *tail)
                {
                    let payload = args
                        .iter()
                        .map(|arg| self.lower_expr(arg, scope).0)
                        .collect();
                    let ty = Type::Enum(head.clone());
                    return (
                        IrExpr::EnumValue {
                            ty: ty.clone(),
                            variant: tail.clone(),
                            tag: def.tag,
                            payload,
                        },
                        ty,
                    );
                }
            }
            if let Resolved::One(entry) = self.table.resolve(segments)
                && entry.is_generic
            {
                return self.lower_generic_call(entry.index, args, scope);
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

    /// Lowers a call to a generic function (spec 0014): infer its type arguments
    /// from the (now concrete) argument types, request the matching
    /// specialization, and call it by its mangled name. `template_index` is the
    /// generic function's index in `Program::functions`.
    fn lower_generic_call(
        &self,
        template_index: usize,
        args: &[Expr],
        scope: &mut Scope,
    ) -> (IrExpr, Type) {
        let template = &self.functions[template_index];
        let lowered: Vec<(IrExpr, Type)> =
            args.iter().map(|arg| self.lower_expr(arg, scope)).collect();
        let mut subst = HashMap::new();
        for (param, (_, actual)) in template.params.iter().zip(lowered.iter()) {
            infer_subst(&param.ty, actual, &mut subst);
        }
        let mangled = mangle(&template.name, &template.type_params, &subst);
        let sig = FunctionType {
            params: template
                .params
                .iter()
                .map(|param| subst_type(&param.ty, &subst))
                .collect(),
            ret: Box::new(subst_type(&template.ret, &subst)),
            throws: template
                .throws
                .as_ref()
                .map(|throws| Box::new(subst_type(throws, &subst))),
            effects: template.effects.clone(),
        };
        self.request_specialization(&mangled, template_index, subst);
        let ret = (*sig.ret).clone();
        (
            IrExpr::Call {
                callee: Box::new(IrExpr::FunctionRef { name: mangled, sig }),
                args: lowered.into_iter().map(|(expr, _)| expr).collect(),
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
        // A path has no subexpressions; when it is a call target its arguments
        // live in the wrapping `Call`, which is handled above.
        Expr::Path { .. } => {}
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

/// Binds a generic function's type parameters by matching a declared parameter
/// type (which may contain type variables) against the concrete `actual` type of
/// the argument (spec 0014). The type checker has already validated the call, so
/// this is total: structural mismatches cannot occur here.
fn infer_subst(declared: &Type, actual: &Type, subst: &mut HashMap<String, Type>) {
    match (declared, actual) {
        (Type::Var(name), _) => {
            // Prefer a concrete binding over `Never` (from `throw`/`None`).
            match subst.get(name) {
                Some(bound) if *bound != Type::Never => {}
                _ => {
                    subst.insert(name.clone(), actual.clone());
                }
            }
        }
        (Type::Array(d), Type::Array(a)) => infer_subst(d, a, subst),
        (Type::Option(d), Type::Option(a)) => infer_subst(d, a, subst),
        (Type::Function(d), Type::Function(a)) if d.params.len() == a.params.len() => {
            for (dp, ap) in d.params.iter().zip(a.params.iter()) {
                infer_subst(dp, ap, subst);
            }
            infer_subst(&d.ret, &a.ret, subst);
        }
        _ => {}
    }
}

/// The mangled name of a specialization, e.g. `identity` at `T = Int` becomes
/// `identity__Int`. Deterministic and identifier-safe so backends can use it
/// verbatim. Type parameters are appended in declaration order.
fn mangle(name: &str, type_params: &[String], subst: &HashMap<String, Type>) -> String {
    let mut mangled = name.to_string();
    for type_param in type_params {
        mangled.push_str("__");
        mangled.push_str(&mangle_type(subst.get(type_param).unwrap_or(&Type::Unit)));
    }
    mangled
}

/// An identifier-safe encoding of a concrete type for name mangling.
fn mangle_type(ty: &Type) -> String {
    match ty {
        Type::Unit => "Unit".to_string(),
        Type::Bool => "Bool".to_string(),
        Type::Int => "Int".to_string(),
        Type::Float => "Float".to_string(),
        Type::String => "String".to_string(),
        Type::Char => "Char".to_string(),
        Type::Array(element) => format!("Array_{}_", mangle_type(element)),
        Type::Record => "Record".to_string(),
        Type::Enum(name) => name.clone(),
        Type::Option(inner) => format!("Option_{}_", mangle_type(inner)),
        Type::Never => "Never".to_string(),
        Type::Function(function) => {
            let params = function
                .params
                .iter()
                .map(mangle_type)
                .collect::<Vec<_>>()
                .join("_");
            format!("Fn_{params}_to_{}_", mangle_type(&function.ret))
        }
        Type::OpaqueFunction => "Fn".to_string(),
        // A type variable cannot survive substitution at a concrete call site.
        Type::Var(name) => name.clone(),
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
    fn monomorphizes_generic_call() {
        let ir =
            lower_source("fn identity<T>(x: T) -> T { x }\nfn main() -> Int { identity(42) }\n");
        let names: Vec<&str> = ir.functions.iter().map(|f| f.name.as_str()).collect();
        // The call is specialized and the generic template is not emitted.
        assert!(names.contains(&"identity__Int"), "names: {names:?}");
        assert!(!names.contains(&"identity"), "names: {names:?}");
        // The specialization is fully concrete.
        let spec = ir
            .functions
            .iter()
            .find(|function| function.name == "identity__Int")
            .expect("identity__Int");
        assert_eq!(spec.ret, Type::Int);
        assert_eq!(spec.params[0].ty, Type::Int);
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
