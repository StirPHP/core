<?php

// Signatures that say more than phpstorm-stubs does, mostly what arrays hold, in the spirit of
// PHPStan's function map and stubs. A declaration here replaces the phpstorm-stubs one, so
// parameter names and native types follow PHP's.

/**
 * @template K of array-key
 * @param array<K, mixed> $array
 * @return list<K>
 */
function array_keys(array $array, mixed $filter_value = null, bool $strict = false): array {}

/**
 * @template V
 * @param array<array-key, V> $array
 * @return list<V>
 */
function array_values(array $array): array {}

/**
 * @template K of array-key
 * @template V
 * @param array<K, V> $array
 * @param (callable(V, K): mixed)|null $callback
 * @return array<K, V>
 */
function array_filter(array $array, ?callable $callback = null, int $mode = 0): array {}

/**
 * @template V
 * @template R
 * @param (callable(V): R) $callback
 * @param list<V> $array
 * @return list<R>
 */
function array_map(?callable $callback, array $array): array {}

/**
 * @template K of array-key
 * @template V
 * @template R
 * @param (callable(V): R) $callback
 * @param array<K, V> $array
 * @return array<K, R>
 */
function array_map(?callable $callback, array $array): array {}

/**
 * @return list<mixed>
 */
function array_map(?callable $callback, array $array, array ...$arrays): array {}

/**
 * @template V
 * @param list<V> ...$arrays
 * @return list<V>
 */
function array_merge(array ...$arrays): array {}

/**
 * @template K of array-key
 * @template V
 * @param array<K, V> ...$arrays
 * @return array<K, V>
 */
function array_merge(array ...$arrays): array {}

/**
 * @template K of array-key
 * @template V
 * @param array<K, V> $array
 * @param array<K, V> ...$replacements
 * @return array<K, V>
 */
function array_replace(array $array, array ...$replacements): array {}

/**
 * @template K of array-key
 * @template V
 * @param array<K, V> $array
 * @return array<K, V>
 */
function array_unique(array $array, int $flags = SORT_STRING): array {}

/**
 * @template V
 * @param list<V> $array
 * @return ($preserve_keys is true ? array<int, V> : list<V>)
 */
function array_reverse(array $array, bool $preserve_keys = false): array {}

/**
 * @template K of array-key
 * @template V
 * @param array<K, V> $array
 * @return array<K, V>
 */
function array_reverse(array $array, bool $preserve_keys = false): array {}

/**
 * @template V
 * @param list<V> $array
 * @return ($preserve_keys is true ? array<int, V> : list<V>)
 */
function array_slice(array $array, int $offset, ?int $length = null, bool $preserve_keys = false): array {}

/**
 * @template K of array-key
 * @template V
 * @param array<K, V> $array
 * @return array<K, V>
 */
function array_slice(array $array, int $offset, ?int $length = null, bool $preserve_keys = false): array {}

/**
 * @template K of array-key
 * @template V
 * @param array<K, V> $array
 * @return list<V>
 */
function array_splice(array &$array, int $offset, ?int $length = null, mixed $replacement = []): array {}

/**
 * @template K of array-key
 * @template V
 * @param array<K, V> $array
 * @return array<K, V>
 */
function array_diff(array $array, array ...$arrays): array {}

/**
 * @template K of array-key
 * @template V
 * @param array<K, V> $array
 * @return array<K, V>
 */
function array_diff_key(array $array, array ...$arrays): array {}

/**
 * @template K of array-key
 * @template V
 * @param array<K, V> $array
 * @return array<K, V>
 */
function array_diff_assoc(array $array, array ...$arrays): array {}

/**
 * @template K of array-key
 * @template V
 * @param array<K, V> $array
 * @return array<K, V>
 */
function array_intersect(array $array, array ...$arrays): array {}

/**
 * @template K of array-key
 * @template V
 * @param array<K, V> $array
 * @return array<K, V>
 */
function array_intersect_key(array $array, array ...$arrays): array {}

/**
 * @template K of array-key
 * @template V
 * @param array<K, V> $array
 * @return array<K, V>
 */
function array_intersect_assoc(array $array, array ...$arrays): array {}

/**
 * @template K of array-key
 * @template V
 * @param array<array-key, K> $keys
 * @param array<array-key, V> $values
 * @return array<K, V>
 */
function array_combine(array $keys, array $values): array {}

/**
 * @template K of array-key
 * @template V of array-key
 * @param array<K, V> $array
 * @return array<V, K>
 */
function array_flip(array $array): array {}

/**
 * @template K of array-key
 * @template V
 * @param array<array-key, K> $keys
 * @param V $value
 * @return array<K, V>
 */
function array_fill_keys(array $keys, mixed $value): array {}

/**
 * @template V
 * @param V $value
 * @return array<int, V>
 */
