<?php
namespace App\Http;

class Pages
{
    public function home(int $count)
    {
        return view('pages.home', ['count' => $count]);
    }

    public function wired(\App\Container $app): void
    {
        $app->when(\App\View\Components\Required::class)->needs('$name')->give('a');
    }
}
