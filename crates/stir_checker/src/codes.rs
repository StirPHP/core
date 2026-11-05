//! Stable diagnostic codes.

pub use stir_db::codes::SYNTAX;

pub const UNKNOWN_TYPE: &str = "SPHP0002";
pub const TYPE_MISMATCH: &str = "SPHP0005";
pub const UNDEFINED_VARIABLE: &str = "SPHP0006";
pub const UNDEFINED_FUNCTION: &str = "SPHP0007";
pub const UNDEFINED_CLASS: &str = "SPHP0008";
pub const UNDEFINED_MEMBER: &str = "SPHP0009";
pub const REASSIGN_IMMUTABLE: &str = "SPHP0010";
pub const ARGUMENT_COUNT: &str = "SPHP0014";
pub const INVALID_OPERAND: &str = "SPHP0015";
pub const NOT_CALLABLE: &str = "SPHP0018";
pub const GENERIC_ARITY: &str = "SPHP0020";
pub const RETURN_VALUE: &str = "SPHP0022";
pub const INSTANTIATION: &str = "SPHP0023";
pub const VISIBILITY: &str = "SPHP0024";
pub const VARIANCE: &str = "SPHP0027";
pub const TYPE_BOUND: &str = "SPHP0028";
pub const INCOMPATIBLE_OVERRIDE: &str = "SPHP0031";
