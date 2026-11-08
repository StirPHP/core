<?php

use Illuminate\Support\Facades\Artisan;

// a closure command runs bound to the command Laravel makes of it
Artisan::command('inspire', function () {
    $this->comment('quote');
    $this->info('hello');
    $this->argument('name');
    $this->nope(); // error: method.notFound
})->purpose('Display an inspiring quote');

// unless it's static: PHP can't bind an object to it
Artisan::command('static', static function () {
    $this->comment('quote'); // error: variable.undefined
});

Artisan::command('arrow', fn () => $this->comment('quote'));

Artisan::call('inspire');

function plain(): void
{
    (function () {
        $this->comment('quote'); // error: variable.undefined
    })();
}
