<?php
// @php 7.4
// @level 5

// what the code tests for is fine where it did
if (PHP_VERSION_ID >= 80000) {
    echo str_contains('a', 'b');
}
if (\PHP_VERSION_ID < 80000) {
    echo 1;
} else {
    echo str_starts_with('a', 'b');
}
echo version_compare(PHP_VERSION, '8.0.0', '>=') ? str_ends_with('a', 'b') : 'no';
echo PHP_VERSION_ID >= 80100 && array_is_list([]);
if (function_exists('array_is_list')) {
    echo array_is_list([1]);
}
if (class_exists(WeakMap::class)) {
    $m = new WeakMap();
}

function later(): void {
    if (PHP_VERSION_ID < 80000) {
        return;
    }
    echo str_contains('a', 'b');
}

// PHP doesn't load these classes
function caught($e): bool {
    try {
        return $e instanceof Stringable;
    } catch (ValueError $v) {
        return false;
    }
}
$name = Stringable::class;

// the wrong side of the test
if (PHP_VERSION_ID < 80000) {
    echo str_contains('a', 'b'); // error: SPHP0061
}
echo str_contains('a', 'b'); // error: SPHP0061
