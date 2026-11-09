<?php

it('has what the folder says, by another way of saying it', function () {
    $this->unitOnly();
    $this->fake();
    $this->get('/'); // error: method.notFound
});
