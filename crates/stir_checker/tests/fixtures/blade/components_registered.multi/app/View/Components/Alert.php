<?php
namespace App\View\Components;

class Alert extends \Illuminate\View\Component
{
    public function __construct(public int $count = 0) {}

    public function render() { return view('components.alert'); }
}
