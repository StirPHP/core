<?php declare(strict_types=1);

/**
 * @param array<string, string> $m
 * @param array<int> $l
 */
function php_keys_and_values(array $m, array $l): void {
    foreach ($m as $k => $v) {
        echo strlen($k) + strlen($v);
    }
    foreach ($l as $i) {
        echo $i + 1;
    }
    echo strlen($l); // error: SPHP0005
}

function php_after_literal_check(): string {
    $d = doc_only();
    if ($d === "") {
        return "";
    }
    return "{$d}!";
}

/** @param array<string, bool> $m */
function php_offset_may_be_missing(array $m): bool {
    return $m["k"] ?? false;
}

function php_known_non_null(string $s): string {
    return $s ?? ""; // warning: SPHP0021
}

function php_both_sides_narrow(?string $v): int {
    if ($v !== null && strlen($v) > 0) {
        return strlen($v);
    }
    return 0;
}

/** @param array<string> $l */
function php_replaced_array(array $l): string {
    return str_replace("a", "b", $l); // error: SPHP0005
}

function php_replaced(string $s): string {
    $r = str_replace("a", "b", $s);
    $p = preg_replace("/a/", "b", $s) ?: "";
    return $r . $p;
}

function php_property_takes_php_values(string $key, int $n): void {
    $entry = new Php_Entry();
    $entry->singular = $key;
    $entry->singular = $n; // error: SPHP0005
}

function php_false_stays_in_unions(string $p): string {
    $found = realpath($p);
    if ($found === false) {
        return "";
    }
    return $found;
}

function php_bare_array_in_union(array|string|null $o): void {
    takes_args($o ?? []); // error: SPHP0005
}
