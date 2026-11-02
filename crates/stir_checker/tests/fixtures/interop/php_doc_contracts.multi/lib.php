<?php
class Failure
{
    public function message(): string
    {
        return '';
    }
}

/**
 * @phpstan-assert-if-true Failure $v
 * @return bool
 */
function is_failure($v)
{
    return $v instanceof Failure;
}

/**
 * @phpstan-assert-if-false Failure $v
 * @return bool
 */
function is_success($v)
{
    return !($v instanceof Failure);
}

/**
 * @template T
 * @param T $v
 * @phpstan-return (T is int ? T : int)
 */
function positive($v): int
{
    return 1;
}

/** @param array{apply?: callable, name: string} $config */
function register_thing($config): void {}

function takes_callable(callable $cb): void {}
