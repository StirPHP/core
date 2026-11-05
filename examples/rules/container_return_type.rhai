// `$container->get(Foo::class)` returns a `Foo`, and a subclass's `get` counts too.
fn return_type_targets() {
    #{ methods: ["App\\Container::get"] }
}

fn return_type_of_method(node, ctx) {
    // nothing, for anything else: the declared return type stands
    ctx.get_type(node.arguments[0]).class_string
}
