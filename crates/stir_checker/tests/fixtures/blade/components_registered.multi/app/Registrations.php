<?php
namespace App;

use App\View\Components\Alert;
use Illuminate\Support\Facades\Blade;

class Registrations
{
    public function boot(): void
    {
        Blade::component('aliased', Alert::class);
        Blade::component(View\Thing::class);
        Blade::component('widgets.real', 'short');
        Blade::components(['listed' => Alert::class]);
        Blade::componentNamespace('App\\Pkg\\Components', 'pkg');
        Blade::anonymousComponentNamespace('flights', 'fly');
        Blade::anonymousComponentPath(resource_path('views/elsewhere'), 'else');
    }
}
