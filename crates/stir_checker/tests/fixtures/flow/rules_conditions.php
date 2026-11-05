<?php

// @level 5

class A
{
    public ?int $n = null;

    public static function make(): static
    {
        return new static(); // error: new.static
    }
}

final class B
{
    public static function make(): static
    {
        return new static();
    }
}

/** @phpstan-consistent-constructor */
class C
{
    public static function make(): static
    {
        return new static();
    }
}

class D extends C
{
    public static function other(): static
    {
        return new static();
    }
}

abstract class E
{
    abstract public function __construct();

    public static function make(): static
    {
        return new static();
    }
}

class F
{
    final public function __construct() {}

    public static function make(): static
    {
        return new static();
    }
}

interface G
{
    public function __construct();
}

class H implements G
{
    public function __construct() {}

    public static function make(): static
    {
        return new static();
    }
}

function identical(int $i, ?string $s, A $a, mixed $m, $u): void
{
    if ($i === null) { // error: identical.alwaysFalse
        echo 'x';
    }
    if ($i !== 'a') { // error: notIdentical.alwaysTrue
        echo 'x';
    }
    if ($s === null) {
        echo 'maybe';
    }
    if ($a === null) { // error: identical.alwaysFalse
        echo 'x';
    }
    if ($m === null || $u === null) {
        echo 'unknown';
    }
    if (null === null) { // error: identical.alwaysTrue
        echo 'x';
    }
    if (1 === 2) { // error: identical.alwaysFalse
        echo 'x';
    }
    if ($s !== null) {
        if ($s === null) { // error: identical.alwaysFalse
            echo 'x';
        }
    }
    $y = null;
    foreach ([1, 2] as $z) {
        if ($y === null) {
            $y = $z;
        }
    }
    if ($i === 1) {
        echo 'a';
    } elseif ($i !== null) { // the last one of a chain can be there to say so
        echo 'b';
    }
    // PHP's library can be false in odd cases that aren't declared
    if (realpath('x') === 5) {
        echo 'x';
    }
}

function conditions(A $a, ?A $b, array $list): void
{
    if ($a) { // error: if.alwaysTrue
        echo 'x';
    }
    if ($b) {
        echo 'maybe';
    }
    if (null) { // error: if.alwaysFalse
        echo 'x';
    }
    if ($list) {
        echo 'maybe';
    }
    $built = [];
    if ($b) {
        $built[] = 'x';
    }
    if ($built) {
        echo 'maybe, however the list came to be';
    }
    $n = $a ? 1 : 2; // error: ternary.alwaysTrue
    $m = $b ? 1 : 2;
    $k = $a ?: 2; // error: ternary.alwaysTrue
    if (!$a) { // error: booleanNot.alwaysFalse
        echo 'x';
    }
    if ($a && $b) { // error: booleanAnd.leftAlwaysTrue
        echo 'x';
    }
    if ($b || $a) { // error: booleanOr.rightAlwaysTrue
        echo 'x';
    }
    if ($b) {
        echo 'a';
    } elseif ($a) {
        echo 'the last of a chain';
    }
    if ($b) {
        echo 'a';
    } elseif ($a) { // error: elseif.alwaysTrue
        echo 'b';
    } else {
        echo 'c';
    }
}

trait Tr
{
    public function t(int $x): void
    {
        // what's known here depends on the class using it
        if ($x === null) {
            echo 'x';
        }
    }
}

class UsesTr
{
    use Tr;
}
