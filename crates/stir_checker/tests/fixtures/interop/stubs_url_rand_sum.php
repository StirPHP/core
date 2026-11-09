<?php
declare(strict_types=1);

function wants_int(int $n): void {}
function wants_string(string $s): void {}

/** @param list<int> $ints */
function builtins(string $url, array $ints): void
{
    $parts = parse_url($url);
    if ($parts !== false) {
        wants_string($parts['host'] ?? '');
        wants_int($parts['port'] ?? 1);
        $merged = array_replace(['path' => '/'], $parts);
    }
    wants_string(parse_url($url, PHP_URL_PATH) ?? ''); // error: argument.type
    wants_int(parse_url($url, PHP_URL_PATH) ?? 1); // error: argument.type
    $port = parse_url($url, PHP_URL_PORT);
    if (is_int($port)) {
        wants_int($port);
    }

    $keys = ['a' => 1, 'b' => 2];
    wants_string(array_rand($keys));
    foreach (array_rand($keys, 2) as $key) {
        wants_string($key);
    }

    wants_int(array_sum($ints));
}
