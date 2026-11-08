<?php

namespace App\Console;

use Illuminate\Foundation\Console\Kernel as ConsoleKernel;

class Kernel extends ConsoleKernel
{
    protected function commands(): void
    {
        // the kernel's own `command()`, as applications before Laravel 11 register them
        $this->command('inspire', function () {
            $this->comment('quote');
            $this->nope(); // error: method.notFound
        });
    }
}
