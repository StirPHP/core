<?php

namespace App\Models;

use Illuminate\Database\Eloquent\Builder;
use Illuminate\Database\Eloquent\Factories\HasFactory;
use Illuminate\Database\Eloquent\Model;
use Illuminate\Database\Eloquent\Relations\HasMany;

class User extends Model
{
    use HasFactory;

    public function id(): int { return 1; }

    // says just the class: what's in it is what it returns
    public function posts(): HasMany
    {
        return $this->hasMany(Post::class);
    }

    public function drafts(): HasMany
    {
        return $this->hasMany(Post::class)->latest();
    }

    // as it's read: through a variable, and through a method of its own
    public function viaVariable(): HasMany
    {
        $relation = $this->hasMany(Post::class);

        return $relation;
    }

    public function viaMethod(): HasMany
    {
        return $this->posts()->latest();
    }

    // one of them, for all it's made from a relation of many
    public function latestPost(): \Illuminate\Database\Eloquent\Relations\HasOne
    {
        return $this->posts()->one();
    }

    public function scopeActive(Builder $query): Builder
    {
        return $query->verified();
    }

    public function scopeVerified(Builder $query): Builder
    {
        $query->nope(); // error: method.notFound
        return $query;
    }

    public function createdPost(): ?Post
    {
        return $this->posts()->latest()->first();
    }
}

/**
 * A model of a package's factory, by `@use`.
 *
 * @use HasFactory<\Database\Factories\AuthorFactory>
 */
class Author extends User
{
    use HasFactory;
}
