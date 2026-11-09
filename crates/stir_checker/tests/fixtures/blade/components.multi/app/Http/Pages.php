<?php
namespace App\Http;

use App\Post;

class Pages
{
    public function home(Post $post, int $count)
    {
        return view('pages.home', ['post' => $post, 'count' => $count, 'name' => 'x']);
    }
}
