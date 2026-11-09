<?php
namespace App\Http;

class Pages
{
    public function home(int $count)
    {
        return view('pages.home', ['count' => $count]);
    }

    public function elsewhere(int $count)
    {
        return view('elsewhere.panel', ['count' => $count]);
    }
}
