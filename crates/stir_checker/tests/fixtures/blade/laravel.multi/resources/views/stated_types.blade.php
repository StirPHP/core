<?php /** @var \App\Models\Post $post */ ?>
{{ $post->title }}
{{ $post->nothing }} // error: property.notFound
@php
    /** @var \App\Models\Author $author */
@endphp
{{ $author->name }}
{{ $author->nothing }} // error: property.notFound
@php
    /** @var list<\App\Models\Post> $posts */
    $first = $posts[0];
@endphp
{{ $first->nothing }} // error: property.notFound
@foreach ($posts as $each)
    {{ $each->nothing }} // error: property.notFound
@endforeach
{{ $seen }}
<?php /** @var \App\Models\Post $seen */ ?>
{{ $seen->nothing }} // error: property.notFound
<?php /** @var \App\Models\Nope $nope */ ?> // error: class.notFound
