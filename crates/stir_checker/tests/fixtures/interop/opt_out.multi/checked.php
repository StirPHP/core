<?php
namespace Legacy;

undefined_function(); // error: SPHP0007

function f(int $n): string
{
    return $n; // error: SPHP0005
}
