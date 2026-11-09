<?php

namespace Tests;

class FeatureTestCase extends \PHPUnit\Framework\TestCase
{
    protected string $base = '';

    public function get(string $uri): Response { return new Response(); }
    protected function login(): int { return 1; }
}

class UnitTestCase extends \PHPUnit\Framework\TestCase
{
    public function unitOnly(): void {}
}

class OtherTestCase extends \PHPUnit\Framework\TestCase
{
    public function otherOnly(): void {}
}

class Response
{
    public function assertOk(): self { return $this; }
}

trait WithFaker
{
    protected string $faker = '';
    protected function fake(): string { return ''; }
}
