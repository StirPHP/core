@section('title', $title->anything)
@include('partials.card', ['card' => new Nothing]) // error: class.notFound
@class(['a' => strlen(1, 2, 3)]) // error: arguments.count
@json(nothing()) // error: function.notFound
@if (nothing()) // error: function.notFound
@endif
@foreach (nothing() as $x) // error: function.notFound
@endforeach
@can('update', new Nothing) // error: class.notFound
@endcan
@unset($title)
{{ $title->anything }}
@php($set = new Nothing) // error: class.notFound
@dd(nothing()) // error: function.notFound
