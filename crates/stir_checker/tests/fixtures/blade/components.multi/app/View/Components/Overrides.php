<?php
namespace App\View\Components;

use Illuminate\View\Component;

class Overrides extends Component
{
    public function data()
    {
        return ['anything' => 1];
    }

    public function render()
    {
        return view('components.overrides');
    }
}
