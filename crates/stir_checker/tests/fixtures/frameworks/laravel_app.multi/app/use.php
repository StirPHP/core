<?php
declare(strict_types=1);

namespace App;

use App\Models\NoFactory;
use App\Models\Author;
use App\Models\Post;
use App\Models\User;
use Database\Factories\UserFactory;
use Illuminate\Database\Eloquent\Builder;
use Illuminate\Database\Eloquent\Collection;
use Illuminate\Support\Facades\Route;

function wants_user(User $user): void {}
function wants_post(Post $post): void {}
/** @param Collection<int, User> $users */
function wants_users(Collection $users): void {}

function factories(?UserFactory $given): void
{
    // one, unless it's been asked for more
    wants_user(User::factory()->create());
    wants_user(User::factory()->make());
    wants_user(User::factory()->admin()->create());
    wants_user(UserFactory::new()->create());
    wants_user(($given ?? User::factory())->create());
    wants_post(User::factory()->create()); // error: argument.type
    User::factory()->create()->nope(); // error: method.notFound
    User::factory()->nope(); // error: method.notFound

    // or more
    wants_users(User::factory()->count(3)->create());
    wants_users(User::factory()->count(3)->moderator()->create());
    wants_users(UserFactory::times(3)->moderator()->create());
    wants_users(User::factory()->count(3)->admin()->create());
    wants_users(User::factory(3)->create());
    wants_users(User::factory()->createMany([[]]));
    wants_user(User::factory()->count(3)->createOne());
    wants_user(User::factory()->count(3)->create()); // error: argument.type
    wants_user(User::factory()->count(null)->create());
    wants_user(User::factory(['name' => 'a'])->create());

    // a factory's relations: `__call` makes these of what the model has
    User::factory()->hasPosts(3)->create();
    Post::factory()->forCommentable()->create();
    User::factory()->hasNope(3); // error: method.notFound
    User::factory()->forNope(); // error: method.notFound

    // what's known of a model with no factory by its name, or says so with `@use`
    NoFactory::factory()->create()->anything(); // error: method.notFound
    wants_post(Models\FromMethod::factory()->create());
    wants_post(Models\TypedFromMethod::factory()->create());
    wants_user(Author::factory()->create());

    // a package's macros
    User::factory()->fromPackage();
    User::factory()->fromMixin();
    User::factory()->protectedOfMixin();
    User::factory()->privateOfMixin(); // error: method.notFound
    Route::feeds();
    Route::nope(); // error: staticMethod.notFound
}

function relations(User $user): void
{
    wants_post($user->posts()->first());  // error: argument.type
    if ($post = $user->posts()->latest()->first()) {
        wants_post($post);
        $post->title();
        $post->nope(); // error: method.notFound
    }
    foreach ($user->posts()->get() as $post) {
        $post->title();
        $post->nope(); // error: method.notFound
    }
    // scopes of what's related, not of what's not
    $user->posts()->published()->first();
    $user->posts()->verified(); // error: method.notFound
    $user->drafts()->published()->first();

    wants_post($user->viaVariable()->first()); // error: argument.type
    $user->viaVariable()->first()?->nope(); // error: method.notFound
    $user->viaMethod()->first()?->nope(); // error: method.notFound
    foreach ($user->viaMethod as $post) {
        $post->nope(); // error: method.notFound
    }

    $user->latestPost?->title();
    $user->latestPost?->nope(); // error: method.notFound

    // what's related isn't known
    $commentable = $user->posts()->first()?->commentable()->first();
    $commentable?->anything(); // error: method.notFound
}

function any_builder(Builder $query): void
{
    // a query of something: any model's scope could be meant
    $query->verified()->published()->get();
    $query->nothingHasThis(); // error: method.notFound
}

function optionals(?Post $post, mixed $anything, ?string $text): void
{
    optional($post)->title();
    optional($post)->nope(); // error: method.notFound
    optional($post)->title; // the model's attribute
    wants_string(optional($post)->title()); // error: argument.type
    optional($anything)->whatever();
    optional($anything)->whatever;
    optional()->whatever();
    optional($post, fn (Post $p) => $p->title());
}

function wants_string(string $s): void {}

function tapping(Post $post): void
{
    wants_post(tap($post)->title());
    tap($post)->nope(); // error: method.notFound
    tap($post, fn ($p) => 1)->title();
}

function views(): \Illuminate\View\View
{
    return view('a');
}

function views_as_contract(): \Illuminate\Contracts\View\View
{
    return view('a')->withErrors([]);
}

function view_factory(): \Illuminate\Contracts\View\Factory
{
    return view();
}

function view_is_not(): Post
{
    return view('a'); // error: return.type
}

function facade_view(): \Illuminate\View\View
{
    return \Illuminate\Support\Facades\View::make('a');
}

function alias(): void
{
    // Laravel's alias rather than the class of the `event` extension
    \Event::fake();
    \Event::nope(); // error: staticMethod.notFound
}

function http(): void
{
    // the client's calls give a response, unless it's asked not to wait for one
    \Illuminate\Support\Facades\Http::get('a')->json();
    \Illuminate\Support\Facades\Http::withToken('a')->get('a')->json();
    \Illuminate\Support\Facades\Http::get('a')->nope(); // error: method.notFound
    \Illuminate\Support\Facades\Http::async()->get('a')->json(); // error: method.notFound
    \Illuminate\Support\Facades\Http::async()->withToken('a')->get('a')->json(); // error: method.notFound
}

function static_variables(): void
{
    static $once;
    if ($once) {
        return;
    }
    $once = 1;

    static $kept = null;
    $kept ??= 1;

    static $count = 0;
    wants_string($count); // error: argument.type
}
