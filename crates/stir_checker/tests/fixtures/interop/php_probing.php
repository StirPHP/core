<?php
function probes(): bool
{
    if (isset($missing) || isset($missing2['a']['b'])) {
        return true;
    }
    echo $missing3 ?? 'default';
    echo $missing4['k'] ?? 'default';
    return empty($missing5);
}

function callable_values(string $name, array $pair, int $n, ?callable $maybe): void
{
    $name();
    $pair();
    $n(); // error: callable.nonCallable
}

function still_undefined()
{
    return $nope ?? $nope2; // error: variable.undefined
}
