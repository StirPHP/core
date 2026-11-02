//! Stable diagnostic codes.

pub use stir_db::codes::SYNTAX;

pub const UNKNOWN_TYPE: &str = "SPHP0002";
pub const TOP_TYPE: &str = "SPHP0003";
pub const MISSING_TYPE: &str = "SPHP0004";
pub const TYPE_MISMATCH: &str = "SPHP0005";
pub const UNDEFINED_VARIABLE: &str = "SPHP0006";
pub const UNDEFINED_FUNCTION: &str = "SPHP0007";
pub const UNDEFINED_CLASS: &str = "SPHP0008";
pub const UNDEFINED_MEMBER: &str = "SPHP0009";
pub const REASSIGN_IMMUTABLE: &str = "SPHP0010";
pub const VAL_VAR_CLASH: &str = "SPHP0011";
pub const NULLABLE_ACCESS: &str = "SPHP0012";
pub const INCOMPATIBLE_COMPARISON: &str = "SPHP0013";
pub const ARGUMENT_COUNT: &str = "SPHP0014";
pub const INVALID_OPERAND: &str = "SPHP0015";
pub const SIGIL_MISMATCH: &str = "SPHP0016";
pub const UNDECLARED_ASSIGNMENT: &str = "SPHP0017";
pub const NOT_CALLABLE: &str = "SPHP0018";
pub const ANY_PLATFORM_ESCAPE: &str = "SPHP0019";
pub const GENERIC_ARITY: &str = "SPHP0020";
pub const UNNECESSARY_NULL_CHECK: &str = "SPHP0021";
pub const REDUNDANT_CONVERSION: &str = "SPHP0033";
pub const RETURN_VALUE: &str = "SPHP0022";
pub const INSTANTIATION: &str = "SPHP0023";
pub const VISIBILITY: &str = "SPHP0024";
pub const DUPLICATE_DECLARATION: &str = "SPHP0025";
pub const UNSUPPORTED_SYNTAX: &str = "SPHP0026";
pub const VARIANCE: &str = "SPHP0027";
pub const TYPE_BOUND: &str = "SPHP0028";
pub const NOT_INLINABLE: &str = "SPHP0029";
pub const MISSING_IMPLEMENTATION: &str = "SPHP0030";
pub const INCOMPATIBLE_OVERRIDE: &str = "SPHP0031";
pub const DATA_CLASS: &str = "SPHP0032";
