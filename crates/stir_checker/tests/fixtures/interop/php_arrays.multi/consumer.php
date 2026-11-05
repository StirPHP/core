<?php declare(strict_types=1);

function php_bare_arrays_are_arrays(array $a, array $b): array {
    $merged = array_merge($a, $b);
    echo count(bare());
    takes_names($merged); // error: SPHP0005
    $parts = unpack("N", "abcd");
    if ($parts === false) {
        return [];
    }
    echo count($parts);
    return $merged;
}

class PhpSub extends PhpEntry {
    public function add(int $x): void {
        $this->items[] = $x;
        $this->names[] = "x";
    }
}

function php_property_offset(PhpEntry $e): void {
    $e->attrs["x"] = 1;
    $e->names[] = "y";
}

/** @return array<string, string|int|null> */
function php_declared_array(): array {
    /** @var array<string, string|int|null> $m */
    $m = ["a" => null];
    $m["a"] = 1;
    $m["b"] = "x";
    return $m;
}

/** @param array<string, int> $a */
function php_array_functions_keep_types(array $a, string $s): string {
    foreach (array_keys($a) as $k) {
        echo strlen($k);
        echo $k + 1;
    }
    foreach (array_values($a) as $v) {
        echo $v + 1;
    }
    foreach (array_filter($a) as $k => $v) {
        echo strlen($k) + $v;
        echo strlen($v); // error: SPHP0005
    }
    return str_replace(array_keys(["a" => "b"]), array_values(["a" => "b"]), $s);
}

function php_explode_is_indexable(string $s): string {
    return trim(explode(",", $s)[0]);
}
