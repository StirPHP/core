<?php

// The functions of the phpstan/phpstan package, which ships them inside its PHAR.

namespace PHPStan {
    function debugScope() {}

    /**
     * @param mixed $value
     * @param mixed ...$values
     * @return mixed
     */
    function dumpType($value, ...$values) {}

    /**
     * @param mixed $value
     * @param mixed ...$values
     * @return mixed
     */
    function dumpNativeType($value, ...$values) {}

    /**
     * @param mixed $value
     * @param mixed ...$values
     * @return mixed
     */
    function dumpPhpDocType($value, ...$values) {}
}

namespace PHPStan\Testing {
    /**
     * @param mixed $value
     * @return mixed
     */
    function assertType(string $type, $value) {}

    /**
     * @param mixed $value
     * @return mixed
     */
    function assertNativeType(string $type, $value) {}

    /**
     * @param mixed $value
     * @return mixed
     */
    function assertSuperType(string $superType, $value) {}

    /**
     * @param mixed $certainty
     * @param mixed $variable
     * @return mixed
     */
    function assertVariableCertainty($certainty, $variable) {}
}
