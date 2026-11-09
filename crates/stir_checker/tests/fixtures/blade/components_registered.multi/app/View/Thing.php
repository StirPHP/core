<?php
namespace App\View;

class Thing extends \Illuminate\View\Component
{
    public function __construct(public int $size = 0) {}

    private function template() { return view('widgets.thing'); }

    public function render() { return $this->template(); }
}
