<?php declare(strict_types=1);
// Generics declared in PHPDoc.

/**
 * @template T
 */
final class Stack
{
    /** @var list<T> */
    private array $items = [];

    /** @param T $item */
    public function push(mixed $item): void
    {
        $this->items[] = $item;
    }

    /** @return T|null */
    public function top(): mixed
    {
        return $this->items[0] ?? null;
    }
}

/**
 * @template T of Countable
 * @param T $v
 * @return T
 */
function bounded(mixed $v): mixed
{
    return $v;
}

/**
 * @template T
 * @param list<T> $xs
 * @return T|null
 */
function head(array $xs): mixed
{
    return $xs[0] ?? null;
}

/**
 * @template K of array-key
 * @template V
 * @param array<K, V> $a
 * @return list<K>
 */
function keys_of(array $a): array
{
    return array_keys($a);
}

function wants_int(int $v): void {}
function wants_string(string $v): void {}

function uses_generics(): void {
    /** @var Stack<int> $s */
    $s = new Stack();
    $s->push(1);
    $s->push("a"); // error: SPHP0005

    $n = head([1, 2]);
    if ($n !== null) {
        wants_int($n);
        wants_string($n); // error: SPHP0005
    }

    bounded(new ArrayObject());
    bounded(1); // error: SPHP0005

    foreach (keys_of(["a" => 1]) as $k) {
        wants_string($k);
    }
}
