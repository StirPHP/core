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

// a call-site variance annotation
class Animal {}
class Dog extends Animal {}

/** @template T */
class Box1 {}

/** @return Box1<Dog> */
function dog_box(): Box1 { return new Box1(); }

/** @param Box1<Animal> $b */
function exact_box(Box1 $b): void {}

/** @param Box1<covariant Animal> $b */
function any_animal_box(Box1 $b): void {}

/** @param Box1<*> $b */
function any_box(Box1 $b): void {}

function call_site_variance(): void {
    exact_box(dog_box()); // error: SPHP0005
    any_animal_box(dog_box());
    any_box(dog_box());
}

// a template solved by a default value
/** @template T of 'array'|'scalar' */
class Tree1 {
    /** @param T $type */
    public function __construct(string $name, string $type = 'array') {}
}

/** @return Tree1<'array'> */
function tree_default(): Tree1 {
    return new Tree1('t');
}

/** @return Tree1<'array'> */
function tree_other(): Tree1 {
    return new Tree1('t', 'scalar'); // error: SPHP0005
}

// a generic class without arguments leaves nothing unresolved behind
/** @return iterable<string, int> */
function from_a_generator(): iterable {
    return raw_generator(); // error: SPHP0005
}
function raw_generator(): Generator { yield "a" => 1; }

// a dynamic static property isn't a property of that name
class Dyn { public static array $a = []; public function get(string $n): mixed { return self::$$n; } }

// a template in scope is a type like any other
/**
 * @template T of Animal
 * @param T $a
 * @return T
 */
function keeps_template(Animal $a): Animal {
    want_animal($a);
    wants_string($a); // error: SPHP0005
    return new Dog(); // error: SPHP0005
}
function want_animal(Animal $a): void {}
