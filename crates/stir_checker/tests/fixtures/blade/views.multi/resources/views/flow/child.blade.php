@extends('flow.layout', ['bodyClass' => 'home'])
@php $assigned_in_child = new \App\Models\Post; @endphp
@section('content') {{ $author->name }} @endsection
