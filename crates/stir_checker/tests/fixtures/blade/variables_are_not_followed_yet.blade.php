<h1>{{ $title }}</h1>
@foreach ($posts as $post)
    {{ $post->title->nothing() }}
@endforeach
