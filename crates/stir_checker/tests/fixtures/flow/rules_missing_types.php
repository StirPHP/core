<?php

// @level 6

function a($x, int $y, array $z, ?array $w = null, array ...$rest) // error: missingType.parameter, missingType.iterableValue, missingType.return
{
}

function b(): array { return []; } // error: missingType.iterableValue

/**
 * @param array<int> $a
 * @param mixed $b
 * @param int[] $c
 * @param array $d
 * @return list<int>
 */
function c(array $a, $b, array $c, array $d): array { return []; } // error: missingType.iterableValue

/** @return iterable */
function d(): iterable { return []; } // error: missingType.iterableValue

function e(callable $c, iterable $i, mixed $m): mixed { return 1; } // error: missingType.iterableValue

class A
{
    public $a; // error: missingType.property
    public int $b = 1;
    /** @var array<int> */
    public array $c = [];
    public array $d = []; // error: missingType.iterableValue
    /** @var string */
    public $e = '';
    private ?array $f = null; // error: missingType.iterableValue

    public function __construct(public $promoted = 1, array $x = []) // error: missingType.parameter, missingType.iterableValue
    {
    }

    public function __destruct() {}

    public function __toString() { return ''; }

    public function m($x) { return 1; } // error: missingType.parameter, missingType.return

    public static function s(): array { return []; } // error: missingType.iterableValue
}

class B extends A
{
    /** @var string */
    public $g = '';

    public function n(array $y): void {} // error: missingType.iterableValue
}

interface I
{
    /**
     * @param array<int> $a
     * @return array<string>
     */
    public function run(array $a): array;

    public function other($x); // error: missingType.parameter, missingType.return
}

class C implements I
{
    public function run(array $a): array // the types come from the interface
    {
        return [];
    }

    /**
     * @param int $x
     * @return int
     */
    public function other($x)
    {
        return 1;
    }
}

abstract class D
{
    abstract public function m($x): void; // error: missingType.parameter
}
