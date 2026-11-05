<?php
// A class PHP can't load because something it has to implement isn't there.

interface Shape
{
    public function area(): float;
    public function name(): string;
}

abstract class Base implements Shape
{
    public function name(): string { return 'base'; }
    abstract protected function sides(): int;
}

class Square extends Base // error: SPHP0062
{
    public function area(): float { return 1.0; }
}

class Full extends Base
{
    public function area(): float { return 1.0; }
    protected function sides(): int { return 4; }
}

// abstract, an interface or a trait: not yet
abstract class Later implements Shape {}
interface More extends Shape {}
trait Needs { abstract public function need(): int; }

class FromTrait
{
    use Needs;
    public function need(): int { return 1; }
}

class Missing // error: SPHP0062
{
    use Needs;
}

// a trait's method, under another name too
trait Gives { public function give(): int { return 1; } }
interface Wants { public function want(): int; public function give(): int; }
class Aliased implements Wants
{
    use Gives { give as want; }
}

// what an enum has by being one
enum Suit: string implements JsonSerializable
{
    case Hearts = 'h';
    public function jsonSerialize(): mixed { return $this->value; }
}

// a parent that can't be found may have them all
class Unknown extends NotThere {} // error: class.notFound

// PHP's own: what Exception implements of Throwable
class Failure extends Exception {}

class Counted implements Countable // error: SPHP0062
{
}

$anonymous = new class implements Countable { // error: SPHP0062
};
