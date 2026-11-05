<?php
class PhpEntry
{
    /** @var array */
    public $items = array();
    /** @var array|null */
    public $attrs = null;
    /** @var string[] */
    public $names = array();
}

function bare(): array
{
    return array();
}

/** @param string[] $names */
function takes_names($names): void {}

/** @return array */
function doc_bare()
{
    return array();
}
