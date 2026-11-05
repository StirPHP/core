<?php

// What phpstorm-stubs has wrong or vague about methods: `T|false` for what's only false in odd
// cases, a parameter's type lost on return, and a document's elements.

function nodes(DOMDocument $d, DOMElement $parent): void
{
    $e = $d->createElement('a');
    $e->setAttribute('a', 'b');
    $added = $parent->appendChild($e);
    $added->setAttribute('b', 'c');
    $removed = $parent->removeChild($added);
    $removed->setAttribute('c', 'd');
    $list = $d->getElementsByTagName('x');
    foreach ($list as $item) {
        $item->setAttribute('d', 'e');
    }
    $parent->nosuch(); // error: method.notFound
}

function xml(SimpleXMLElement $x): string
{
    foreach ($x->item as $child) {
        echo $child->name;
    }
    foreach ($x as $k => $child) {
        echo $k, $child->other;
    }
    return (string) $x->title;
}

function files(SplFileInfo $f): int
{
    strlen($f->getRealPath());
    return $f->getSize();
}

function dates(DateTimeImmutable $d): DateTimeImmutable
{
    return $d->modify('+1 day');
}

function statements(PDO $p): void
{
    $p->prepare('x')->execute();
}
