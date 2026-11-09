<?php
namespace App;

class Container
{
    public function when($a) { return $this; }
    public function needs($a) { return $this; }
    public function give($a) { return $this; }
}
