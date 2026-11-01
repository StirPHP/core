<?php
namespace Lib;

/**
 * @phpstan-import-type UserRow from Types
 * @phpstan-import-type Ids from Types as IdList
 * @phpstan-type Name non-empty-string
 * @property-read int $count
 * @method UserRow|null find(int $id)
 */
class Repo
{
    /**
     * @return list<UserRow>
     */
    public function all(): array
    {
        return [];
    }

    /**
     * @return IdList
     */
    public function ids(): array
    {
        return [1, 2];
    }

    /**
     * @return Name
     */
    public function name(): string
    {
        return 'repo';
    }

    /**
     * @return string // error: SPHP0050
     */
    public function size(): int
    {
        return 0;
    }

    public function __call(string $name, array $args): mixed
    {
        return null;
    }

    public function __get(string $name): mixed
    {
        return 0;
    }
}
