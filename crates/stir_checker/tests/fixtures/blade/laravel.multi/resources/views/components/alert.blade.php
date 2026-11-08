@props(['type' => 'info', 'message', 'size' => $defaultSize])
@aware(['color' => 'gray'])
<div {{ $attributes->merge(['class' => $type]) }}>
    {{ $attributes->nothing() }} // error: method.notFound
    @foreach ($attributes as $name => $value)
        {{ $name }}
    @endforeach
    {{ $slot }} {{ $slot->isEmpty() }}
    {{ $slot->nothing() }} // error: method.notFound
    {{ $component->anyMethodOfItsClass() }}
    {{ $type->anything }} {{ $message->anything }} {{ $size->anything }} {{ $color->anything }}
    @if ($type === 'info')
        info
    @endif
</div>
