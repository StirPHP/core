//! Stable diagnostic codes.

pub use stir_db::codes::SYNTAX;

pub const UNKNOWN_TYPE: &str = "SPHP0002";
pub const TYPE_MISMATCH: &str = "SPHP0005";
pub const UNDEFINED_VARIABLE: &str = "SPHP0006";
pub const UNDEFINED_FUNCTION: &str = "SPHP0007";
pub const UNDEFINED_CLASS: &str = "SPHP0008";
pub const UNDEFINED_MEMBER: &str = "SPHP0009";
pub const REASSIGN_IMMUTABLE: &str = "SPHP0010";
pub const NULLABLE_ACCESS: &str = "SPHP0012";
pub const UNNECESSARY_NULL_CHECK: &str = "SPHP0021";
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
/// Syntax newer than the project's PHP version.
pub const PHP_SYNTAX_VERSION: &str = "SPHP0060";
/// Something of PHP's library that isn't on the project's PHP version.
pub const PHP_LIBRARY_VERSION: &str = "SPHP0061";
/// Fine on the project's PHP version, but deprecated there.
pub const PHP_DEPRECATED: &str = "SPHP0062";
