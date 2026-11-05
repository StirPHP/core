<?php
// @level 1

// From level 1 they have to be declared with `@method` and `@property`.

/**
 * @method int declared()
 * @property int $declaredProperty
 */
class Magic {
    public function __get($name) { return 1; }
    public function __call($name, $args) { return 1; }
    public static function __callStatic($name, $args) { return 1; }

    public function own(): void {
        $this->anything(); // error: SPHP0009
        echo $this->anywhere; // error: SPHP0009
        $this->declared();
        echo $this->declaredProperty;
    }
}

function outside(Magic $m): void {
    // from level 2 it's anything, not just `$this`
    $m->anything();
    echo $m->anywhere;
    Magic::anything(); // error: SPHP0009
}
