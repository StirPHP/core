<?php declare(strict_types=1);
// Null safety in plain PHP.

class Account
{
    public ?string $nick = null;
    public string $name = "";

    public function label(): string
    {
        return $this->name;
    }

    public function next(): ?Account
    {
        return null;
    }
}

function wants_string(string $v): void {}

function nullable_argument(?string $s): void {
    wants_string($s); // error: SPHP0005
    if ($s !== null) {
        wants_string($s);
    }
    wants_string($s ?? "");
}

function nullable_member_access(?Account $a): string {
    return $a->label(); // error: SPHP0012
}

function nullable_member_guarded(?Account $a): string {
    if ($a === null) {
        return "";
    }
    return $a->label();
}

function nullable_member_nullsafe(?Account $a): ?string {
    return $a?->label();
}

function nullable_property_read(?Account $a): void {
    $a->nothing ?? null;
    isset($a->nothing);
}

function nullable_return(?string $s): string {
    return $s; // error: SPHP0005
}

function early_return(?Account $a): string {
    if ($a === null) {
        return "";
    }
    return $a->label();
}

function property_is_nullable(Account $a): string {
    return $a->nick; // error: SPHP0005
}

function nullsafe_chain_stays_nullable(Account $a): string {
    return $a->next()?->label(); // error: SPHP0005
}
