🎉 <x-alert type="danger" :limit="$count" :post="$post" user-name="a">
    {{ $post->title }} {{ $post->nope }} // error: property.notFound
    <x-slot:heading class="b">{{ $count }} {{ $not_in_this_page }} // error: variable.undefined
    </x-slot:heading>
    <x-slot name="other" :class="$post->nope"></x-slot> // error: property.notFound
</x-alert>
<x-alert type="danger" :limit="[1]" /> // error: argument.type
<x-alert type="danger" limit="many" /> // error: argument.type
<x-alert type="danger" limit="5" />
<x-alert type="danger" :limit="$name" />
<x-alert type="danger" :post="$count" /> // error: argument.type
<x-alert type="danger" :user-name="[1]" /> // error: argument.type
<x-alert type="danger" :options="'text'" /> // error: argument.type
<x-alert type="danger" options /> // error: argument.type
<x-alert :type="$post" /> // error: argument.type
<x-alert /> // error: arguments.count
<x-alert class="no matter" unknown="no matter" :other="[1]" type="danger" />
<x-forms.input name="a" :max-length="[1]" /> // error: argument.type
<x:forms.input :$name />
<x-layout :$post><x-slot:header>{{ $count }}</x-slot:header></x-layout>
<x-plain color="red" :count="$count" data-id="1" disabled wire:click="go" @click="go" ::thing="not.php + +" x-on:click="$dispatch('a', { b })">
    <x-slot:footer>a</x-slot:footer>
</x-plain>
<x-card :title="$name" :post="$post" class="c" data-id="1" />
<x-card title="d" :kind="$post" size="2" :post="$post"></x-card>
<x-tabs :active="$name" /><x-nested :deep="1" />
<x-spreading />
<x-aware_child />
<x-card @class(['a' => $post->nope]) @style(['b' => $count]) title="{{ $post->nope }}" data="{!! $not_here !!}" /> // error: property.notFound, variable.undefined
<x-pkg::thing :attr="$count" other="b" /> <x-pkg::thing :attr="$post->nope" /> // error: property.notFound
<x-heroicon-o-check class="a" :missing="[1]" />
@component('components.boxed', ['a' => 1])
    @slot('s') b @endslot
    @slot('v', 'text')
@endcomponent
<x-slot_worked_out><x-slot :name="$name"></x-slot></x-slot_worked_out>
{{ $not_on_this_page }} // error: variable.undefined
<x-recursive />
@include('components.rendered_openly', (array) $post)
<x-late_props a="1" /> <x-props_of_a_variable a="1" />
@if ($count)
    <x-card title="e" :post="$post"> {{ $not_here_either }} // error: variable.undefined
    </x-card>
@endif
<x-with_slot_in_a_condition>
    @if ($count) <x-slot:maybe>{{ $count }}</x-slot:maybe> @endif
</x-with_slot_in_a_condition>
