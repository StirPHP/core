<?php
function want(string $s): string
{
    return $s;
}

/** @return list<string> */
function appended(): array
{
    $a = array();
    $a[] = 'x';
    return $a;
}

/** @return array<string, int> */
function keyed(): array
{
    $a = [];
    $a['k'] = 1;
    return $a;
}

function nested(): array
{
    $a = array();
    $a['n']['m'] = 1;
    $a['n']['o'][] = 'x';
    return $a;
}

function autovivified(): array
{
    $b[] = 1;
    $c['k']['l'] = 1;
    return [$b, $c];
}

function mixed_elements(): array
{
    $a = array('x');
    $a[] = 1;
    $a['k'] = true;
    foreach ($a as $v) {
        echo $v;
    }
    return $a;
}

/** @return list<string> */
function grown_wrongly(): array
{
    $a = array();
    $a[] = 1;
    return $a; // error: SPHP0005
}

function empty_read(): void
{
    $a = array();
    foreach ($a as $v) {
        want($v['x']);
    }
}

function string_offset(string $s): string
{
    $s[0] = 'x';
    return $s;
}

class Node
{
    public function add(): void {}
}

function written_later_in_loop(array $items): void
{
    $ref = array();
    foreach ($items as $k => $item) {
        if (isset($ref[$k])) {
            $ref[$k]->add();
        } else {
            $ref[$k] = new Node();
        }
    }
}

class Guarded
{
    public function has(): bool
    {
        return isset($this->dynamic);
    }

    public function missing()
    {
        return $this->dynamic; // error: SPHP0009
    }
}

/**
 * @template T
 * @param T $v
 */
function template_index($v)
{
    return $v['k'];
}

function add_to_nullable(?array $a): array
{
    $a += array('k' => 1);
    return $a;
}
