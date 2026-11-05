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
        return new static(); // error: new.static
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
        return $this->secret; // error: property.private
    }
}

enum Status: string
{
    case Active = "active";
    case Off = "off";

    public function label(): string
    {
        return $this->value;
    }

    public function number(): int
    {
        return $this->value; // error: return.type
    }
}

function members(Person $p): void {
    echo $p->name();
    echo $p->id;      // error: property.protected
    echo $p->nope;    // error: property.notFound
    echo $p->nope();  // error: method.notFound
    $p->first = "x";  // error: assign.propertyProtectedSet
    echo Person::LIMIT;
    echo Person::NOPE; // error: classConstant.notFound
}

function construction(): void {
    $p = new Person("a");
    echo $p->name();
    new Person(1);    // error: argument.type
    new Person("a", "b"); // error: arguments.count
    new Base();       // error: new.abstract
    new Missing();    // error: class.notFound
}

function enums(): string {
    $s = Status::Active;
    echo Status::Gone; // error: classConstant.notFound
    return $s->label();
}

function statics(): int {
    return Person::make()->id();
}

function arguments(): void {
    strlen();       // error: arguments.count
    strlen(1, 2);   // error: argument.type, arguments.count
    undefined_fn(); // error: function.notFound
}
