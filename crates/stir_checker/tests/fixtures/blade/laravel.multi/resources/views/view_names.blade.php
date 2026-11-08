@include('partials.card')
@include('partials/card')
@include('partials.nope') // error: view.notFound
@includeIf('partials.nope')
@includeWhen($c, 'partials.card')
@includeWhen($c, 'partials.nope') // error: view.notFound
@includeUnless($c, 'partials.nope') // error: view.notFound
@includeFirst(['partials.nope', 'partials.card'])
@extends('partials.nope') // error: view.notFound
@each('partials.card', $cards, 'card')
@each('partials.nope', $cards, 'card') // error: view.notFound
@each('partials.card', $cards, 'card', 'partials.nope') // error: view.notFound
@each('partials.card', $cards, 'card', 'raw|Nothing here')
@component('partials.nope') // error: view.notFound
@endcomponent
@component('App\View\Components\Alert')
@endcomponent
@include('mail::message')
@include($dynamic)
@include("partials.$dynamic")
@include('partials.' . $dynamic)
