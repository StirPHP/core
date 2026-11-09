<?php
namespace App\View\Components;

use App\Post;
use Illuminate\View\Component;

class Layout extends Component
{
    /** a class's parameter is the container's to make, and the rest have something to fall back on */
    public function __construct(public Post $post, public ?string $title, public string $site = 'x')
    {
    }

    public function render()
    {
        return view('layouts_of_a_class.main');
    }
}
