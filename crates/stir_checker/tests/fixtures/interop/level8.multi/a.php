<?php declare(strict_types=1);
// @level 8

class Thing { public function run(): int { return 1; } public string $name = ""; }

function want_string(string $s): void {}

function nullable(?string $s, ?Thing $t, ?array $a, ?\Closure $f): string {
    want_string($s); // error: SPHP0005
    $t->run(); // error: SPHP0012
    $t->name; // error: SPHP0012
    $t?->run();
    foreach ($a as $x) { // error: SPHP0012
    }
    $f(); // error: SPHP0012
    return $s; // error: SPHP0005
}

function mixed_still_fine(mixed $m, $untyped): void {
    want_string($m);
    want_string($untyped);
}
