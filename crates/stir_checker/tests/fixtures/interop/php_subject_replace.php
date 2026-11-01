<?php
/** @param array<string> $l */
function replaced_php(string $s, array $l): string
{
    return strtolower(str_replace('a', 'b', $s));
}
