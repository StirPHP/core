<?php
// A few of Laravel's classes, as its PHPDoc declares them.

namespace Illuminate\Database\Query {
    class Builder
    {
        /** @return $this */
        public function orderBy(string $column, string $direction = 'asc') { return $this; }
        public function count(): int { return 0; }
        public function __call(string $method, array $parameters): mixed { return null; }
    }
}

namespace Illuminate\Database\Eloquent {
    /**
     * @template TModel of Model
     * @mixin \Illuminate\Database\Query\Builder
     */
    class Builder
    {
        /** @return $this */
        public function where(string $column, mixed $value = null) { return $this; }
        /** @return TModel|null */
        public function first() { return null; }
        /** @return \Illuminate\Database\Eloquent\Collection<int, TModel> */
        public function get() { return new Collection(); }
        /**
         * @param callable(\Illuminate\Support\Collection<int, TModel>, int): mixed $callback
         */
        public function chunk(int $count, callable $callback): bool { return true; }
        public function __call(string $method, array $parameters): mixed { return null; }
    }

    /**
     * @template TKey of array-key
     * @template TModel of Model
     * @extends \Illuminate\Support\Collection<TKey, TModel>
     */
    class Collection extends \Illuminate\Support\Collection {}

    abstract class Model
    {
        /** @return \Illuminate\Database\Eloquent\Builder<static> */
        public static function query() { return new Builder(); }

        /**
         * @template TRelatedModel of Model
         * @param class-string<TRelatedModel> $related
         * @return \Illuminate\Database\Eloquent\Relations\HasMany<TRelatedModel, $this>
         */
        public function hasMany(string $related) { return new Relations\HasMany(); }

        /**
         * @template TRelatedModel of Model
         * @param class-string<TRelatedModel> $related
         * @return \Illuminate\Database\Eloquent\Relations\BelongsTo<TRelatedModel, $this>
         */
        public function belongsTo(string $related, ?string $foreignKey = null) { return new Relations\BelongsTo(); }

        public function __get(string $key): mixed { return null; }
        public function __call(string $method, array $parameters): mixed { return null; }
        public static function __callStatic(string $method, array $parameters): mixed { return null; }
    }

    /**
     * @method static \Illuminate\Database\Eloquent\Builder<static> withTrashed(bool $withTrashed = true)
     */
    trait SoftDeletes {}
}

namespace Illuminate\Database\Eloquent\Relations {
    use Illuminate\Database\Eloquent\Model;

    /**
     * @template TRelatedModel of Model
     * @template TDeclaringModel of Model
     * @template TResult
     * @mixin \Illuminate\Database\Eloquent\Builder<TRelatedModel>
     */
    abstract class Relation
    {
        /** @return TResult */
        public function getResults() { return null; }
        public function __call(string $method, array $parameters): mixed { return null; }
    }

    /**
     * @template TRelatedModel of Model
     * @template TDeclaringModel of Model
     * @extends Relation<TRelatedModel, TDeclaringModel, \Illuminate\Database\Eloquent\Collection<int, TRelatedModel>>
     */
    class HasMany extends Relation {}

    /**
     * @template TRelatedModel of Model
     * @template TDeclaringModel of Model
     * @extends Relation<TRelatedModel, TDeclaringModel, ?TRelatedModel>
     */
    class BelongsTo extends Relation {}
}

namespace Illuminate\Support {
    /**
     * @template TKey of array-key
     * @template-covariant TValue
     */
    class Collection {}
}
