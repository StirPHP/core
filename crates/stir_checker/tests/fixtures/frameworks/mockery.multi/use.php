<?php

interface Adapter
{
    public function run(): void;
}

class Real implements Adapter
{
    public function run(): void {}
}

function wants_adapter(Adapter $adapter): void {}
function wants_real(Real $real): void {}

function mocks(string $name): void
{
    // a mock of a class is an object of it, and a mock
    $mock = Mockery::mock(Adapter::class);
    $mock->shouldReceive('run');
    $mock->run();
    $mock->nope(); // error: method.notFound
    wants_adapter($mock);
    wants_real($mock); // error: argument.type

    wants_adapter(Mockery::spy(Adapter::class));
    wants_adapter(Mockery::mock('Adapter'));
    wants_adapter(Mockery::mock(new Real()));

    // nothing's known of what's not named
    wants_adapter(Mockery::mock($name)); // error: argument.type
    Mockery::mock()->shouldReceive('a');
    Mockery::mock()->nope(); // error: method.notFound
}
