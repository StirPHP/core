<?php
// @php 8.4
// @level 0

function f(int $x = null): void {} // warning: parameter.implicitlyNullable
function g(?int $x = null, int|null $y = null, mixed $z = null, $u = null): void {}
function h(int $x = 0): void {}
class A {
    public function m(self $x = null): void {} // warning: parameter.implicitlyNullable
}
