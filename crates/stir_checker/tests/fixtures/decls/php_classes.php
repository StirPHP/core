<?php declare(strict_types=1);
// Class declarations and member checks in plain PHP.

interface Named
{
    public function name(): string;
}

abstract class Base implements Named
{
    protected int $id = 0;
    private string $secret = "";

    public static function make(): static
    {
        // KNOWN FALSE ALARM: `static` is a concrete subclass here.
        return new static(); // error: SPHP0023
    }

    public function id(): int
    {
        return $this->id;
    }
}

final class Person extends Base
{
    public const LIMIT = 3;

    public function __construct(public readonly string $first = "") {}

    public function name(): string
    {
        return $this->first;
    }

    public function peek(): string
    {
        return $this->secret; // error: SPHP0024
    }
}

enum Status: string
{
    case Active = "active";
    case Off = "off";

    public function label(): string
    {
        // KNOWN FALSE ALARM: the value of a string-backed enum is a string.
        return $this->value; // error: SPHP0005
    }
}

function members(Person $p): void {
    echo $p->name();
    echo $p->id;      // error: SPHP0024
    echo $p->nope;    // error: SPHP0009
    echo $p->nope();  // error: SPHP0009
    $p->first = "x";  // error: SPHP0010
    echo Person::LIMIT;
    echo Person::NOPE; // error: SPHP0009
}

function construction(): void {
    $p = new Person("a");
    echo $p->name();
    new Person(1);    // error: SPHP0005
    new Person("a", "b"); // error: SPHP0014
    new Base();       // error: SPHP0023
    new Missing();    // error: SPHP0008
}

function enums(): string {
    $s = Status::Active;
    echo Status::Gone; // error: SPHP0009
    return $s->label();
}

function statics(): int {
    return Person::make()->id();
}

function arguments(): void {
    strlen();       // error: SPHP0014
    strlen(1, 2);   // error: SPHP0005, SPHP0014
    undefined_fn(); // error: SPHP0007
}
