@use('App\Models\Post')
@use('App\Models\Author', 'Writer')
@use(App\Models\Metrics)
@use('function App\Models\helper')
@use('const App\Models\LIMIT')
{{ new Nothing }} // error: class.notFound
{{ new Post }}
{{ new Writer }}
{{ new Metrics }}
{{ helper() }}
{{ LIMIT }}
{{ (new Post)->nothing }} // error: property.notFound
<?php use App\Models\Post as Article; ?>
{{ (new Article)->nothing }} // error: property.notFound
