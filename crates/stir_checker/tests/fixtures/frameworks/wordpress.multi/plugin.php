<?php
// @framework wordpress
declare(strict_types=1);

function wants_int(int $n): void {}

function plugin_init(): void
{
    global $wpdb, $post;
    wants_int($wpdb->prefix); // error: argument.type
    wants_int($post?->ID); // error: argument.type
    $label = apply_filters('plugin_label', 'x', 1, 2);
    do_action('plugin_done');
}
add_action('init', 'plugin_init');
add_action('init', 'plugin_missing'); // error: argument.type
add_filter('plugin_label', fn (string $l, int $a) => $l); // error: arguments.count
add_filter('plugin_label', fn (string $l, int $a, int $b) => $l, 10, 3);
add_filter('plugin_label', fn (string $l) => $l, 10, 2); // error: arguments.count
add_filter('plugin_label', fn (string $l, mixed ...$rest) => $l, 10, 4); // error: wordpress.hookArguments
