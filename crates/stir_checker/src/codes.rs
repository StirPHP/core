//! Stable diagnostic codes: PHPStan's identifier where one of its rules means the same, so that
//! `@phpstan-ignore`, `ignoreErrors` and baselines written for PHPStan apply here too, and
//! `SPHP00xx` for what only this checker reports.

pub use stir_db::codes::SYNTAX;

pub const ARGUMENTS_COUNT: &str = "arguments.count";
pub const ARGUMENT_MISSING: &str = "argument.missing";
pub const ARGUMENT_TYPE: &str = "argument.type";
pub const ARGUMENT_UNKNOWN: &str = "argument.unknown";
pub const ASSIGN_PROPERTY_PROTECTED_SET: &str = "assign.propertyProtectedSet";
pub const ASSIGN_PROPERTY_READ_ONLY: &str = "assign.propertyReadOnly";
pub const ASSIGN_PROPERTY_TYPE: &str = "assign.propertyType";
pub const ASSIGN_THIS: &str = "assign.this";
pub const CALLABLE_NON_CALLABLE: &str = "callable.nonCallable";
pub const CLASS_CONSTANT_NOT_FOUND: &str = "classConstant.notFound";
pub const CLASS_NOT_FOUND: &str = "class.notFound";
pub const CLASS_NO_PARENT: &str = "class.noParent";
pub const CONSTANT_NOT_FOUND: &str = "constant.notFound";
pub const FOREACH_NON_ITERABLE: &str = "foreach.nonIterable";
pub const FUNCTION_NOT_FOUND: &str = "function.notFound";
pub const GENERICS_MORE_TYPES: &str = "generics.moreTypes";
pub const GENERICS_NOT_GENERIC: &str = "generics.notGeneric";
pub const GENERICS_NOT_SUBTYPE: &str = "generics.notSubtype";
pub const GENERICS_VARIANCE: &str = "generics.variance";
pub const IGNORE_PARSE_ERROR: &str = "ignore.parseError";
pub const INTERFACE_NOT_FOUND: &str = "interface.notFound";
pub const METHOD_CHILD_PARAMETER_TYPE: &str = "method.childParameterType";
pub const METHOD_CHILD_RETURN_TYPE: &str = "method.childReturnType";
pub const METHOD_NON_OBJECT: &str = "method.nonObject";
pub const METHOD_NOT_FOUND: &str = "method.notFound";
pub const METHOD_PRIVATE: &str = "method.private";
pub const METHOD_PROTECTED: &str = "method.protected";
pub const MISSING_TYPE_ITERABLE_VALUE: &str = "missingType.iterableValue";
pub const MISSING_TYPE_PARAMETER: &str = "missingType.parameter";
pub const MISSING_TYPE_PROPERTY: &str = "missingType.property";
pub const MISSING_TYPE_RETURN: &str = "missingType.return";
pub const NEW_ABSTRACT: &str = "new.abstract";
pub const NEW_ENUM: &str = "new.enum";
pub const NEW_INTERFACE: &str = "new.interface";
pub const NEW_STATIC: &str = "new.static";
pub const NEW_TRAIT: &str = "new.trait";
pub const NULLSAFE_ASSIGN: &str = "nullsafe.assign";
pub const NULL_COALESCE_VARIABLE: &str = "nullCoalesce.variable";
pub const OFFSET_ASSIGN_VALUE: &str = "offsetAssign.valueType";
pub const OFFSET_NOT_ACCESSIBLE: &str = "offsetAccess.nonOffsetAccessible";
pub const OFFSET_NOT_FOUND: &str = "offsetAccess.notFound";
pub const OUT_OF_CLASS_PARENT: &str = "outOfClass.parent";
pub const OUT_OF_CLASS_SELF: &str = "outOfClass.self";
pub const OUT_OF_CLASS_STATIC: &str = "outOfClass.static";
pub const PARAMETER_DEFAULT_VALUE: &str = "parameter.defaultValue";
pub const PARAMETER_IMPLICITLY_NULLABLE: &str = "parameter.implicitlyNullable";
pub const PARAMETER_MISSING: &str = "parameter.missing";
pub const PARAMETER_NOT_OPTIONAL: &str = "parameter.notOptional";
pub const PARAMETER_PHPDOC_TYPE: &str = "parameter.phpDocType";
pub const PROPERTY_DEFAULT_VALUE: &str = "property.defaultValue";
pub const PROPERTY_NON_OBJECT: &str = "property.nonObject";
pub const PROPERTY_NOT_FOUND: &str = "property.notFound";
pub const PROPERTY_PHPDOC_TYPE: &str = "property.phpDocType";
pub const PROPERTY_PRIVATE: &str = "property.private";
pub const PROPERTY_PROTECTED: &str = "property.protected";
pub const PROPERTY_READONLY_NOT_IN_CONSTRUCTOR: &str = "property.readOnlyAssignNotInConstructor";
pub const PROPERTY_READONLY_OUT_OF_CLASS: &str = "property.readOnlyAssignOutOfClass";
pub const CLASS_CONSTANT_PHPDOC_TYPE: &str = "classConstant.phpDocType";
pub const RETURN_MISSING: &str = "return.missing";
pub const RETURN_PHPDOC_TYPE: &str = "return.phpDocType";
pub const RETURN_TYPE: &str = "return.type";
pub const RETURN_VOID: &str = "return.void";
pub const STATIC_METHOD_NOT_FOUND: &str = "staticMethod.notFound";
pub const STATIC_METHOD_PRIVATE: &str = "staticMethod.private";
pub const STATIC_METHOD_PROTECTED: &str = "staticMethod.protected";
pub const STATIC_PROPERTY_NOT_FOUND: &str = "staticProperty.notFound";
pub const STATIC_PROPERTY_PRIVATE: &str = "staticProperty.private";
pub const STATIC_PROPERTY_PROTECTED: &str = "staticProperty.protected";
pub const THROW_NOT_THROWABLE: &str = "throw.notThrowable";
pub const TRAIT_NOT_FOUND: &str = "trait.notFound";
pub const TYPE_ALIAS_NOT_FOUND: &str = "typeAlias.notFound";
pub const VARIABLE_UNDEFINED: &str = "variable.undefined";

