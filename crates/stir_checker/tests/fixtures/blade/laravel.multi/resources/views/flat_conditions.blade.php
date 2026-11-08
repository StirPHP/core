@php
    $post = new \App\Models\Post;
    $maybe = new \App\Models\Post;
    $inCustom = new \App\Models\Post;
@endphp
@auth
    @php $post = null; @endphp
@endauth
{{ $post?->title }}
{{ $post->title }} // error: property.nonObject
@admin
    @php $inCustom = null; @endphp
@else
    @php $inCustom = 1; @endphp
@endadmin
@guest('web')
    a
@else
    @php $maybe = null; @endphp
@endguest
{{ $inCustom === 1 ? 'a' : 'b' }}
{{ $maybe === null ? 'a' : 'b' }}
@foreach ($xs as $x)
    @auth
        @break
    @endauth
    @can('see', $x)
        @continue
    @endcan
    {{ $x }}
@endforeach
@auth
    @php \App\Models\abort(403); @endphp
@endauth
<p>still here</p>
@admin
    @php return; @endphp
@endadmin
<p>and here</p>
@php $always = true; @endphp
@if ($always)
    no complaint about that: what's in a condition of the project's own can't all be seen
@endif
@hasSection('x')
    @php $a = 1; @endphp
@endif
{{ $a ?? 0 }}
