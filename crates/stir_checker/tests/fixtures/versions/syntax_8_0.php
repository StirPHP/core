<?php
// @php 8.0
// @level 0

// fine on 8.0
function promoted_union(int|string $a, mixed $b = null): static|null { return null; }

class Point {
    public function __construct(public int $x) {}
}

$m = match (1) { default => 2 };

// newer than 8.0
enum Suit { case Hearts; } // error: SPHP0060

class Reading {
    public readonly int $x; // error: property.readOnlyNotSupported
}

function no_return(): never { exit; } // error: return.neverTypeNotSupported

readonly class Frozen {} // error: classConstant.nativeTypeNotSupported
