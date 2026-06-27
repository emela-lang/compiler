use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Program {
    pub(crate) items: Vec<TopLevelItem>,
}

impl Program {
    pub(crate) fn functions(&self) -> Vec<&Function> {
        self.items
            .iter()
            .filter_map(|item| match item {
                TopLevelItem::Function(function) => Some(function),
                TopLevelItem::Import(_) | TopLevelItem::Struct(_) | TopLevelItem::Enum(_) => None,
            })
            .collect()
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) enum TopLevelItem {
    Import(ImportDecl),
    Struct(StructDecl),
    Enum(EnumDecl),
    Function(Function),
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ImportDecl {
    pub(crate) path: Vec<String>,
    pub(crate) name: String,
    pub(crate) origin: ImportOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) enum ImportOrigin {
    User,
    Stdlib,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct StructDecl {
    pub(crate) name: String,
    pub(crate) type_params: Vec<String>,
    pub(crate) field: StructField,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct StructField {
    pub(crate) name: String,
    pub(crate) ty: Type,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct EnumDecl {
    pub(crate) name: String,
    pub(crate) type_params: Vec<String>,
    pub(crate) variants: Vec<EnumVariant>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct EnumVariant {
    pub(crate) name: String,
    pub(crate) payload: Option<Type>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Function {
    pub(crate) name: String,
    pub(crate) params: Vec<FunctionParam>,
    pub(crate) return_annotation: Option<Type>,
    pub(crate) requires: Option<Vec<Capability>>,
    pub(crate) body: Block,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct FunctionParam {
    pub(crate) name: String,
    pub(crate) ty: Option<Type>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Block {
    pub(crate) items: Vec<BlockItem>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) enum BlockItem {
    Binding {
        name: String,
        ty: Option<Type>,
        expr: Expr,
    },
    Expr(Expr),
}

#[derive(Debug, Clone, Serialize)]
pub(crate) enum Expr {
    Int(i32),
    Bool(bool),
    String(String),
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
    FieldAccess {
        receiver: Box<Expr>,
        field: String,
    },
    StructLiteral {
        name: String,
        field: String,
        value: Box<Expr>,
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

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MatchArm {
    pub(crate) pattern: Pattern,
    pub(crate) expr: Expr,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) enum Pattern {
    Int(i32),
    Bool(bool),
    Unit,
    Var(String),
    Variant {
        name: String,
        payload: Option<Box<Pattern>>,
    },
    Wildcard,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) enum BinaryOp {
    Add,
    Sub,
    Mul,
    Eq,
    Lt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum PrimType {
    I32,
    Bool,
    String,
    Unit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) enum Type {
    Prim(PrimType),
    Named(String),
    GenericParam(String),
    Apply { name: String, args: Vec<Type> },
    Function(FunctionType),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct FunctionType {
    pub(crate) params: Vec<Type>,
    pub(crate) ret: Box<Type>,
    pub(crate) effectful: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) enum Capability {
    Stdout,
    Stdin,
    Stderr,
    FileRead,
    FileWrite,
    Clock,
    Random,
    Env,
    Process,
    Network,
    HostImport,
}

impl Capability {
    pub(crate) fn parse(name: &str) -> Option<Self> {
        match name {
            "Stdout" => Some(Self::Stdout),
            "Stdin" => Some(Self::Stdin),
            "Stderr" => Some(Self::Stderr),
            "FileRead" => Some(Self::FileRead),
            "FileWrite" => Some(Self::FileWrite),
            "Clock" => Some(Self::Clock),
            "Random" => Some(Self::Random),
            "Env" => Some(Self::Env),
            "Process" => Some(Self::Process),
            "Network" => Some(Self::Network),
            "HostImport" => Some(Self::HostImport),
            _ => None,
        }
    }
}
