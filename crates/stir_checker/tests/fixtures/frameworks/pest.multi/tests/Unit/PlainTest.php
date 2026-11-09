<?php

it('expects of what the value has', function () {
    $response = new \Tests\Response();
    expect($response)->toHaveCount(1)->assertOk();
    expect($response)->assertOk()->anything;
    expect($response)->nope(); // error: method.notFound
    expect(1)->toBeOne();
    expect(1)->nope(); // error: method.notFound
    expect(mixed())->anything();
});

function mixed(): mixed { return 1; }

it('has what the folder says, by another way of saying it', function () {
    $this->unitOnly();
    $this->fake();
    $this->get('/'); // error: method.notFound
});
