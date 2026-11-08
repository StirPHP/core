@use('App\Models\Metrics')
<input @error('title') class="bad" @enderror>
@error('title')
    {{ $message }}
    {{ $message->nothing }} // error: property.nonObject
@enderror
{{ $message->anything }}
@error('title', 'login')
    {{ strlen($message) }}
@else
    {{ $message->anything }}
@enderror
@session('status')
    {{ $value }} {{ $value->anything }}
@endsession
{{ $value->anything }}
@context('key')
    {{ $value->anything }}
@endcontext
@inject('metrics', 'App\Models\Metrics')
{{ $metrics->total() }}
{{ $metrics->nothing() }} // error: method.notFound
@inject('other', Metrics::class)
{{ $other->total() }}
{{ $other->nothing() }} // error: method.notFound
@inject('bound', 'some.container.name')
{{ $bound->anything() }}
{{ $errors->any() }} {{ $errors->first('x') }} {{ $errors->login->first('y') }} {{ $errors->has('x') }}
{{ $errors->getBag('x')->nothing() }} // error: method.notFound
{{ $__env->yieldContent('x') }}
{{ $__env->nothing() }} // error: method.notFound
@isset($errors)
    {{ $errors->any() }}
@endisset
{{ $errors ?? 'none' }}
{{-- only a component has these for sure: here they're names like any other --}}
{{ $slot->anything }} {{ $attributes->anything }} {{ $component->anything }} {{ $exception->anything }}
