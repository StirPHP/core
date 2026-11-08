{{ $a->title }} {{ $b->name }} {{ $c + 1 }} {{ strtoupper($deep) }}
{{ $b->title }} // error: property.notFound
{{ $d }} // error: variable.undefined
