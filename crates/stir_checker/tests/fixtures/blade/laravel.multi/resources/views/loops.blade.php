@foreach ($posts as $post)
    {{ $loop->index + 1 }} {{ $loop->iteration }} {{ $loop->remaining }} {{ $loop->count }}
    {{ $loop->first ? 'a' : 'b' }} {{ $loop->last }} {{ $loop->even }} {{ $loop->odd }} {{ $loop->depth }}
    {{ $loop->nothing }} // error: property.notFound
    {{ $loop->parent->iteration }}
    @foreach ($post->comments as $comment)
        {{ $loop->parent->iteration }}
        {{ $loop->parent->nothing }} // error: property.notFound
        {{ $loop->parent->parent->anything }}
    @endforeach
    {{ $loop->nothing }} // error: property.notFound
@endforeach
{{ $loop->notALoopHere }}
@forelse ($posts as $post)
    {{ $loop->index }}
    {{ $loop->nothing }} // error: property.notFound
@empty
    {{ $loop->notALoopHere }}
@endforelse
<?php foreach ($posts as $post): ?>
    {{ $loop->notALoopHere }}
<?php endforeach; ?>
@for ($i = 0; $i < 3; $i++)
    {{ $loop->notALoopHere }}
@endfor
@while ($more)
    {{ $loop->notALoopHere }}
@endwhile
