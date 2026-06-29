use crate::ast::{
    BinaryOp, Block, BlockItem, EffectRow, Expr, Function, Import, Param, Program, Type,
};
use crate::error::{Diagnostic, Error, Result, Span};
use crate::lexer::{lex, Token, TokenKind};

pub(crate) fn parse_program(label: &str, source: &str) -> Result<Program> {
    let tokens = lex(label, source)?;
    Parser { tokens, current: 0 }.parse_program()
}

struct Parser {
    tokens: Vec<Token>,
    current: usize,
}

impl Parser {
    fn parse_program(&mut self) -> Result<Program> {
        let mut module = None;
        let mut imports = Vec::new();
        let mut functions = Vec::new();
        self.skip_newlines();
        if self.eat(&TokenKind::Module) {
            module = Some(self.parse_path_name()?);
            self.skip_newlines();
        }
        while !self.at(&TokenKind::Eof) {
            if self.at(&TokenKind::Import) {
                imports.push(self.parse_import()?);
            } else {
                let is_public = self.eat(&TokenKind::Pub);
                functions.push(self.parse_function(is_public)?);
            }
            self.skip_newlines();
        }
        Ok(Program {
            module,
            imports,
            functions,
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
        self.expect(&TokenKind::LParen)?;
        let params = self.parse_params()?;
        self.expect(&TokenKind::RParen)?;
        self.expect(&TokenKind::Arrow)?;
        let ret = self.parse_type()?;
        let effects = self.parse_effect_row()?;
        let body = self.parse_block()?;
        Ok(Function {
            name,
            name_span,
            is_public,
            params,
            ret,
            effects,
            body,
        })
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
        let name = self.expect_ident()?;
        match name.as_str() {
            "Unit" => Ok(Type::Unit),
            "Bool" => Ok(Type::Bool),
            "Int" => Ok(Type::Int),
            "String" => Ok(Type::String),
            _ => Err(Error::diagnostic(
                Diagnostic::new("Unknown type")
                    .label(span, format!("unknown type `{name}`"))
                    .help("The minimal compiler currently supports Unit, Bool, Int, and String."),
            )),
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
        let mut expr = self.parse_primary()?;
        while self.eat(&TokenKind::Star) {
            let right = self.parse_primary()?;
            let span = expr.span().merge(&right.span());
            expr = Expr::Binary {
                op: BinaryOp::Mul,
                left: Box::new(expr),
                right: Box::new(right),
                span,
            };
        }
        Ok(expr)
    }

    fn parse_primary(&mut self) -> Result<Expr> {
        match self.peek().kind.clone() {
            TokenKind::Int(value) => {
                let span = self.bump().span;
                Ok(Expr::Int(value, span))
            }
            TokenKind::String(value) => {
                let span = self.bump().span;
                Ok(Expr::String(value, span))
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
                if self.eat(&TokenKind::LParen) {
                    let mut args = Vec::new();
                    if !self.at(&TokenKind::RParen) {
                        args.push(self.parse_expr()?);
                        while self.eat(&TokenKind::Comma) {
                            args.push(self.parse_expr()?);
                        }
                    }
                    let end = self.expect(&TokenKind::RParen)?.span;
                    Ok(Expr::Call {
                        name,
                        args,
                        span: span.merge(&end),
                    })
                } else {
                    Ok(Expr::Var(name, span))
                }
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

#[allow(dead_code)]
fn _span(_: &Span) {}
