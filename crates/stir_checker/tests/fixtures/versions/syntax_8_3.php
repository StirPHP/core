<?php
// @php 8.3
// @level 0

class Temperature {
    const int MAX = 100;
    public int $degrees { get => 1; } // error: SPHP0060
    public private(set) int $locked = 0; // error: SPHP0060
}

$t = new Temperature()->degrees; // error: SPHP0060
$u = (new Temperature())->degrees;
