<?php declare(strict_types=1);
// @level 7

class Thing { public function run(): int { return 1; } }
class Other {}

function want_string(string $s): void {}
function want_thing(Thing $t): void {}
function want_list(array $a): void {}
/** @param non-empty-string $s */
function want_non_empty(string $s): void {}

function maybes(object $o, iterable $i, string $s, int|string $k): void {
    want_thing($o); // error: SPHP0005
    want_list($i); // error: SPHP0005
    want_non_empty($s); // error: SPHP0005
    want_string($k); // error: SPHP0005
}

function partial(Thing|Other $t): int {
    return $t->run(); // error: SPHP0009
}

function nullable_still_fine(?string $s, ?Thing $t): void {
    want_string($s);
    $t->run();
}
