use std::collections::{BTreeSet, HashMap};

use serde::Serialize;

use crate::ast::{
    BinaryOp, Block, BlockItem, Capability, EnumDecl, Expr, FunctionType as AstFunctionType,
    ImportOrigin, MatchArm, Pattern, PrimType, Program, StructDecl, TopLevelItem, Type,
};
use crate::error::{Error, Result};
use crate::platform::PlatformSpec;

#[derive(Debug, Clone)]
struct TypeSlot {
    parent: usize,
    value: Option<Type>,
}

#[derive(Debug, Clone)]
struct FunctionType {
    params: Vec<usize>,
    ret: usize,
    effectful: bool,
    declared_capabilities: Option<BTreeSet<Capability>>,
}

#[derive(Debug, Clone)]
struct VariantInfo {
    enum_name: String,
    enum_type_params: Vec<String>,
    payload: Option<Type>,
}

#[derive(Debug, Clone)]
struct ExprInfo {
    ty: usize,
    effectful: bool,
    capabilities: BTreeSet<Capability>,
}

pub(crate) struct TypeChecker<'a> {
    program: &'a Program,
    platform: &'a PlatformSpec,
    mode: CheckMode,
    types: Vec<TypeSlot>,
    structs: HashMap<String, &'a StructDecl>,
    enums: HashMap<String, &'a EnumDecl>,
    variants: HashMap<String, VariantInfo>,
    functions: HashMap<String, FunctionType>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckMode {
    Executable,
    Library,
}

impl<'a> TypeChecker<'a> {
    #[cfg(test)]
    pub(crate) fn new(program: &'a Program, platform: &'a PlatformSpec) -> Self {
        Self::new_with_mode(program, platform, CheckMode::Executable)
    }

    pub(crate) fn new_with_mode(
        program: &'a Program,
        platform: &'a PlatformSpec,
        mode: CheckMode,
    ) -> Self {
        Self {
            program,
            platform,
            mode,
            types: Vec::new(),
            structs: HashMap::new(),
            enums: HashMap::new(),
            variants: HashMap::new(),
            functions: HashMap::new(),
        }
    }

    pub(crate) fn check(mut self) -> Result<TypedProgram> {
        self.register_types()?;
        self.register_imports()?;
        self.register_functions()?;
        if self.mode == CheckMode::Executable {
            self.check_main()?;
        }

        let functions = self.program.functions();
        let mut function_capabilities = HashMap::new();
        for function in &functions {
            let signature = self
                .functions
                .get(&function.name)
                .cloned()
                .ok_or_else(|| Error::new("internal type checker error"))?;
            let mut scope = HashMap::new();
            for (param, ty) in function.params.iter().zip(signature.params.iter()) {
                if scope.insert(param.name.clone(), *ty).is_some() {
                    return Err(Error::new(format!(
                        "duplicate parameter `{}` in function `{}`",
                        param.name, function.name
                    )));
                }
            }

            let body = self.check_block(&function.body, &mut scope)?;
            self.unify(signature.ret, body.ty)?;

            if !signature.effectful && body.effectful {
                return Err(Error::new(format!(
                    "pure function `{}` cannot contain unhandled effects",
                    function.name
                )));
            }

            if let Some(declared) = &signature.declared_capabilities {
                if !body.capabilities.is_subset(declared) {
                    let missing = body
                        .capabilities
                        .difference(declared)
                        .map(|capability| format!("{capability:?}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Err(Error::new(format!(
                        "function `{}` uses capability outside #[requires(...)]: {missing}",
                        function.name
                    )));
                }
            }

            let function_caps = signature
                .declared_capabilities
                .clone()
                .unwrap_or(body.capabilities);
            function_capabilities.insert(function.name.clone(), function_caps);
        }

        if self.mode == CheckMode::Executable {
            self.check_runtime_boundary(&function_capabilities)?;
        }

        let mut typed_functions = Vec::new();
        for function in &functions {
            let signature = self
                .functions
                .get(&function.name)
                .cloned()
                .ok_or_else(|| Error::new("internal type checker error"))?;
            let params = signature
                .params
                .iter()
                .map(|id| self.resolve_known(*id, &format!("parameter in `{}`", function.name)))
                .collect::<Result<Vec<_>>>()?;
            let ret = self.resolve_known(
                signature.ret,
                &format!("return type of `{}`", function.name),
            )?;
            typed_functions.push(TypedFunction {
                name: function.name.clone(),
                params,
                ret,
                effectful: signature.effectful,
                capabilities: function_capabilities
                    .remove(&function.name)
                    .unwrap_or_default()
                    .into_iter()
                    .collect(),
            });
        }

