<?php
declare(strict_types=1);

namespace App;

use App\Models\Post;
use App\Models\User;
use Illuminate\Support\Facades\Cache;
use Illuminate\Support\Facades\Theme;

function wants_int(int $n): void {}
function wants_string(string $s): void {}

function columns(Post $post): void
{
    wants_int($post->views);
    wants_int($post->title); // error: argument.type
    wants_string($post->published_at); // error: argument.type
    wants_int($post->user_id);
    wants_string($post->slug);
    wants_string($post->flags); // error: argument.type
    wants_string($post->body); // error: argument.type
    $post->nope();  // error: method.notFound
}

function relations(Post $post, User $user): void
{
    wants_string($post->author); // error: argument.type
    wants_string($user->posts); // error: argument.type
    $user->posts()->published(2024)->first();
    $user->posts()->published('x'); // error: argument.type
    $user->posts()->orderBy('title')->where('a')->count();
}

function queries(): void
{
    $post = Post::where('title', 'x')->published(2024)->first();
    wants_string($post); // error: argument.type
    $post?->author;
    Post::query()->whereTitle('x')->get();
    Post::withTrashed()->published(1);
    Post::query()->withTrashed();
    Post::notAScope(); // error: staticMethod.notFound
    Post::query()->chunk(10, function (\Illuminate\Database\Eloquent\Collection $posts): void {});
}

function facades(): void
{
    Cache::remember('k', 1, fn () => 1);
    wants_int(Theme::name()); // error: argument.type
    Theme::nope(); // error: staticMethod.notFound
}

function auth_user(): void
{
    $user = auth()->user();
    wants_string($user); // error: argument.type
    auth()->logout();
}
