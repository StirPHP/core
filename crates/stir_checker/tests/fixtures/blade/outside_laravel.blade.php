{{-- no Laravel here: none of it is an error, or any type --}}
{{ route('home') }} {{ new App\Thing }} {{ Str::upper($given) }} {{ SOME_CONSTANT }}
{{ $errors->anything() }} {{ $__env->anything() }} {{ $slot->anything }}
@foreach ($given as $x)
    {{ $loop->anything }} {{ $x->y }}
@endforeach
{{ strlen('a', 'b') }} // error: arguments.count
@error('x') {{ $message }} @enderror
