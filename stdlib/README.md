# StirPHP standard library

The methods of StirPHP's builtin types (`String`, `Int`, `Float`, `Bool`, `List`,
`MutableList`, `Map`, `MutableMap`), written in StirPHP as extension functions, plus a
few top-level helpers. At runtime these types are plain PHP strings, ints, floats, bools
and arrays, so every function here is a typed facade over PHP builtins.

All files declare `namespace Stir`. Each one parses with zero syntax errors:

```sh
cargo run -q -p stir_syntax --example dump -- stir stdlib/List.sphp | grep -E '^error'
```

## Layout

| File | Contents |
|---|---|
| `String.sphp` | `String` methods. Lengths and indices count Unicode code points (`mb_*`). |
| `Int.sphp` | `Int` methods, plus the `intOrOverflow` helper. |
| `Float.sphp` | `Float` methods. |
| `Bool.sphp` | `Bool` methods. |
| `List.sphp` | Read-only `List<T>` methods, plus the `IndexedValue<T>` record. |
| `MutableList.sphp` | Mutating `MutableList<T>` methods (by-reference receiver). |
| `Map.sphp` | Read-only `Map<K, V>` methods, plus the `Entry<K, V>` record. |
| `MutableMap.sphp` | Mutating `MutableMap<K, V>` methods (by-reference receiver). |
| `Collections.sphp` | `Pair`/`Triple`, `to`, `listOf`/`mapOf` and the other constructors, `compareValues`, `repeat`, and the preconditions (`requireThat`, `check`, `error`, `TODO`, ...). |
| `Nullable.sphp` | `nullIfFalse`, the scope functions (`let`, `also`, `takeIf`, `takeUnless`, `run`), and `orEmpty`/`isNullOrEmpty`/`isNullOrBlank`. |

The read-only functions on `List`/`Map` also apply to `MutableList`/`MutableMap`, because
the mutable types are subtypes of the read-only ones.

## Runtime conventions

- **Target: PHP 8.4.** The library uses `mb_trim`/`mb_ltrim`/`mb_rtrim`, `mb_ucfirst`/`mb_lcfirst`,
  `array_find`, `array_find_key`, `array_any` and `array_all` (8.4), and `mb_str_pad` (8.3).
- **Lists stay lists.** Every function that returns a `List` gives back keys `0..n-1`.
  Results of `array_filter`, `array_unique` and similar functions are passed through `array_values`.
- **Comparisons are strict.** The code uses `===` and `!==`, and calls `in_array`/`array_search` with `strict = true`.
- **"Not found" is `null`.** Lookups such as `indexOf` return `Int?` instead of `-1`/`false`.
  `nullIfFalse` converts PHP's `false` sentinel to `null`.
- **Failures throw SPL exceptions:**

  | Exception | Thrown for |
  |---|---|
  | `\OutOfBoundsException` | a bad index, an empty list, or a missing key |
  | `\InvalidArgumentException` | malformed input, an empty range, or `requireThat` |
  | `\RuntimeException` | `check`, `error` |
  | `\LogicException` | `TODO` |
  | `\ArithmeticError` | Int overflow |
  | `\LengthException` | `reduce` on an empty list |
  | `\ValueError` | what PHP builtins already throw, such as an empty `split` separator, a negative `repeat`, or `chunked(0)` |

- **Int overflow.** PHP silently turns an overflowing int into a float. `Int.abs`, `Int.pow`,
  `List<Int>.sum` and `sumOf` throw `\ArithmeticError` instead (through `intOrOverflow`).
- **Map keys are PHP array keys.** They can only be ints or strings, so functions that build
  a map from user-computed keys bound them with `K of Int | String`. A numeric-string key such as `"1"` becomes the Int `1`.
