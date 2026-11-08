<?php
namespace App\Http\Controllers;

use App\Models\Author;

class OtherController
{
    public function union(Author $author)
    {
        return view('posts.union', ['item' => $author]);
    }

    public function flow(Author $author)
    {
        return view('flow.parent', ['author' => $author]);
    }

    public function child(Author $author)
    {
        return view('flow.child', ['author' => $author]);
    }

    public function directives()
    {
        view('directives.declares')->render();
        view('directives.outputs')->render();
        view('directives.unread')->render();
        return view('flow.from_error_page');
    }

    public function recursive()
    {
        view('recursive.tree', ['node' => 1])->render();
        return view('recursive.a', ['node' => 1]);
    }
}
