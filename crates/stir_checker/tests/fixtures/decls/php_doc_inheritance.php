<?php declare(strict_types=1);

// An override without a PHPDoc type takes it from what it overrides.

class Node { public string $name = ""; }
class Special extends Node { public int $weight = 0; }

/** @template T of Node */
interface Rule {
    /** @return class-string<T> */
    public function nodeType(): string;

    /**
     * @param T $node
     * @return list<string>
     */
    public function process(Node $node, int $depth): array;
}

/** @implements Rule<Special> */
class SpecialRule implements Rule {
    public function nodeType(): string {
        return Special::class;
    }

    public function process(Node $node, int $depth): array {
        $w = $node->weight;
        $node->nothing; // error: SPHP0009
        return [(string) $w];
    }
}

/** @implements Rule<Special> */
class WrongReturn implements Rule {
    public function nodeType(): string {
        return Special::class;
    }

    public function process(Node $node, int $depth): array {
        return [1]; // error: SPHP0005
    }
}

class Base {
    /** @var list<string> */
    protected array $names = [];

    /** @return list<int> */
    public function ids(): array { return []; }

    /** @return static */
    public function copy() { return $this; }

    /** @param list<int> $ids */
    public function takes(array $ids, ?string $tag = null): void {}
}

class Child extends Base {
    protected array $names = ["a"];

    public function ids(): array { return ["a"]; } // error: SPHP0005

    public function copy() { return $this; }

    // renamed: the position counts
    public function takes(array $other, ?string $tag = null): void {
        foreach ($other as $id) {
            takes_int($id);
        }
    }

    public function own(): int {
        return $this->copy()->ownOther();
    }

    public function ownOther(): int { return 1; }

    public function wrong(): string {
        return $this->names; // error: SPHP0005
    }
}

function takes_int(int $i): void {}

// a narrower native type than the inherited PHPDoc type stands
class Lax {
    /** @return string|int */
    public function get() { return 1; }
}
class Strict extends Lax {
    public function get(): int { return 1; }
}
function strict_gets_int(Strict $s): int { return $s->get(); }

// a class's own trait comes before its parent
trait Greets { public function hello(string $a, string $b): string { return $a . $b; } }
class Greeter { public function hello(): int { return 1; } }
class Both extends Greeter { use Greets; }
function both(Both $b): string { return $b->hello("a", "b"); }
