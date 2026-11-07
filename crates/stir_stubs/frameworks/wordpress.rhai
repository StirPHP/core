// What WordPress has at runtime that its declarations don't say, after what phpstan-wordpress does
// for PHPStan: the types of its global variables, and checks of what's hooked onto its actions and
// filters against how they're run.

// `global $wpdb;` and the others WordPress sets up.
fn global_type(name) {
    let globals = #{
        wpdb: "wpdb",
        post: "?WP_Post",
        wp_query: "WP_Query",
        wp_the_query: "WP_Query",
        wp: "WP",
        wp_rewrite: "WP_Rewrite",
        wp_roles: "WP_Roles",
        wp_locale: "WP_Locale",
        wp_admin_bar: "WP_Admin_Bar",
        wp_scripts: "WP_Scripts",
        wp_styles: "WP_Styles",
        wp_widget_factory: "WP_Widget_Factory",
        wp_embed: "WP_Embed",
        wp_filesystem: "?WP_Filesystem_Base",
        current_user: "WP_User",
        pagenow: "string",
        typenow: "string",
        wp_version: "string",
        wp_db_version: "int",
        blog_id: "int",
        comment: "?WP_Comment",
    };
    let t = globals[name];
    if t == () { return; }
    // only what's declared: without WordPress's classes, nothing's known of them
    let class_name = if t.starts_with("?") { t.sub_string(1) } else { t };
    if class_name != "string" && class_name != "int" && class(class_name) == () { return; }
    t
}

// `add_action('init', 'my_callback', 10, 2)`: the callback needs no more arguments than it's
// given, and it can't be given more than the hook passes, where that's known.
fn visit_func_call(node, ctx) {
    let name = node.name.to_lower();
    if name != "add_action" && name != "add_filter" { return; }
    let args = node.arguments;
    if args.len() < 2 { return; }
    let accepted = 1;
    if args.len() >= 4 {
        let given = ctx.get_type(args[3]).literal;
        if type_of(given) != "i64" { return; }
        accepted = given;
    }
    let callback_type = ctx.get_type(args[1]);
    let callback = callback_type.callable_params();
    let named = callback_type.literal;
    // a function's name: it has to be one
    if callback == () && type_of(named) == "string" && !named.contains("::") {
        ctx.error(
            "the callback `" + named + "` isn't a function",
            "argument.type",
            #{ node: args[1] },
        );
        return;
    }
    if callback != () && callback.required > accepted {
        ctx.error(
            "the callback needs " + count(callback.required) + ", but the hook is added to pass it " + accepted,
            "arguments.count",
            #{ node: args[1] },
        );
    } else if callback != () && args.len() >= 4 && accepted > 1 && callback.max >= 0 && callback.max < accepted {
        ctx.error(
            "the callback takes " + count(callback.max) + ", but the hook is added to pass it " + accepted,
            "arguments.count",
            #{ node: args[1] },
        );
    }
    let hook = ctx.get_type(args[0]).literal;
    if type_of(hook) != "string" { return; }
    let most = ();
    for run in wordpress_hook(hook) {
        if run.args == () { return; }
        if most == () || run.args > most { most = run.args; }
    }
    if most != () && accepted > most {
        ctx.error(
            "the hook `" + hook + "` is run with " + count(most) + ", not " + accepted,
            "wordpress.hookArguments",
            #{ node: if args.len() >= 4 { args[3] } else { args[0] } },
        );
    }
}

fn count(n) {
    if n == 1 { "1 argument" } else { n + " arguments" }
}
