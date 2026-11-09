<?php

use Tests\WithFaker;

uses(WithFaker::class);

beforeEach(function () {
    $this->count = 1;
    $this->rows['a'] = 2;
});

test('what the folder is configured with, and what the file adds', function () {
    // its own class and what it inherits, protected or not
    $this->get('/')->assertOk();
    $this->login();
    $this->base;
    $this->expectException(Exception::class);
    self::assertTrue(true);

    // the traits Pest uses, and its own
    $this->fake();
    $this->faker;
    $this->defer(fn () => 1);

    // not what's configured for another folder
    $this->unitOnly(); // error: method.notFound
    $this->nope(); // error: method.notFound
    $this->get('/')->nope(); // error: method.notFound
    $this->get([]); // error: argument.type

    // what's set on it can be read: it's `#[AllowDynamicProperties]`
    $this->count;
    $this->rows;
    $this->shared;
    $this->set = 2;
    $this->notSetAnywhere; // error: property.notFound
});

it('passes the closure on to closures in it', function () {
    array_map(fn () => $this->get('/'), [1]);
    array_map(function () {
        $this->nope(); // error: method.notFound
    }, [1]);
});

// what it gives is the pending call, with the rest of it to be called later
it('runs higher order')
    ->get('/')
    ->assertOk()
    ->anything->atAll();

test('chained', function () {
    $this->login();
})->with([1])->after(function () {
    $this->login();
    $this->nope(); // error: method.notFound
})->helperOfThisFile();

function helperOfThisFile(): void
{
    // a proxy of the test being run: reflection gets it past the visibility of its methods
    test()->get('/')->assertOk();
    test()->login();
    test()->nope(); // error: method.notFound
    test()->base; // error: property.protected
    takes_test_case(test()); // error: argument.type
}

function takes_test_case(\PHPUnit\Framework\TestCase $case): void {}

// a static closure can't be bound
test('static', static function () {
    $this->login(); // error: variable.undefined
});
