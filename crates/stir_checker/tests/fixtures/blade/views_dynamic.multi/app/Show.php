<?php
namespace App;

class Show
{
    public function named(int $given)
    {
        return view('named', ['given' => $given]);
    }

    public function worked_out(string $name)
    {
        return view($name);
    }
}
