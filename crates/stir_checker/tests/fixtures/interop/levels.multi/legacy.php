<?php
// @level 2
namespace Legacy;

function typed(int $n): string
{
    return $n; // reported from level 3
}

function caller(): void
{
    typed("x"); // reported from level 5
    undefined_function(); // error: SPHP0007
    typed(); // error: SPHP0014
}

/**
 * @param string $n // error: SPHP0050
 */
function documented(int $n): void
{
}
