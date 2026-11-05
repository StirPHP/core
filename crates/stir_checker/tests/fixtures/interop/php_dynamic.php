<?php
// @level 5
function includes_define_variables(): string
{
    require __DIR__ . '/version.php';
    return $wp_version;
}

function extract_defines_variables(array $args): string
{
    extract($args);
    return $title;
}

function no_include(): string
{
    return $wp_version; // error: variable.undefined
}

if (defined('MAYBE_SET') && MAYBE_SET) {
    echo MAYBE_SET;
}
echo NEVER_TESTED; // error: constant.notFound

if (function_exists('maybe_there')) {
    maybe_there();
}
never_tested(); // error: function.notFound

function outer(): void
{
    function inner_fn(): int
    {
        return 1;
    }
}
echo inner_fn();

function variadic_by_body()
{
    return func_get_args();
}
variadic_by_body(1, 2, 3);

class Zip
{
    public function extract()
    {
        $args = func_num_args();
        return $args;
    }
}
(new Zip())->extract(1, 2);

echo strtr('abc', ['a' => 'b']);
echo strtr('abc', 'a', 'b');
strtr('abc'); // error: arguments.count

/** @param stdClass $o */
function std($o): void {}
std((object) ['a' => 1]);

/** @param array<string, string> $m */
function keyed($m = array()): void {}
keyed(array());

/** @param list<int> $l */
function takes_list(array $l): void {}
/** @param array<int> $a */
function gives_array($a): void
{
    takes_list($a);
}

/** @return Generator<int, string> */
function gen()
{
    yield 1 => 'x';
}

/** @param non-decimal-int-string $s */
function refined($s): void {}
refined('a');
