<?php declare(strict_types=1);

// Anonymous classes, `@mixin`, trait aliases and bound closures.

interface Runs { public function run(): int; }
class Base1 { public function __construct(public string $name) {} protected int $secret = 1; }

function want_runs(Runs $r): void {}
function want_base(Base1 $b): void {}

function anonymous(): void {
    want_runs(new class implements Runs {
        public function run(): int { return 1; }
    });
    want_runs(new class { // error: argument.type
        public function run(): int { return "a"; } // error: return.type
    });
    $a = new class("n") extends Base1 {
        public function secret(): int { return $this->secret; }
        public function wrong(): string { return $this->secret; } // error: return.type
    };
    want_base($a);
    $a->secret();
    $a->nope(); // error: method.notFound
    new class(1) extends Base1 {}; // error: argument.type
}

class Real { public function real(): int { return 1; } public int $count = 0; private function hidden(): void {} }

/** @mixin Real */
class Forwards {
    public function __call(string $name, array $args): mixed { return null; }
}

/**
 * @template T of object
 * @mixin T
 */
class Generic1 {}

/** @mixin Real */
class NoMagic {}

function mixins(Forwards $f, NoMagic $n): int {
    $f->anything(); // error: method.notFound
    $n->nope(); // error: method.notFound
    $n->hidden(); // error: method.notFound
    echo $n->count;
    return $n->real();
}

/** @param Generic1<Real> $g */
function generic_mixin(Generic1 $g): string {
    return $g->real(); // error: return.type
}

trait Hello {
    public function hello(): string { return "hi"; }
}

class Aliases {
    use Hello { hello as greet; }
}

function aliased(Aliases $a): string {
    $a->nope(); // error: method.notFound
    $a->hello();
    return $a->greet();
}

class Secret {
    private int $hidden = 1;
    private function peek(): int { return 1; }
    public function open(): int { return 1; }
}

function bound(Secret $s): void {
    $f = Closure::bind(function () {
        echo $this->hidden;
        echo $this->peek();
        echo $this->nope; // error: property.notFound
    }, $s, Secret::class);
    $g = (function () { return $this->hidden; })->call($s);
    $h = Closure::bind(static function (Secret $x) {
        return $x->hidden;
    }, null, Secret::class);
    echo $s->hidden; // error: property.private
}

class Router {
    /** @param-closure-this Secret $f */
    public function group(Closure $f): void {}
}

function closure_this(Router $r): void {
    $r->group(function () {
        $this->nope(); // error: method.notFound
    });
    $r->group(fn () => $this->open());
    // a static closure can't be bound to an object
    $r->group(static function () {
        $this->open(); // error: variable.undefined
    });
}

class Statics {
    public int $v = 0;
    public function run(): void {
        $a = function () { return $this->v; };
        $b = static function () { return $this->v; }; // error: variable.undefined
        $c = fn () => $this->v;
    }
}

// a closure's parameter type takes what's passed, if that says more
/**
 * @template T of Base1
 * @param T $v
 * @param callable(T): void $f
 */
function with_callback(Base1 $v, callable $f): void {}

class Child1 extends Base1 { public int $extra = 0; }

function narrowed_closure_parameter(Child1 $c): void {
    with_callback($c, function (Base1 $b): void {
        echo $b->extra;
        echo $b->nope; // error: property.notFound
    });
}

// by reference, a variable exists from where a closure captures it
function by_ref_capture(): ?string {
    $f = function () use (&$error) { $error = "x"; };
    $f();
    return $error;
}

function generator_closure(): iterable {
    $g = function () { yield 1; };
    return $g();
}

/** @param list<int> $vs */
function variadic_doc(int ...$vs): void {
    want_int_list($vs);
}
/** @param list<int> $l */
function want_int_list(array $l): void {}
function call_variadic(): void {
    variadic_doc(1, 2);
    variadic_doc("a"); // error: argument.type
}

function by_reference_out(array $a): void {
    /** @var array<string, int> $a */
    sort($a);
    want_int_list($a);
}
