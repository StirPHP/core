<?php declare(strict_types=1);
// `static` in a receiver's type arguments is the class it was written in, not the receiver, and
// in an invariant position it may be that class.

/** @template T of object */
final class Query
{
    /** @return T */
    public function one(): object { throw new \LogicException(); }
    /** @return list<T> */
    public function all(): array { return []; }
    /** @return $this */
    public function filtered(): static { return $this; }
}

/**
 * @template T of object
 */
final class Bag {}

class Record
{
    /** @return Query<static> */
    public static function query(): Query { return new Query(); }

    public static function first(): static
    {
        return static::query()->filtered()->one();
    }

    /** @return list<static> */
    public static function every(): array
    {
        return static::query()->all();
    }

    /** @return Bag<Record> */
    public static function bag(): Bag
    {
        /** @var Bag<static> $bag */
        $bag = new Bag();
        return $bag;
    }

    /** @return Bag<Other> */
    public static function wrong(): Bag
    {
        /** @var Bag<static> $bag */
        $bag = new Bag();
        return $bag; // error: return.type
    }
}

class Other {}
