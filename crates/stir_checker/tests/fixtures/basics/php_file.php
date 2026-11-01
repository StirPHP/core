<?php
namespace App;

function plain($a, $b) { return $a . $b; }
$x = plain(1, 2);
echo $x;
echo undefined_fn(); // error: SPHP0007
