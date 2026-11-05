<?php declare(strict_types=1);

// A trait's code runs as part of the classes using it, and is checked there.

trait UsesHost {
    public function name(): string {
        return $this->hostName . static::SUFFIX . self::SUFFIX . $this->hostMethod();
    }

    public function missing(): void {
        $this->notOnAnyHost(); // error: SPHP0009
    }

    public function wrongType(): int {
        return $this->hostName; // error: SPHP0005
    }

    public function copy(): self {
        return new self();
    }

    public function fromParent(): int {
        return parent::parentMethod();
    }

    public static int $counter = 0;

    public function count(): int {
        return self::$counter;
    }
}

class HostBase {
    public function parentMethod(): int { return 1; }
}

class Host extends HostBase {
    use UsesHost;

    public const SUFFIX = "x";
    public string $hostName = "";
    public function hostMethod(): string { return ""; }
}

function host_copies(Host $h): Host {
    return $h->copy();
}

// nobody here uses this one, so what `$this` has can't be known
trait Unused {
    public function name(): string {
        return $this->hostName . self::SUFFIX . parent::foo();
    }

    public function own(): int {
        return $this->ownProperty; // error: SPHP0005
    }

    private string $ownProperty = "";
}

/** @template T */
trait Holds {
    /** @var list<T> */
    private array $items = [];

    /** @return list<T> */
    public function all(): array {
        return $this->items;
    }

    /** @param T $x */
    public function add($x): void {
        $this->items[] = $x;
    }
}

class Ints {
    /** @use Holds<int> */
    use Holds;

    public function first(): string {
        return $this->all()[0]; // error: SPHP0005
    }
}

trait Readonly1 {
    public readonly int $once;
    public function __construct() {
        $this->once = 1;
    }
    public function later(): void {
        $this->once = 2; // error: SPHP0010
    }
}
class UsesReadonly { use Readonly1; }

class Restored {
    public readonly int $value;
    public function __construct() { $this->value = 1; }
    public function __unserialize(array $data): void { $this->value = 2; }
    public function reset(): void { $this->value = 3; } // error: SPHP0010
}
