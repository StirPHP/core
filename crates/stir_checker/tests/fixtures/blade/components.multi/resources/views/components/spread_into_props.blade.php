@props(['given', 'other'])
{{ $given->anything }} {{ $other->anything }}
{{ $not_a_prop }} // error: variable.undefined
