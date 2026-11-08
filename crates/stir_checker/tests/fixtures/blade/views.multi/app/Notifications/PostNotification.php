<?php
namespace App\Notifications;

use App\Models\Post;
use Illuminate\Notifications\Messages\MailMessage;

class PostNotification
{
    public function toMail(Post $post): MailMessage
    {
        return (new MailMessage)->view('mail.notified', ['post' => $post]);
    }
}
