{{ $node->anything() }} {{ $unknown }}
@include('recursive.a', ['node' => 3])
@include('recursive.after', ['given' => 1])
