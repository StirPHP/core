<?php declare(strict_types=1);

function php_asserted(Failure|string $v): string {
    if (is_failure($v)) {
        return $v->message();
    }
    return $v;
}

function php_asserted_false(Failure|string $v): string {
    if (is_success($v)) {
        return $v;
    }
    return $v->message();
}

function php_not_narrowed_without_the_call(Failure|string $v): string {
    return $v; // error: return.type
}

function php_conditional_return(string $a): int {
    return positive($a);
}

function php_shapes(): void {
    register_thing(["apply" => "strlen", "name" => "a"]);
    register_thing(["name" => "a"]);
    register_thing(["name" => 1]); // error: argument.type
    register_thing(["apply" => "strlen"]); // error: argument.type
}

function php_callable_strings(): void {
    takes_callable("strlen");
}