function array_fill(int $start_index, int $count, mixed $value): array {}

/**
 * @template V
 * @param array<array-key, V> $array
 * @param mixed ...$values
 * @return list<V|mixed>
 */
function array_pad(array $array, int $length, mixed $value): array {}

/**
 * @template K of array-key
 * @template V
 * @param array<K, V> $array
 * @return ($preserve_keys is true ? list<array<K, V>> : list<list<V>>)
 */
function array_chunk(array $array, int $length, bool $preserve_keys = false): array {}

/**
 * @template K of array-key
 * @param array<K, mixed> $haystack
 * @return K|false
 */
function array_search(mixed $needle, array $haystack, bool $strict = false): int|string|false {}

/**
 * @template V
 * @param array<array-key, V> $array
 * @return V|null
 */
function array_pop(array &$array): mixed {}

/**
 * @template V
 * @param array<array-key, V> $array
 * @return V|null
 */
function array_shift(array &$array): mixed {}

/**
 * @template K of array-key
 * @param array<K, mixed> $array
 * @return K|null
 */
function array_key_first(array $array): string|int|null {}

/**
 * @template K of array-key
 * @param array<K, mixed> $array
 * @return K|null
 */
function array_key_last(array $array): string|int|null {}

/**
 * @template V
 * @param array<array-key, V> $array
 * @return V|false
 */
function reset(array|object &$array): mixed {}

/**
 * @template V
 * @param array<array-key, V> $array
 * @return V|false
 */
function end(array|object &$array): mixed {}

/**
 * @template V
 * @param array<array-key, V> $array
 * @return V|false
 */
function current(array|object $array): mixed {}

/**
 * @template V
 * @param array<array-key, V> $array
 * @return V|false
 */
function next(array|object &$array): mixed {}

/**
 * @template V
 * @param array<array-key, V> $array
 * @return V|false
 */
function prev(array|object &$array): mixed {}

/**
 * @template K of array-key
 * @param array<K, mixed> $array
 * @return K|null
 */
function key(array|object $array): string|int|null {}

/**
 * @template K of array-key
 * @template V
 * @param array<K, V> $array
 * @return array<V, int>
 */
function array_count_values(array $array): array {}

/**
 * @template K of array-key
 * @template T
 * @template R
 * @param array<K, T> $array
 * @param callable(R, T): R $callback
 * @param R $initial
 * @return R
 */
function array_reduce(array $array, callable $callback, mixed $initial = null): mixed {}

/**
 * @template K
 * @template V
 * @param Traversable<K, V>|array<K, V> $iterator
 * @return ($preserve_keys is true ? array<K, V> : list<V>)
 */
function iterator_to_array(Traversable|array $iterator, bool $preserve_keys = true): array {}

/** @return non-empty-list<string> */
function explode(string $separator, string $string, int $limit = PHP_INT_MAX): array {}

/** @return non-empty-list<string> */
function str_split(string $string, int $length = 1): array {}

/** @return list<string>|false */
function file(string $filename, int $flags = 0, $context = null): array|false {}

/** @return list<string>|false */
function scandir(string $directory, int $sorting_order = SCANDIR_SORT_ASCENDING, $context = null): array|false {}

/** @return list<string>|false */
function glob(string $pattern, int $flags = 0): array|false {}

/** @return list<string>|list<array{string, int}>|false */
function preg_split(string $pattern, string $subject, int $limit = -1, int $flags = 0): array|false {}

/**
 * @param string|array<string> $search
 * @param string|array<string> $replace
 * @param string|array<string> $subject
 * @param-out int $count
 * @return ($subject is array ? array<string> : string)
 */
function str_replace(array|string $search, array|string $replace, array|string $subject, &$count = null): array|string {}

/**
 * @param string|array<string> $search
 * @param string|array<string> $replace
 * @param string|array<string> $subject
 * @param-out int $count
 * @return ($subject is array ? array<string> : string)
 */
function str_ireplace(array|string $search, array|string $replace, array|string $subject, &$count = null): array|string {}

/**
 * @template V
 * @param array<array-key, V> $array
 * @param-out list<V> $array
 */
function sort(array &$array, int $flags = SORT_REGULAR): bool {}

/**
 * @template V
 * @param array<array-key, V> $array
 * @param-out list<V> $array
 */
function rsort(array &$array, int $flags = SORT_REGULAR): bool {}

/**
 * @template V
 * @param array<array-key, V> $array
 * @param callable(V, V): int $callback
 * @param-out list<V> $array
 */
function usort(array &$array, callable $callback): bool {}

/**
 * @template V
 * @param array<array-key, V> $array
 * @param-out list<V> $array
 */
function shuffle(array &$array): bool {}

/**
 * @template T of object
 * @param T $object
 * @return class-string<T>
 */
function get_class(object $object = UNKNOWN): string {}
