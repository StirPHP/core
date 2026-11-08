{{ $item->name }}
{{ $item->publish() }} // error: method.notFound
{{ $only_here }}
{{ $only_here + 1 }}
{{ strtoupper($only_here ?? 'none') }}
{{ $not_anywhere }} // error: variable.undefined
