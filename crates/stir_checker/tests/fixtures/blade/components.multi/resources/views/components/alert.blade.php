{{ $type }} {{ $limit + 1 }} {{ $shout }} {{ $withArgument(2) }} {{ $extra }}
{{ $type->nope() }} // error: method.nonObject
{{ $post?->title }} {{ $notInTheConstructor }}
{{ $slot }} {{ $slot->isEmpty() }} {{ $attributes->merge() }} {{ $attributes->nope() }} // error: method.notFound
{{ $heading }} {{ $heading->isEmpty() }} {{ $heading->nope() }} // error: method.notFound
{{ $userName }} // error: variable.undefined
{{ $options }} // error: variable.undefined
{{ $hiddenProperty }} // error: variable.undefined
{{ $hiddenMethod }} // error: variable.undefined
{{ $staticProperty }} // error: variable.undefined
{{ $render }} // error: variable.undefined
{{ $nothing }} // error: variable.undefined
{{ $errors->any() }} {{ $component }} {{ $__env }}
@include('partials.in_a_component')
