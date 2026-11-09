<?php
namespace App\Pkg\Components;

class Button extends \Illuminate\View\Component
{
    public function __construct(public int $size = 0) {}

    public function render() { return view('components.button_of_pkg'); }
}
