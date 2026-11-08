<?php
namespace App;

class Show
{
    public function named(int $given)
    {
        \Illuminate\Support\Facades\View::share('from_share', 1);
        return view('named', ['given' => $given]);
    }
}
