<?php
// @framework mockery

namespace Mockery {
    interface MockInterface
    {
        public function shouldReceive(string ...$methods): self;
    }
}

namespace {
    class Mockery
    {
        /** @return \Mockery\MockInterface */
        public static function mock(...$args) { return null; }
        /** @return \Mockery\MockInterface */
        public static function spy(...$args) { return null; }
    }
}
