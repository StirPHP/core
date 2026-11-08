<?php
use Illuminate\Support\Facades\Blade;

Blade::directive('title', function ($expression) {
    return "<?php \$title = $expression ?>";
});
Blade::directive('outputs', fn ($expression) => "<?php echo $expression; ?>");
Blade::directive('unread', ['App\Directives', 'unread']);
