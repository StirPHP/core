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
{{-- what a query added to a model is read like this, but a method that isn't there is a typo --}}
{{ (new \App\Models\Magic)->comments_count }}
{{ (new \App\Models\Magic)->nothing() }} // error: method.notFound
