<?php
function fills(array &$out, ?int &$count = null): void
{
    $out = [1];
    $count = 1;
}

function uses_out_arguments(string $s): string
{
    preg_match('/x/', $s, $matches);
    return $matches[0];
}

function null_initialized(string $s)
{
    $found = null;
    preg_match('/x/', $s, $found);
    return $found[0];
}

function own_function(): array
{
    fills($list, $n);
    return [$list, $n];
}

function value_argument_stays_undefined(): string
{
    return strtolower($nope); // error: variable.undefined
}
