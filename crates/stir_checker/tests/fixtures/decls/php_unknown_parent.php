<?php declare(strict_types=1);
// A class with an unknown ancestor reports that class once, not every inherited member.

final class Child extends MissingParent // error: SPHP0002
{
    public function run(): void
    {
        $this->inherited();
        echo $this->inheritedProperty;
        echo self::INHERITED;
        echo self::$inheritedStatic;
        self::inheritedStatic();
    }
}

final class Implementer implements MissingInterface // error: SPHP0002
{
    public function run(): void
    {
        $this->fromDefaultMethod();
    }
}

final class Known
{
    public function run(): void
    {
        $this->nope(); // error: SPHP0009
    }
}

function outside(Child $c): void
{
    $c->inherited();
}
