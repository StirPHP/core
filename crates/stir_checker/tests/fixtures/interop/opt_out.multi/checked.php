<?php
namespace Legacy;

undefined_function(); // error: function.notFound

function f(int $n): string
{
    return $n; // error: return.type
}
