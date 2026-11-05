<?php
/** @return string The text. */
function doc_only() {
    return '';
}

/** @param array<string, mixed> $a */
function wants_mixed($a) {}

class Php_Entry
{
    /** @var string|null */
    public $singular = null;
}

/** @param string|array<string, mixed>|object $a */
function takes_args($a) {}
