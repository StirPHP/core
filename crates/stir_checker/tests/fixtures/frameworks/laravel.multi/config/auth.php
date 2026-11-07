<?php

return [
    'providers' => [
        'users' => ['driver' => 'eloquent', 'model' => env('AUTH_MODEL', App\Models\User::class)],
    ],
];
