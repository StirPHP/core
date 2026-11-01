<?php
// @level 5
new Missing1(); // @phpstan-ignore class.notFound
// @phpstan-ignore class.notFound
new Missing2();
// @phpstan-ignore-next-line
new Missing3();
new Missing4(); // @phpstan-ignore-line
new Missing5(); // error: SPHP0008

/**
 * @template T
 * @param callable(string, int): bool $cb
 */
function takes_two_arg_callback(callable $cb): void {}
takes_two_arg_callback(fn($s) => true);
echo array_all(['a'], fn($s) => true);

setcookie('n', 'v', 1, '/', 'example.com', true, true);
setcookie('n', 'v', ['path' => '/']);

/** @param array<string, string> $a */
function sum_arrays($a): array
{
    $a += array('k' => 'v');
    return $a + array('l' => 'w');
}

/** @var list<int> */
class Priorities
{
    /** @var list<int> */
    private $priorities = array();
    /** @var array<int, string> */
    private $callbacks = array();
    public function resync(): void
    {
        $this->priorities = array_keys($this->callbacks);
    }
}

/** @phpstan-param self::NOPE_* $v */
function doc_only_const(string $v): void {}
