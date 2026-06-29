use crate::error::Span;

#[derive(Debug, Clone)]
pub(crate) struct Program {
    pub(crate) functions: Vec<Function>,
}

#[derive(Debug, Clone)]
pub(crate) struct Function {
    pub(crate) name: String,
    pub(crate) name_span: Span,
    pub(crate) params: Vec<Param>,
    pub(crate) ret: Type,
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
    Array(Vec<Expr>, Span),
    Unit(Span),
    Var(String, Span),
    Call {
        name: String,
        args: Vec<Expr>,
        span: Span,
    },
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
        span: Span,
    },
    Block(Block),
}

impl Expr {
    pub(crate) fn span(&self) -> Span {
        match self {
            Expr::Int(_, span)
            | Expr::Float(_, span)
            | Expr::Bool(_, span)
            | Expr::String(_, span)
            | Expr::Array(_, span)
            | Expr::Unit(span)
            | Expr::Var(_, span) => span.clone(),
            Expr::Call { span, .. } | Expr::Binary { span, .. } => span.clone(),
            Expr::Block(block) => block.span.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BinaryOp {
    Add,
    Sub,
    Mul,
    Eq,
    Lt,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Type {
    Unit,
    Bool,
    Int,
    Float,
    String,
    Array(Box<Type>),
    Record,
    Enum,
    Function,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct EffectRow {
    pub(crate) effects: Vec<String>,
}

impl EffectRow {
    pub(crate) fn sorted(mut effects: Vec<String>) -> Self {
        effects.sort();
        effects.dedup();
        Self { effects }
    }

    pub(crate) fn union(&mut self, other: &EffectRow) {
        self.effects.extend(other.effects.iter().cloned());
        self.effects.sort();
        self.effects.dedup();
    }

    pub(crate) fn is_subset_of(&self, other: &EffectRow) -> bool {
        self.effects
            .iter()
            .all(|effect| other.effects.contains(effect))
    }
}
