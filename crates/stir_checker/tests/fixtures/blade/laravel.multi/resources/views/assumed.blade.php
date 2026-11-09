{{-- what's a user's depends on the route this is for, and there's no saying so here --}}
{{ \Illuminate\Support\Facades\Auth::user()->name }}
{{ \Illuminate\Support\Facades\Auth::user()->nothing }} // error: property.notFound
{{ str_replace('_', ' ', $given) }}
{{ ucfirst(str_replace('_', ' ', $given)) }}
{{ (new \App\Models\Post)->publish(1, 2) }}
{{ strlen('a', 'b') }} // error: arguments.count
@foreach ((new \App\Models\WithThing)->things() as $thing)
    {{ $thing->title }}
    {{ $thing->nothing }} // error: property.notFound
@endforeach
{{-- it's an object of no model's: what `__get` has is as much in question as anywhere else --}}
{{ (new \App\Models\Magic)->comments_count }} // error: property.notFound
{{-- a named form's messages --}}
{{ $errors->login->first('email') }}
{{ (new \App\Models\Magic)->nothing() }} // error: method.notFound
