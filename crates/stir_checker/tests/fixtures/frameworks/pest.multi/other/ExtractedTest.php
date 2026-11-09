<?php

use Tests\OtherTestCase;

// it counts wherever it's written
$uses = uses(OtherTestCase::class);

test('its own class', function () {
    $this->otherOnly();
    $this->nope(); // error: method.notFound
});
