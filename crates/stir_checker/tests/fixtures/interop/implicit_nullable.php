<?php
function native(int $n = null, string $s = NULL): ?int
{
    return $n;
}

/**
 * @param string $s
 */
function documented($s = null): void
{
    native(null);
    native(null, null);
}

native("no"); // error: SPHP0005
