<?php
// What a package does in its service provider: its macros count for the application's code too.

namespace Package;

use Illuminate\Database\Eloquent\Factories\Factory;
use Illuminate\Routing\Router;
use Illuminate\Support\Optional;

class Testing
{
    public function fromMixin(): \Closure { return fn () => $this; }
    protected function protectedOfMixin(): \Closure { return fn () => $this; }
    private function privateOfMixin(): void {}
}

class Provider
{
    public function boot(Router $router): void
    {
        $router->macro('feeds', fn () => 1);
        Factory::macro('fromPackage', fn () => 1);
        Factory::mixin(new Testing);
    }
}
