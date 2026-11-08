<?php
use App\Models\Post;
use Illuminate\Support\Facades\Route;
use Illuminate\Support\Facades\View;

Route::view('about', 'kinds.route', ['page' => new Post]);
Route::view('plain', 'kinds.route_plain');
View::composer('open.composed', fn ($view) => $view->with('from_composer', 1));
View::composer(['open.composed_list'], 'App\Composers\Thing');
View::composer('open.wild.*', fn ($view) => null);
