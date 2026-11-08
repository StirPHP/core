@php
    /** @var \App\Models\Post|null $post */
    $post = \App\Models\post();
@endphp
{{ $post->title }} // error: property.nonObject
@if ($post)
    {{ $post->title }}
@endif
@unless ($post)
    {{ $post->title }} // error: property.nonObject
@else
    {{ $post->title }}
@endunless
@isset($post)
    {{ $post->title }}
@endisset
@empty($post)
    {{ $post->title }} // error: property.nonObject
@else
    {{ $post->title }}
@endempty
@foreach ($items as $item)
    @break($post === null)
    {{ $post->title }}
    @continue(! $post)
@endforeach
@if ($given instanceof \App\Models\Post)
    {{ $given->title }}
    {{ $given->nothing }} // error: property.notFound
@endif
{{ $given->anything }}
@isset($other)
    {{ $other->anything }}
@endisset
@empty($another)
    {{ $another }}
@endempty
@if ($post === null)
    a
@elseif ($post->title)
    b
@endif
@switch($post->title ?? '')
    @case('a')
        @if ($post === null) c @endif
        @break
    @default
        d
@endswitch
