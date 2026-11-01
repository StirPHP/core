<?php
function reads_superglobals()
{
    return [$_GET, $_POST, $_REQUEST, $_COOKIE, $_FILES, $_SERVER, $_ENV, $_SESSION, $GLOBALS];
}

function writes_superglobal(string $s): void
{
    $_SESSION['key'] = $s;
    unset($_POST['key']);
}

function not_a_superglobal()
{
    return $_NOPE; // error: SPHP0006
}
