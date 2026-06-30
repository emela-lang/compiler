use crate::error::Span;

// The type-system types are part of the IR contract and live in
// `emela-codegen`; the frontend AST re-uses them.
pub(crate) use emela_codegen::{BinaryOp, EffectRow, FunctionType, Type};

#[derive(Debug, Clone)]
pub(crate) struct Program {
    pub(crate) module: Option<String>,
    pub(crate) imports: Vec<Import>,
    pub(crate) functions: Vec<Function>,
    pub(crate) externs: Vec<Extern>,
    pub(crate) enums: Vec<EnumDecl>,
}

/// An `enum` declaration (spec 0005).
#[derive(Debug, Clone)]
pub(crate) struct EnumDecl {
    pub(crate) name: String,
    pub(crate) name_span: Span,
    pub(crate) variants: Vec<EnumVariant>,
}

/// One variant of an enum, with its payload field types (possibly empty).
#[derive(Debug, Clone)]
pub(crate) struct EnumVariant {
    pub(crate) name: String,
    pub(crate) name_span: Span,
    pub(crate) fields: Vec<Type>,
}

/// A platform-function declaration (`extern fn`, spec 0013). It has no body; the
/// backend supplies the implementation. `module` is the declaring file's module
/// path, used with `name` to form the canonical platform name.
#[derive(Debug, Clone)]
pub(crate) struct Extern {
    pub(crate) name: String,
    pub(crate) name_span: Span,
    pub(crate) module: Option<String>,
    pub(crate) params: Vec<Param>,
    pub(crate) ret: Type,
    pub(crate) throws: Option<Type>,
    pub(crate) effects: EffectRow,
}

impl Extern {
    /// The canonical platform name, e.g. `io.write_stdout`.
    pub(crate) fn canonical(&self) -> String {
        match &self.module {
            Some(module) if !module.is_empty() => format!("{module}.{}", self.name),
            _ => self.name.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Import {
    pub(crate) path: Vec<String>,
    pub(crate) span: Span,
}

impl Import {
    pub(crate) fn item_name(&self) -> &str {
        self.path.last().map(String::as_str).unwrap_or("")
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Function {
    pub(crate) name: String,
    pub(crate) name_span: Span,
    pub(crate) is_public: bool,
    pub(crate) params: Vec<Param>,
    pub(crate) ret: Type,
    pub(crate) throws: Option<Type>,
    pub(crate) effects: EffectRow,
    pub(crate) body: Block,
}

#[derive(Debug, Clone)]
pub(crate) struct Param {
    pub(crate) name: String,
    pub(crate) name_span: Span,
    pub(crate) ty: Type,
}

#[derive(Debug, Clone)]
pub(crate) struct Block {
    pub(crate) items: Vec<BlockItem>,
    pub(crate) span: Span,
}

#[derive(Debug, Clone)]
pub(crate) enum BlockItem {
    Let {
        name: String,
        name_span: Span,
        ty: Option<Type>,
        value: Expr,
    },
    Expr(Expr),
}

#[derive(Debug, Clone)]
pub(crate) enum Expr {
    Int(i32, Span),
    Float(f64, Span),
    Bool(bool, Span),
    String(String, Span),
    Char(char, Span),
    Array(Vec<Expr>, Span),
    Unit(Span),
    Var(String, Span),
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
        span: Span,
    },
    Fn {
        params: Vec<Param>,
        ret: Type,
        throws: Option<Type>,
        effects: EffectRow,
        body: Block,
        span: Span,
    },
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
        span: Span,
    },
    Block(Block),
    /// `if cond { then } else { els }` (spec 0015).
    If {
        cond: Box<Expr>,
        then: Block,
        els: Block,
        span: Span,
    },
    /// `throw e` (spec 0011).
    Throw {
        value: Box<Expr>,
        span: Span,
    },
    /// `panic(msg)` (spec 0011).
    Panic {
        message: Box<Expr>,
        span: Span,
    },
    /// `expr?` (spec 0011): error / `None` propagation.
    Question {
        value: Box<Expr>,
        span: Span,
    },
    /// `match scrutinee { arms }` (spec 0005).
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
        span: Span,
    },
    /// `try { body } catch { arms }` (spec 0011).
    Try {
        body: Block,
        arms: Vec<MatchArm>,
        span: Span,
    },
    /// An enum/`Option` variant: `Some(x)`, `None`, `Color.Red`.
    Variant {
        enum_name: Option<String>,
        variant: String,
        args: Vec<Expr>,
        span: Span,
    },
}

impl Expr {
    pub(crate) fn span(&self) -> Span {
        match self {
            Expr::Int(_, span)
            | Expr::Float(_, span)
            | Expr::Bool(_, span)
            | Expr::String(_, span)
            | Expr::Char(_, span)
            | Expr::Array(_, span)
            | Expr::Unit(span)
            | Expr::Var(_, span) => span.clone(),
            Expr::Call { span, .. }
            | Expr::Fn { span, .. }
            | Expr::Binary { span, .. }
            | Expr::Throw { span, .. }
            | Expr::Panic { span, .. }
            | Expr::Question { span, .. }
            | Expr::Match { span, .. }
            | Expr::Try { span, .. }
            | Expr::If { span, .. }
            | Expr::Variant { span, .. } => span.clone(),
            Expr::Block(block) => block.span.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct MatchArm {
    pub(crate) pattern: Pattern,
    pub(crate) guard: Option<Expr>,
    pub(crate) body: Expr,
    #[allow(dead_code)]
    pub(crate) span: Span,
}

#[derive(Debug, Clone)]
pub(crate) enum Pattern {
    /// A variant pattern, optionally qualified by enum name: `Some(v)`, `None`,
    /// `Color.Red`.
    Variant {
        enum_name: Option<String>,
        variant: String,
        fields: Vec<FieldBinding>,
        span: Span,
    },
    /// `_`: ignore the scrutinee.
    Wildcard(Span),
    /// Bind the whole scrutinee to a name (catch-all).
    Binding { name: String, span: Span },
}

#[derive(Debug, Clone)]
pub(crate) enum FieldBinding {
    /// Bind the payload field to a name.
    Name(String),
    /// `_`: ignore the payload field.
    Ignore,
}
