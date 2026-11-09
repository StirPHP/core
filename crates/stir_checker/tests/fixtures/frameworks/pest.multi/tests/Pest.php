<?php

use Tests\FeatureTestCase;
use Tests\UnitTestCase;

uses(FeatureTestCase::class)->in('Feature');
pest()->extend(UnitTestCase::class)->use(\Tests\WithFaker::class)->in(__DIR__.'/Unit');

pest()->beforeEach(function () {
    $this->shared = 1;
})->in('Feature');
