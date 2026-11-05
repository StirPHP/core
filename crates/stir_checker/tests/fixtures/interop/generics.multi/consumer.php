<?php declare(strict_types=1);
namespace App;

use Lib\Box;

function wants_int(int $v): void {}
function wants_string(string $v): void {}
/** @param list<string> $v */
function wants_strings(array $v): void {}
/** @param list<int> $v */
function wants_ints(array $v): void {}
/** @param Box<string> $v */
function wants_string_box(Box $v): void {}
/** @param Box<int> $v */
function wants_int_box(Box $v): void {}

$b = new Box("a");
wants_string($b->first());
wants_int($b->first()); // error: SPHP0005
wants_strings($b->all());
wants_ints($b->all()); // error: SPHP0005
wants_string_box($b->add("c"));
wants_int_box($b); // error: SPHP0005

$viaFn = \Lib\box(1);
wants_int_box($viaFn);
wants_int($viaFn->first());
wants_ints($viaFn->add(2)->all());
