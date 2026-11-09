// What a mock is: an object of the class it's a mock of, too, where `Mockery::mock()` only says it's
// a mock.

fn return_type_targets() {
    #{
        static_methods: ["Mockery::mock", "Mockery::spy"],
        methods: [
            "Illuminate\\Foundation\\Testing\\Concerns\\InteractsWithContainer::mock",
            "Illuminate\\Foundation\\Testing\\Concerns\\InteractsWithContainer::partialMock",
            "Illuminate\\Foundation\\Testing\\Concerns\\InteractsWithContainer::spy",
        ],
    }
}

// What the first argument names, which can be `Foo::class`, an object of it, or a string of several
// or with a prefix (`alias:Foo`, `Foo, Bar`, `Foo[method]`), which says something else again.
fn mocked(node, ctx) {
    let args = node.arguments;
    if args.len() == 0 { return; }
    let given = ctx.get_type(args[0]);
    let named = given.class_string;
    if named == () { named = given.class_name; }
    if named == () {
        let text = given.literal;
        if type_of(text) == "string" && class(text) != () { named = class(text).name; }
    }
    if named != () { "Mockery\\MockInterface&" + named }
}

fn return_type_of_static_method(node, ctx) { mocked(node, ctx) }
fn return_type_of_method(node, ctx) { mocked(node, ctx) }
