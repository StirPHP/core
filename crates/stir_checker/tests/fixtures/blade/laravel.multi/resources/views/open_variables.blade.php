{{-- What nothing declares comes from whoever renders this, and holds whatever they give it. --}}
<h1>{{ $title }}</h1>
{{ $user->profile->avatar() }}
{{ $user['name']['first'] }}
{{ $count + 1 }}
{{ $count . 'a' }}
{!! $html !!}
@foreach ($posts as $post)
    {{ $post->author->name->anything() }}
    @foreach ($post->comments as $key => $comment)
        {{ $key }} {{ $comment->body }}
    @endforeach
@endforeach
{{ $posts->map(fn ($p) => $p->id == $current->id) }}
@php
    $fn = function ($x) use ($prefix) {
        return $prefix . $x;
    };
    $total = $subtotal + $tax;
@endphp
{{ $this->computed }}
{{ $late }}
@php $late = 1; @endphp
{{ $late }}
@php
    $inner = function () {
        return $nobody; // error: variable.undefined
    };
@endphp
