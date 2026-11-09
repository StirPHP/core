<?php

use Tests\FeatureTestCase;
use Tests\UnitTestCase;

uses(FeatureTestCase::class)->in('Feature');
pest()->extend(UnitTestCase::class)->use(\Tests\WithFaker::class)->in(__DIR__.'/Unit');

expect()->extend('toBeOne', function () {
    return $this->toBe(1);
});

pest()->beforeEach(function () {
    $this->shared = 1;
})->in('Feature');
