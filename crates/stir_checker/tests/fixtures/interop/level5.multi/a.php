<?php declare(strict_types=1);
// @level 5

// What PHPStan accepts below level 7 (`checkUnionTypes`) and level 8 (`checkNullables`).

class Thing { public function run(): int { return 1; } public string $name = ""; }
class Other {}

function want_string(string $s): void {}
function want_thing(Thing $t): void {}
function want_list(array $a): void {}
/** @param non-empty-string $s */
function want_non_empty(string $s): void {}
/** @param list<string> $l */
function want_list_of_strings(array $l): void {}
/** @param class-string $c */
function want_class_string(string $c): void {}

function nullable_args(?string $s, ?Thing $t): void {
    want_string($s);
    want_thing($t);
    $t->run();
    $t->name;
}

function nullable_return(?string $s): string {
    return $s;
}

function maybes(object $o, iterable $i, string $s, array $a, int|string $k): void {
    want_thing($o);
    want_list($i);
    want_non_empty($s);
    want_list_of_strings($a);
    want_class_string($s);
    want_string($k);
}

function partial(Thing|Other $t): int {
    return $t->run();
}

function only_null_is_wrong(): void {
    want_string(null); // error: SPHP0005
}

function wrong(Other $o, array $a, int $i): void {
    want_thing($o); // error: SPHP0005
    want_string($a); // error: SPHP0005
    want_string($i); // error: SPHP0005
    want_class_string("NoSuchClass"); // error: SPHP0005
    want_class_string("Thing");
    $o->run(); // error: SPHP0009
}

function superclass_is_no(Node2 $n): void {
    want_special($n); // error: SPHP0005
}
class Node2 {}
class Special2 extends Node2 {}
function want_special(Special2 $s): void {}
