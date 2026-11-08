<?php
namespace App\Livewire;

class Counter
{
    public int $count = 0;

    public function render()
    {
        return view('open.livewire');
    }
}
