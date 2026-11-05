<?php
// @level 5
define('MINUTE', 60);
define('HOUR', 60 * MINUTE);
define('MODE', 'ARRAY_A');
define('RATIO', 1.5);

function takes_int(int $i): void {}

/** @param 'OBJECT'|'ARRAY_A' $output */
function takes_mode($output): void {}

takes_int(MINUTE);
takes_int(HOUR);
takes_int(2 * HOUR);
takes_int(time() + 2 * HOUR);
takes_mode(MODE);
takes_mode('NOPE'); // error: argument.type
