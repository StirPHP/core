<?php declare(strict_types=1);

// What a variable holds after paths come together.

class Thing { public function run(): int { return 1; } public ?Thing $next = null; public function next(): ?Thing { return $this->next; } }

function want_thing(Thing $t): void {}
function want_string(string $s): void {}

function assigned_on_the_missing_path(?Thing $t): Thing {
    if (!$t) {
        $t = new Thing();
    }
    return $t;
}

function assigned_in_both_branches(bool $b): string {
    if ($b) {
        $x = "a";
    } else {
        $x = "b";
    }
    return $x;
}

function assigned_in_one_branch(bool $b): string {
    if ($b) {
        $x = "a";
    }
    return $x; // error: variable.undefined
}

function not_seen_by_the_other_branch(bool $b): void {
    if ($b) {
        $x = "a";
    } else {
        want_string($x); // error: variable.undefined
    }
}

function wrong_on_one_path(bool $b): int {
    $x = 1;
    if ($b) {
        $x = new Thing();
    }
    return $x; // error: return.type
}

function narrowing_survives_a_loop(?Thing $t, array $rows): int {
    if ($t === null) {
        return 0;
    }
    foreach ($rows as $row) {
        echo $row;
    }
    return $t->run();
}

function loop_assigns_what_later_rounds_read(array $rows): void {
    $last = null;
    foreach ($rows as $row) {
        if ($last !== null) {
            $last->run();
        }
        $last = new Thing();
    }
}

function found_by_break(array $rows): string {
    foreach ($rows as $row) {
        $found = "x";
        break;
    }
    return $found ?? "none";
}

function while_condition_narrows(?Thing $t): void {
    while ($t !== null) {
        $t->run();
        $t = $t->next();
    }
    want_thing($t); // error: argument.type
}

function assignment_in_condition(Thing $t): void {
    if (($n = $t->next()) !== null) {
        $n->run();
    }
    if ($m = $t->next()) {
        $m->run();
    }
    $n->run(); // error: method.nonObject
}

function early_exit_narrows(?Thing $t): int {
    if ($t === null) {
        throw new \Exception();
    }
    return $t->run();
}

function either_exit(?Thing $a, ?Thing $b): int {
    if ($a === null || $b === null) {
        return 0;
    }
    return $a->run() + $b->run();
}

function either_true(int|string|array $x): void {
    if (is_int($x) || is_string($x)) {
        want_string($x); // error: argument.type
        return;
    }
    foreach ($x as $v) {
        echo $v;
    }
}

function both_false(int|string|null $x): void {
    if (!($x === null || is_int($x))) {
        want_string($x);
    }
}

function switch_paths(int $k): string {
    switch ($k) {
        case 1:
            $s = "one";
            break;
        case 2:
            $s = "two";
            break;
        default:
            throw new \Exception();
    }
    return $s;
}

function switch_no_default(int $k): string {
    switch ($k) {
        case 1:
            $s = "one";
            break;
    }
    return $s; // error: variable.undefined
}

function try_assigns(): string {
    try {
        $s = "a";
    } catch (\Exception $e) {
        return "b";
    }
    return $s;
}

function caught_may_not_have_it(): void {
    try {
        $s = "a";
        want_string($s);
    } catch (\Exception $e) {
        want_string($s); // error: variable.undefined
    }
}

function set_means_defined(array $rows): void {
    foreach ($rows as $r) {
        $x = 1;
    }
    if (isset($x)) {
        echo $x;
    }
    if (($y ?? false) === false) {
        return;
    }
    echo $y;
}

// narrowed method results and offsets
function method_narrowed(Thing $t): int {
    if ($t->next() === null) {
        return 0;
    }
    return $t->next()->run();
}

function method_not_narrowed(Thing $t): int {
    return $t->next()->run(); // error: method.nonObject
}

function offset_narrowed(array $a): string {
    if (!is_string($a['k'])) {
        return "";
    }
    return $a['k'];
}

class Forgets {
    public ?Thing $t = null;
    public function reset(): void { $this->t = null; }
    public function fill(): Thing { return new Thing(); }
    public function run(): int {
        if ($this->t === null) {
            return 0;
        }
        $this->fill();
        $this->t->run();
        $this->reset();
        return $this->t->run(); // error: method.nonObject
    }
}

function closure_captures_itself(): void {
    $f = function (int $n) use (&$f) {
        return $n > 0 ? $f($n - 1) : 0;
    };
    $f(3);
}

function class_may_exist(): int {
    if (!class_exists(\Not\Installed::class)) {
        return 0;
    }
    return \Not\Installed::count();
}

function class_may_not_exist(): int {
    return \Not\Installed::count(); // error: class.notFound
}

function name_only(): string {
    return \Not\Installed::class;
}

abstract class Makes {
    protected static string $builder = "x";
    public static function make(): static { return new static(); } // error: new.static
    public static function wrong(): self { return new self(); } // error: new.abstract
    public function other(): object { return new static::$builder(); }
    public function test(mixed $x): bool { return $x instanceof static::$builder || $x instanceof self::$builder; }
}

// inline `@var`
function inline_var(mixed $m, array $rows): void {
    /** @var Thing $t */
    $t = $m;
    $t->run();
    $t->nope(); // error: method.notFound

    /** @var Thing $row */
    foreach ($rows as $row) {
        $row->nope(); // error: method.notFound
    }

    /** @var Thing $existing */
    $existing->run();
}

// a member found with `method_exists` or `property_exists`
function guarded_member(Thing $t): void {
    if (method_exists($t, 'extra')) {
        $t->extra();
    }
    $t->extra(); // error: method.notFound
    if (property_exists($t, 'more')) {
        echo $t->more;
    }
}

/**
 * @template T
 */
class Builder1 {
    /** @return self<T|int> */
    public function withInt(): self { return $this; }
    /** @return T */
    public function get() { throw new \Exception(); }
}

/** @param Builder1<string> $b */
function self_generic(Builder1 $b): void {
    want_string($b->withInt()->get()); // error: argument.type
    want_string($b->get());
}

// writing to an array offset that was found not to be set
class Caches {
    /** @var array<string, list<int>> */
    private static array $cache = [];
    /** @return list<int> */
    public function get(string $k): array {
        if (isset(self::$cache[$k])) {
            return self::$cache[$k];
        }
        return self::$cache[$k] = [1];
    }
    /** @return list<int> */
    public function wrong(string $k): array {
        if (!isset(self::$cache[$k])) {
            self::$cache[$k] = "x"; // error: assign.propertyType
        }
        return self::$cache[$k];
    }
}

// `new self` on a trait used by an anonymous class is that class
trait MakesItself { public static function make(): static { return new static(); } } // error: new.static
function anonymous_uses_trait(): object {
    return new class { use MakesItself; };
}

function string_or_int_offset(int|string $k): string {
    return $k[0]; // error: offsetAccess.nonOffsetAccessible
}

// taking a reference defines a variable
function reference_defines(array $a): void {
    $a[0] = &$ref;
    echo $ref;
}

// a non-empty array runs a loop at least once
function non_empty_loop(): string {
    foreach ([1, 2] as $v) {
        $s = "x";
    }
    return $s;
}

// string bit operations give strings
function bytes(string $p): string {
    $p[4] = $p[4] | "\x80";
    return $p ^ "a";
}
