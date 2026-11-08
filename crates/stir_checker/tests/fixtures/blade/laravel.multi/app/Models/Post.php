<?php

namespace App\Models;

class Post
{
    public string $title = '';
    public ?Author $author = null;

    public function publish(): void {}
}

class Author
{
    public string $name = '';
}

class Metrics
{
    public function total(): int { return 0; }
}

function helper(): int { return 1; }

/** @return Post|null */
function post() { return null; }

/** @return never */
function abort(int $code) { exit($code); }

const LIMIT = 10;

/** @template T of Author = Post */
trait HasThing
{
    /** @return list<T> */
    public function things() { return []; }
}

class WithThing
{
    use HasThing;
}

class Magic
{
    public function __get(string $name): mixed { return null; }
    /** @param list<mixed> $args */
    public function __call(string $name, array $args): mixed { return null; }
}
