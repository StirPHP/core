<?php

namespace App\Models;

use Illuminate\Database\Eloquent\Builder;
use Illuminate\Database\Eloquent\Factories\HasFactory;
use Illuminate\Database\Eloquent\Model;
use Illuminate\Database\Eloquent\Relations\MorphTo;

class Post extends Model
{
    use HasFactory;

    public function title(): string { return ''; }

    public function scopePublished(Builder $query): Builder
    {
        return $query;
    }

    public function commentable(): MorphTo
    {
        return $this->morphTo();
    }
}

class FromMethod extends Post
{
    protected static function newFactory()
    {
        return \Database\Factories\OddlyNamed::new();
    }
}

class TypedFromMethod extends Post
{
    protected static function newFactory(): \Database\Factories\OddlyNamed
    {
        return \Database\Factories\OddlyNamed::new();
    }
}

class NoFactory extends Model
{
    use HasFactory;
}
