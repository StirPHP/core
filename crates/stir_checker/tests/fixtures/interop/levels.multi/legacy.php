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
    undefined_function(); // error: function.notFound
    typed(); // error: arguments.count
}

/**
 * @param string $n // error: parameter.phpDocType
 */
function documented(int $n): void
{
}
