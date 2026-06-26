use std::collections::HashMap;
use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone)]
struct Error {
    message: String,
}

impl Error {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for Error {}

#[derive(Debug, Clone, PartialEq)]
enum TokenKind {
    Fn,
    Match,
    True,
    False,
    Ident(String),
    Int(i32),
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Dot,
    Hash,
    Bang,
    Eq,
    EqEq,
    Arrow,
    Lt,
    Plus,
    Minus,
    Star,
    Newline,
    Eof,
}

#[derive(Debug, Clone)]
struct Token {
    kind: TokenKind,
    pos: usize,
}

fn lex(source: &str) -> Result<Vec<Token>> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;

    while i < bytes.len() {
        let pos = i;
        match bytes[i] {
            b' ' | b'\t' | b'\r' => i += 1,
            b'\n' => {
                tokens.push(Token {
                    kind: TokenKind::Newline,
                    pos,
                });
                i += 1;
            }
            b'-' if i + 1 < bytes.len() && bytes[i + 1] == b'-' => {
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'-' if i + 1 < bytes.len() && bytes[i + 1] == b'>' => {
                tokens.push(Token {
                    kind: TokenKind::Arrow,
                    pos,
                });
                i += 2;
            }
            b'(' => push_one(&mut tokens, TokenKind::LParen, pos, &mut i),
            b')' => push_one(&mut tokens, TokenKind::RParen, pos, &mut i),
            b'{' => push_one(&mut tokens, TokenKind::LBrace, pos, &mut i),
            b'}' => push_one(&mut tokens, TokenKind::RBrace, pos, &mut i),
            b'[' => push_one(&mut tokens, TokenKind::LBracket, pos, &mut i),
            b']' => push_one(&mut tokens, TokenKind::RBracket, pos, &mut i),
            b',' => push_one(&mut tokens, TokenKind::Comma, pos, &mut i),
            b'.' => push_one(&mut tokens, TokenKind::Dot, pos, &mut i),
            b'#' => push_one(&mut tokens, TokenKind::Hash, pos, &mut i),
            b'!' => push_one(&mut tokens, TokenKind::Bang, pos, &mut i),
            b'<' => push_one(&mut tokens, TokenKind::Lt, pos, &mut i),
            b'+' => push_one(&mut tokens, TokenKind::Plus, pos, &mut i),
            b'*' => push_one(&mut tokens, TokenKind::Star, pos, &mut i),
            b'-' => push_one(&mut tokens, TokenKind::Minus, pos, &mut i),
            b'=' if i + 1 < bytes.len() && bytes[i + 1] == b'=' => {
                tokens.push(Token {
                    kind: TokenKind::EqEq,
                    pos,
                });
                i += 2;
            }
            b'=' => push_one(&mut tokens, TokenKind::Eq, pos, &mut i),
            b'0'..=b'9' => {
                let start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                let text = &source[start..i];
                let value = text.parse::<i32>().map_err(|_| {
                    Error::new(format!("integer literal out of I32 range at byte {start}"))
                })?;
                tokens.push(Token {
                    kind: TokenKind::Int(value),
                    pos: start,
                });
            }
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                let start = i;
                i += 1;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                let text = &source[start..i];
                let kind = match text {
                    "fn" => TokenKind::Fn,
                    "match" => TokenKind::Match,
                    "true" => TokenKind::True,
                    "false" => TokenKind::False,
                    _ => TokenKind::Ident(text.to_string()),
                };
                tokens.push(Token { kind, pos: start });
            }
            other => {
                return Err(Error::new(format!(
                    "unexpected byte {:?} at byte {pos}",
                    other as char
                )));
            }
        }
    }

    tokens.push(Token {
        kind: TokenKind::Eof,
        pos: source.len(),
    });
    Ok(tokens)
}

fn push_one(tokens: &mut Vec<Token>, kind: TokenKind, pos: usize, i: &mut usize) {
    tokens.push(Token { kind, pos });
    *i += 1;
}

#[derive(Debug, Clone)]
struct Program {
    functions: Vec<Function>,
}

