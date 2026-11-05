<?php
// @php 8.3
// @level 3

class Point {
    public function __construct(public readonly int $x) {}

    public function __clone() {
        $this->x = 1;
    }
}
