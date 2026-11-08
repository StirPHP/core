@props(['title'])
{{ $title->anything }}
{{ $attributes->merge([]) }}
{{ $attributes->nothing() }} // error: method.notFound
