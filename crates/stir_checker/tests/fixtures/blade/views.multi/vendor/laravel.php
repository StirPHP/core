<?php
// @framework laravel

namespace Illuminate\Contracts\View {
    interface View
    {
        public function with($key, $value = null);
    }
}

namespace Illuminate\View {
    class Factory
    {
        public function yieldContent(string $section): string { return ''; }
        public function make($view, $data = [], $mergeData = []) { return new View; }
    }

    /** @method static withFoo($x) */
    class View implements \Illuminate\Contracts\View\View
    {
        public function with($key, $value = null) { return $this; }
        public function render(): string { return ''; }
        /** @param list<mixed> $args */
        public function __call(string $method, array $args): static { return $this; }
    }
}

namespace Illuminate\Support {
    class MessageBag
    {
        public function has(string $key): bool { return true; }
    }

    /** @mixin MessageBag */
    class ViewErrorBag
    {
        public function any(): bool { return true; }
    }
}

namespace Illuminate\Support\Facades {
    class View
    {
        public static function make($view, $data = [], $mergeData = []) { return new \Illuminate\View\View; }
        public static function composer($views, $callback) {}
        public static function first($views, $data = []) { return new \Illuminate\View\View; }
    }

    class Blade
    {
        public static function directive($name, $handler) {}
    }

    class Route
    {
        public static function view($uri, $view, $data = []) {}
    }
}

namespace Illuminate\Mail {
    class Message {
        public function embed(string $file): string { return ''; }
    }

    class Mailable
    {
        public $subject;
        public function view($view, array $data = []) { return $this; }
        public function markdown($view, array $data = []) { return $this; }
        public function with($key, $value = null) { return $this; }
        public function subject(string $s) { return $this; }
    }
}

namespace Illuminate\Mail\Mailables {
    class Content
    {
        /** @param array<string, mixed> $with */
        public function __construct(
            public ?string $view = null,
            public ?string $html = null,
            public ?string $text = null,
            public ?string $markdown = null,
            public array $with = [],
            public ?string $htmlString = null,
        ) {}
    }
}

namespace Illuminate\Notifications\Messages {
    class MailMessage
    {
        /** @return $this */
        public function view($view, array $data = []) { return $this; }
        /** @return $this */
        public function markdown($view, array $data = []) { return $this; }
    }
}

namespace {
    class ResponseFactoryStub
    {
        public function view($view, $data = []) { return null; }
    }

    function response(): ResponseFactoryStub { return new ResponseFactoryStub; }

    /** @return \Illuminate\View\View */
    function view($view = null, $data = [], $mergeData = []) { return new \Illuminate\View\View; }
}
