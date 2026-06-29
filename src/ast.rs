use crate::error::Span;

#[derive(Debug, Clone)]
pub(crate) struct Program {
    pub(crate) module: Option<String>,
    pub(crate) imports: Vec<Import>,
    pub(crate) functions: Vec<Function>,
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
    Bool(bool, Span),
    String(String, Span),
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
            | Expr::Bool(_, span)
            | Expr::String(_, span)
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
    String,
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
