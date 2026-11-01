<?php
// @sphp-nocheck: generated code

namespace Stir;

/**
 * @template K of int|string
 * @template V
 * @param array{first: K, second: V} ...$pairs
 * @return array<K, V>
 */
function mutableMapOf(array ...$pairs): array
{
    return \array_column($pairs, 'second', 'first');
}

/**
 * @template T
 * @param T ...$elements
 * @return list<T>
 */
function mutableListOf(...$elements): array
{
    return \array_values($elements);
}
