<?php
if (!function_exists('str_contains')) {
    function str_contains(string $a, string $b): bool { return true; }
}
if (!class_exists('Attribute')) {
    class Attribute {}
}
