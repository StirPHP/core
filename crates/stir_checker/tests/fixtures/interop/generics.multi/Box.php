<?php
namespace Lib;

/**
 * @template T
 */
final class Box
{
    /** @var list<T> */
    private array $items = [];

    /**
     * @param T $first
     */
    public function __construct(private mixed $first)
    {
    }

    /**
     * @return T
     */
    public function first(): mixed
    {
        return $this->first;
    }

    /**
     * @return list<T>
     */
    public function all(): array
    {
        return $this->items;
    }

    /**
     * @param T $item
     * @return static
     */
    public function add(mixed $item): static
    {
        $this->items[] = $item;
        return $this;
    }

    /**
     * @template U
     * @param callable(T): U $fn
     * @return Box<U>
     */
    public function map(callable $fn): Box
    {
        return new Box($fn($this->first));
    }
}

/**
 * @template T
 * @param T $value
 * @return Box<T>
 */
function box(mixed $value): Box
{
    return new Box($value);
}