#[derive(Debug, Clone)]
struct Function {
    name: String,
    params: Vec<String>,
    return_annotation: Option<PrimType>,
    body: Block,
}

#[derive(Debug, Clone)]
struct Block {
    items: Vec<BlockItem>,
}

#[derive(Debug, Clone)]
enum BlockItem {
    Binding { name: String, expr: Expr },
    Expr(Expr),
}

#[derive(Debug, Clone)]
enum Expr {
    Int(i32),
    Bool(bool),
    Unit,
    Var(String),
    Call {
        name: String,
        args: Vec<Expr>,
    },
    MethodCall {
        receiver: Box<Expr>,
        name: String,
        args: Vec<Expr>,
    },
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    Block(Block),
}

#[derive(Debug, Clone)]
struct MatchArm {
    pattern: Pattern,
    expr: Expr,
}

#[derive(Debug, Clone)]
enum Pattern {
    Int(i32),
    Bool(bool),
    Unit,
    Wildcard,
}

#[derive(Debug, Clone, Copy)]
enum BinaryOp {
    Add,
    Sub,
    Mul,
    Eq,
    Lt,
}

struct Parser {
    tokens: Vec<Token>,
    current: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, current: 0 }
    }

    fn parse_program(&mut self) -> Result<Program> {
        let mut functions = Vec::new();
        self.skip_newlines();
        while !self.at(&TokenKind::Eof) {
            self.parse_attributes()?;
            functions.push(self.parse_function()?);
            self.skip_newlines();
        }
        Ok(Program { functions })
    }

    fn parse_attributes(&mut self) -> Result<()> {
        loop {
            self.skip_newlines();
            if !self.at(&TokenKind::Hash) {
                return Ok(());
            }
            self.bump();
            self.expect(&TokenKind::LBracket)?;
            self.expect_ident()?;
            if self.eat(&TokenKind::LParen) {
                if !self.at(&TokenKind::RParen) {
                    self.expect_ident()?;
                    while self.eat(&TokenKind::Comma) {
                        self.expect_ident()?;
                    }
                }
                self.expect(&TokenKind::RParen)?;
            }
            self.expect(&TokenKind::RBracket)?;
        }
    }

    fn parse_function(&mut self) -> Result<Function> {
        self.expect(&TokenKind::Fn)?;
        let name = self.parse_function_name()?;
        self.expect(&TokenKind::LParen)?;
        let mut params = Vec::new();
        if !self.at(&TokenKind::RParen) {
            params.push(self.expect_ident()?);
            while self.eat(&TokenKind::Comma) {
                params.push(self.expect_ident()?);
            }
        }
        self.expect(&TokenKind::RParen)?;
        let return_annotation = if self.eat(&TokenKind::Arrow) {
            Some(self.parse_type()?)
        } else {
            None
        };
        let body = self.parse_block()?;
        Ok(Function {
            name,
            params,
            return_annotation,
            body,
        })
    }

    fn parse_function_name(&mut self) -> Result<String> {
        let mut name = self.expect_ident()?;
        if self.eat(&TokenKind::Bang) {
            name.push('!');
        }
        Ok(name)
    }

    fn parse_block(&mut self) -> Result<Block> {
        self.expect(&TokenKind::LBrace)?;
        let mut items = Vec::new();
        self.skip_newlines();
        while !self.at(&TokenKind::RBrace) {
            if self.at(&TokenKind::Eof) {
                return Err(Error::new("unterminated block"));
            }

            if let TokenKind::Ident(name) = self.peek().kind.clone() {
                if self
                    .peek_n(1)
                    .is_some_and(|token| token.kind == TokenKind::Eq)
                {
                    self.bump();
                    self.bump();
                    let expr = self.parse_expr()?;
                    items.push(BlockItem::Binding { name, expr });
                } else {
                    items.push(BlockItem::Expr(self.parse_expr()?));
                }
            } else {
                items.push(BlockItem::Expr(self.parse_expr()?));
            }
            self.skip_newlines();
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(Block { items })
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
            expr = Expr::Binary {
                op,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_sum(&mut self) -> Result<Expr> {
        let mut expr = self.parse_product()?;
        loop {
            let op = if self.eat(&TokenKind::Plus) {
                BinaryOp::Add
            } else if self.eat(&TokenKind::Minus) {
                BinaryOp::Sub
            } else {
                break;
            };
            let right = self.parse_product()?;
            expr = Expr::Binary {
                op,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_product(&mut self) -> Result<Expr> {
        let mut expr = self.parse_postfix()?;
        while self.eat(&TokenKind::Star) {
            let right = self.parse_postfix()?;
            expr = Expr::Binary {
                op: BinaryOp::Mul,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_postfix(&mut self) -> Result<Expr> {
        let mut expr = self.parse_primary()?;
        loop {
            if self.eat(&TokenKind::Dot) {
                let name = self.expect_ident()?;
                self.expect(&TokenKind::LParen)?;
                let args = self.parse_argument_list()?;
                self.expect(&TokenKind::RParen)?;
                expr = Expr::MethodCall {
                    receiver: Box::new(expr),
                    name,
                    args,
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
                self.bump();
                Ok(Expr::Int(value))
            }
            TokenKind::True => {
                self.bump();
                Ok(Expr::Bool(true))
            }
            TokenKind::False => {
                self.bump();
                Ok(Expr::Bool(false))
            }
            TokenKind::Ident(_) => {
                let name = self.parse_function_name()?;
                if self.eat(&TokenKind::LParen) {
                    let args = self.parse_argument_list()?;
                    self.expect(&TokenKind::RParen)?;
                    Ok(Expr::Call { name, args })
                } else {
                    if name.ends_with('!') {
                        return Err(Error::new("local variable names cannot end with !"));
                    }
                    Ok(Expr::Var(name))
                }
            }
            TokenKind::Match => {
                self.bump();
                let scrutinee = self.parse_expr()?;
                self.expect(&TokenKind::LBrace)?;
                let mut arms = Vec::new();
                self.skip_newlines();
                while !self.at(&TokenKind::RBrace) {
                    if self.at(&TokenKind::Eof) {
                        return Err(Error::new("unterminated match expression"));
                    }
                    let pattern = self.parse_pattern()?;
                    self.expect(&TokenKind::Arrow)?;
                    let expr = self.parse_expr()?;
                    arms.push(MatchArm { pattern, expr });
                    self.skip_newlines();
                }
                self.expect(&TokenKind::RBrace)?;
                Ok(Expr::Match {
                    scrutinee: Box::new(scrutinee),
                    arms,
                })
            }
            TokenKind::LBrace => Ok(Expr::Block(self.parse_block()?)),
            TokenKind::LParen => {
                self.bump();
                if self.eat(&TokenKind::RParen) {
                    Ok(Expr::Unit)
                } else {
                    let expr = self.parse_expr()?;
                    self.expect(&TokenKind::RParen)?;
                    Ok(expr)
                }
            }
            _ => Err(Error::new(format!(
                "expected expression at byte {}",
                self.peek().pos
            ))),
        }
    }

    fn parse_argument_list(&mut self) -> Result<Vec<Expr>> {
        let mut args = Vec::new();
        if !self.at(&TokenKind::RParen) {
            args.push(self.parse_expr()?);
            while self.eat(&TokenKind::Comma) {
                args.push(self.parse_expr()?);
            }
        }
        Ok(args)
    }

    fn parse_type(&mut self) -> Result<PrimType> {
        let name = self.expect_ident()?;
        match name.as_str() {
            "I32" | "i32" => Ok(PrimType::I32),
            "Bool" | "bool" => Ok(PrimType::Bool),
            "Unit" | "unit" => Ok(PrimType::Unit),
            _ => Err(Error::new(format!("unknown type `{name}`"))),
        }
    }

    fn parse_pattern(&mut self) -> Result<Pattern> {
        match self.peek().kind.clone() {
            TokenKind::Int(value) => {
                self.bump();
                Ok(Pattern::Int(value))
            }
            TokenKind::True => {
                self.bump();
                Ok(Pattern::Bool(true))
            }
            TokenKind::False => {
                self.bump();
                Ok(Pattern::Bool(false))
            }
            TokenKind::LParen => {
                self.bump();
                self.expect(&TokenKind::RParen)?;
                Ok(Pattern::Unit)
            }
            TokenKind::Ident(name) if name == "_" => {
                self.bump();
                Ok(Pattern::Wildcard)
            }
            _ => Err(Error::new(format!(
                "expected pattern at byte {}",
                self.peek().pos
            ))),
        }
    }

    fn expect_ident(&mut self) -> Result<String> {
        match self.peek().kind.clone() {
            TokenKind::Ident(name) => {
                self.bump();
                Ok(name)
            }
            _ => Err(Error::new(format!(
                "expected identifier at byte {}",
                self.peek().pos
            ))),
        }
    }

    fn skip_newlines(&mut self) {
        while self.at(&TokenKind::Newline) {
            self.bump();
        }
    }

    fn expect(&mut self, kind: &TokenKind) -> Result<()> {
        if self.eat(kind) {
            Ok(())
        } else {
            Err(Error::new(format!(
                "expected {:?} at byte {}",
                kind,
                self.peek().pos
            )))
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
        self.peek().kind == *kind
    }

    fn bump(&mut self) {
        self.current += 1;
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.current]
    }

    fn peek_n(&self, n: usize) -> Option<&Token> {
        self.tokens.get(self.current + n)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrimType {
    I32,
    Bool,
    Unit,
}

#[derive(Debug, Clone)]
struct TypeSlot {
    parent: usize,
    value: Option<PrimType>,
}

#[derive(Debug, Clone)]
struct FunctionType {
    params: Vec<usize>,
    ret: usize,
}

struct TypeChecker<'a> {
    program: &'a Program,
    types: Vec<TypeSlot>,
    functions: HashMap<String, FunctionType>,
}

impl<'a> TypeChecker<'a> {
    fn new(program: &'a Program) -> Self {
        Self {
            program,
            types: Vec::new(),
            functions: HashMap::new(),
        }
    }

    fn check(mut self) -> Result<TypedProgram> {
        self.register_functions()?;
        self.check_main()?;

        for function in &self.program.functions {
            let signature = self
                .functions
                .get(&function.name)
                .cloned()
                .ok_or_else(|| Error::new("internal type checker error"))?;
            let mut scope = HashMap::new();
            for (param, ty) in function.params.iter().zip(signature.params.iter()) {
                if scope.insert(param.clone(), *ty).is_some() {
                    return Err(Error::new(format!(
                        "duplicate parameter `{param}` in function `{}`",
                        function.name
                    )));
                }
            }

            let body_ty = self.check_block(&function.body, &mut scope)?;
            self.unify(signature.ret, body_ty)?;
        }

        let mut typed_functions = Vec::new();
        for function in &self.program.functions {
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
            });
        }

        Ok(TypedProgram {
            functions: typed_functions,
        })
    }

    fn register_functions(&mut self) -> Result<()> {
        for function in &self.program.functions {
            if self.functions.contains_key(&function.name) {
                return Err(Error::new(format!(
                    "duplicate top-level function `{}`",
                    function.name
                )));
            }
            let params = function.params.iter().map(|_| self.fresh()).collect();
            let ret = match function.return_annotation {
                Some(ty) => self.known(ty),
                None => self.fresh(),
            };
            self.functions
                .insert(function.name.clone(), FunctionType { params, ret });
        }
        Ok(())
    }

    fn check_main(&self) -> Result<()> {
        let main_count = self
            .program
            .functions
            .iter()
            .filter(|function| function.name == "main")
            .count();
        if main_count != 1 {
            return Err(Error::new(
                "executable program must contain exactly one top-level `main` function",
            ));
        }
        let main = self
            .program
            .functions
            .iter()
            .find(|function| function.name == "main")
            .expect("main was counted above");
        if !main.params.is_empty() {
            return Err(Error::new("`main` must take zero parameters"));
        }
        Ok(())
    }

    fn check_block(
        &mut self,
        block: &Block,
        outer_scope: &mut HashMap<String, usize>,
    ) -> Result<usize> {
        let mut scope = outer_scope.clone();
        let mut last_expr = None;
        for item in &block.items {
            match item {
                BlockItem::Binding { name, expr } => {
                    if scope.contains_key(name) {
                        return Err(Error::new(format!(
                            "duplicate binding `{name}` in the same block"
                        )));
                    }
                    let ty = self.check_expr(expr, &mut scope)?;
                    scope.insert(name.clone(), ty);
                    last_expr = None;
                }
                BlockItem::Expr(expr) => {
                    last_expr = Some(self.check_expr(expr, &mut scope)?);
                }
            }
        }
        Ok(last_expr.unwrap_or_else(|| self.known(PrimType::Unit)))
    }

    fn check_expr(&mut self, expr: &Expr, scope: &mut HashMap<String, usize>) -> Result<usize> {
        match expr {
            Expr::Int(_) => Ok(self.known(PrimType::I32)),
            Expr::Bool(_) => Ok(self.known(PrimType::Bool)),
            Expr::Unit => Ok(self.known(PrimType::Unit)),
            Expr::Var(name) => scope
                .get(name)
                .copied()
                .ok_or_else(|| Error::new(format!("unknown local binding `{name}`"))),
            Expr::Call { name, args } => {
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
                for (arg, param_ty) in args.iter().zip(signature.params.iter()) {
                    let arg_ty = self.check_expr(arg, scope)?;
                    self.unify(arg_ty, *param_ty)?;
                }
                Ok(signature.ret)
            }
            Expr::MethodCall { .. } => Err(Error::new(
                "method calls are parsed but trait method resolution is not implemented yet",
            )),
            Expr::Binary { op, left, right } => {
                let left_ty = self.check_expr(left, scope)?;
                let right_ty = self.check_expr(right, scope)?;
                match op {
                    BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul => {
                        let i32_ty = self.known(PrimType::I32);
                        self.unify(left_ty, i32_ty)?;
                        let i32_ty = self.known(PrimType::I32);
                        self.unify(right_ty, i32_ty)?;
                        Ok(self.known(PrimType::I32))
                    }
                    BinaryOp::Eq => {
                        self.unify(left_ty, right_ty)?;
                        Ok(self.known(PrimType::Bool))
                    }
                    BinaryOp::Lt => {
                        let i32_ty = self.known(PrimType::I32);
                        self.unify(left_ty, i32_ty)?;
                        let i32_ty = self.known(PrimType::I32);
                        self.unify(right_ty, i32_ty)?;
                        Ok(self.known(PrimType::Bool))
                    }
                }
            }
            Expr::Match { scrutinee, arms } => {
                if arms.is_empty() {
                    return Err(Error::new("match expression must have at least one arm"));
                }

                let scrutinee_ty = self.check_expr(scrutinee, scope)?;
                for arm in arms {
                    if let Some(pattern_ty) = self.pattern_type(&arm.pattern) {
                        let pattern_ty = self.known(pattern_ty);
                        self.unify(scrutinee_ty, pattern_ty)?;
                    }
                }

                let mut result_ty = None;
                for arm in arms {
                    let arm_ty = self.check_expr(&arm.expr, scope)?;
                    if let Some(existing) = result_ty {
                        self.unify(existing, arm_ty)?;
                    } else {
                        result_ty = Some(arm_ty);
                    }
                }

                let scrutinee_prim = self.resolve_known(scrutinee_ty, "match scrutinee type")?;
                if !self.match_is_exhaustive(scrutinee_prim, arms) {
                    return Err(Error::new("match expression is not exhaustive"));
                }

                Ok(result_ty.expect("non-empty arms checked above"))
            }
            Expr::Block(block) => self.check_block(block, scope),
        }
    }

    fn pattern_type(&self, pattern: &Pattern) -> Option<PrimType> {
        match pattern {
            Pattern::Int(_) => Some(PrimType::I32),
            Pattern::Bool(_) => Some(PrimType::Bool),
            Pattern::Unit => Some(PrimType::Unit),
            Pattern::Wildcard => None,
        }
    }

    fn match_is_exhaustive(&self, scrutinee: PrimType, arms: &[MatchArm]) -> bool {
        if arms
            .iter()
            .any(|arm| matches!(arm.pattern, Pattern::Wildcard))
        {
            return true;
        }

        match scrutinee {
            PrimType::Bool => {
                let has_true = arms
                    .iter()
                    .any(|arm| matches!(arm.pattern, Pattern::Bool(true)));
                let has_false = arms
                    .iter()
                    .any(|arm| matches!(arm.pattern, Pattern::Bool(false)));
                has_true && has_false
            }
            PrimType::Unit => arms.iter().any(|arm| matches!(arm.pattern, Pattern::Unit)),
            PrimType::I32 => false,
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

    fn known(&mut self, prim: PrimType) -> usize {
        let id = self.fresh();
        self.types[id].value = Some(prim);
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
        match (self.types[ra].value, self.types[rb].value) {
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

    fn resolve_known(&mut self, id: usize, label: &str) -> Result<PrimType> {
        let root = self.find(id);
        self.types[root]
            .value
            .ok_or_else(|| Error::new(format!("could not infer {label}")))
    }
}

#[derive(Debug, Clone)]
struct TypedProgram {
    functions: Vec<TypedFunction>,
}

#[derive(Debug, Clone)]
struct TypedFunction {
    name: String,
    params: Vec<PrimType>,
    ret: PrimType,
}

fn compile_source(source: &str) -> Result<(Program, TypedProgram)> {
    let tokens = lex(source)?;
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program()?;
    let typed = TypeChecker::new(&program).check()?;
    Ok((program, typed))
}

fn emit_rust(program: &Program, typed: &TypedProgram) -> String {
    let types_by_name = typed
        .functions
        .iter()
        .map(|function| (function.name.as_str(), function))
        .collect::<HashMap<_, _>>();
    let mut out = String::new();
    out.push_str("// Generated by the Emela compiler.\n\n");

    for function in &program.functions {
        let typed_function = types_by_name[function.name.as_str()];
        if function.name == "main" {
            out.push_str("fn main() {\n    let _emela_result: ");
            out.push_str(rust_type(typed_function.ret));
            out.push_str(" = ");
            out.push_str(&emit_block(&function.body, 1));
            out.push_str(";\n");
            match typed_function.ret {
                PrimType::I32 => out.push_str("    std::process::exit(_emela_result);\n"),
                PrimType::Bool => {
                    out.push_str("    std::process::exit(if _emela_result { 0 } else { 1 });\n")
                }
                PrimType::Unit => {}
            }
            out.push_str("}\n\n");
        } else {
            out.push_str("fn ");
            out.push_str(&rust_name(&function.name));
            out.push('(');
            for (index, (param, ty)) in function
                .params
                .iter()
                .zip(typed_function.params.iter())
                .enumerate()
            {
                if index > 0 {
                    out.push_str(", ");
                }
                out.push_str(param);
                out.push_str(": ");
                out.push_str(rust_type(*ty));
            }
            out.push_str(") -> ");
            out.push_str(rust_type(typed_function.ret));
            out.push_str(" {\n    ");
            out.push_str(&emit_block(&function.body, 1));
            out.push_str("\n}\n\n");
        }
    }

    out
}

fn emit_block(block: &Block, indent: usize) -> String {
    if block.items.is_empty() {
        return "{}".to_string();
    }

    let pad = "    ".repeat(indent);
    let inner_pad = "    ".repeat(indent + 1);
    let mut out = String::new();
    out.push_str("{\n");
    for (index, item) in block.items.iter().enumerate() {
        let is_last = index + 1 == block.items.len();
        match item {
            BlockItem::Binding { name, expr } => {
                out.push_str(&inner_pad);
                out.push_str("let ");
                out.push_str(name);
                out.push_str(" = ");
                out.push_str(&emit_expr(expr, indent + 1));
                out.push_str(";\n");
                if is_last {
                    out.push_str(&inner_pad);
                    out.push_str("()\n");
                }
            }
            BlockItem::Expr(expr) if is_last => {
                out.push_str(&inner_pad);
                out.push_str(&emit_expr(expr, indent + 1));
                out.push('\n');
            }
            BlockItem::Expr(expr) => {
                out.push_str(&inner_pad);
                out.push_str("let _ = ");
                out.push_str(&emit_expr(expr, indent + 1));
                out.push_str(";\n");
            }
        }
    }
    out.push_str(&pad);
    out.push('}');
    out
}

fn emit_expr(expr: &Expr, indent: usize) -> String {
    match expr {
        Expr::Int(value) => format!("{value}i32"),
        Expr::Bool(value) => value.to_string(),
        Expr::Unit => "()".to_string(),
        Expr::Var(name) => name.clone(),
        Expr::Call { name, args } => {
            let args = args
                .iter()
                .map(|arg| emit_expr(arg, indent))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{}({args})", rust_name(name))
        }
        Expr::MethodCall {
            receiver,
            name,
            args,
        } => {
            let args = args
                .iter()
                .map(|arg| emit_expr(arg, indent))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{}.{}({args})", emit_expr(receiver, indent), name)
        }
        Expr::Binary { op, left, right } => {
            let left = emit_expr(left, indent);
            let right = emit_expr(right, indent);
            match op {
                BinaryOp::Add => format!("{left}.wrapping_add({right})"),
                BinaryOp::Sub => format!("{left}.wrapping_sub({right})"),
                BinaryOp::Mul => format!("{left}.wrapping_mul({right})"),
                BinaryOp::Eq => format!("({left} == {right})"),
                BinaryOp::Lt => format!("({left} < {right})"),
            }
        }
        Expr::Match { scrutinee, arms } => {
            let arms = arms
                .iter()
                .map(|arm| {
                    format!(
                        "{} => {},",
                        emit_pattern(&arm.pattern),
                        emit_expr(&arm.expr, indent)
                    )
                })
                .collect::<Vec<_>>()
                .join(" ");
            format!("match {} {{ {arms} }}", emit_expr(scrutinee, indent))
        }
        Expr::Block(block) => emit_block(block, indent),
    }
}

fn emit_pattern(pattern: &Pattern) -> String {
    match pattern {
        Pattern::Int(value) => format!("{value}i32"),
        Pattern::Bool(value) => value.to_string(),
        Pattern::Unit => "()".to_string(),
        Pattern::Wildcard => "_".to_string(),
    }
}

fn rust_name(name: &str) -> String {
    name.strip_suffix('!')
        .map(|base| format!("{base}_effect"))
        .unwrap_or_else(|| name.to_string())
}

fn rust_type(ty: PrimType) -> &'static str {
    match ty {
        PrimType::I32 => "i32",
        PrimType::Bool => "bool",
        PrimType::Unit => "()",
    }
}

fn build(input: &Path, output: &Path, rust_source: &str) -> Result<()> {
    let temp = env::temp_dir().join(format!(
        "emela-{}-{}.rs",
        std::process::id(),
        input.file_stem().and_then(|s| s.to_str()).unwrap_or("out")
    ));
    fs::write(&temp, rust_source).map_err(|err| {
        Error::new(format!(
            "failed to write temporary Rust source `{}`: {err}",
            temp.display()
        ))
    })?;

    let status = Command::new("rustc")
        .arg(&temp)
        .arg("-o")
        .arg(output)
        .status()
        .map_err(|err| Error::new(format!("failed to execute rustc: {err}")))?;

    let _ = fs::remove_file(&temp);

    if !status.success() {
        return Err(Error::new(format!(
            "rustc failed while building `{}`",
            output.display()
        )));
    }
    Ok(())
}

#[derive(Debug)]
struct Args {
    input: PathBuf,
    output: PathBuf,
    check_only: bool,
    emit_rust: Option<PathBuf>,
}

fn parse_args() -> Result<Args> {
    let mut args = env::args().skip(1);
    let mut input = None;
    let mut output = None;
    let mut check_only = false;
    let mut emit_rust_path = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--check" => check_only = true,
            "--emit-rust" => {
                let path = args
                    .next()
                    .ok_or_else(|| Error::new("--emit-rust requires a path"))?;
                emit_rust_path = Some(PathBuf::from(path));
            }
            "-o" | "--output" => {
                let path = args
                    .next()
                    .ok_or_else(|| Error::new("--output requires a path"))?;
                output = Some(PathBuf::from(path));
            }
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            _ if arg.starts_with('-') => {
                return Err(Error::new(format!("unknown option `{arg}`")));
            }
            _ => {
                if input.replace(PathBuf::from(arg)).is_some() {
                    return Err(Error::new("only one input file is supported"));
                }
            }
        }
    }

    let input = input.ok_or_else(|| Error::new("missing input file"))?;
    if input.extension().and_then(|ext| ext.to_str()) != Some("emel") {
        return Err(Error::new("input file extension must be .emel"));
    }
    let output = output.unwrap_or_else(|| input.with_extension(""));

    Ok(Args {
        input,
        output,
        check_only,
        emit_rust: emit_rust_path,
    })
}

fn print_help() {
    eprintln!("Usage: compiler [--check] [--emit-rust PATH] [-o OUTPUT] INPUT.emel");
}

fn run() -> Result<()> {
    let args = parse_args()?;
    let source = fs::read_to_string(&args.input).map_err(|err| {
        Error::new(format!(
            "failed to read input file `{}`: {err}",
            args.input.display()
        ))
    })?;

    let (program, typed) = compile_source(&source)?;
    let rust_source = emit_rust(&program, &typed);

    if let Some(path) = &args.emit_rust {
        fs::write(path, &rust_source).map_err(|err| {
            Error::new(format!(
                "failed to write Rust output `{}`: {err}",
                path.display()
            ))
        })?;
    }

    if !args.check_only {
        build(&args.input, &args.output, &rust_source)?;
        eprintln!("built {}", args.output.display());
    }

    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_empty_main() {
        let (_, typed) = compile_source("fn main() {\n}\n").unwrap();
        assert_eq!(typed.functions[0].name, "main");
        assert_eq!(typed.functions[0].ret, PrimType::Unit);
    }

    #[test]
    fn infers_i32_function() {
        let (_, typed) = compile_source(
            r#"
fn add(x, y) {
  x + y
}

fn main() {
  add(20, 22)
}
"#,
        )
        .unwrap();
        let add = typed
            .functions
            .iter()
            .find(|function| function.name == "add")
            .unwrap();
        assert_eq!(add.params, vec![PrimType::I32, PrimType::I32]);
        assert_eq!(add.ret, PrimType::I32);
    }

    #[test]
    fn accepts_return_annotation_and_exits_with_main_i32() {
        let source = r#"
fn add(x, y) -> i32 {
  x + y
}

fn main() {
  add(20, 22)
}
"#;
        let (program, typed) = compile_source(source).unwrap();
        let main = typed
            .functions
            .iter()
            .find(|function| function.name == "main")
            .unwrap();
        assert_eq!(main.ret, PrimType::I32);

        let rust_source = emit_rust(&program, &typed);
        assert!(rust_source.contains("std::process::exit(_emela_result);"));
    }

    #[test]
    fn emits_rust_match_expression() {
        let (program, typed) =
            compile_source("fn main() -> I32 { match true { true -> 1 false -> 0 } }").unwrap();
        let rust_source = emit_rust(&program, &typed);
        assert!(rust_source.contains("match true { true => 1i32, false => 0i32, }"));
    }

    #[test]
    fn rejects_match_pattern_type_mismatch() {
        let error = compile_source("fn main() { match 1 { true -> 2 false -> 3 } }").unwrap_err();
        assert!(error.to_string().contains("type mismatch"));
    }
}
