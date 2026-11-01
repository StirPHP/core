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
