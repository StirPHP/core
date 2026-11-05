<?php declare(strict_types=1);
// An intersection type in a variance-annotated class doesn't overflow the stack.

interface Tagged {}

/**
 * @template-covariant T of object
 */
final class Source
{
    /** @return T&Tagged */
    public function read(): object
    {
        throw new LogicException();
    }

    // KNOWN GAP: `T` is in an input position here, which should be SPHP0027.
    /** @param T&Tagged $v */
    public function write(object $v): void {}
}
