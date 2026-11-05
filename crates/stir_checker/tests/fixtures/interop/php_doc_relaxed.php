<?php
// @level 5
interface Message
{
    public function with(): Message;
}

trait MessageTrait
{
    public function with(): Message
    {
        return $this;
    }
}

class Base
{
    /** @return true|Base */
    public function clear()
    {
        return true;
    }

    /** @param Base $o */
    public function take($o): void {}
}

class Child extends Base
{
    /** @return bool|Base */
    public function clear()
    {
        return false;
    }

    /** @param Child $o */
    public function take($o): void {}

    public function fewer(): void {}
}

class Narrower extends Child
{
    public function fewer(int $required): void {} // error: parameter.notOptional
}

/** @return non-empty-string */
function name(string $s): string
{
    return $s;
}

/** @var non-negative-int $count */
$count = 1;

function after_isset(): void
{
    if (isset($seen['a'])) {
        echo $seen['a'];
    }
    echo $seen['b'];
}

function has_imagick(Imagick $i): int
{
    return $i->getImageWidth();
}
