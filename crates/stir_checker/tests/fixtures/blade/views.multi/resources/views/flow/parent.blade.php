@php $assigned = new \App\Models\Post; @endphp
@include('flow.included', ['explicit' => $author])
@include('flow.isolated', ['explicit' => $author])
@includeWhen($author, 'flow.when', ['w' => $author])
@includeIf('flow.if')
@includeFirst(['flow.if2', 'flow.if3'], ['z' => 1])
@includeUnless($author, 'flow.unless', ['u' => 1])
@includeIsolated('flow.isolated2', ['explicit' => $author])
@foreach ([new \App\Models\Post] as $binding)
    @include('flow.in_loop')
@endforeach
@each('flow.item', [new \App\Models\Post], 'entry', 'flow.none')
@component('flow.as_component', ['x' => 1]) slot @endcomponent
