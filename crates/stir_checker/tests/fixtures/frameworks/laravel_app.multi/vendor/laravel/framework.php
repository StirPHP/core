<?php
// @framework laravel
// A few of Laravel's classes, as its PHPDoc declares them.

namespace Illuminate\Support\Traits {
    trait Macroable
    {
        public static function macro(string $name, $macro): void {}
        public static function mixin(object $mixin, bool $replace = true): void {}
        public function __call(string $method, array $parameters): mixed { return null; }
        public static function __callStatic(string $method, array $parameters): mixed { return null; }
    }
}

namespace Illuminate\Support {
    class Optional implements \ArrayAccess
    {
        use Traits\Macroable;
        protected $value;
        public function __construct($value) { $this->value = $value; }
        public function __get($key) { return null; }
        public function offsetExists($offset): bool { return false; }
        public function offsetGet($offset): mixed { return null; }
        public function offsetSet($offset, $value): void {}
        public function offsetUnset($offset): void {}
    }

    /**
     * @template TTarget
     */
    class HigherOrderTapProxy
    {
        /** @var TTarget */
        public $target;
        /** @param TTarget $target */
        public function __construct($target) { $this->target = $target; }
        /** @return TTarget */
        public function __call($method, $parameters) { return $this->target; }
    }
}

namespace Illuminate\Contracts\View {
    interface View
    {
        public function render();
    }

    interface Factory
    {
        /** @return \Illuminate\Contracts\View\View */
        public function make($view, $data = [], $mergeData = []);
    }
}

namespace Illuminate\View {
    class View implements \Illuminate\Contracts\View\View
    {
        public function render() { return ''; }
        /** @return $this */
        public function withErrors($provider) { return $this; }
    }

    class Factory implements \Illuminate\Contracts\View\Factory
    {
        /** @return \Illuminate\Contracts\View\View */
        public function make($view, $data = [], $mergeData = []) { return new View(); }
    }
}

namespace Illuminate\Support\Facades {
    abstract class Facade
    {
        public static function __callStatic(string $method, array $args): mixed { return null; }
    }

    /**
     * @method static \Illuminate\Contracts\View\View make(string $view, array $data = [], array $mergeData = [])
     */
    class View extends Facade
    {
        protected static function getFacadeAccessor() { return 'view'; }
    }

    class Route extends Facade
    {
        protected static function getFacadeAccessor() { return 'router'; }
    }

    /**
     * @method static void fake()
     */
    class Event extends Facade
    {
        protected static function getFacadeAccessor() { return 'events'; }
    }
}

namespace Illuminate\Routing {
    class Router
    {
        use \Illuminate\Support\Traits\Macroable;
    }
}

namespace Illuminate\Events {
    class Dispatcher
    {
        public function fake(): void {}
    }
}

namespace Illuminate\Database\Query {
    class Builder
    {
        public function count(): int { return 0; }
    }
}

namespace Illuminate\Database\Eloquent {
    /**
     * @template TKey of array-key
     * @template TModel of Model
     * @implements \IteratorAggregate<TKey, TModel>
     */
    class Collection implements \IteratorAggregate
    {
        /** @return TModel|null */
        public function first() { return null; }
        public function getIterator(): \Traversable { return new \ArrayIterator([]); }
    }

    /**
     * @template TModel of Model
     * @mixin \Illuminate\Database\Query\Builder
     */
    class Builder
    {
        /** @return $this */
        public function latest() { return $this; }
        /** @return TModel|null */
        public function first() { return null; }
        /** @return Collection<int, TModel> */
        public function get() { return new Collection(); }
        public function __call(string $method, array $parameters): mixed { return null; }
    }

    abstract class Model
    {
        /** @return Builder<static> */
        public static function query() { return new Builder(); }
        public function __call(string $method, array $parameters): mixed { return null; }
        public static function __callStatic(string $method, array $parameters): mixed { return null; }
        public function __get(string $key): mixed { return null; }
        public function __set(string $key, mixed $value): void {}
        /**
         * @template TRelated of Model
         * @param class-string<TRelated> $related
         * @return Relations\HasMany<TRelated, $this>
         */
        public function hasMany($related, $foreignKey = null) { return new Relations\HasMany(); }
        /**
         * @template TRelated of Model
         * @param class-string<TRelated> $related
         * @return Relations\HasMany<TRelated, $this>
         */
        public function morphMany($related, $name) { return new Relations\HasMany(); }
        /** @return Relations\MorphTo<Model, $this> */
        public function morphTo($name = null) { return new Relations\MorphTo(); }
    }

