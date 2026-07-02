use crate::ast::{
    BinaryOp, Block, BlockItem, EffectRow, EnumDecl, EnumVariant, Expr, Extern, FieldBinding,
    Function, FunctionType, Import, MatchArm, Param, Pattern, Program, Type,
};
use crate::error::{Diagnostic, Error, Result, Span};
use crate::lexer::{Token, TokenKind, lex};

pub(crate) fn parse_program(label: &str, source: &str) -> Result<Program> {
    let tokens = lex(label, source)?;
    Parser {
        tokens,
        current: 0,
        type_params: Vec::new(),
    }
    .parse_program()
}

struct Parser {
    tokens: Vec<Token>,
    current: usize,
    /// The type parameters declared by the function currently being parsed
    /// (spec 0014). While non-empty, `parse_type` resolves a bare name in this
    /// set to `Type::Var` instead of a named enum. Functions never nest, so a
    /// single set (cleared per function) is enough.
    type_params: Vec<String>,
}

impl Parser {
    fn parse_program(&mut self) -> Result<Program> {
        let mut module = None;
        let mut imports = Vec::new();
        let mut functions = Vec::new();
        let mut externs = Vec::new();
        let mut enums = Vec::new();
        self.skip_newlines();
        if self.eat(&TokenKind::Module) {
            module = Some(self.parse_path_name()?);
            self.skip_newlines();
        }
        while !self.at(&TokenKind::Eof) {
            if self.at(&TokenKind::Import) {
                imports.push(self.parse_import()?);
            } else if self.at(&TokenKind::Extern) {
                externs.push(self.parse_extern()?);
            } else if self.at(&TokenKind::Enum) {
                enums.push(self.parse_enum()?);
            } else {
                let is_public = self.eat(&TokenKind::Pub);
                if self.at(&TokenKind::Enum) {
                    enums.push(self.parse_enum()?);
                } else {
                    functions.push(self.parse_function(is_public)?);
                }
            }
            self.skip_newlines();
        }
        // The declaring module qualifies each extern's canonical platform name.
        for declaration in &mut externs {
            declaration.module = module.clone();
        }
        Ok(Program {
            module,
            imports,
            functions,
            externs,
            enums,
        })
    }

