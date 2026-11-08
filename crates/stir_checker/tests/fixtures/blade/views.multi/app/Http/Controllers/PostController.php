<?php
namespace App\Http\Controllers;

use App\Models\Author;
use App\Models\Post;
use Illuminate\Support\Facades\View;

class PostController
{
    /** @param list<Post> $posts */
    public function index(array $posts)
    {
        return view('posts.index', ['posts' => $posts, 'title' => 'All', 'first' => $posts[0]]);
    }

    public function union(Post $post)
    {
        return view('posts.union', ['item' => $post, 'only_here' => 1]);
    }

    public function forms(Post $post, Author $author)
    {
        $one = 1;
        $two = 'two';
        if ($one > 2) {
            return view('kinds.compact', compact('post', 'one') + ['added' => $author]);
        }
        if ($one > 3) {
            return view('kinds.with', ['a' => $post])->with('b', $author)->with(['c' => $one])->withDeep($two);
        }
        if ($one > 4) {
            return View::make('kinds.make', ['made' => $post]);
        }
        if ($one > 5) {
            return view('kinds.merge', array_merge(['x' => $post], ['y' => $author]));
        }
        if ($one > 6) {
            $data = ['shaped' => $post];
            return view('kinds.shape', $data);
        }
        if ($one > 7) {
            return view('kinds.rendered', ['r' => $post])->render();
        }
        if ($one > 8) {
            return View::first(['kinds.first_a', 'kinds.first_b'], ['f' => $post]);
        }
        if ($one > 9) {
            return response()->view('kinds.response', ['resp' => $post]);
        }
        return view('kinds.conditional', ['maybe' => $one > 0 ? $post : $author]);
    }

    /** @param array<string, mixed> $elsewhere */
    public function opens(array $elsewhere, string $which, Post $post)
    {
        if ($which === 'a') {
            return view('open.elsewhere', $elsewhere);
        }
        if ($which === 'b') {
            return view('open.spread', [...$elsewhere, 'a' => $post]);
        }
        if ($which === 'c') {
            return view('open.interpolated.' . $which, ['a' => $post]);
        }
        if ($which === 'd') {
            return view("open.interpolated2.{$which}", ['a' => $post]);
        }
        if ($which === 'e') {
            $view = view('open.kept', ['a' => $post]);
            return $view;
        }
        if ($which === 'f') {
            return view('admin::panel', ['a' => $post]);
        }
        return view('open.keyed', [$which => $post]);
    }
}
