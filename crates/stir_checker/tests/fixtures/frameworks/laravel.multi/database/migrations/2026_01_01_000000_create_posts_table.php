<?php

use Illuminate\Support\Facades\Schema;

return new class {
    public function up(): void
    {
        Schema::create('posts', function ($table) {
            $table->id();
            $table->foreignId('user_id');
            $table->string('title');
            $table->text('body')->nullable();
            $table->unsignedInteger('views');
            $table->timestamp('published_at')->nullable();
            $table->timestamps();
        });
        Schema::table('posts', function ($table) {
            $table->dropColumn('body');
        });
    }
};