    fn parse_enum(&mut self) -> Result<EnumDecl> {
        self.expect(&TokenKind::Enum)?;
        let name_span = self.peek().span.clone();
        let name = self.expect_ident()?;
        self.expect(&TokenKind::LBrace)?;
        let mut variants = Vec::new();
        self.skip_newlines();
        while !self.at(&TokenKind::RBrace) {
            let variant_span = self.peek().span.clone();
            let variant_name = self.expect_ident()?;
            let mut fields = Vec::new();
            if self.eat(&TokenKind::LParen) {
                if !self.at(&TokenKind::RParen) {
                    fields.push(self.parse_type()?);
                    while self.eat(&TokenKind::Comma) {
                        fields.push(self.parse_type()?);
                    }
                }
                self.expect(&TokenKind::RParen)?;
            }
            variants.push(EnumVariant {
                name: variant_name,
                name_span: variant_span,
                fields,
            });
            // Variants are separated by newlines and/or commas.
            self.eat(&TokenKind::Comma);
            self.skip_newlines();
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(EnumDecl {
            name,
            name_span,
            variants,
        })
    }

    /// Parses an optional `throws E` clause (spec 0008/0011). `throws Never` is
    /// non-throwing, equivalent to omitting the clause.
    fn parse_throws_clause(&mut self) -> Result<Option<Type>> {
        if self.eat(&TokenKind::Throws) {
            let ty = self.parse_type()?;
            if matches!(ty, Type::Never) {
                Ok(None)
            } else {
                Ok(Some(ty))
            }
        } else {
            Ok(None)
        }
    }

    fn parse_extern(&mut self) -> Result<Extern> {
        self.expect(&TokenKind::Extern)?;
        self.expect(&TokenKind::Fn)?;
        let name_span = self.peek().span.clone();
        let name = self.expect_ident()?;
        self.expect(&TokenKind::LParen)?;
        let params = self.parse_params()?;
        self.expect(&TokenKind::RParen)?;
        self.expect(&TokenKind::Arrow)?;
        let ret = self.parse_type()?;
        let throws = self.parse_throws_clause()?;
        let effects = self.parse_effect_row()?;
        Ok(Extern {
            name,
            name_span,
            module: None,
            params,
            ret,
            throws,
            effects,
        })
    }

    fn parse_import(&mut self) -> Result<Import> {
        let start = self.expect(&TokenKind::Import)?.span;
        let mut path = vec![self.expect_ident()?];
        while self.eat(&TokenKind::Dot) {
            path.push(self.expect_ident()?);
        }
        if path.len() < 2 {
            return Err(Error::diagnostic(Diagnostic::new("Invalid import").label(
                start.clone(),
                "import path must contain at least two names",
            )));
        }
        let end = self.previous_span();
        Ok(Import {
            path,
            span: start.merge(&end),
        })
    }

    fn parse_function(&mut self, is_public: bool) -> Result<Function> {
        self.expect(&TokenKind::Fn)?;
        let name_span = self.peek().span.clone();
        let name = self.expect_ident()?;
        // Type parameters (spec 0014) are in scope for the whole definition, so
        // `parse_type` resolves them to `Type::Var` throughout the signature and
        // body. Functions never nest, and a parse error aborts the whole parse,
        // so resetting on the success path is enough.
        let type_params = self.parse_type_params()?;
        self.type_params = type_params.clone();
        self.expect(&TokenKind::LParen)?;
        let params = self.parse_params()?;
        self.expect(&TokenKind::RParen)?;
        self.expect(&TokenKind::Arrow)?;
        let ret = self.parse_type()?;
        let throws = self.parse_throws_clause()?;
        let effects = self.parse_effect_row()?;
        let body = self.parse_block()?;
        self.type_params = Vec::new();
        Ok(Function {
            name,
            name_span,
            is_public,
            // The compilation root's own functions carry no import qualifier;
            // `imports.rs` stamps `module_path` on functions pulled in by an
            // `import` (spec 0018).
            module_path: Vec::new(),
            type_params,
            params,
            ret,
            throws,
            effects,
            body,
        })
    }

    /// Parses an optional `<T, U, ...>` type-parameter list (spec 0014). Returns
    /// an empty vec when there is no list. An empty `<>` is rejected.
    fn parse_type_params(&mut self) -> Result<Vec<String>> {
        if !self.eat(&TokenKind::Lt) {
            return Ok(Vec::new());
        }
        let mut params = Vec::new();
        let first_span = self.peek().span.clone();
        params.push(self.expect_ident()?);
        while self.eat(&TokenKind::Comma) {
            params.push(self.expect_ident()?);
        }
        self.expect(&TokenKind::Gt)?;
        // Guard against duplicate names like `<T, T>`.
        let mut seen = std::collections::HashSet::new();
        for name in &params {
            if !seen.insert(name.clone()) {
                return Err(Error::diagnostic(
                    Diagnostic::new("Duplicate type parameter").label(
                        first_span.clone(),
                        format!("type parameter `{name}` is declared more than once"),
                    ),
                ));
            }
        }
        Ok(params)
    }

    fn parse_params(&mut self) -> Result<Vec<Param>> {
        let mut params = Vec::new();
        if self.at(&TokenKind::RParen) {
            return Ok(params);
        }
        loop {
            let name_span = self.peek().span.clone();
            let name = self.expect_ident()?;
            self.expect(&TokenKind::Colon)?;
            let ty = self.parse_type()?;
            params.push(Param {
                name,
                name_span,
                ty,
            });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        Ok(params)
    }

    fn parse_type(&mut self) -> Result<Type> {
        let span = self.peek().span.clone();
        if self.eat(&TokenKind::LParen) {
            let mut params = Vec::new();
            if !self.at(&TokenKind::RParen) {
                params.push(self.parse_type()?);
                while self.eat(&TokenKind::Comma) {
                    params.push(self.parse_type()?);
                }
            }
            self.expect(&TokenKind::RParen)?;
            if self.eat(&TokenKind::Arrow) {
                let ret = self.parse_type()?;
                let throws = self.parse_throws_clause()?;
                let effects = self.parse_effect_row()?;
                return Ok(Type::Function(FunctionType {
                    params,
                    ret: Box::new(ret),
                    throws: throws.map(Box::new),
                    effects,
                }));
            }
            return match params.len() {
                1 => Ok(params.remove(0)),
                _ => Err(Error::diagnostic(
                    Diagnostic::new("Expected function type")
                        .label(span, "parenthesized type lists need `-> ReturnType`"),
                )),
            };
        }
        let name = self.expect_ident()?;
        match name.as_str() {
            "Unit" => Ok(Type::Unit),
            "Bool" => Ok(Type::Bool),
            "Int" => Ok(Type::Int),
            "Float" => Ok(Type::Float),
            "String" => Ok(Type::String),
            "Char" => Ok(Type::Char),
            "Array" => {
                self.expect(&TokenKind::Lt)?;
                let element = self.parse_type()?;
                self.expect(&TokenKind::Gt)?;
                Ok(Type::Array(Box::new(element)))
            }
            "Record" => Ok(Type::Record),
            "Never" => Ok(Type::Never),
            "Option" => {
                self.expect(&TokenKind::Lt)?;
                let inner = self.parse_type()?;
                self.expect(&TokenKind::Gt)?;
                Ok(Type::Option(Box::new(inner)))
            }
            "Function" => Ok(Type::OpaqueFunction),
            // A name declared as a type parameter of the enclosing function
            // resolves to a type variable (spec 0014).
            _ if self.type_params.contains(&name) => Ok(Type::Var(name)),
            // Any other capitalized name refers to a declared enum type; it is
            // resolved and validated during type checking (spec 0005).
            _ => Ok(Type::Enum(name)),
        }
    }

    fn parse_effect_row(&mut self) -> Result<EffectRow> {
        if !self.eat(&TokenKind::Uses) {
            return Ok(EffectRow::default());
        }
        self.expect(&TokenKind::LBrace)?;
        let mut effects = Vec::new();
        if !self.at(&TokenKind::RBrace) {
            effects.push(self.expect_ident()?);
            while self.eat(&TokenKind::Comma) {
                effects.push(self.expect_ident()?);
            }
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(EffectRow::sorted(effects))
    }

    fn parse_block(&mut self) -> Result<Block> {
        let start = self.expect(&TokenKind::LBrace)?.span;
        let mut items = Vec::new();
        self.skip_newlines();
        while !self.at(&TokenKind::RBrace) {
            if self.at(&TokenKind::Eof) {
                return Err(Error::diagnostic(
                    Diagnostic::new("Unterminated block")
                        .label(self.peek().span.clone(), "block is missing a closing `}`"),
                ));
            }
            if self.eat(&TokenKind::Let) {
                let name_span = self.peek().span.clone();
                let name = self.expect_ident()?;
                let ty = if self.eat(&TokenKind::Colon) {
                    Some(self.parse_type()?)
                } else {
                    None
                };
                self.expect(&TokenKind::Eq)?;
                let value = self.parse_expr()?;
                items.push(BlockItem::Let {
                    name,
                    name_span,
                    ty,
                    value,
                });
            } else {
                items.push(BlockItem::Expr(self.parse_expr()?));
            }
            self.skip_newlines();
        }
        let end = self.expect(&TokenKind::RBrace)?.span;
        Ok(Block {
            items,
            span: start.merge(&end),
        })
    }

    fn parse_expr(&mut self) -> Result<Expr> {
        self.parse_equality()
    }

    fn parse_equality(&mut self) -> Result<Expr> {
        let mut expr = self.parse_sum()?;
        loop {
            let op = if self.eat(&TokenKind::EqEq) {
                BinaryOp::Eq
            } else if self.eat(&TokenKind::Lt) {
                BinaryOp::Lt
            } else {
                break;
            };
            let right = self.parse_sum()?;
            let span = expr.span().merge(&right.span());
            expr = Expr::Binary {
                op,
                left: Box::new(expr),
                right: Box::new(right),
                span,
            };
        }
        Ok(expr)
    }

    fn parse_sum(&mut self) -> Result<Expr> {
        let mut expr = self.parse_product()?;
        loop {
            let op = if self.eat(&TokenKind::Plus) {
                BinaryOp::Add
            } else if self.eat(&TokenKind::PlusPlus) {
                BinaryOp::Concat
            } else if self.eat(&TokenKind::Minus) {
                BinaryOp::Sub
            } else {
                break;
            };
            let right = self.parse_product()?;
            let span = expr.span().merge(&right.span());
            expr = Expr::Binary {
                op,
                left: Box::new(expr),
                right: Box::new(right),
                span,
            };
        }
        Ok(expr)
    }

    fn parse_product(&mut self) -> Result<Expr> {
        let mut expr = self.parse_call()?;
        loop {
            let op = if self.eat(&TokenKind::Star) {
                BinaryOp::Mul
            } else if self.eat(&TokenKind::Slash) {
                BinaryOp::Div
            } else if self.eat(&TokenKind::Percent) {
                BinaryOp::Rem
            } else {
                break;
            };
            let right = self.parse_call()?;
            let span = expr.span().merge(&right.span());
            expr = Expr::Binary {
                op,
                left: Box::new(expr),
                right: Box::new(right),
                span,
            };
        }
        Ok(expr)
    }

    fn parse_call(&mut self) -> Result<Expr> {
        let mut expr = self.parse_primary()?;
        loop {
            if self.eat(&TokenKind::LParen) {
                let mut args = Vec::new();
                if !self.at(&TokenKind::RParen) {
                    args.push(self.parse_expr()?);
                    while self.eat(&TokenKind::Comma) {
                        args.push(self.parse_expr()?);
                    }
                }
                let end = self.expect(&TokenKind::RParen)?.span;
                let span = expr.span().merge(&end);
                expr = Expr::Call {
                    callee: Box::new(expr),
                    args,
                    span,
                };
            } else if self.at(&TokenKind::Question) {
                // Postfix `?` propagates errors / `None` (spec 0011).
                let end = self.bump().span;
                let span = expr.span().merge(&end);
                expr = Expr::Question {
                    value: Box::new(expr),
                    span,
                };
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn parse_primary(&mut self) -> Result<Expr> {
        match self.peek().kind.clone() {
            TokenKind::Int(value) => {
                let span = self.bump().span;
                Ok(Expr::Int(value, span))
            }
            TokenKind::Float(value) => {
                let span = self.bump().span;
                Ok(Expr::Float(value, span))
            }
            TokenKind::String(value) => {
                let span = self.bump().span;
                Ok(Expr::String(value, span))
            }
            TokenKind::Char(value) => {
                let span = self.bump().span;
                Ok(Expr::Char(value, span))
            }
            TokenKind::True => {
                let span = self.bump().span;
                Ok(Expr::Bool(true, span))
            }
            TokenKind::False => {
                let span = self.bump().span;
                Ok(Expr::Bool(false, span))
            }
            TokenKind::Ident(_) => {
                let span = self.peek().span.clone();
                let name = self.expect_ident()?;
                if self.at(&TokenKind::Dot) {
                    // A dotted path: `Enum.Variant`, `Char.from_code`,
                    // `int.to_string`, `std.int.to_string` (spec 0018). The path
                    // is parsed uniformly here; any trailing `(args)` is attached
                    // by `parse_call`, and the meaning is resolved later. Whether
                    // it is a variant, a built-in conversion, or a (qualified)
                    // function call cannot be decided syntactically.
                    let mut segments = vec![name];
                    while self.eat(&TokenKind::Dot) {
                        segments.push(self.expect_ident()?);
                    }
                    let end = self.previous_span();
                    Ok(Expr::Path {
                        segments,
                        span: span.merge(&end),
                    })
                } else {
                    Ok(Expr::Var(name, span))
                }
            }
            TokenKind::Throw => {
                let start = self.bump().span;
                let value = self.parse_expr()?;
                let span = start.merge(&value.span());
                Ok(Expr::Throw {
                    value: Box::new(value),
                    span,
                })
            }
            TokenKind::Panic => {
                let start = self.bump().span;
                self.expect(&TokenKind::LParen)?;
                let message = self.parse_expr()?;
                let end = self.expect(&TokenKind::RParen)?.span;
                Ok(Expr::Panic {
                    message: Box::new(message),
                    span: start.merge(&end),
                })
            }
            TokenKind::Match => self.parse_match(),
            TokenKind::Try => self.parse_try(),
            TokenKind::If => self.parse_if(),
            TokenKind::Fn => self.parse_fn_expr(),
            TokenKind::LBracket => {
                let start = self.bump().span;
                let mut values = Vec::new();
                if !self.at(&TokenKind::RBracket) {
                    values.push(self.parse_expr()?);
                    while self.eat(&TokenKind::Comma) {
                        values.push(self.parse_expr()?);
                    }
                }
                let end = self.expect(&TokenKind::RBracket)?.span;
                Ok(Expr::Array(values, start.merge(&end)))
            }
            TokenKind::LBrace => Ok(Expr::Block(self.parse_block()?)),
            TokenKind::LParen => {
                let start = self.bump().span;
                if self.eat(&TokenKind::RParen) {
                    Ok(Expr::Unit(start))
                } else {
                    let expr = self.parse_expr()?;
                    self.expect(&TokenKind::RParen)?;
                    Ok(expr)
                }
            }
            _ => Err(Error::diagnostic(
                Diagnostic::new("Expected an expression")
                    .label(self.peek().span.clone(), "expected an expression here"),
            )),
        }
    }

    fn parse_fn_expr(&mut self) -> Result<Expr> {
        let start = self.expect(&TokenKind::Fn)?.span;
        self.expect(&TokenKind::LParen)?;
        let params = self.parse_params()?;
        self.expect(&TokenKind::RParen)?;
        self.expect(&TokenKind::Arrow)?;
        let ret = self.parse_type()?;
        let throws = self.parse_throws_clause()?;
        let effects = self.parse_effect_row()?;
        let body = self.parse_block()?;
        let span = start.merge(&body.span);
        Ok(Expr::Fn {
            params,
            ret,
            throws,
            effects,
            body,
            span,
        })
    }

    fn parse_if(&mut self) -> Result<Expr> {
        let start = self.expect(&TokenKind::If)?.span;
        let cond = self.parse_expr()?;
        let then = self.parse_block()?;
        self.expect(&TokenKind::Else)?;
        let els = self.parse_block()?;
        let span = start.merge(&els.span);
        Ok(Expr::If {
            cond: Box::new(cond),
            then,
            els,
            span,
        })
    }

    fn parse_match(&mut self) -> Result<Expr> {
        let start = self.expect(&TokenKind::Match)?.span;
        let scrutinee = self.parse_expr()?;
        self.expect(&TokenKind::LBrace)?;
        let arms = self.parse_match_arms()?;
        let end = self.expect(&TokenKind::RBrace)?.span;
        Ok(Expr::Match {
            scrutinee: Box::new(scrutinee),
            arms,
            span: start.merge(&end),
        })
    }

    fn parse_try(&mut self) -> Result<Expr> {
        let start = self.expect(&TokenKind::Try)?.span;
        let body = self.parse_block()?;
        self.expect(&TokenKind::Catch)?;
        self.expect(&TokenKind::LBrace)?;
        let arms = self.parse_match_arms()?;
        let end = self.expect(&TokenKind::RBrace)?.span;
        Ok(Expr::Try {
            body,
            arms,
            span: start.merge(&end),
        })
    }

    fn parse_match_arms(&mut self) -> Result<Vec<MatchArm>> {
        let mut arms = Vec::new();
        self.skip_newlines();
        while !self.at(&TokenKind::RBrace) {
            let pattern = self.parse_pattern()?;
            let guard = if self.eat(&TokenKind::If) {
                Some(self.parse_expr()?)
            } else {
                None
            };
            self.expect(&TokenKind::Arrow)?;
            let body = self.parse_expr()?;
            let span = pattern_span(&pattern).merge(&body.span());
            arms.push(MatchArm {
                pattern,
                guard,
                body,
                span,
            });
            self.eat(&TokenKind::Comma);
            self.skip_newlines();
        }
        Ok(arms)
    }

    fn parse_pattern(&mut self) -> Result<Pattern> {
        let span = self.peek().span.clone();
        let name = self.expect_ident()?;
        if name == "_" {
            return Ok(Pattern::Wildcard(span));
        }
        // A lowercase-leading name binds the whole scrutinee (catch-all).
        if name.chars().next().is_some_and(char::is_lowercase) {
            return Ok(Pattern::Binding { name, span });
        }
        // An uppercase-leading name is a variant, optionally `Enum.Variant`.
        let (enum_name, variant) = if self.eat(&TokenKind::Dot) {
            (Some(name), self.expect_ident()?)
        } else {
            (None, name)
        };
        let mut fields = Vec::new();
        if self.eat(&TokenKind::LParen) {
            if !self.at(&TokenKind::RParen) {
                fields.push(self.parse_field_binding()?);
                while self.eat(&TokenKind::Comma) {
                    fields.push(self.parse_field_binding()?);
                }
            }
            self.expect(&TokenKind::RParen)?;
        }
        let end = self.previous_span();
        Ok(Pattern::Variant {
            enum_name,
            variant,
            fields,
            span: span.merge(&end),
        })
    }

    fn parse_field_binding(&mut self) -> Result<FieldBinding> {
        let name = self.expect_ident()?;
        if name == "_" {
            Ok(FieldBinding::Ignore)
        } else {
            Ok(FieldBinding::Name(name))
        }
    }

    fn skip_newlines(&mut self) {
        while self.at(&TokenKind::Newline) {
            self.bump();
        }
    }

    fn expect_ident(&mut self) -> Result<String> {
        match self.peek().kind.clone() {
            TokenKind::Ident(name) => {
                self.bump();
                Ok(name)
            }
            _ => Err(Error::diagnostic(
                Diagnostic::new("Expected a name")
                    .label(self.peek().span.clone(), "expected a name here"),
            )),
        }
    }

    fn parse_path_name(&mut self) -> Result<String> {
        let mut parts = vec![self.expect_ident()?];
        while self.eat(&TokenKind::Dot) {
            parts.push(self.expect_ident()?);
        }
        Ok(parts.join("."))
    }

    fn expect(&mut self, expected: &TokenKind) -> Result<Token> {
        if self.at(expected) {
            Ok(self.bump())
        } else {
            Err(Error::diagnostic(
                Diagnostic::new("Unexpected token")
                    .label(self.peek().span.clone(), format!("expected `{expected:?}`")),
            ))
        }
    }

    fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.at(kind) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn at(&self, kind: &TokenKind) -> bool {
        std::mem::discriminant(&self.peek().kind) == std::mem::discriminant(kind)
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.current]
    }

    fn bump(&mut self) -> Token {
        let token = self.tokens[self.current].clone();
        self.current += 1;
        token
    }

    fn previous_span(&self) -> Span {
        self.tokens[self.current.saturating_sub(1)].span.clone()
    }
}

fn pattern_span(pattern: &Pattern) -> Span {
    match pattern {
        Pattern::Variant { span, .. } | Pattern::Binding { span, .. } => span.clone(),
        Pattern::Wildcard(span) => span.clone(),
    }
}

#[allow(dead_code)]
fn _span(_: &Span) {}
