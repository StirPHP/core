//! Well-known PHPStan type keywords. The parser keeps them as plain identifiers;
//! this enum is a convenience for consumers.

macro_rules! keywords {
    ($($variant:ident => [$($name:literal),+]),* $(,)?) => {
        /// A well-known PHPStan/PHP type keyword.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Keyword {
            $($variant),*
        }

        impl Keyword {
            /// Looks a name up case-insensitively. Fully-qualified names (`\int`) are never keywords.
            pub fn from_name(name: &str) -> Option<Keyword> {
                let lower = name.to_ascii_lowercase();
                match lower.as_str() {
                    $($($name)|+ => Some(Keyword::$variant),)*
                    _ => None,
                }
            }

            /// The canonical spelling.
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Keyword::$variant => keywords!(@first $($name),+),)*
                }
            }
        }
    };
    (@first $first:literal $(, $rest:literal)*) => { $first };
}

keywords! {
    Int => ["int", "integer"],
    String => ["string"],
    Bool => ["bool", "boolean"],
    Float => ["float", "double"],
    Mixed => ["mixed"],
    NonEmptyMixed => ["non-empty-mixed"],
    Void => ["void"],
    Never => ["never", "never-return", "never-returns", "no-return", "noreturn"],
    Null => ["null"],
    True => ["true"],
    False => ["false"],
    Array => ["array"],
    AssociativeArray => ["associative-array"],
    NonEmptyArray => ["non-empty-array"],
    List => ["list"],
    NonEmptyList => ["non-empty-list"],
    Iterable => ["iterable"],
    Callable => ["callable"],
    PureCallable => ["pure-callable"],
    PureClosure => ["pure-closure"],
    CallableArray => ["callable-array"],
    CallableObject => ["callable-object"],
    Object => ["object"],
    Resource => ["resource"],
    ClosedResource => ["closed-resource"],
    OpenResource => ["open-resource"],
    Scalar => ["scalar"],
    NonEmptyScalar => ["non-empty-scalar"],
    EmptyScalar => ["empty-scalar"],
    Empty => ["empty"],
    Numeric => ["numeric"],
    ArrayKey => ["array-key"],
    PositiveInt => ["positive-int"],
    NegativeInt => ["negative-int"],
    NonNegativeInt => ["non-negative-int"],
    NonPositiveInt => ["non-positive-int"],
    NonZeroInt => ["non-zero-int"],
    IntMask => ["int-mask"],
    IntMaskOf => ["int-mask-of"],
    NonEmptyString => ["non-empty-string"],
    NonFalsyString => ["non-falsy-string", "truthy-string"],
    NumericString => ["numeric-string"],
    LiteralString => ["literal-string"],
    NonEmptyLiteralString => ["non-empty-literal-string"],
    LowercaseString => ["lowercase-string"],
    NonEmptyLowercaseString => ["non-empty-lowercase-string"],
    UppercaseString => ["uppercase-string"],
    NonEmptyUppercaseString => ["non-empty-uppercase-string"],
    CallableString => ["callable-string"],
    ClassString => ["class-string"],
    InterfaceString => ["interface-string"],
    TraitString => ["trait-string"],
    EnumString => ["enum-string"],
    KeyOf => ["key-of"],
    ValueOf => ["value-of"],
    New => ["new"],
    TemplateType => ["template-type"],
    Static => ["static"],
    SelfType => ["self"],
    Parent => ["parent"],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup() {
        assert_eq!(Keyword::from_name("INT"), Some(Keyword::Int));
        assert_eq!(Keyword::from_name("integer"), Some(Keyword::Int));
        assert_eq!(
            Keyword::from_name("non-empty-list"),
            Some(Keyword::NonEmptyList)
        );
        assert_eq!(Keyword::from_name("\\int"), None);
        assert_eq!(Keyword::from_name("Foo"), None);
        assert_eq!(Keyword::NonFalsyString.as_str(), "non-falsy-string");
    }
}
