<?php
namespace App\Mail;

use App\Models\Author;
use App\Models\Post;
use Illuminate\Mail\Mailable;
use Illuminate\Mail\Mailables\Content;

class PostMail extends Mailable
{
    public Post $post;
    private string $hidden = '';

    public function __construct(public Author $author)
    {
        $this->post = new Post;
    }

    public function build()
    {
        return $this->subject('x')->markdown('mail.built')->with('extra', 1);
    }

    public function content(): Content
    {
        return new Content(view: 'mail.content', with: ['given' => $this->author]);
    }
}
