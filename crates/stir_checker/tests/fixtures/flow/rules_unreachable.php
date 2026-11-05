<?php

// @level 5

function a(): int
{
    return 1;
    echo 'x'; // error: deadCode.unreachable
    echo 'only the first is reported';
}

function b(int $x): void
{
    foreach ([1] as $y) {
        continue;
        echo $y; // error: deadCode.unreachable
    }
    while ($x) {
        break;
        $x--; // error: deadCode.unreachable
    }
    throw new Exception();
    function declared_anyway(): void {}
    class AlsoDeclared {}
}

function c(int $x): void
{
    goto end;
    echo 'skipped'; // error: deadCode.unreachable
    end:
    echo 'reached by the label';
    if ($x) {
        exit;
    }
    echo 'reached by the other path';
}

function d(int $x): int
{
    if ($x) {
        return 1;
    } else {
        return 2;
    }
    return 3; // error: deadCode.unreachable
}

function e(int $x): void
{
    switch ($x) {
        case 1:
            return;
            echo 'x'; // error: deadCode.unreachable
        case 2:
            echo 'a case starts over';
            break;
    }
    echo 'after';
}

/** @return never */
function stops() { exit(1); }

function f(): void
{
    stops();
    echo 'x'; // error: deadCode.unreachable
}

function g(): void
{
    try {
        throw new Exception();
    } catch (Exception $e) {
        echo 'caught';
    }
    echo 'after';
}
