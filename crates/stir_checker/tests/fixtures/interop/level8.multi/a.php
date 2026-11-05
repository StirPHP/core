<?php declare(strict_types=1);
// @level 8

class Thing { public function run(): int { return 1; } public string $name = ""; }

function want_string(string $s): void {}

function nullable(?string $s, ?Thing $t, ?array $a, ?\Closure $f): string {
    want_string($s); // error: argument.type
    $t->run(); // error: method.nonObject
    $t->name; // error: property.nonObject
    $t?->run();
    foreach ($a as $x) { // error: foreach.nonIterable
    }
    $f(); // error: callable.nonCallable
    return $s; // error: return.type
}

function mixed_still_fine(mixed $m, $untyped): void {
    want_string($m);
    want_string($untyped);
}
