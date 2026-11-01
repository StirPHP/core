<?php
class Foo
{
    public function run(): int
    {
        return 1;
    }
}

function set_in_branches(bool $c): string
{
    if ($c) {
        $v = 'a';
    } else {
        $v = 'b';
    }
    return $v;
}

function set_in_loop(array $items)
{
    foreach ($items as $k => $item) {
        $last = $item;
    }
    return [$k, $item, $last];
}

function set_in_try()
{
    try {
        $r = new Foo();
    } catch (Exception $e) {
        return $e;
    }
    return $r->run();
}

function joined_after_branch(bool $c): int|string
{
    $v = 1;
    if ($c) {
        $v = 'a';
    }
    return $v;
}

function overwritten(): string
{
    $v = 1;
    $v = 'a';
    return $v;
}

function overwritten_wrong(): string
{
    $v = 'a';
    $v = 1;
    return $v; // error: SPHP0005
}

function still_undefined()
{
    return $nope; // error: SPHP0006
}
