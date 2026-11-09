@props(['title', 'kind' => 'info', 'post' => null, 'size' => 1, 'never' => 'text', 'none' => null, 'absent'])
{{ $title }} {{ $title->nope() }} // error: method.nonObject
{{ $kind }} {{ $kind->nope() }} // error: method.notFound
{{ $post->title }} {{ $post->nope }} // error: property.notFound
{{ $size }} {{ $never->nope() }} // error: method.nonObject
{{ $none }} {{ $none->anything }} {{ $absent }}
{{ $class }} // error: variable.undefined
{{ $dataId }}
{{ $nothing }} // error: variable.undefined
{{ $slot }} {{ $attributes }}
