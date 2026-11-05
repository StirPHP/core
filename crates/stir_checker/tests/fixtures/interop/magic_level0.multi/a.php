<?php
// @level 0

// Below level 1, what `__call`, `__get` and `__callStatic` take on goes unreported.

class Magic {
    public function __get($name) { return 1; }
    public function __call($name, $args) { return 1; }
    public static function __callStatic($name, $args) { return 1; }

    public function own(): void {
        $this->anything();
        echo $this->anywhere;
    }
}

class NotMagic {
    public function own(): void {
        $this->nothing(); // error: method.notFound
        echo $this->nowhere; // error: property.notFound
    }
}

function outside(Magic $m, NotMagic $n): void {
    $m->anything();
    echo $m->anywhere;
    Magic::anything();
    // below level 2 only `$this` is looked at
    $n->nothing();
}
