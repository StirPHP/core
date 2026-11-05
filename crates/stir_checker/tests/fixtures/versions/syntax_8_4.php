<?php
// @php 8.4
// @level 0

class Temperature {
    public int $degrees { get => 1; }
    public private(set) int $locked = 0;
}

$t = new Temperature()->degrees;
$n = "a" |> strlen(...); // error: SPHP0060
$c = clone($t, ['locked' => 1]); // error: SPHP0060
