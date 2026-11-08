<?php
// @framework laravel

namespace Illuminate\Support {
    class MessageBag
    {
        /** @return bool */
        public function has(string $key) { return true; }
        /** @return string */
        public function first(string $key = '') { return ''; }
    }

    /** @mixin MessageBag */
    class ViewErrorBag
    {
        public function any(): bool { return true; }
        public function getBag(string $name): MessageBag { return new MessageBag; }
        public function __get(string $name): MessageBag { return new MessageBag; }
        /** @param list<mixed> $args */
        public function __call(string $method, array $args): mixed { return null; }
    }
}

namespace Illuminate\View {
    class Factory
    {
        public function yieldContent(string $section): string { return ''; }
    }

    class ComponentSlot
    {
        public function isEmpty(): bool { return true; }
    }

    /** @implements \IteratorAggregate<string, mixed> */
    class ComponentAttributeBag implements \IteratorAggregate
    {
        /** @param array<string, mixed> $attrs */
        public function merge(array $attrs): static { return $this; }
        public function getIterator(): \ArrayIterator { return new \ArrayIterator([]); }
    }
}

namespace Symfony\Component\HttpKernel\Exception {
    interface HttpExceptionInterface extends \Throwable
    {
        public function getStatusCode(): int;
    }
}

namespace Illuminate\Support\Facades {
    class Auth
    {
        /** @return \App\Models\Author|null */
        public static function user() { return null; }
    }
}
