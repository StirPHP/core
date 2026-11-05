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
    return $x; // error: SPHP0006
}

function not_seen_by_the_other_branch(bool $b): void {
    if ($b) {
        $x = "a";
    } else {
        want_string($x); // error: SPHP0006
    }
}

function wrong_on_one_path(bool $b): int {
    $x = 1;
    if ($b) {
        $x = new Thing();
    }
    return $x; // error: SPHP0005
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
    want_thing($t); // error: SPHP0005
}

function assignment_in_condition(Thing $t): void {
    if (($n = $t->next()) !== null) {
        $n->run();
    }
    if ($m = $t->next()) {
        $m->run();
    }
    $n->run(); // error: SPHP0012
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
        want_string($x); // error: SPHP0005
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
    return $s; // error: SPHP0006
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
        want_string($s); // error: SPHP0006
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
    return $t->next()->run(); // error: SPHP0012
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
        return $this->t->run(); // error: SPHP0012
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
    return \Not\Installed::count(); // error: SPHP0008
}

function name_only(): string {
    return \Not\Installed::class;
}

abstract class Makes {
    protected static string $builder = "x";
    public static function make(): static { return new static(); }
    public static function wrong(): self { return new self(); } // error: SPHP0023
    public function other(): object { return new static::$builder(); }
    public function test(mixed $x): bool { return $x instanceof static::$builder || $x instanceof self::$builder; }
}
