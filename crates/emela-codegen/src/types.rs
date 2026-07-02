//! The shared Emela type-system types referenced by the IR.
//!
//! These live in `emela-codegen` (not the frontend AST) because they are part
//! of the IR contract: backends and external plugins reason about them.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Type {
    Unit,
    Bool,
    Int,
    Float,
    String,
    /// A single Unicode scalar value (spec 0017).
    Char,
    Array(Box<Type>),
    Record,
    /// A named enum type (spec 0005), identified by its declared name.
    Enum(String),
    /// An optional value (spec 0001): `Option<T>`.
    Option(Box<Type>),
    /// The empty type of `throw` and `panic` (spec 0011). It is assignable to
    /// any expected type; no value ever has this type.
    Never,
    Function(FunctionType),
    OpaqueFunction,
    /// A generic function's type parameter (spec 0014), e.g. `T`. It only ever
    /// appears in the frontend (function signatures and the AST while checking a
    /// generic body); monomorphization substitutes it for a concrete type before
    /// lowering, so it never reaches the typed IR or a backend.
    Var(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FunctionType {
    pub params: Vec<Type>,
    pub ret: Box<Type>,
    /// The error type the function may throw (spec 0008/0011), if any. `None`
    /// is a non-throwing function. It is part of the type: two functions that
    /// differ only in `throws` are different types.
    #[serde(default)]
    pub throws: Option<Box<Type>>,
    pub effects: EffectRow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    /// String concatenation `++` (spec 0017).
    Concat,
    Eq,
    Lt,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct EffectRow {
    pub effects: Vec<String>,
}

impl EffectRow {
    pub fn sorted(mut effects: Vec<String>) -> Self {
        effects.sort();
        effects.dedup();
        Self { effects }
    }

    pub fn union(&mut self, other: &EffectRow) {
        self.effects.extend(other.effects.iter().cloned());
        self.effects.sort();
        self.effects.dedup();
    }

    pub fn is_subset_of(&self, other: &EffectRow) -> bool {
        self.effects
            .iter()
            .all(|effect| other.effects.contains(effect))
    }
}
