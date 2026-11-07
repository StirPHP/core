<?php

namespace Illuminate\Support\Facades {
    abstract class Facade
    {
        public static function __callStatic(string $method, array $args): mixed { return null; }
    }

    class Cache extends Facade
    {
        protected static function getFacadeAccessor() { return 'cache'; }
    }

    class Schema extends Facade
    {
        protected static function getFacadeAccessor() { return 'db.schema'; }
    }

    class Theme extends Facade
    {
        protected static function getFacadeAccessor() { return \App\ThemeService::class; }
    }
}

namespace Illuminate\Database\Schema {
    class Builder
    {
        public function create(string $table, \Closure $callback): void {}
        public function table(string $table, \Closure $callback): void {}
    }
}

namespace Illuminate\Cache {
    class CacheManager
    {
        public function remember(string $key, int $ttl, \Closure $callback): mixed { return null; }
    }
}

namespace Illuminate\Contracts\Auth {
    interface Authenticatable {}

    interface Factory
    {
        public function guard(?string $name = null): Guard;
    }

    interface Guard
    {
        /** @return Authenticatable|null */
        public function user();
    }
}

namespace Illuminate\Auth {
    /** @mixin \Illuminate\Contracts\Auth\Guard */
    class AuthManager implements \Illuminate\Contracts\Auth\Factory
    {
        public function guard(?string $name = null): \Illuminate\Contracts\Auth\Guard { return new SessionGuard(); }
        public function __call(string $method, array $parameters): mixed { return null; }
    }

    class SessionGuard implements \Illuminate\Contracts\Auth\Guard
    {
        public function user() { return null; }
        public function logout(): void {}
    }
}

namespace Illuminate\Support {
    class Carbon {}
}

namespace {
    function auth(): \Illuminate\Contracts\Auth\Factory { return new \Illuminate\Auth\AuthManager(); }
    function env(string $key, mixed $default = null): mixed { return $default; }
}
