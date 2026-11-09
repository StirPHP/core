// What Pest does at runtime that its classes can't declare. It writes a class for each test file,
// extending the test case that file's `uses()` and `tests/Pest.php` name and using the traits they
// name, and binds the file's closures to an instance of it. Pest's own PHPDoc says `TestCall` for
// `$this` there, for what editors make of `@mixin`, which isn't what runs.

// The class of a test file, as a type: it has no name of its own to go by.
fn test_case(ctx) {
    let base = "PHPUnit\\Framework\\TestCase";
    let traits = [];
    for name in ctx.pest_uses() {
        let c = class(name);
        if c == () { continue; }
        if c.kind == "trait" { traits.push(c.name); } else if c.kind == "class" { base = c.name; }
    }
    traits.push("Pest\\Concerns\\Testable");
    let out = base;
    for t in traits { out += "&" + t; }
    out
}

fn closure_this(declared, ctx) {
    if declared.class_name != "Pest\\PendingCalls\\TestCall" { return; }
    // it's `#[AllowDynamicProperties]`: what the file's hooks set, its tests can read
    #{ type: test_case(ctx), properties: ctx.pest_properties() }
}

fn return_type_targets() {
    #{ functions: ["test"] }
}

// `test()` without a description, inside a test, hands over the test being run, behind a proxy.
fn return_type_of_function(node, ctx) {
    if node.arguments.len() == 0 { "Pest\\Support\\HigherOrderTapProxy<" + test_case(ctx) + ">" }
}

// Calls that aren't on these pending calls, as in `it('works')->get('/')->assertOk()`, are written
// down to be made once the test runs: on the test, on what the last of them gave back, or of a
// function by that name. Each gives back the pending call, and there's no telling more until then.
fn higher_order(class_name) {
    class_name in [
        "Pest\\PendingCalls\\TestCall",
        "Pest\\PendingCalls\\BeforeEachCall",
        "Pest\\PendingCalls\\AfterEachCall",
        "Pest\\PendingCalls\\DescribeCall",
    ]
}

fn magic_method(type, name, is_static) {
    let class_name = type.class_name;
    if class_name == () { return; }
    if class_name == "Pest\\Support\\HigherOrderTapProxy" {
        let args = type.type_args;
        // it goes through reflection to the test's method, whichever that is
        if args.len() > 0 { return #{ forward_to: args[0].to_string(), any_visibility: true }; }
        return;
    }
    if higher_order(class_name) { return "$this"; }
}

fn magic_property(type, name) {
    let class_name = type.class_name;
    if class_name == () { return; }
    if class_name == "Pest\\Support\\HigherOrderTapProxy" {
        let args = type.type_args;
        if args.len() > 0 { return #{ forward_to: args[0].to_string() }; }
        return;
    }
    if higher_order(class_name) { return "$this"; }
}
