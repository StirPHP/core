<?php
// @framework laravel
// @level 9

namespace App\Models;

use Illuminate\Database\Eloquent\Builder;
use Illuminate\Database\Eloquent\Model;
use Illuminate\Database\Eloquent\SoftDeletes;

class Post extends Model
{
    use SoftDeletes;

    protected $casts = ['published_at' => 'datetime', 'flags' => 'array'];

    public function author()
    {
        return $this->belongsTo(User::class, 'user_id');
    }

    /** @param Builder<Post> $query */
    public function scopePublished(Builder $query, int $year): void {}

    public function getSlugAttribute(): string
    {
        return strtolower($this->title);
    }
}
