<?php
namespace App\View\Components;

use App\Post;
use Illuminate\View\Component;

class Alert extends Component
{
    public string $notInTheConstructor = '';
    protected string $hiddenProperty = '';
    public static string $staticProperty = '';

    public function __construct(
        public string $type,
        public int $limit = 5,
        public ?Post $post = null,
        string $userName = '',
        array $options = [],
    ) {
    }

    public function shout(): string
    {
        return strtoupper($this->type);
    }

    public function withArgument(int $times): string
    {
        return str_repeat($this->type, $times);
    }

    protected function hiddenMethod(): string
    {
        return '';
    }

    public function render()
    {
        return view('components.alert', ['extra' => 1]);
    }
}
