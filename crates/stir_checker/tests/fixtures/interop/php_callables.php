<?php
class Hooks
{
    public function handler(): void {}

    public function register(): void
    {
        add_hook([$this, 'handler']);
        add_hook(array($this, 'handler'));
    }
}

function add_hook(callable $cb): void {}

add_hook('strlen');
add_hook(array('Hooks', 'handler'));
array_map('intval', ['1']);
add_hook(1); // error: SPHP0005

// a closure's optional and variadic parameters
function closure_parameters(): void {
    $optional = function (int $a, ?string $b = null): int { return $a; };
    $optional(1);
    $optional(1, "x");
    $optional(); // error: SPHP0014
    $optional(1, "x", 2); // error: SPHP0014

    $variadic = function (string ...$rest): int { return count($rest); };
    $variadic();
    $variadic("a", "b");
    $variadic([]); // error: SPHP0005
}
