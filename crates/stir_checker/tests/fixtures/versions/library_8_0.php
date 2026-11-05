<?php
// @php 8.0
// @level 5

// on 8.0
echo str_contains('a', 'b');
$map = new WeakMap();

// newer than 8.0
echo array_is_list([]); // error: SPHP0061
$fiber = new Fiber(fn() => 1); // error: SPHP0061
echo json_validate('{}'); // error: SPHP0061
echo E_STRICT;

// removed before 8.0
echo each($a); // error: SPHP0061
echo create_function('', ''); // error: SPHP0061

$d = new DateTimeImmutable();
echo $d->format('Y');

class Named implements Stringable {
    public function __toString(): string { return ''; }
}

// a type or a method of a class that only later versions have
function waits(Fiber $f): void {} // error: SPHP0061
echo (new ReflectionClass(stdClass::class))->isEnum(); // error: SPHP0061
echo ReflectionProperty::IS_READONLY; // error: SPHP0061
