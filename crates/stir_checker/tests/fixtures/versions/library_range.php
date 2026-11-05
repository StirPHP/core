<?php
// @php 7.4..8.1
// @level 0

// on the lowest version, but not on all of them
echo each($a); // error: SPHP0061
if (PHP_VERSION_ID < 80000) {
    echo each($a);
}

// not on the lowest version
echo str_contains('a', 'b'); // error: SPHP0061

// on all of them
echo strlen('a');
