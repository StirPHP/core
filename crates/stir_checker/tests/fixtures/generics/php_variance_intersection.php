<?php declare(strict_types=1);
// Variance is checked through intersection types.

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

    /** @param T&Tagged $v */
    public function write(object $v): void {} // error: generics.variance
}
