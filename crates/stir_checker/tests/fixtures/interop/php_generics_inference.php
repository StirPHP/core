<?php
// @level 5
class Table
{
    public function page(): int
    {
        return 1;
    }
}
class PostsTable extends Table {}

/**
 * @template T of Table
 * @param class-string<T> $class
 * @return T
 */
function make_table($class)
{
    return new $class();
}

/**
 * @template T
 * @param class-string<T> $class
 */
function reflect($class): void {}

echo make_table('PostsTable')->page();
echo make_table('\PostsTable')->page();

function from_string(string $name): int
{
    return make_table($name)->page();
}

function unbounded(string $name): void
{
    reflect($name);
    new ReflectionClass($name);
}

make_table(1); // error: SPHP0005
echo make_table('PostsTable')->nope(); // error: SPHP0009
