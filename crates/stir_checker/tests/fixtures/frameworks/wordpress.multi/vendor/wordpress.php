<?php

class wpdb
{
    public string $prefix = 'wp_';
    public function get_var(?string $query = null): ?string { return null; }
}

class WP_Post
{
    public int $ID = 0;
}

/** @param callable $callback */
function add_action(string $hook_name, $callback, int $priority = 10, int $accepted_args = 1): bool { return true; }

/** @param callable $callback */
function add_filter(string $hook_name, $callback, int $priority = 10, int $accepted_args = 1): bool { return true; }

function do_action(string $hook_name, mixed ...$arg): void {}

function apply_filters(string $hook_name, mixed $value, mixed ...$args): mixed { return $value; }