        Ok(TypedProgram {
            functions: typed_functions,
        })
    }

    fn register_types(&mut self) -> Result<()> {
        self.register_builtin_types();
        for item in &self.program.items {
            match item {
                TopLevelItem::Struct(decl) => {
                    if self.structs.contains_key(&decl.name) || self.enums.contains_key(&decl.name)
                    {
                        return Err(Error::new(format!("duplicate type `{}`", decl.name)));
                    }
                    self.structs.insert(decl.name.clone(), decl);
                }
                TopLevelItem::Enum(decl) => {
                    if decl.variants.is_empty() {
                        return Err(Error::new(format!(
                            "enum `{}` must declare at least one variant",
                            decl.name
                        )));
                    }
                    if self.structs.contains_key(&decl.name) || self.enums.contains_key(&decl.name)
                    {
                        return Err(Error::new(format!("duplicate type `{}`", decl.name)));
                    }
                    for variant in &decl.variants {
                        if self.variants.contains_key(&variant.name) {
                            return Err(Error::new(format!(
                                "duplicate enum variant `{}`",
                                variant.name
                            )));
                        }
                        self.variants.insert(
                            variant.name.clone(),
                            VariantInfo {
                                enum_name: decl.name.clone(),
                                enum_type_params: decl.type_params.clone(),
                                payload: variant.payload.clone(),
                            },
                        );
                    }
                    self.enums.insert(decl.name.clone(), decl);
                }
                TopLevelItem::Function(_) | TopLevelItem::Import(_) => {}
            }
        }

        for decl in self.structs.values() {
            let mut seen = BTreeSet::new();
            for param in &decl.type_params {
                if !seen.insert(param) {
                    return Err(Error::new(format!(
                        "duplicate type parameter `{param}` in struct `{}`",
                        decl.name
                    )));
                }
            }
            self.validate_type_in_scope(&decl.field.ty, &decl.type_params)?;
        }
        for decl in self.enums.values() {
            let mut seen = BTreeSet::new();
            for param in &decl.type_params {
                if !seen.insert(param) {
                    return Err(Error::new(format!(
                        "duplicate type parameter `{param}` in enum `{}`",
                        decl.name
                    )));
                }
            }
            for variant in &decl.variants {
                if let Some(payload) = &variant.payload {
                    self.validate_type_in_scope(payload, &decl.type_params)?;
                }
            }
        }
        Ok(())
    }

    fn register_builtin_types(&mut self) {
        self.variants.insert(
            "Ok".to_string(),
            VariantInfo {
                enum_name: "Result".to_string(),
                enum_type_params: vec!["T".to_string(), "E".to_string()],
                payload: Some(Type::GenericParam("T".to_string())),
            },
        );
        self.variants.insert(
            "Err".to_string(),
            VariantInfo {
                enum_name: "Result".to_string(),
                enum_type_params: vec!["T".to_string(), "E".to_string()],
                payload: Some(Type::GenericParam("E".to_string())),
            },
        );
        for name in [
            "Unsupported",
            "Unavailable",
            "Interrupted",
            "InvalidUtf8",
            "Unknown",
        ] {
            self.variants.insert(
                name.to_string(),
                VariantInfo {
                    enum_name: "PlatformError".to_string(),
                    enum_type_params: Vec::new(),
                    payload: None,
                },
            );
        }
    }

    fn register_imports(&mut self) -> Result<()> {
        for item in &self.program.items {
            let TopLevelItem::Import(import) = item else {
                continue;
            };
            if import.origin == ImportOrigin::User
                && import
                    .path
                    .first()
                    .is_some_and(|package| package == "platform")
            {
                return Err(Error::new(format!(
                    "platform import `{}` is only available to stdlib",
                    format_import_path(&import.path, &import.name)
                )));
            }
            if self.functions.contains_key(&import.name) {
                return Err(Error::new(format!(
                    "duplicate imported function `{}`",
                    import.name
                )));
            }
            let function = self
                .platform
                .externs
                .resolve_import(&import.path, &import.name)
                .ok_or_else(|| {
                    Error::new(format!(
                        "unknown external import `{}`",
                        format_import_path(&import.path, &import.name)
                    ))
                })?;
            let params = function
                .params
                .iter()
                .map(|ty| self.known(ty.clone()))
                .collect();
            let ret = self.known(function.ret.clone());
            self.functions.insert(
                import.name.clone(),
                FunctionType {
                    params,
                    ret,
                    effectful: function.effectful,
                    declared_capabilities: Some(function.capabilities.iter().copied().collect()),
                },
            );
        }
        Ok(())
    }

    fn register_functions(&mut self) -> Result<()> {
        for function in self.program.functions() {
            if self.functions.contains_key(&function.name) {
                return Err(Error::new(format!(
                    "duplicate top-level function `{}`",
                    function.name
                )));
            }

            let declared_capabilities = function
                .requires
                .as_ref()
                .map(|capabilities| capabilities.iter().copied().collect::<BTreeSet<_>>());
            let has_capabilities = declared_capabilities
                .as_ref()
                .is_some_and(|capabilities| !capabilities.is_empty());
            let effectful = function.name.ends_with('!');
            if has_capabilities && !effectful {
                return Err(Error::new(format!(
                    "function `{}` requires platform capabilities and must be marked with !",
                    function.name
                )));
            }

            let params = function
                .params
                .iter()
                .map(|param| match &param.ty {
                    Some(ty) => {
                        self.validate_type(ty)?;
                        Ok(self.known(ty.clone()))
                    }
                    None => Err(Error::new(format!(
                        "parameter `{}` in function `{}` must have a type annotation",
                        param.name, function.name
                    ))),
                })
                .collect::<Result<Vec<_>>>()?;
            let ret = match &function.return_annotation {
                Some(ty) => {
                    self.validate_type(ty)?;
                    self.known(ty.clone())
                }
                None => {
                    return Err(Error::new(format!(
                        "function `{}` must have a return type annotation",
                        function.name
                    )));
                }
            };
            self.functions.insert(
                function.name.clone(),
                FunctionType {
                    params,
                    ret,
                    effectful,
                    declared_capabilities,
                },
            );
        }
        Ok(())
    }

    fn check_main(&self) -> Result<()> {
        let functions = self.program.functions();
        let entry_count = functions
            .iter()
            .filter(|function| function.name == "main" || function.name == "main!")
            .count();
        if entry_count != 1 {
            return Err(Error::new(
                "executable program must contain exactly one top-level `main` or `main!` function",
            ));
        }
        let main = functions
            .iter()
            .find(|function| function.name == "main" || function.name == "main!")
            .expect("entry point was counted above");
        if !main.params.is_empty() {
            return Err(Error::new("`main` and `main!` must take zero parameters"));
        }
        Ok(())
    }

    fn check_runtime_boundary(
        &self,
        function_capabilities: &HashMap<String, BTreeSet<Capability>>,
    ) -> Result<()> {
        let functions = self.program.functions();
        let entry = functions
            .iter()
            .find(|function| function.name == "main" || function.name == "main!")
            .expect("entry point was checked above");
        let required = function_capabilities
            .get(&entry.name)
            .cloned()
            .unwrap_or_default();
        let provided = &self.platform.provided_capabilities;
        if !required.is_subset(provided) {
            let missing = required
                .difference(provided)
                .map(|capability| format!("{capability:?}"))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(Error::new(format!(
                "platform `{}` does not provide required capability: {missing}",
                self.platform
            )));
        }
        Ok(())
    }

    fn check_block(
        &mut self,
        block: &Block,
        outer_scope: &mut HashMap<String, usize>,
    ) -> Result<ExprInfo> {
        let mut scope = outer_scope.clone();
        let mut last_expr = None;
        let mut effectful = false;
        let mut capabilities = BTreeSet::new();
        for item in &block.items {
            match item {
                BlockItem::Binding { name, ty, expr } => {
                    if scope.contains_key(name) {
                        return Err(Error::new(format!(
                            "duplicate binding `{name}` in the same block"
                        )));
                    }
                    let info = self.check_expr(expr, &mut scope)?;
                    let Some(annotation) = ty else {
                        return Err(Error::new(format!(
                            "binding `{name}` must have a type annotation"
                        )));
                    };
                    self.validate_type(annotation)?;
                    let annotated_ty = self.known(annotation.clone());
                    self.unify(info.ty, annotated_ty)?;
                    effectful |= info.effectful;
                    capabilities.extend(info.capabilities);
                    scope.insert(name.clone(), info.ty);
                    last_expr = None;
                }
                BlockItem::Expr(expr) => {
                    let info = self.check_expr(expr, &mut scope)?;
                    effectful |= info.effectful;
                    capabilities.extend(info.capabilities.clone());
                    last_expr = Some(info);
                }
            }
        }
        Ok(ExprInfo {
            ty: last_expr
                .map(|info| info.ty)
                .unwrap_or_else(|| self.known(Type::Prim(PrimType::Unit))),
            effectful,
            capabilities,
        })
    }

    fn check_expr(&mut self, expr: &Expr, scope: &mut HashMap<String, usize>) -> Result<ExprInfo> {
        match expr {
            Expr::Int(_) => {
                let ty = self.known(Type::Prim(PrimType::I32));
                Ok(self.info(ty))
            }
            Expr::Bool(_) => {
                let ty = self.known(Type::Prim(PrimType::Bool));
                Ok(self.info(ty))
            }
            Expr::String(_) => {
                let ty = self.known(Type::Prim(PrimType::String));
                Ok(self.info(ty))
            }
            Expr::Unit => {
                let ty = self.known(Type::Prim(PrimType::Unit));
                Ok(self.info(ty))
            }
            Expr::Var(name) => {
                if let Some(ty) = scope.get(name).copied() {
                    return Ok(self.info(ty));
                }
                if let Some(variant) = self.variants.get(name).cloned() {
                    if variant.payload.is_some() {
                        return Err(Error::new(format!(
                            "enum variant `{name}` requires a payload"
                        )));
                    }
                    let ty = self.known(enum_result_type(&variant, Vec::new()));
                    return Ok(self.info(ty));
                }
                if let Some(function_ty) = self.function_value_type(name) {
                    let ty = self.known(function_ty);
                    return Ok(self.info(ty));
                }
                Err(Error::new(format!("unknown local binding `{name}`")))
            }
            Expr::Call { name, args } => {
                if let Some(variant) = self.variants.get(name).cloned() {
                    return self.check_variant_constructor(name, &variant, args, scope);
                }

                if let Some(callee_ty) = scope.get(name).copied() {
                    return self.check_function_value_call(name, callee_ty, args, scope);
                }

                let signature = self
                    .functions
                    .get(name)
                    .cloned()
                    .ok_or_else(|| Error::new(format!("unknown function `{name}`")))?;
                if args.len() != signature.params.len() {
                    return Err(Error::new(format!(
                        "function `{name}` expects {} argument(s), got {}",
                        signature.params.len(),
                        args.len()
                    )));
                }

                let mut effectful = signature.effectful;
                let mut capabilities = signature.declared_capabilities.clone().unwrap_or_default();
                for (arg, param_ty) in args.iter().zip(signature.params.iter()) {
                    let arg = self.check_expr(arg, scope)?;
                    self.unify(arg.ty, *param_ty)?;
                    effectful |= arg.effectful;
                    capabilities.extend(arg.capabilities);
                }
                Ok(ExprInfo {
                    ty: signature.ret,
                    effectful,
                    capabilities,
                })
            }
            Expr::MethodCall {
                receiver,
                name,
                args,
            } => self.check_method_call(receiver, name, args, scope),
            Expr::FieldAccess { receiver, field } => {
                let receiver = self.check_expr(receiver, scope)?;
                let receiver_ty = self.resolve_known(receiver.ty, "field receiver type")?;
                let Type::Named(type_name) = receiver_ty else {
                    return Err(Error::new(format!(
                        "type {:?} does not have field `{field}`",
                        receiver_ty
                    )));
                };
                let decl = self
                    .structs
                    .get(&type_name)
                    .ok_or_else(|| Error::new(format!("type `{type_name}` has no fields")))?;
                if decl.field.name != *field {
                    return Err(Error::new(format!(
                        "struct `{type_name}` does not have field `{field}`"
                    )));
                }
                let ty = self.known(decl.field.ty.clone());
                Ok(ExprInfo {
                    ty,
                    effectful: receiver.effectful,
                    capabilities: receiver.capabilities,
                })
            }
            Expr::StructLiteral { name, field, value } => {
                let decl = self
                    .structs
                    .get(name)
                    .ok_or_else(|| Error::new(format!("unknown struct `{name}`")))?;
                if decl.field.name != *field {
                    return Err(Error::new(format!(
                        "struct `{name}` does not have field `{field}`"
                    )));
                }
                let expected_ty = decl.field.ty.clone();
                let value = self.check_expr(value, scope)?;
                let expected_ty = self.known(expected_ty);
                self.unify(value.ty, expected_ty)?;
                Ok(ExprInfo {
                    ty: self.known(Type::Named(name.clone())),
                    effectful: value.effectful,
                    capabilities: value.capabilities,
                })
            }
            Expr::Binary { op, left, right } => self.check_binary(*op, left, right, scope),
            Expr::Match { scrutinee, arms } => self.check_match(scrutinee, arms, scope),
            Expr::Block(block) => self.check_block(block, scope),
        }
    }

    fn check_variant_constructor(
        &mut self,
        name: &str,
        variant: &VariantInfo,
        args: &[Expr],
        scope: &mut HashMap<String, usize>,
    ) -> Result<ExprInfo> {
        let Some(payload_ty) = &variant.payload else {
            return Err(Error::new(format!(
                "enum variant `{name}` does not take a payload"
            )));
        };
        if args.len() != 1 {
            return Err(Error::new(format!(
                "enum variant `{name}` expects 1 payload argument, got {}",
                args.len()
            )));
        }
        let mut substitutions = HashMap::new();
        let arg = self.check_expr(&args[0], scope)?;
        let arg_ty = self.resolve_known(arg.ty, "enum variant payload type")?;
        infer_type_arguments(payload_ty, &arg_ty, &mut substitutions);
        let result_args = variant
            .enum_type_params
            .iter()
            .map(|param| {
                substitutions
                    .get(param)
                    .cloned()
                    .unwrap_or_else(|| Type::GenericParam(param.clone()))
            })
            .collect::<Vec<_>>();
        let expected_ty = substitute_type(payload_ty, &substitutions);
        let expected = self.known(expected_ty);
        self.unify(arg.ty, expected)?;
        Ok(ExprInfo {
            ty: self.known(enum_result_type(variant, result_args)),
            effectful: arg.effectful,
            capabilities: arg.capabilities,
        })
    }

    fn check_function_value_call(
        &mut self,
        name: &str,
        callee_ty: usize,
        args: &[Expr],
        scope: &mut HashMap<String, usize>,
    ) -> Result<ExprInfo> {
        let callee_ty = self.resolve_known(callee_ty, "function value callee type")?;
        let Type::Function(function_ty) = callee_ty else {
            return Err(Error::new(format!("`{name}` is not callable")));
        };
        if args.len() != function_ty.params.len() {
            return Err(Error::new(format!(
                "function value `{name}` expects {} argument(s), got {}",
                function_ty.params.len(),
                args.len()
            )));
        }

        let mut effectful = function_ty.effectful;
        let mut capabilities = BTreeSet::new();
        for (arg, param_ty) in args.iter().zip(function_ty.params.iter()) {
            let arg = self.check_expr(arg, scope)?;
            let param_ty = self.known(param_ty.clone());
            self.unify(arg.ty, param_ty)?;
            effectful |= arg.effectful;
            capabilities.extend(arg.capabilities);
        }

        Ok(ExprInfo {
            ty: self.known(*function_ty.ret),
            effectful,
            capabilities,
        })
    }

    fn check_match(
        &mut self,
        scrutinee: &Expr,
        arms: &[MatchArm],
        scope: &mut HashMap<String, usize>,
    ) -> Result<ExprInfo> {
        if arms.is_empty() {
            return Err(Error::new("match expression must have at least one arm"));
        }

        let scrutinee = self.check_expr(scrutinee, scope)?;
        let scrutinee_ty = self.resolve_known(scrutinee.ty, "match scrutinee type")?;
        let mut effectful = scrutinee.effectful;
        let mut capabilities = scrutinee.capabilities;
        let mut result_ty = None;

        for arm in arms {
            let mut arm_scope = scope.clone();
            self.check_pattern(&arm.pattern, &scrutinee_ty, &mut arm_scope)?;
            let arm = self.check_expr(&arm.expr, &mut arm_scope)?;
            effectful |= arm.effectful;
            capabilities.extend(arm.capabilities);
            if let Some(existing) = result_ty {
                self.unify(existing, arm.ty)?;
            } else {
                result_ty = Some(arm.ty);
            }
        }

        if !self.match_is_exhaustive(&scrutinee_ty, arms) {
            return Err(Error::new("match expression is not exhaustive"));
        }

        Ok(ExprInfo {
            ty: result_ty.expect("non-empty arms checked above"),
            effectful,
            capabilities,
        })
    }

    fn check_pattern(
        &mut self,
        pattern: &Pattern,
        expected: &Type,
        scope: &mut HashMap<String, usize>,
    ) -> Result<()> {
        match pattern {
            Pattern::Int(_) => self.expect_pattern_type(expected, Type::Prim(PrimType::I32)),
            Pattern::Bool(_) => self.expect_pattern_type(expected, Type::Prim(PrimType::Bool)),
            Pattern::Unit => self.expect_pattern_type(expected, Type::Prim(PrimType::Unit)),
            Pattern::Wildcard => Ok(()),
            Pattern::Var(name) => {
                if scope.contains_key(name) {
                    return Err(Error::new(format!(
                        "pattern binding `{name}` shadows an existing binding"
                    )));
                }
                let ty = self.known(expected.clone());
                scope.insert(name.clone(), ty);
                Ok(())
            }
            Pattern::Variant { name, payload } => {
                let variant = self
                    .variants
                    .get(name)
                    .cloned()
                    .ok_or_else(|| Error::new(format!("unknown enum variant `{name}`")))?;
                let substitutions = self.pattern_type_arguments(expected, &variant)?;
                self.expect_pattern_type(
                    expected,
                    enum_result_type(&variant, substitutions.clone()),
                )?;
                match (&variant.payload, payload) {
                    (Some(payload_ty), Some(payload_pattern)) => {
                        let payload_ty = substitute_type_for_params(
                            payload_ty,
                            &variant.enum_type_params,
                            &substitutions,
                        );
                        self.check_pattern(payload_pattern, &payload_ty, scope)
                    }
                    (Some(_), None) => Err(Error::new(format!(
                        "enum variant `{name}` pattern requires a payload"
                    ))),
                    (None, Some(_)) => Err(Error::new(format!(
                        "enum variant `{name}` pattern does not take a payload"
                    ))),
                    (None, None) => Ok(()),
                }
            }
        }
    }

    fn expect_pattern_type(&self, expected: &Type, actual: Type) -> Result<()> {
        if *expected == actual {
            Ok(())
        } else {
            Err(Error::new(format!(
                "type mismatch: expected {:?}, got {:?}",
                expected, actual
            )))
        }
    }

    fn check_method_call(
        &mut self,
        receiver: &Expr,
        name: &str,
        args: &[Expr],
        scope: &mut HashMap<String, usize>,
    ) -> Result<ExprInfo> {
        let receiver = self.check_expr(receiver, scope)?;
        let mut effectful = receiver.effectful;
        let mut capabilities = receiver.capabilities;

        let (receiver_constraint, expected_args, ret) = match name {
            "add" | "sub" | "mul" => (Some(PrimType::I32), vec![PrimType::I32], PrimType::I32),
            "lt" => (Some(PrimType::I32), vec![PrimType::I32], PrimType::Bool),
            "eq" => {
                let receiver_ty = self.resolve_optional(receiver.ty);
                match receiver_ty {
                    Some(Type::Prim(PrimType::I32)) => {
                        (Some(PrimType::I32), vec![PrimType::I32], PrimType::Bool)
                    }
                    Some(Type::Prim(PrimType::Bool)) => {
                        (Some(PrimType::Bool), vec![PrimType::Bool], PrimType::Bool)
                    }
                    Some(Type::Prim(PrimType::Unit)) => {
                        return Err(Error::new("type Unit does not implement method `eq`"));
                    }
                    Some(other) => {
                        return Err(Error::new(format!(
                            "type {:?} does not implement method `eq`",
                            other
                        )));
                    }
                    None => (None, Vec::new(), PrimType::Bool),
                }
            }
            _ => {
                let receiver_ty = self
                    .resolve_optional(receiver.ty)
                    .map(|ty| format!("{ty:?}"))
                    .unwrap_or_else(|| "unknown type".to_string());
                return Err(Error::new(format!(
                    "{receiver_ty} does not implement method `{name}`"
                )));
            }
        };

        if name == "eq" && receiver_constraint.is_none() {
            if args.len() != 1 {
                return Err(Error::new(format!(
                    "method `{name}` expects 1 argument(s), got {}",
                    args.len()
                )));
            }
            let arg = self.check_expr(&args[0], scope)?;
            self.unify(receiver.ty, arg.ty)?;
            effectful |= arg.effectful;
            capabilities.extend(arg.capabilities);
            return Ok(ExprInfo {
                ty: self.known(Type::Prim(ret)),
                effectful,
                capabilities,
            });
        }

        if let Some(receiver_constraint) = receiver_constraint {
            let receiver_constraint = self.known(Type::Prim(receiver_constraint));
            self.unify(receiver.ty, receiver_constraint)?;
        }

        if args.len() != expected_args.len() {
            return Err(Error::new(format!(
                "method `{name}` expects {} argument(s), got {}",
                expected_args.len(),
                args.len()
            )));
        }

        for (arg, expected_ty) in args.iter().zip(expected_args.iter()) {
            let arg = self.check_expr(arg, scope)?;
            let expected_ty = self.known(Type::Prim(*expected_ty));
            self.unify(arg.ty, expected_ty)?;
            effectful |= arg.effectful;
            capabilities.extend(arg.capabilities);
        }

        Ok(ExprInfo {
            ty: self.known(Type::Prim(ret)),
            effectful,
            capabilities,
        })
    }

    fn check_binary(
        &mut self,
        op: BinaryOp,
        left: &Expr,
        right: &Expr,
        scope: &mut HashMap<String, usize>,
    ) -> Result<ExprInfo> {
        let method = match op {
            BinaryOp::Add => "add",
            BinaryOp::Sub => "sub",
            BinaryOp::Mul => "mul",
            BinaryOp::Eq => "eq",
            BinaryOp::Lt => "lt",
        };
        self.check_method_call(left, method, std::slice::from_ref(right), scope)
    }

    fn match_is_exhaustive(&self, scrutinee: &Type, arms: &[MatchArm]) -> bool {
        if arms
            .iter()
            .any(|arm| matches!(arm.pattern, Pattern::Wildcard | Pattern::Var(_)))
        {
            return true;
        }

        match scrutinee {
            Type::Prim(PrimType::Bool) => {
                let has_true = arms
                    .iter()
                    .any(|arm| matches!(arm.pattern, Pattern::Bool(true)));
                let has_false = arms
                    .iter()
                    .any(|arm| matches!(arm.pattern, Pattern::Bool(false)));
                has_true && has_false
            }
            Type::Prim(PrimType::Unit) => {
                arms.iter().any(|arm| matches!(arm.pattern, Pattern::Unit))
            }
            Type::Prim(PrimType::I32) | Type::Prim(PrimType::String) => false,
            Type::Function(_) | Type::GenericParam(_) => false,
            Type::Named(name) => self.enum_variant_names(name).is_some_and(|variants| {
                variants.iter().all(|variant_name| {
                    arms.iter().any(|arm| match &arm.pattern {
                        Pattern::Variant { name, .. } => name == variant_name,
                        _ => false,
                    })
                })
            }),
            Type::Apply { name, .. } => self.enum_variant_names(name).is_some_and(|variants| {
                variants.iter().all(|variant_name| {
                    arms.iter().any(|arm| match &arm.pattern {
                        Pattern::Variant { name, .. } => name == variant_name,
                        _ => false,
                    })
                })
            }),
        }
    }

    fn enum_variant_names(&self, name: &str) -> Option<Vec<String>> {
        if name == "Result" {
            return Some(vec!["Ok".to_string(), "Err".to_string()]);
        }
        if name == "PlatformError" {
            return Some(
                [
                    "Unsupported",
                    "Unavailable",
                    "Interrupted",
                    "InvalidUtf8",
                    "Unknown",
                ]
                .into_iter()
                .map(str::to_string)
                .collect(),
            );
        }
        self.enums.get(name).map(|decl| {
            decl.variants
                .iter()
                .map(|variant| variant.name.clone())
                .collect()
        })
    }

    fn validate_type(&self, ty: &Type) -> Result<()> {
        self.validate_type_in_scope(ty, &[])
    }

    fn validate_type_in_scope(&self, ty: &Type, type_params: &[String]) -> Result<()> {
        match ty {
            Type::Prim(_) => Ok(()),
            Type::Named(name) => {
                if type_params.contains(name)
                    || name == "PlatformError"
                    || self
                        .structs
                        .get(name)
                        .is_some_and(|decl| decl.type_params.is_empty())
                    || self
                        .enums
                        .get(name)
                        .is_some_and(|decl| decl.type_params.is_empty())
                {
                    Ok(())
                } else {
                    Err(Error::new(format!("unknown type `{name}`")))
                }
            }
            Type::GenericParam(name) => {
                if type_params.contains(name) {
                    Ok(())
                } else {
                    Err(Error::new(format!("unknown type parameter `{name}`")))
                }
            }
            Type::Apply { name, args } => {
                let expected = if name == "Result" {
                    Some(2)
                } else if let Some(decl) = self.structs.get(name) {
                    Some(decl.type_params.len())
                } else {
                    self.enums.get(name).map(|decl| decl.type_params.len())
                }
                .ok_or_else(|| Error::new(format!("unknown generic type `{name}`")))?;
                if args.len() != expected {
                    return Err(Error::new(format!(
                        "generic type `{name}` expects {expected} type argument(s), got {}",
                        args.len()
                    )));
                }
                for arg in args {
                    self.validate_type_in_scope(arg, type_params)?;
                }
                Ok(())
            }
            Type::Function(function) => {
                for param in &function.params {
                    self.validate_type_in_scope(param, type_params)?;
                }
                self.validate_type_in_scope(&function.ret, type_params)
            }
        }
    }

    fn pattern_type_arguments(&self, expected: &Type, variant: &VariantInfo) -> Result<Vec<Type>> {
        if variant.enum_type_params.is_empty() {
            self.expect_pattern_type(expected, Type::Named(variant.enum_name.clone()))?;
            return Ok(Vec::new());
        }
        let Type::Apply { name, args } = expected else {
            return Err(Error::new(format!(
                "type mismatch: expected generic enum `{}`, got {:?}",
                variant.enum_name, expected
            )));
        };
        if name != &variant.enum_name {
            return Err(Error::new(format!(
                "type mismatch: expected {:?}, got {:?}",
                enum_result_type(variant, args.clone()),
                expected
            )));
        }
        Ok(args.clone())
    }

    fn function_value_type(&mut self, name: &str) -> Option<Type> {
        let signature = self.functions.get(name)?.clone();
        let params = signature
            .params
            .iter()
            .map(|param| self.resolve_known(*param, &format!("parameter in `{name}`")))
            .collect::<Result<Vec<_>>>()
            .ok()?;
        let ret = self
            .resolve_known(signature.ret, &format!("return type of `{name}`"))
            .ok()?;
        Some(Type::Function(AstFunctionType {
            params,
            ret: Box::new(ret),
            effectful: signature.effectful,
        }))
    }

    fn info(&self, ty: usize) -> ExprInfo {
        ExprInfo {
            ty,
            effectful: false,
            capabilities: BTreeSet::new(),
        }
    }

    fn fresh(&mut self) -> usize {
        let id = self.types.len();
        self.types.push(TypeSlot {
            parent: id,
            value: None,
        });
        id
    }

    fn known(&mut self, ty: Type) -> usize {
        let id = self.fresh();
        self.types[id].value = Some(ty);
        id
    }

    fn find(&mut self, id: usize) -> usize {
        if self.types[id].parent != id {
            let parent = self.types[id].parent;
            let root = self.find(parent);
            self.types[id].parent = root;
        }
        self.types[id].parent
    }

    fn unify(&mut self, a: usize, b: usize) -> Result<()> {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra == rb {
            return Ok(());
        }
        match (self.types[ra].value.clone(), self.types[rb].value.clone()) {
            (Some(left), Some(right)) if type_is_generic_placeholder(&left) => {
                self.types[ra].value = Some(right);
                self.types[rb].parent = ra;
                Ok(())
            }
            (Some(left), Some(right)) if type_is_generic_placeholder(&right) => {
                self.types[rb].value = Some(left);
                self.types[ra].parent = rb;
                Ok(())
            }
            (Some(left), Some(right)) if left != right => Err(Error::new(format!(
                "type mismatch: expected {:?}, got {:?}",
                left, right
            ))),
            (Some(_), _) => {
                self.types[rb].parent = ra;
                Ok(())
            }
            (_, Some(_)) => {
                self.types[ra].parent = rb;
                Ok(())
            }
            (None, None) => {
                self.types[rb].parent = ra;
                Ok(())
            }
        }
    }

    fn resolve_known(&mut self, id: usize, label: &str) -> Result<Type> {
        let root = self.find(id);
        self.types[root]
            .value
            .clone()
            .ok_or_else(|| Error::new(format!("could not infer {label}")))
    }

    fn resolve_optional(&mut self, id: usize) -> Option<Type> {
        let root = self.find(id);
        self.types[root].value.clone()
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TypedProgram {
    pub(crate) functions: Vec<TypedFunction>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TypedFunction {
    pub(crate) name: String,
    pub(crate) params: Vec<Type>,
    pub(crate) ret: Type,
    pub(crate) effectful: bool,
    pub(crate) capabilities: Vec<Capability>,
}

fn format_import_path(path: &[String], name: &str) -> String {
    let mut parts = path.to_vec();
    parts.push(name.to_string());
    parts.join(".")
}

fn enum_result_type(variant: &VariantInfo, args: Vec<Type>) -> Type {
    if args.is_empty() {
        Type::Named(variant.enum_name.clone())
    } else {
        Type::Apply {
            name: variant.enum_name.clone(),
            args,
        }
    }
}

fn infer_type_arguments(pattern: &Type, actual: &Type, substitutions: &mut HashMap<String, Type>) {
    match pattern {
        Type::GenericParam(name) => {
            substitutions
                .entry(name.clone())
                .or_insert_with(|| actual.clone());
        }
        Type::Apply { name, args } => {
            if let Type::Apply {
                name: actual_name,
                args: actual_args,
            } = actual
            {
                if name == actual_name && args.len() == actual_args.len() {
                    for (left, right) in args.iter().zip(actual_args.iter()) {
                        infer_type_arguments(left, right, substitutions);
                    }
                }
            }
        }
        Type::Function(function) => {
            if let Type::Function(actual_function) = actual {
                for (left, right) in function.params.iter().zip(actual_function.params.iter()) {
                    infer_type_arguments(left, right, substitutions);
                }
                infer_type_arguments(&function.ret, &actual_function.ret, substitutions);
            }
        }
        Type::Prim(_) | Type::Named(_) => {}
    }
}

fn substitute_type(ty: &Type, substitutions: &HashMap<String, Type>) -> Type {
    match ty {
        Type::GenericParam(name) => substitutions
            .get(name)
            .cloned()
            .unwrap_or_else(|| Type::GenericParam(name.clone())),
        Type::Apply { name, args } => Type::Apply {
            name: name.clone(),
            args: args
                .iter()
                .map(|arg| substitute_type(arg, substitutions))
                .collect(),
        },
        Type::Function(function) => Type::Function(AstFunctionType {
            params: function
                .params
                .iter()
                .map(|param| substitute_type(param, substitutions))
                .collect(),
            ret: Box::new(substitute_type(&function.ret, substitutions)),
            effectful: function.effectful,
        }),
        Type::Prim(_) | Type::Named(_) => ty.clone(),
    }
}

fn substitute_type_for_params(ty: &Type, params: &[String], args: &[Type]) -> Type {
    let substitutions = params
        .iter()
        .cloned()
        .zip(args.iter().cloned())
        .collect::<HashMap<_, _>>();
    substitute_type(ty, &substitutions)
}

fn type_is_generic_placeholder(ty: &Type) -> bool {
    match ty {
        Type::GenericParam(_) => true,
        Type::Apply { args, .. } => args.iter().any(type_is_generic_placeholder),
        Type::Function(function) => {
            function.params.iter().any(type_is_generic_placeholder)
                || type_is_generic_placeholder(&function.ret)
        }
        Type::Prim(_) | Type::Named(_) => false,
    }
}