    trait SoftDeletes {}
}

namespace Illuminate\Database\Eloquent\Relations {
    /**
     * @template TRelatedModel of \Illuminate\Database\Eloquent\Model
     * @template TDeclaringModel of \Illuminate\Database\Eloquent\Model
     * @template TResult
     * @mixin \Illuminate\Database\Eloquent\Builder<TRelatedModel>
     */
    abstract class Relation
    {
        /** @return TResult */
        abstract public function getResults();
        public function __call(string $method, array $parameters): mixed { return null; }
    }

    /**
     * @template TRelatedModel of \Illuminate\Database\Eloquent\Model
     * @template TDeclaringModel of \Illuminate\Database\Eloquent\Model
     * @extends Relation<TRelatedModel, TDeclaringModel, \Illuminate\Database\Eloquent\Collection<int, TRelatedModel>>
     */
    class HasMany extends Relation
    {
        public function getResults() { return new \Illuminate\Database\Eloquent\Collection(); }
    }

    /**
     * @template TRelatedModel of \Illuminate\Database\Eloquent\Model
     * @template TDeclaringModel of \Illuminate\Database\Eloquent\Model
     * @extends Relation<TRelatedModel, TDeclaringModel, TRelatedModel|null>
     */
    class MorphTo extends Relation
    {
        public function getResults() { return null; }
        public function associate($model) { return null; }
    }
}

namespace Illuminate\Database\Eloquent\Factories {
    /**
     * @template TFactory of Factory
     */
    trait HasFactory
    {
        /**
         * @param callable|array|int|null $count
         * @param callable|array $state
         * @return TFactory
         */
        public static function factory($count = null, $state = []) { return null; }
    }

    /**
     * @template TModel of \Illuminate\Database\Eloquent\Model
     */
    abstract class Factory
    {
        use \Illuminate\Support\Traits\Macroable;

        /** @return array<string, mixed> */
        abstract public function definition();

        /** @return static */
        public static function new() { return new static(); }

        /** @return \Illuminate\Database\Eloquent\Collection<int, TModel>|TModel */
        public function create($attributes = [], $parent = null) { return null; }

        /** @return \Illuminate\Database\Eloquent\Collection<int, TModel>|TModel */
        public function make($attributes = [], $parent = null) { return null; }

        /** @return TModel */
        public function createOne($attributes = []) { return null; }

        /** @return \Illuminate\Database\Eloquent\Collection<int, TModel> */
        public function createMany($records = null) { return new \Illuminate\Database\Eloquent\Collection(); }

        /** @return TModel */
        public function newModel(array $attributes = []) { return null; }

        /** @return static */
        public function count(?int $count) { return $this; }

        /** @return static */
        public static function times(int $count) { return new static(); }

        /** @return static */
        public function state($state) { return $this; }

        /** @return static */
        public function for($factory, $relationship = null) { return $this; }
    }
}

namespace {
    /**
     * @template TValue
     * @template TReturn
     * @param TValue $value
     * @param (callable(TValue): TReturn)|null $callback
     * @return ($callback is null ? \Illuminate\Support\Optional : ($value is null ? null : TReturn))
     */
    function optional($value = null, ?callable $callback = null) { return null; }

    /**
     * @template TValue
     * @param TValue $value
     * @return ($callback is null ? \Illuminate\Support\HigherOrderTapProxy<TValue> : TValue)
     */
    function tap($value, $callback = null) { return $value; }

    /**
     * @param string|null $view
     * @return ($view is null ? \Illuminate\Contracts\View\Factory : \Illuminate\Contracts\View\View)
     */
    function view($view = null, $data = [], $mergeData = []): \Illuminate\Contracts\View\Factory|\Illuminate\Contracts\View\View
    {
        return new \Illuminate\View\Factory();
    }
}
