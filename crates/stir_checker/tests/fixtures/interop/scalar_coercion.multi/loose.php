<?php
// @level 5
function s(string $s): void {}
function i(int $i): void {}
function a(array $a): void {}
/** @param 'a'|'b' $l */
function lit($l): void {}
/** @param array<string, int> $map */
function keys(array $map): void
{
    foreach ($map as $k => $v) {
        s($k);
        i($k);
    }
}

s(1);
s(1.5);
s(true);
i('1');
strlen(1);
s([]); // error: SPHP0005
a(1); // error: SPHP0005
lit('c'); // error: SPHP0005
