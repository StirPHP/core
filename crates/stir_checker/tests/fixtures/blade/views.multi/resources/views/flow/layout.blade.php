{{ $bodyClass }}
{{ $author->wrong() }} // error: method.notFound
{{ $assigned_in_child->wrong() }} // error: method.notFound
{{ $not_given_anywhere }} // error: variable.undefined
@yield('content')
