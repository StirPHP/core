<?php
namespace App\View\Components\Forms;

use Illuminate\View\Component;

class Input extends Component
{
    public function __construct(public string $name, public int $maxLength = 0)
    {
    }

    public function render()
    {
        return view('components.forms.input');
    }
}
