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

    /**
     * @method static \Illuminate\Foundation\Console\ClosureCommand command(string $signature, \Closure $callback)
     * @method static int call(string $command, array $parameters = [])
     */
    class Artisan extends Facade
    {
        protected static function getFacadeAccessor() { return \Illuminate\Contracts\Console\Kernel::class; }
    }
}

namespace Illuminate\Contracts\Console {
    interface Kernel
    {
        public function call(string $command, array $parameters = []): int;
    }
}

namespace Illuminate\Console {
    class Command
    {
        public function info(string $string): void {}
        public function comment(string $string): void {}
        /** @return array<string, mixed>|string|bool|null */
        public function argument(?string $key = null) { return null; }
    }
}

namespace Illuminate\Foundation\Console {
    class Kernel implements \Illuminate\Contracts\Console\Kernel
    {
        public function call(string $command, array $parameters = []): int { return 0; }

        /**
         * @param string $signature
         * @param \Closure $callback
         * @return \Illuminate\Foundation\Console\ClosureCommand
         */
        public function command($signature, \Closure $callback) { return new ClosureCommand(); }

        protected function commands(): void {}
    }

    class ClosureCommand extends \Illuminate\Console\Command
    {
        public function purpose(string $description): static { return $this; }
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
