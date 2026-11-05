<?php declare(strict_types=1);
// A `@template` bound can name its own class, directly or through another class.

/**
 * @template T of Node|null
 */
class Node
{
    /** @return T */
    public function parent(): mixed
    {
        return null;
    }
}

/**
 * @template T of Right|null
 */
class Left {}

/**
 * @template T of Left|null
 */
class Right {}

/** @param Left<Right<null>> $l */
function uses(Left $l, Node $n): void
{
    echo $n->nope(); // error: SPHP0009
}
