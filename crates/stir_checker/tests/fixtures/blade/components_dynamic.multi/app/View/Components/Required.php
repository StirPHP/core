<?php
namespace App\View\Components;

class Required extends \Illuminate\View\Component
{
    public function __construct(public string $name) {}

    public function render() { return view('components.required'); }
}
