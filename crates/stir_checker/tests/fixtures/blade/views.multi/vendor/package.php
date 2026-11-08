<?php
namespace Package;

trait Pages
{
    public function page()
    {
        return view('package.page');
    }

    public function reset(string $token)
    {
        return view('package.reset')->with(['token' => $token]);
    }

    /** @param array<string, mixed> $data */
    public function unread(array $data)
    {
        return view('package.unread', $data);
    }
}
