{{ $title }}
@foreach ($posts as $post)
    {{ $post->title }}
    {{ $post->wrong() }} // error: method.notFound
@endforeach
{{ $first->wrong() }} // error: method.notFound
{{ $nothing }} // error: variable.undefined
{{ $errors->any() }} {{ $__env->yieldContent('x') }} {{ $slot ?? '' }} {{ $app }}
{{ $tested ?? 'fine' }}
@isset($tested2)
    {{ $tested2 }}
@endisset
@empty($tested3)
    nothing
@endempty
{{ isset($tested4) ? $tested4 : '' }}
{{ empty($tested5) ? '' : $tested5 }}
@php
    preg_match('/a/', 'a', $matches);
    $own = 1;
@endphp
{{ $matches[0] }} {{ $own }}
{{ (fn () => $title)() }}
{{ (fn () => $nothing_in_arrow)() }} // error: variable.undefined
@php
    $late = function () use ($title) { return $title; };
    $wrong = function () use ($nothing_used) { return 1; }; // error: variable.undefined
@endphp
