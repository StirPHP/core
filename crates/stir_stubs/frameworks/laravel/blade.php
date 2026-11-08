<?php

// What a template's `$loop` is. At runtime it's a `stdClass` made from an array, by
// `Illuminate\View\Concerns\ManagesLoops`, so Laravel has no class that says what's in it.

namespace Illuminate\View;

/**
 * The loop around this point of a template, in `@foreach` and `@forelse`.
 *
 * `count`, `remaining` and `last` are null when looping over what can't be counted, such as a
 * `LazyCollection` or a generator. They're typed for what can, as most things can: the alternative
 * has every `$loop->last` be something to check for null.
 *
 * @template TParent
 */
final class BladeLoop
{
    /** The index of this round, from 0. */
    public int $index;

    /** The number of this round, from 1. */
    public int $iteration;

    /** The rounds left after this one. */
    public int $remaining;

    /** The number of items being looped over. */
    public int $count;

    /** Is this the first round? */
    public bool $first;

    /** Is this the last round? */
    public bool $last;

    /** Is this an even round, by its number? */
    public bool $even;

    /** Is this an odd round, by its number? */
    public bool $odd;

    /** How deep this loop is, from 1. */
    public int $depth;

    /**
     * The loop around this one.
     *
     * @var TParent
     */
    public $parent;
}