// Syntax newer than the project's PHP version, where PHPStan has a rule for that piece of syntax.
pub const NAMED_ARGUMENTS_NOT_SUPPORTED: &str = "argument.namedNotSupported";
pub const CALLABLE_NOT_SUPPORTED: &str = "callable.notSupported";
pub const CAST_VOID_NOT_SUPPORTED: &str = "cast.voidNotSupported";
pub const CATCH_NON_CAPTURING_NOT_SUPPORTED: &str = "catch.nonCapturingNotSupported";
pub const CLASS_CONSTANT_DYNAMIC_FETCH: &str = "classConstant.dynamicFetch";
pub const CLASS_CONSTANT_FINAL_NOT_SUPPORTED: &str = "classConstant.finalNotSupported";
pub const CLASS_CONSTANT_IN_TRAIT: &str = "classConstant.inTrait";
/// Also what PHPStan's rule for readonly classes reports them as.
pub const CLASS_CONSTANT_NATIVE_TYPE_NOT_SUPPORTED: &str = "classConstant.nativeTypeNotSupported";
pub const CLASS_CONSTANT_NOT_SUPPORTED: &str = "classConstant.notSupported";
pub const CONSTANT_ATTRIBUTES_NOT_SUPPORTED: &str = "constant.attributesNotSupported";
pub const PARAMETER_UNION_NOT_SUPPORTED: &str = "parameter.unionTypeNotSupported";
pub const PROPERTY_HOOKS_NOT_SUPPORTED: &str = "property.hooksNotSupported";
pub const PROPERTY_PROMOTED_NOT_SUPPORTED: &str = "property.promotedNotSupported";
pub const PROPERTY_READONLY_NOT_SUPPORTED: &str = "property.readOnlyNotSupported";
pub const PROPERTY_STATIC_ASYMMETRIC_VISIBILITY: &str = "property.staticAsymmetricVisibility";
pub const RETURN_NEVER_NOT_SUPPORTED: &str = "return.neverTypeNotSupported";
pub const RETURN_UNION_NOT_SUPPORTED: &str = "return.unionTypeNotSupported";
pub const THROW_NOT_SUPPORTED: &str = "throw.notSupported";

/// An assignment to something else than a property or an offset: `$$name = ...`.
pub const TYPE_MISMATCH: &str = "SPHP0005";
/// Syntax newer than the project's PHP version, where PHPStan has no rule for it.
pub const PHP_SYNTAX_VERSION: &str = "SPHP0060";
/// Something of PHP's library that isn't on the project's PHP version.
pub const PHP_LIBRARY_VERSION: &str = "SPHP0061";
/// A project's script failed while running: it took too long, or its code went wrong.
pub const SCRIPT_FAILED: &str = "SPHP0070";
/// A project's script can't run at all: it can't be read, or it doesn't compile.
pub const SCRIPT_NOT_USABLE: &str = "SPHP0071";
