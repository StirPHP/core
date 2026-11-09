<?php

use Tests\OtherTestCase;

// no folder is configured for this one
uses(OtherTestCase::class);

test('its own class', function () {
    $this->otherOnly();
    $this->unitOnly(); // error: method.notFound
});
