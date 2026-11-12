<?php
declare(strict_types=1);
namespace App;
// @level 9

// A function that declares no return type gives its callers what its body returns.

function status(bool $paid) {
    if ($paid) {
        return 'paid';
    }
    return 'pending';
}

function takesInt(int $x): void {}

takesInt(status(true)); // error: argument.type
if (status(false) === 'refunded') {} // error: identical.alwaysFalse
if (status(false) === 'paid') {}

// falling off the end gives `null`
function maybe(bool $b) {
    if ($b) {
        return 1;
    }
}
takesInt(maybe(true)); // error: argument.type

// a parameter with no type leaves it unknown, as before
function same($x) { return $x; }
takesInt(same('a'));

// calling itself: unknown
function fact(int $n) { return $n <= 1 ? 1 : $n * fact($n - 1); }
takesInt(fact(3));

// many literals are their type
function many(int $n) {
    return match ($n) {
        1 => 'a', 2 => 'b', 3 => 'c', 4 => 'd', 5 => 'e',
        6 => 'f', 7 => 'g', 8 => 'h', default => 'i',
    };
}
if (many(1) === 'z') {}

function gen() { yield 1; }
foreach (gen() as $v) {}

class Base {
    public function name() { return 'base'; }
    public function code() { return 1; }
    private function secret() { return 'x'; }
    public function fluent() { return $this; }
    public function useSecret() { return $this->secret(); }
}

class Child extends Base {
    public function code() { return 'one'; }
}

function methods(Base $b, Child $c): void {
    takesInt($b->name()); // error: argument.type
    // a subclass declares it again, so which body runs isn't known
    takesInt($b->code());
    takesInt($c->code()); // error: argument.type
    takesInt($b->useSecret()); // error: argument.type
    $c->fluent()->code();
    takesInt($c->fluent()->code()); // error: argument.type
}

// an empty body stands in for something, and says nothing
class Placeholder {
    public function count() {}
}
function placeholder(Placeholder $p): void {
    takesInt($p->count());
}
