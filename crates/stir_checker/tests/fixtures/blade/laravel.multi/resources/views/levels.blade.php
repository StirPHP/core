{{-- every one of these is fine at the highest level there is --}}
{{ strlen($given) }}
{{ $given->x->y->z() + 1 }}
@foreach ($given as $k => $v)
    {{ $v[$k]->x }}
@endforeach
@php
    $a = $given->x;
    $b = $a ?? null;
    $given['x'] = 1;
    $another[] = 1;
    $more .= 'a';
    $counter++;
@endphp
{{ new $given }}
{{ $given::class }}
{{ $given() }}
