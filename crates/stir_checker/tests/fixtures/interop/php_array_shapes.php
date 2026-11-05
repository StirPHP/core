<?php declare(strict_types=1);

// An array literal is a shape while its keys are known.

function want_string(string $s): void {}
function want_int(int $i): void {}
/** @param array{name: string, age: int} $p */
function want_person(array $p): void {}
/** @param array{0: string, 1: int} $t */
function want_tuple(array $t): void {}
/** @param list<string> $l */
function want_strings(array $l): void {}
/** @param array<string, int> $m */
function want_map(array $m): void {}
/** @param array{mode: 'r'|'w'} $o */
function want_mode(array $o): void {}

function literals(): void {
    want_person(["name" => "a", "age" => 1]);
    want_person(["name" => "a", "age" => "1"]); // error: SPHP0005
    want_person(["name" => "a"]); // error: SPHP0005
    want_person(["name" => "a", "age" => 1, "extra" => true]);
    want_tuple(["a", 1]);
    want_tuple([1, "a"]); // error: SPHP0005
    want_strings(["a", "b"]);
    want_strings(["a", 1]); // error: SPHP0005
    want_map(["a" => 1]);
    want_map(["a" => "b"]); // error: SPHP0005
    want_mode(["mode" => "r"]);
    want_mode(["mode" => "x"]); // error: SPHP0005
}

function reads(): void {
    $p = ["name" => "a", "age" => 1];
    want_string($p["name"]);
    want_int($p["age"]);
    want_string($p["age"]); // error: SPHP0005
    echo $p["nope"]; // error: SPHP0015
    echo $p["nope"] ?? "x";

    [$a, $b] = ["x", 1];
    want_string($a);
    want_string($b); // error: SPHP0005

    ["k" => $k] = ["k" => 1];
    want_string($k); // error: SPHP0005

    [, $second] = [1, "s"];
    want_string($second);

    foreach (["a" => 1, "b" => 2] as $key => $value) {
        want_string($key);
        want_string($value); // error: SPHP0005
    }
}

function writes(bool $c): void {
    $p = [];
    $p["name"] = "a";
    $p["age"] = 1;
    want_person($p);

    $q = ["name" => "a"];
    want_person($q); // error: SPHP0005
    $q["age"] = 1;
    want_person($q);
    $q["age"] = "x";
    want_person($q); // error: SPHP0005

    $l = [];
    $l[] = "a";
    $l[] = "b";
    want_strings($l);

    $m = [];
    foreach ([1, 2] as $i) {
        $m[] = "x";
    }
    want_strings($m);

    $nested = ["a" => ["b" => 1]];
    $nested["a"]["c"] = "s";
    want_string($nested["a"]["c"]);
    want_string($nested["a"]["b"]); // error: SPHP0005

    $maybe = ["a" => 1];
    if ($c) {
        $maybe = ["a" => "x"];
    }
    want_int($maybe["a"]); // error: SPHP0005

    $grow = ["x"];
    foreach ([1] as $_) {
        $grow[] = "y";
    }
    want_strings($grow);
}

/** @return array{ok: bool} */
function returns_shape(): array {
    return ["ok" => true];
}

/** @return array{ok: bool} */
function returns_wrong_shape(): array {
    return ["ok" => 1]; // error: SPHP0005
}

// conditional return types
/**
 * @template T
 * @param class-string<T>|null $c
 * @return ($c is null ? stdClass : T)
 */
function make_it(?string $c = null): object { throw new \Exception(); }

class Thing1 { public function run(): int { return 1; } }

function conditional_on_template_arg(): void {
    make_it(Thing1::class)->run();
    make_it(Thing1::class)->nope(); // error: SPHP0009
    make_it()->anything;
}

/** @return ($flag is true ? string : int) */
function by_flag(bool $flag = false): int|string { return $flag ? "a" : 1; }

function conditional_on_literal(bool $b): void {
    want_string(by_flag(true));
    want_string(by_flag(false)); // error: SPHP0005
    want_int(by_flag());
    want_string(by_flag()); // error: SPHP0005
    by_flag($b);
}

/**
 * @template T
 * @param T $v
 * @return (T is int ? string : int)
 */
function flips(mixed $v): int|string { throw new \Exception(); }

function conditional_on_template(): void {
    want_string(flips(1));
    want_string(flips("a")); // error: SPHP0005
}

function functions_keep_types(array $plain): void {
    /** @var array<string, int> $m */
    $m = ["a" => 1];
    foreach (array_keys($m) as $k) {
        want_string($k);
        want_int($k); // error: SPHP0005
    }
    foreach (array_values($m) as $v) {
        want_int($v);
    }
    foreach (array_map(fn (int $x): string => (string) $x, $m) as $k => $v) {
        want_string($k);
        want_string($v);
        want_int($v); // error: SPHP0005
    }
    want_strings(explode(",", "a,b"));
}
