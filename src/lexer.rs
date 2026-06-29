use std::sync::Arc;

use crate::error::{Diagnostic, Error, Result, SourceFile, Span};

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TokenKind {
    Fn,
    Import,
    Let,
    Module,
    Pub,
    Uses,
    True,
    False,
    Ident(String),
    Int(i32),
    String(String),
    LParen,
    RParen,
    LBrace,
    RBrace,
    Comma,
    Colon,
    Dot,
    Eq,
    EqEq,
    Arrow,
    Lt,
    Gt,
    Plus,
    Minus,
    Star,
    Newline,
    Eof,
}

#[derive(Debug, Clone)]
pub(crate) struct Token {
    pub(crate) kind: TokenKind,
    pub(crate) span: Span,
}

pub(crate) fn lex(label: &str, source: &str) -> Result<Vec<Token>> {
    let file = SourceFile::new(label, source.to_string());
    lex_with_file(source, file)
}

fn lex_with_file(source: &str, file: Arc<SourceFile>) -> Result<Vec<Token>> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        match bytes[i] {
            b' ' | b'\t' | b'\r' => i += 1,
            b'\n' => push(&mut tokens, TokenKind::Newline, file.clone(), start, &mut i),
            b'-' if i + 1 < bytes.len() && bytes[i + 1] == b'-' => {
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'-' if i + 1 < bytes.len() && bytes[i + 1] == b'>' => {
                tokens.push(Token {
                    kind: TokenKind::Arrow,
                    span: Span::new(file.clone(), start, start + 2),
                });
                i += 2;
            }
            b'=' if i + 1 < bytes.len() && bytes[i + 1] == b'=' => {
                tokens.push(Token {
                    kind: TokenKind::EqEq,
                    span: Span::new(file.clone(), start, start + 2),
                });
                i += 2;
            }
            b'(' => push(&mut tokens, TokenKind::LParen, file.clone(), start, &mut i),
            b')' => push(&mut tokens, TokenKind::RParen, file.clone(), start, &mut i),
            b'{' => push(&mut tokens, TokenKind::LBrace, file.clone(), start, &mut i),
            b'}' => push(&mut tokens, TokenKind::RBrace, file.clone(), start, &mut i),
            b',' => push(&mut tokens, TokenKind::Comma, file.clone(), start, &mut i),
            b':' => push(&mut tokens, TokenKind::Colon, file.clone(), start, &mut i),
            b'.' => push(&mut tokens, TokenKind::Dot, file.clone(), start, &mut i),
            b'=' => push(&mut tokens, TokenKind::Eq, file.clone(), start, &mut i),
            b'<' => push(&mut tokens, TokenKind::Lt, file.clone(), start, &mut i),
            b'>' => push(&mut tokens, TokenKind::Gt, file.clone(), start, &mut i),
            b'+' => push(&mut tokens, TokenKind::Plus, file.clone(), start, &mut i),
            b'-' => push(&mut tokens, TokenKind::Minus, file.clone(), start, &mut i),
            b'*' => push(&mut tokens, TokenKind::Star, file.clone(), start, &mut i),
            b'0'..=b'9' => {
                i += 1;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                let text = &source[start..i];
                let value = text.parse::<i32>().map_err(|_| {
                    Error::diagnostic(
                        Diagnostic::new("Integer literal is too large")
                            .label(Span::new(file.clone(), start, i), "does not fit in Int"),
                    )
                })?;
                tokens.push(Token {
                    kind: TokenKind::Int(value),
                    span: Span::new(file.clone(), start, i),
                });
            }
            b'"' => {
                i += 1;
                let mut value = String::new();
                while i < bytes.len() {
                    match bytes[i] {
                        b'"' => {
                            i += 1;
                            break;
                        }
                        b'\\' => {
                            i += 1;
                            if i >= bytes.len() {
                                return Err(unterminated_string(file.clone(), start));
                            }
                            let escaped = match bytes[i] {
                                b'n' => '\n',
                                b't' => '\t',
                                b'"' => '"',
                                b'\\' => '\\',
                                other => {
                                    return Err(Error::diagnostic(
                                        Diagnostic::new("Unsupported string escape").label(
                                            Span::new(file.clone(), i - 1, i + 1),
                                            format!("unsupported escape `\\{}`", other as char),
                                        ),
                                    ));
                                }
                            };
                            value.push(escaped);
                            i += 1;
                        }
                        b'\n' => return Err(unterminated_string(file.clone(), start)),
                        _ => {
                            let ch = source[i..].chars().next().expect("char boundary");
                            value.push(ch);
                            i += ch.len_utf8();
                        }
                    }
                }
                if bytes.get(i.saturating_sub(1)) != Some(&b'"') {
                    return Err(unterminated_string(file.clone(), start));
                }
                tokens.push(Token {
                    kind: TokenKind::String(value),
                    span: Span::new(file.clone(), start, i),
                });
            }
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                i += 1;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                let text = &source[start..i];
                let kind = match text {
                    "fn" => TokenKind::Fn,
                    "import" => TokenKind::Import,
                    "let" => TokenKind::Let,
                    "module" => TokenKind::Module,
                    "pub" => TokenKind::Pub,
                    "uses" => TokenKind::Uses,
                    "true" => TokenKind::True,
                    "false" => TokenKind::False,
                    _ => TokenKind::Ident(text.to_string()),
                };
                tokens.push(Token {
                    kind,
                    span: Span::new(file.clone(), start, i),
                });
            }
            other => {
                return Err(Error::diagnostic(
                    Diagnostic::new("Unexpected character").label(
                        Span::new(file.clone(), start, start + 1),
                        format!("unexpected character `{}`", other as char),
                    ),
                ));
            }
        }
    }
    tokens.push(Token {
        kind: TokenKind::Eof,
        span: Span::point(file, source.len()),
    });
    Ok(tokens)
}

fn push(
    tokens: &mut Vec<Token>,
    kind: TokenKind,
    file: Arc<SourceFile>,
    start: usize,
    i: &mut usize,
) {
    tokens.push(Token {
        kind,
        span: Span::new(file, start, start + 1),
    });
    *i += 1;
}

fn unterminated_string(file: Arc<SourceFile>, start: usize) -> Error {
    Error::diagnostic(
        Diagnostic::new("Unterminated string")
            .label(Span::new(file, start, start + 1), "string starts here")
            .help("Add a closing double quote before the end of the line."),
    )
}
