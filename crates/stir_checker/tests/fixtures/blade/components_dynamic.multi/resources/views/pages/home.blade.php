<x-dynamic-component :component="$count" :anything="1"><x-slot:title></x-slot:title></x-dynamic-component>
<x-required />
<x-anon />
{{ $not_given }} // error: variable.undefined
