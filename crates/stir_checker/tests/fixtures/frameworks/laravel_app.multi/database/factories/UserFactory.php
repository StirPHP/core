<?php

namespace Database\Factories;

use App\Models\Post;
use App\Models\User;
use Illuminate\Database\Eloquent\Factories\Factory;

// a model by its name, and nothing declaring it
class UserFactory extends Factory
{
    public function definition(): array { return []; }
    public function admin(): self { return $this; }
    public function moderator(): static { return $this; }
}

// a model by `$model`, as another name wouldn't lead to it
class WriterFactory extends Factory
{
    protected $model = User::class;
    public function definition(): array { return []; }
}

// a model by `@extends`
/** @extends Factory<User> */
class AuthorFactory extends Factory
{
    public function definition(): array { return []; }
}

class OddlyNamed extends Factory
{
    protected $model = Post::class;
    public function definition(): array { return []; }
}

class PostFactory extends Factory
{
    public function definition(): array
    {
        // it's `null` the first time: what's been kept is what it is after that
        static $slug;

        return ['slug' => $slug ?: $slug = 'a', 'author_id' => User::factory()->create()->id()];
    }
}
