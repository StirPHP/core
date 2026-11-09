<?php

// @framework pest

namespace PHPUnit\Framework {
    abstract class TestCase
    {
        public static function assertTrue(bool $condition): void {}
        protected function expectException(string $exception): void {}
    }
}

namespace Pest\Concerns {
    trait Testable
    {
        public function defer(callable $callable): object { return $this; }
    }
}

namespace Pest\Support {
    final class HigherOrderTapProxy
    {
        public function __construct(public \PHPUnit\Framework\TestCase $target) {}
        public function __get(string $property): mixed { return null; }
        /** @return mixed */
        public function __call(string $methodName, array $arguments) { return null; }
    }
}

namespace Pest\PendingCalls {
    /**
     * @mixin \PHPUnit\Framework\TestCase|\Pest\Concerns\Testable
     */
    final class TestCall
    {
        public function with(mixed ...$data): self { return $this; }
        public function after(?\Closure $closure = null): self { return $this; }
        public function __get(string $name): self { return $this; }
        public function __call(string $name, array $arguments): self { return $this; }
    }

    final class BeforeEachCall
    {
        public function __call(string $name, array $arguments): self { return $this; }
    }

    final class UsesCall
    {
        public function in(string ...$targets): self { return $this; }
        public function extend(string ...$classAndTraits): self { return $this; }
        public function use(string ...$classAndTraits): self { return $this; }
        public function beforeEach(\Closure $hook): self { return $this; }
    }
}

namespace Pest {
    final class Configuration
    {
        public function in(string ...$targets): PendingCalls\UsesCall { return new PendingCalls\UsesCall(); }
        public function extend(string ...$classAndTraits): PendingCalls\UsesCall { return new PendingCalls\UsesCall(); }
        public function use(string ...$classAndTraits): PendingCalls\UsesCall { return new PendingCalls\UsesCall(); }
        public function __call(string $name, array $arguments): mixed { return null; }
    }
}

namespace {
    use Pest\PendingCalls\BeforeEachCall;
    use Pest\PendingCalls\TestCall;
    use Pest\PendingCalls\UsesCall;
    use Pest\Support\HigherOrderTapProxy;

    /** @param class-string ...$classAndTraits */
    function uses(string ...$classAndTraits): UsesCall { return new UsesCall(); }

    function pest(): \Pest\Configuration { return new \Pest\Configuration(); }

    /**
     * @param-closure-this TestCall $closure
     * @return ($description is string ? TestCall : HigherOrderTapProxy|TestCall)
     */
    function test(?string $description = null, ?Closure $closure = null): HigherOrderTapProxy|TestCall
    {
        return new TestCall();
    }

    /** @param-closure-this TestCall $closure */
    function it(string $description, ?Closure $closure = null): TestCall { return new TestCall(); }

    /** @param-closure-this TestCall $closure */
    function beforeEach(?Closure $closure = null): BeforeEachCall { return new BeforeEachCall(); }
}
