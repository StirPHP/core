<?php
class Hooks
{
    public function handler(): void {}

    public function register(): void
    {
        add_hook([$this, 'handler']);
        add_hook(array($this, 'handler'));
    }
}

function add_hook(callable $cb): void {}

add_hook('strlen');
add_hook(array('Hooks', 'handler'));
array_map('intval', ['1']);
add_hook(1); // error: SPHP0005