- **Ordering.** `compareValues` puts null first, compares strings by code point with `strcmp`
  (PHP's numeric-string comparison never applies), and compares numbers numerically.
  `sorted`, `sortedBy`, `minOrNull`, `maxOrNull` and `MutableList.sort*` are built on it.
  Sorting is stable, since PHP 8's `usort` is stable.
- **Equality in `distinct`/`distinctBy`** compares by `serialize()`. As a result, `1` and `"1"` are
  different, objects compare by value, and closures throw.
- **PHP formatting.** `Float.toString()` returns `strval`'s form: `1.0` becomes `"1"`, and the same
  output comes from `"${x}"`. `Bool.toString()` returns `"true"`/`"false"`.

## Inlinability rules followed

A build option inlines extension functions at their call sites, so every function here keeps
to these rules:

1. **Expression bodies (`= expr`) wherever possible.** Block bodies appear only where PHP offers
   no expression form. Each one has a comment saying why:
   - `List.sortedWith` and `List.shuffled` need a local copy, because `usort`/`shuffle` sort by reference.
   - `List.groupBy` needs an accumulator and a loop.
   - `List.forEach`, `List.forEachIndexed` and `Map.forEach` would otherwise leak `array_walk`'s Bool.
   - `T.also` has to sequence the call before `return this`.
   - `repeat` needs a loop counter.
   - `requireThat` and `check` are a Unit function with a conditional throw.
2. **Only global symbols and parameters.** A body references PHP builtin functions and constants,
   other stdlib functions, its own parameters and `this`. Nothing else: no classes with state,
   no statics, and no private helpers. The shared helpers `nullIfFalse`, `intOrOverflow` and
   `compareValues` are ordinary public top-level functions.
3. **No recursion, direct or mutual.** Stdlib functions call each other only in one direction,
   for example `sorted` calls `sortedWith`, and `minByOrNull` calls `sortedBy`, which calls `sortedWith`.
4. **Parameters may be used more than once.** The inliner binds each argument to a temporary,
   so it is evaluated once. That is why `nullIfFalse(value)` and `compareValues(a, b)` can test
   their parameter and then return it. A *call result* is never reused: wherever one would be
   needed twice, it goes through a parameter of a helper.
5. **Callbacks are wrapped in a lambda.** PHP passes the key as an extra argument to
   `array_any`, `array_all`, `array_find`, `array_find_key`, `array_walk` and `ARRAY_FILTER_USE_BOTH`,
   and an internal PHP function used as a callable throws on surplus arguments. Callbacks
   are passed bare only where PHP passes exactly the expected arguments (`array_map` with one array, `array_reduce`, `usort`).
6. **Stir syntax only.** `?:` is used for null-coalescing and lowers to `??`.
   `takeWhile`/`dropWhile` rely on that, because a found key `0` must not be treated as missing.

## By-reference receivers

PHP arrays are values, so `MutableList`/`MutableMap` functions that mutate their receiver only
work if the receiver is passed **by reference**. Each such function carries a
`// TODO(stir): needs by-reference receiver` comment and is written on that assumption:

- `this` is an alias of the caller's variable. When the function is emitted, it takes `array &$receiver`.
  When it is inlined, it operates on the caller's place directly.
- Assigning to `this` (`this = []`) or to `this[...]` mutates the caller's collection.
  By-reference builtins such as `array_push`, `array_splice`, `array_shift`, `array_pop`, `usort`
  and `shuffle` receive `this` itself.
- At a call site the receiver must be an assignable place (a `var $x`, a mutable property, or an
  array element). The checker should reject temporaries and `val`s as receivers.
- When a Unit function has an expression body, the expression's value is discarded.
  For example, `add(e) = array_push(this, e)` returns PHP's new count, and that count is dropped.
- `MutableMap.put` and `MutableMap.remove` return the previous value without a local variable.
  They use `[old, mutation][0]`, which works because PHP evaluates array-literal elements left to right.

## Naming notes

- `require` is a reserved word in PHP, so the argument precondition is called `requireThat`.
  `check`, `error` and `TODO` keep their usual names.
- Functions are never overloaded on parameter type. The forms are `plus(elements: List<T>)` and
  `plusElement(element: T)`, `minus`/`minusElement`, and, on Map, `plus` and `plusEntry`, `minusKey` and `minusKeys`.
  Overloading on the **receiver** type is used, for example `List<Int>.sum()` and `List<Float>.sum()`.
- The Map callbacks of `filter`, `filterNot`, `forEach`, `any`, `all`, `none`, `count` and `map`
  take `(key, value)`. `mapValues` takes the value, and `mapKeys` takes the key.

## Function list

### `String.sphp` (47 functions)

- `function String.length(): Int = mb_strlen(this)`
- `function String.isEmpty(): Bool`
- `function String.isNotEmpty(): Bool`
- `function String.isBlank(): Bool = preg_match('/\A\s*\z/u', this)`
- `function String.isNotBlank(): Bool`
- `function String.ifEmpty(defaultValue: () -> String): String`
- `function String.ifBlank(defaultValue: () -> String): String = if (preg_match('/\A\s*\z/u', this)`
- `function String.uppercase(): String = mb_strtoupper(this)`
- `function String.lowercase(): String = mb_strtolower(this)`
- `function String.capitalize(): String = mb_ucfirst(this)`
- `function String.decapitalize(): String = mb_lcfirst(this)`
- `function String.trim(): String = mb_trim(this)`
- `function String.trimStart(): String = mb_ltrim(this)`
- `function String.trimEnd(): String = mb_rtrim(this)`
- `function String.startsWith(prefix: String, ignoreCase: Bool = false): Bool`
- `function String.endsWith(suffix: String, ignoreCase: Bool = false): Bool`
- `function String.contains(other: String, ignoreCase: Bool = false): Bool`
- `function String.indexOf(other: String, startIndex: Int = 0): Int? = nullIfFalse(mb_strpos(this, other, startIndex))`
- `function String.lastIndexOf(other: String): Int? = nullIfFalse(mb_strrpos(this, other))`
- `function String.substring(startIndex: Int, endIndex: Int? = null): String`
- `function String.substringBefore(delimiter: String): String`
- `function String.substringAfter(delimiter: String): String`
- `function String.take(n: Int): String = mb_substr(this, 0, max(0, n))`
- `function String.drop(n: Int): String = mb_substr(this, max(0, n))`
- `function String.takeLast(n: Int): String = if (n <= 0) "" else mb_substr(this, -n)`
- `function String.dropLast(n: Int): String = mb_substr(this, 0, max(0, mb_strlen(this) - max(0, n)))`
- `function String.replace(oldValue: String, newValue: String): String = str_replace(oldValue, newValue, this)`
- `function String.replaceRegex(pattern: String, replacement: String): String = preg_replace(pattern, replacement, this)`
- `function String.matches(pattern: String): Bool = preg_match(pattern, this)`
- `function String.split(separator: String, limit: Int = 0): List<String>`
- `function String.lines(): List<String> = explode("\n", str_replace(["\r\n", "\r"], "\n", this))`
- `function String.repeat(n: Int): String = str_repeat(this, n)`
- `function String.reversed(): String = implode("", array_reverse(mb_str_split(this)))`
- `function String.padStart(length: Int, padChar: String = " "): String = mb_str_pad(this, length, padChar, STR_PAD_LEFT)`
- `function String.padEnd(length: Int, padChar: String = " "): String = mb_str_pad(this, length, padChar, STR_PAD_RIGHT)`
- `function String.toIntOrNull(): Int?`
- `function String.toInt(): Int = this->toIntOrNull() ?: throw new \InvalidArgumentException("Invalid Int format: '${this}'.")`
- `function String.toFloatOrNull(): Float?`
- `function String.toFloat(): Float = this->toFloatOrNull() ?: throw new \InvalidArgumentException("Invalid Float format: '${this}'.")`
- `function String.toBool(): Bool = strtolower(this)`
- `function String.get(index: Int): String`
- `function String.getOrNull(index: Int): String?`
- `function String.chars(): List<String> = mb_str_split(this)`
- `function String.removePrefix(prefix: String): String`
- `function String.removeSuffix(suffix: String): String`
- `function String.equalsIgnoreCase(other: String): Bool`
- `function String.compareTo(other: String): Int`

### `Int.sphp` (20 functions)

- `function intOrOverflow(value: Int | Float): Int = if (value is Int) value else throw new \ArithmeticError("Integer overflow.")`
- `function Int.toString(): String = strval(this)`
- `function Int.toFloat(): Float = floatval(this)`
- `function Int.abs(): Int = intOrOverflow(abs(this))`
- `function Int.coerceAtLeast(minimumValue: Int): Int = max(this, minimumValue)`
- `function Int.coerceAtMost(maximumValue: Int): Int = min(this, maximumValue)`
- `function Int.coerceIn(minimumValue: Int, maximumValue: Int): Int`
- `function Int.isEven(): Bool`
- `function Int.isOdd(): Bool`
- `function Int.pow(exponent: Int): Int`
- `function Int.sign(): Int`
- `function Int.min(other: Int): Int = min(this, other)`
- `function Int.max(other: Int): Int = max(this, other)`
- `function Int.compareTo(other: Int): Int`
- `function Int.div(other: Int): Int = intdiv(this, other)`
- `function Int.rem(other: Int): Int`
- `function Int.mod(other: Int): Int`
- `function Int.rangeTo(end: Int): List<Int> = if (end < this) [] else range(this, end)`
- `function Int.until(end: Int): List<Int> = if (end <= this) [] else range(this, end - 1)`
- `function Int.downTo(to: Int): List<Int> = if (to > this) [] else range(this, to)`

### `Float.sphp` (20 functions)

- `function Float.toString(): String = strval(this)`
- `function Float.toInt(): Int`
- `function Float.roundToInt(): Int`
- `function Float.round(decimals: Int = 0): Float = round(this, decimals)`
- `function Float.floor(): Float = floor(this)`
- `function Float.ceil(): Float = ceil(this)`
- `function Float.abs(): Float = abs(this)`
- `function Float.coerceAtLeast(minimumValue: Float): Float`
- `function Float.coerceAtMost(maximumValue: Float): Float`
- `function Float.coerceIn(minimumValue: Float, maximumValue: Float): Float`
- `function Float.pow(exponent: Float): Float`
- `function Float.sqrt(): Float = sqrt(this)`
- `function Float.sign(): Float = if (is_nan(this)) NAN else floatval(this <=> 0.0)`
- `function Float.min(other: Float): Float = if (is_nan(this) || is_nan(other)) NAN else min(this, other)`
- `function Float.max(other: Float): Float = if (is_nan(this) || is_nan(other)) NAN else max(this, other)`
- `function Float.compareTo(other: Float): Int`
- `function Float.rem(other: Float): Float = fmod(this, other)`
- `function Float.isNaN(): Bool = is_nan(this)`
- `function Float.isFinite(): Bool = is_finite(this)`
- `function Float.isInfinite(): Bool = is_infinite(this)`

### `Bool.sphp` (6 functions)

- `function Bool.toString(): String`
- `function Bool.not(): Bool`
- `function Bool.and(other: Bool): Bool`
- `function Bool.or(other: Bool): Bool`
- `function Bool.xor(other: Bool): Bool`
- `function Bool.compareTo(other: Bool): Int`

### `List.sphp` (78 functions)

- `record IndexedValue<T>`
- `function <T> List<T>.size(): Int = count(this)`
- `function <T> List<T>.isEmpty(): Bool`
- `function <T> List<T>.isNotEmpty(): Bool`
- `function <T> List<T>.indices(): List<Int> = array_keys(this)`
- `function <T> List<T>.lastIndex(): Int`
- `function <T> List<T>.get(index: Int): T`
- `function <T> List<T>.getOrNull(index: Int): T?`
- `function <T> List<T>.getOrElse(index: Int, defaultValue: (Int) -> T): T`
- `function <T> List<T>.first(): T`
- `function <T> List<T>.firstOrNull(): T?`
- `function <T> List<T>.last(): T`
- `function <T> List<T>.lastOrNull(): T?`
- `function <T> List<T>.random(): T`
- `function <T> List<T>.randomOrNull(): T?`
- `function <T> List<T>.contains(element: T): Bool = in_array(element, this, true)`
- `function <T> List<T>.containsAll(elements: List<T>): Bool = array_all(elements, { in_array(it, this, true) })`
- `function <T> List<T>.indexOf(element: T): Int? = nullIfFalse(array_search(element, this, true))`
- `function <T> List<T>.lastIndexOf(element: T): Int? = nullIfFalse(array_search(element, array_reverse(this, true), true))`
- `function <T> List<T>.indexOfFirst(predicate: (T) -> Bool): Int? = array_find_key(this, { predicate(it) })`
- `function <T> List<T>.indexOfLast(predicate: (T) -> Bool): Int? = array_find_key(array_reverse(this, true), { predicate(it) })`
- `function <T, R> List<T>.map(transform: (T) -> R): List<R> = array_map(transform, this)`
- `function <T, R> List<T>.mapIndexed(transform: (Int, T) -> R): List<R> = array_map(transform, array_keys(this), this)`
- `function <T, R> List<T>.mapNotNull(transform: (T) -> R?): List<R> = array_values(array_filter(array_map(transform, this), { it !== null }))`
- `function <T> List<T>.filter(predicate: (T) -> Bool): List<T> = array_values(array_filter(this, { predicate(it) }))`
- `function <T> List<T>.filterIndexed(predicate: (Int, T) -> Bool): List<T>`
- `function <T> List<T>.filterNot(predicate: (T) -> Bool): List<T> = array_values(array_filter(this, { !predicate(it) }))`
- `function <T> List<T? >.filterNotNull(): List<T> = array_values(array_filter(this, { it !== null }))`
- `function <T> List<T>.forEach(action: (T) -> Unit): Unit`
- `function <T> List<T>.forEachIndexed(action: (Int, T) -> Unit): Unit`
- `function <T> List<T>.any(predicate: (T) -> Bool): Bool = array_any(this, { predicate(it) })`
- `function <T> List<T>.all(predicate: (T) -> Bool): Bool = array_all(this, { predicate(it) })`
- `function <T> List<T>.none(predicate: (T) -> Bool): Bool = !array_any(this, { predicate(it) })`
- `function <T> List<T>.count(predicate: (T) -> Bool): Int = count(array_filter(this, { predicate(it) }))`
- `function <T> List<T>.find(predicate: (T) -> Bool): T? = array_find(this, { predicate(it) })`
- `function <T> List<T>.findLast(predicate: (T) -> Bool): T? = array_find(array_reverse(this), { predicate(it) })`
- `function <T, R> List<T>.fold(initial: R, operation: (R, T) -> R): R = array_reduce(this, operation, initial)`
- `function <T> List<T>.reduce(operation: (T, T) -> T): T`
- `function List<Int>.sum(): Int = intOrOverflow(array_sum(this))`
- `function List<Float>.sum(): Float = floatval(array_sum(this))`
- `function <T> List<T>.sumOf(selector: (T) -> Int): Int = intOrOverflow(array_sum(array_map(selector, this)))`
- `function <T of Int | Float> List<T>.average(): Float = fdiv(array_sum(this), count(this))`
- `function <T of Int | Float | String> List<T>.minOrNull(): T?`
- `function <T of Int | Float | String> List<T>.maxOrNull(): T?`
- `function <T, R of Int | Float | String> List<T>.minByOrNull(selector: (T) -> R): T? = this->sortedBy(selector)->firstOrNull()`
- `function <T, R of Int | Float | String> List<T>.maxByOrNull(selector: (T) -> R): T? = this->sortedByDescending(selector)->firstOrNull()`
- `function <T> List<T>.joinToString(separator: String = ", ", prefix: String = "", postfix: String = "", transform: ((T) -> String)? = null): String`
- `function <T> List<T>.reversed(): List<T> = array_reverse(this)`
- `function <T> List<T>.sortedWith(comparator: (T, T) -> Int): List<T>`
- `function <T of Int | Float | String> List<T>.sorted(): List<T> = this->sortedWith({ a, b -> compareValues(a, b) })`
- `function <T of Int | Float | String> List<T>.sortedDescending(): List<T> = this->sortedWith({ a, b -> compareValues(b, a) })`
- `function <T, R of Int | Float | String> List<T>.sortedBy(selector: (T) -> R): List<T>`
- `function <T, R of Int | Float | String> List<T>.sortedByDescending(selector: (T) -> R): List<T>`
- `function <T> List<T>.shuffled(): List<T>`
- `function <T> List<T>.distinct(): List<T> = array_values(array_intersect_key(this, array_unique(array_map({ serialize(it) }, this))))`
- `function <T, K> List<T>.distinctBy(selector: (T) -> K): List<T>`
- `function <T> List<T>.take(n: Int): List<T> = array_slice(this, 0, max(0, n))`
- `function <T> List<T>.drop(n: Int): List<T> = array_slice(this, max(0, n))`
- `function <T> List<T>.takeLast(n: Int): List<T> = if (n <= 0) [] else array_slice(this, -n)`
- `function <T> List<T>.dropLast(n: Int): List<T> = array_slice(this, 0, max(0, count(this) - max(0, n)))`
- `function <T> List<T>.takeWhile(predicate: (T) -> Bool): List<T>`
- `function <T> List<T>.dropWhile(predicate: (T) -> Bool): List<T>`
- `function <T> List<T>.subList(fromIndex: Int, toIndex: Int): List<T> = array_slice(this, max(0, fromIndex), max(0, toIndex - max(0, fromIndex)))`
- `function <T> List<T>.plus(elements: List<T>): List<T> = array_merge(this, elements)`
- `function <T> List<T>.plusElement(element: T): List<T>`
- `function <T> List<T>.minus(elements: List<T>): List<T> = array_values(array_filter(this, { !in_array(it, elements, true) }))`
- `function <T> List<T>.minusElement(element: T): List<T> = array_values(array_filter(this, { it !== element }))`
- `function <T> List<T>.chunked(size: Int): List<List<T>> = array_chunk(this, size)`
- `function <T, R> List<T>.zip(other: List<R>): List<Pair<T, R>>`
- `function <T> List<T>.partition(predicate: (T) -> Bool): Pair<List<T>, List<T>>`
- `function <T, R> List<T>.flatMap(transform: (T) -> List<R>): List<R> = array_merge(...array_map(transform, this))`
- `function <T> List<List<T>>.flatten(): List<T> = array_merge(...this)`
- `function <T, K of Int | String> List<T>.groupBy(keySelector: (T) -> K): Map<K, List<T>>`
- `function <T, K of Int | String> List<T>.associateBy(keySelector: (T) -> K): Map<K, T> = array_combine(array_map(keySelector, this), this)`
- `function <K of Int | String, V> List<K>.associateWith(valueSelector: (K) -> V): Map<K, V> = array_combine(this, array_map(valueSelector, this))`
- `function <T, K of Int | String, V> List<T>.associate(transform: (T) -> Pair<K, V>): Map<K, V> = array_column(array_map(transform, this), "second", "first")`
- `function <T> List<T>.withIndex(): List<IndexedValue<T>> = array_map({ i, v -> IndexedValue { index = i, value = v } }, array_keys(this), this)`
- `function <T> List<T>.toMutableList(): MutableList<T>`
- `function <T> List<T>.toList(): List<T>`

### `MutableList.sphp` (20 functions)

- `function <T> MutableList<T>.add(element: T): Unit = array_push(this, element)`
- `function <T> MutableList<T>.addAt(index: Int, element: T): Unit = array_splice(this, index, 0, [element])`
- `function <T> MutableList<T>.addAll(elements: List<T>): Unit = array_push(this, ...elements)`
- `function <T> MutableList<T>.set(index: Int, element: T): Unit`
- `function <T> MutableList<T>.removeAt(index: Int): T`
- `function <T> MutableList<T>.remove(element: T): Bool`
- `function <T> MutableList<T>.removeFirst(): T = if (this === []) throw new \OutOfBoundsException("List is empty.") else array_shift(this)`
- `function <T> MutableList<T>.removeLast(): T = if (this === []) throw new \OutOfBoundsException("List is empty.") else array_pop(this)`
- `function <T> MutableList<T>.removeFirstOrNull(): T? = array_shift(this)`
- `function <T> MutableList<T>.removeLastOrNull(): T? = array_pop(this)`
- `function <T> MutableList<T>.removeAll(predicate: (T) -> Bool): Unit = this = array_values(array_filter(this, { !predicate(it) }))`
- `function <T> MutableList<T>.retainAll(predicate: (T) -> Bool): Unit = this = array_values(array_filter(this, { predicate(it) }))`
- `function <T> MutableList<T>.clear(): Unit`
- `function <T of Int | Float | String> MutableList<T>.sort(): Unit = usort(this, { a, b -> compareValues(a, b) })`
- `function <T of Int | Float | String> MutableList<T>.sortDescending(): Unit = usort(this, { a, b -> compareValues(b, a) })`
- `function <T, R of Int | Float | String> MutableList<T>.sortBy(selector: (T) -> R): Unit`
- `function <T, R of Int | Float | String> MutableList<T>.sortByDescending(selector: (T) -> R): Unit`
- `function <T> MutableList<T>.sortWith(comparator: (T, T) -> Int): Unit = usort(this, comparator)`
- `function <T> MutableList<T>.reverse(): Unit = this = array_reverse(this)`
- `function <T> MutableList<T>.shuffle(): Unit = shuffle(this)`

### `Map.sphp` (32 functions)

- `record Entry<K, V>`
- `function <K, V> Map<K, V>.size(): Int = count(this)`
- `function <K, V> Map<K, V>.isEmpty(): Bool`
- `function <K, V> Map<K, V>.isNotEmpty(): Bool`
- `function <K, V> Map<K, V>.get(key: K): V?`
- `function <K, V> Map<K, V>.getOrDefault(key: K, defaultValue: V): V`
- `function <K, V> Map<K, V>.getOrElse(key: K, defaultValue: () -> V): V = if (array_key_exists(key, this)) this[key] else defaultValue()`
- `function <K, V> Map<K, V>.getValue(key: K): V`
- `function <K, V> Map<K, V>.containsKey(key: K): Bool = array_key_exists(key, this)`
- `function <K, V> Map<K, V>.containsValue(value: V): Bool = in_array(value, this, true)`
- `function <K, V> Map<K, V>.keys(): List<K> = array_keys(this)`
- `function <K, V> Map<K, V>.values(): List<V> = array_values(this)`
- `function <K, V> Map<K, V>.entries(): List<Entry<K, V>>`
- `function <K, V> Map<K, V>.toList(): List<Pair<K, V>>`
- `function <K, V, R> Map<K, V>.map(transform: (K, V) -> R): List<R> = array_map(transform, array_keys(this), array_values(this))`
- `function <K, V, R> Map<K, V>.mapValues(transform: (V) -> R): Map<K, R> = array_map(transform, this)`
- `function <K, V, R of Int | String> Map<K, V>.mapKeys(transform: (K) -> R): Map<R, V> = array_combine(array_map(transform, array_keys(this)), this)`
- `function <K, V> Map<K, V>.filter(predicate: (K, V) -> Bool): Map<K, V> = array_filter(this, { v, k -> predicate(k, v) }, ARRAY_FILTER_USE_BOTH)`
- `function <K, V> Map<K, V>.filterNot(predicate: (K, V) -> Bool): Map<K, V> = array_filter(this, { v, k -> !predicate(k, v) }, ARRAY_FILTER_USE_BOTH)`
- `function <K, V> Map<K, V>.filterKeys(predicate: (K) -> Bool): Map<K, V> = array_filter(this, { predicate(it) }, ARRAY_FILTER_USE_KEY)`
- `function <K, V> Map<K, V>.filterValues(predicate: (V) -> Bool): Map<K, V> = array_filter(this, { predicate(it) })`
- `function <K, V> Map<K, V>.forEach(action: (K, V) -> Unit): Unit`
- `function <K, V> Map<K, V>.any(predicate: (K, V) -> Bool): Bool = array_any(this, { v, k -> predicate(k, v) })`
- `function <K, V> Map<K, V>.all(predicate: (K, V) -> Bool): Bool = array_all(this, { v, k -> predicate(k, v) })`
- `function <K, V> Map<K, V>.none(predicate: (K, V) -> Bool): Bool = !array_any(this, { v, k -> predicate(k, v) })`
- `function <K, V> Map<K, V>.count(predicate: (K, V) -> Bool): Int = count(array_filter(this, { v, k -> predicate(k, v) }, ARRAY_FILTER_USE_BOTH))`
- `function <K, V> Map<K, V>.keyOf(value: V): K? = nullIfFalse(array_search(value, this, true))`
- `function <K, V> Map<K, V>.plus(other: Map<K, V>): Map<K, V> = array_replace(this, other)`
- `function <K, V> Map<K, V>.plusEntry(key: K, value: V): Map<K, V> = array_replace(this, [key => value])`
- `function <K, V> Map<K, V>.minusKey(key: K): Map<K, V> = array_diff_key(this, [key => true])`
- `function <K, V> Map<K, V>.minusKeys(keys: List<K>): Map<K, V> = array_diff_key(this, array_flip(keys))`
- `function <K, V> Map<K, V>.toMutableMap(): MutableMap<K, V>`
- `function <K, V> Map<K, V>.toMap(): Map<K, V>`

### `MutableMap.sphp` (8 functions)

- `function <K, V> MutableMap<K, V>.set(key: K, value: V): Unit`
- `function <K, V> MutableMap<K, V>.put(key: K, value: V): V?`
- `function <K, V> MutableMap<K, V>.putAll(from: Map<K, V>): Unit = this = array_replace(this, from)`
- `function <K, V> MutableMap<K, V>.remove(key: K): V?`
- `function <K, V> MutableMap<K, V>.getOrPut(key: K, defaultValue: () -> V): V`
- `function <K, V> MutableMap<K, V>.removeAll(predicate: (K, V) -> Bool): Unit`
- `function <K, V> MutableMap<K, V>.retainAll(predicate: (K, V) -> Bool): Unit`
- `function <K, V> MutableMap<K, V>.clear(): Unit`

### `Collections.sphp` (18 functions)

- `record Pair<A, B>`
- `record Triple<A, B, C>`
- `function <A, B> A.to(other: B): Pair<A, B>`
- `function <T> Pair<T, T>.toList(): List<T>`
- `function <T> listOf(...elements: T): List<T> = array_values(elements)`
- `function <T> emptyList(): List<T>`
- `function <T> listOfNotNull(...elements: T? ): List<T> = array_values(array_filter(elements, { it !== null }))`
- `function <T> mutableListOf(...elements: T): MutableList<T> = array_values(elements)`
- `function <T> listOfSize(size: Int, init: (Int) -> T): List<T>`
- `function <K of Int | String, V> mapOf(...pairs: Pair<K, V>): Map<K, V> = array_column(pairs, "second", "first")`
- `function <K of Int | String, V> emptyMap(): Map<K, V>`
- `function <K of Int | String, V> mutableMapOf(...pairs: Pair<K, V>): MutableMap<K, V> = array_column(pairs, "second", "first")`
- `function <T of Int | Float | String> compareValues(a: T?, b: T?): Int`
- `function repeat(times: Int, action: (Int) -> Unit): Unit`
- `function requireThat(condition: Bool, message: String = "Failed requirement."): Unit`
- `function <T> requireNotNull(value: T?, message: String = "Required value was null."): T`
- `function check(condition: Bool, message: String = "Check failed."): Unit`
- `function <T> checkNotNull(value: T?, message: String = "Required value was null."): T`
- `function error(message: String): Nothing = throw new \RuntimeException(message)`
- `function TODO(reason: String = "Not implemented."): Nothing = throw new \LogicException("An operation is not implemented: ${reason}")`

### `Nullable.sphp` (13 functions)

- `function <T> nullIfFalse(value: T | false): T?`
- `function <T, R> T.let(block: (T) -> R): R = block(this)`
- `function <T> T.also(block: (T) -> Unit): T`
- `function <T> T.takeIf(predicate: (T) -> Bool): T?`
- `function <T> T.takeUnless(predicate: (T) -> Bool): T?`
- `function <R> run(block: () -> R): R = block()`
- `function String?.orEmpty(): String`
- `function <T> List<T>?.orEmpty(): List<T>`
- `function <K, V> Map<K, V>?.orEmpty(): Map<K, V>`
- `function String?.isNullOrEmpty(): Bool`
- `function String?.isNullOrBlank(): Bool = this === null || preg_match('/\A\s*\z/u', this)`
- `function <T> List<T>?.isNullOrEmpty(): Bool`
- `function <K, V> Map<K, V>?.isNullOrEmpty(): Bool`

