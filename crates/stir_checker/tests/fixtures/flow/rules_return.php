<?php

// @level 5

// `return.missing`, and what counts as getting to the end of a function.

function a(): int {} // error: return.missing

function b(int $x): int
{
    if ($x) { // error: return.missing
        return 1;
    }
}

function c(int $x): int
{
    if ($x) {
        return 1;
    } else {
        throw new Exception();
    }
}

function d(int $x): int
{
    while (true) {
        if ($x) {
            return 1;
        }
    }
}

function e(int $x): int
{
    switch ($x) {
        case 1:
            return 1;
        default:
            return 2;
    }
}

function f(): int
{
    try {
        return 1;
    } finally {
        echo 'done';
    }
}

/** @return never */
function stops() { exit(1); }

function g(): int
{
    stops();
}

function h(): void {}

function i(): ?int {} // error: return.missing

function j(): mixed {} // error: return.missing

/** @return int */
function k() {} // error: return.missing

function l() {}

function m(): never {} // error: return.never

function n(): Generator
{
    yield 1;
}

/** @return Generator<int, string, mixed, int> */
function o(): Generator
{
    yield 'x'; // error: return.missing
}

abstract class P
{
    abstract public function q(): int;

    public function r(): int {} // error: return.missing

    public function s(): int
    {
        return 1;
    }
}

interface Q
{
    public function t(): int;
}

$u = function (): int {}; // error: return.missing
$v = function () {};
$w = fn(): int => 1;
