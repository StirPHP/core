<?php declare(strict_types=1);
// Control-flow narrowing in plain PHP.

abstract class Shape {}
final class Circle extends Shape
{
    public function radius(): float { return 1.0; }
}
final class Square extends Shape
{
    public function side(): float { return 1.0; }
}

function by_instanceof(Shape $s): float {
    if ($s instanceof Circle) {
        return $s->radius();
    }
    if ($s instanceof Square) {
        return $s->side();
    }
    return $s->radius(); // error: method.notFound
}

function by_is_functions(int|string|null $v): int {
    if (is_int($v)) {
        return $v;
    }
    if (is_string($v)) {
        return strlen($v);
    }
    return 0;
}

function by_negation(int|string $v): int {
    if (!is_string($v)) {
        return $v;
    }
    return strlen($v);
}

function by_match(int|string $v): string {
    return match (true) {
        is_int($v) => "int",
        default => $v,
    };
}

function by_match_arm(int|string $v): int {
    return match (true) { // error: return.type
        is_int($v) => $v,
        is_string($v) => $v,
    };
}

function by_match_object(Throwable|string $v): string {
    return match (true) {
        $v instanceof Exception => $v->getMessage(),
        $v instanceof Error => $v->getMessage(),
        is_string($v) => $v,
        default => $v->getMessage(),
    };
}

function by_match_value(int|string|null $v): int|string {
    return match ($v) {
        null => 0,
        default => $v,
    };
}

function by_assignment(?string $s): string {
    $s = $s ?? "fallback";
    return $s;
}

function by_loop_guard(array $rows): void {
    foreach ($rows as $row) {
        if ($row === null) {
            continue;
        }
        echo $row;
    }
}

function widened_after_branch(bool $b): string {
    $v = "a";
    if ($b) {
        $v = 1;
    }
    return $v; // error: return.type
}
