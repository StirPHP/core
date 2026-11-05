<?php
// @level 5
class Magic
{
    private string $hidden = '';
    public function __get($name)
    {
        return $this->$name;
    }
    private function secret(): void {}
    public function __call($name, $args) {}
}

class Plain
{
    private string $hidden = '';
    private function secret(): void {}
}

trait Helper
{
    private function help(): void {}
}

class UsesHelper
{
    use Helper;
    public function run(): void
    {
        $this->help();
    }
}

class SubHelper extends UsesHelper {}

echo (new Magic())->hidden;
(new Magic())->secret();
echo (new Plain())->hidden; // error: property.private
(new Plain())->secret(); // error: method.private

function wants_strings(array $a, string $s): void {}
/** @param string[]|string $v */
function wants_doc_strings($v): void {}
/** @param array{a?: bool} $o */
function wants_shape($o): void {}

function loose(array $plain): void
{
    wants_doc_strings($plain);
    wants_doc_strings((array) 'x');
    wants_shape(array('a' => true));
}
