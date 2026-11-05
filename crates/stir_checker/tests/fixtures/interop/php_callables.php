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
add_hook(1); // error: argument.type

// a closure's optional and variadic parameters
function closure_parameters(): void {
    $optional = function (int $a, ?string $b = null): int { return $a; };
    $optional(1);
    $optional(1, "x");
    $optional(); // error: arguments.count
    $optional(1, "x", 2); // error: arguments.count

    $variadic = function (string ...$rest): int { return count($rest); };
    $variadic();
    $variadic("a", "b");
    $variadic([]); // error: argument.type
}

// a callback may take fewer parameters, but a `void` one can't return a value
/** @param callable(int, int): void $f */
function takes_void_callback(callable $f): void {}
/** @param \Closure(int): void $f */
function passes_on(\Closure $f): void { takes_void_callback($f); }
function void_callbacks(): void {
    takes_void_callback(function (int $a): void {});
    takes_void_callback(fn (int $a): int => 1); // error: argument.type
}
