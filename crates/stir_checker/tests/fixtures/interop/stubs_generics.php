<?php

// PHP's iterators are generic, as in PHPStan's stubs, so saying what they hold isn't an error.

/** @extends FilterIterator<int, string, ArrayIterator<int, string>> */
class Evens extends FilterIterator
{
    public function accept(): bool
    {
        return strlen($this->current()) > 0;
    }
}

/** @implements RecursiveIterator<int, string> */
abstract class Tree implements RecursiveIterator {}

/** @param RecursiveIteratorIterator<RecursiveArrayIterator<int, string>> $it */
function walk(RecursiveIteratorIterator $it): void
{
    foreach ($it as $k => $v) {
        echo $k;
    }
}

/**
 * @param ArrayObject<string, int> $o
 * @param SplObjectStorage<DateTime, string> $s
 * @param WeakMap<DateTime, int> $w
 * @param Generator<int, string, bool, float> $g
 * @return string
 */
function holding(ArrayObject $o, SplObjectStorage $s, WeakMap $w, Generator $g): string
{
    foreach ($o as $key => $value) {
        echo strlen($key) + $value;
    }
    strlen($g->current());
    return (string) $g->getReturn();
}

/** @param Iterator<int, string> $i */
function from_iterator(Iterator $i): string
{
    return $i->current();
}

/** @param Iterator<int, string> $i */
function wrong_iterator(Iterator $i): int
{
    return $i->current(); // error: return.type
}
