<x-spread_into_plain {{ $attributes }} />
<x-spread_into_props {{ $attributes->merge(['a' => 'b']) }} :given="1" />
