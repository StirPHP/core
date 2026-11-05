<?php declare(strict_types=1);
namespace App;

use Lib\Repo;

function wants_int(int $v): void {}
function wants_string(string $v): void {}
/** @param list<int> $v */
function wants_ints(array $v): void {}
/** @param list<string> $v */
function wants_strings(array $v): void {}

$repo = new Repo();
wants_ints($repo->ids());
wants_strings($repo->ids()); // error: argument.type
wants_string($repo->name());
wants_int($repo->count);
wants_string($repo->count); // error: argument.type
wants_int($repo->size());
wants_int($repo->all()); // error: argument.type
wants_int($repo->find(1)); // error: argument.type
