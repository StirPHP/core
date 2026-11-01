<?php
// @level 5
class Thing
{
    public string $name = '';
    public function run(): int
    {
        return 1;
    }
}
class Other {}

function want(string $s): string
{
    return $s;
}

/** @param mixed $m */
function mixed_argument($m): string
{
    return want($m);
}

/** @param string|array $v */
function partial_argument($v): string
{
    return want($v);
}

function wrong_argument(array $n): string
{
    return want($n); // error: SPHP0005
}

function partial_property(Thing|bool $t): string
{
    return $t->name;
}

function no_property(Other|bool $t)
{
    return $t->name; // error: SPHP0009
}

function partial_method(Thing|Other $t): int
{
    return $t->run();
}

function no_method(Other|bool $t)
{
    return $t->run(); // error: SPHP0009
}

/** @param array<string, int>|Thing|false $v */
function partial_index($v)
{
    return $v['a'];
}

/** @param string[]|bool $v */
function partial_iterate($v): void
{
    foreach ($v as $s) {
        want($s);
    }
}
