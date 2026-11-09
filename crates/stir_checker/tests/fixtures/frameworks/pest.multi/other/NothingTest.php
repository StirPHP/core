<?php

test('only PHPUnit', function () {
    self::assertTrue(true);
    $this->expectException(Exception::class);
    $this->otherOnly(); // error: method.notFound
});
