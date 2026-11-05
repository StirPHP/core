// What `__get` and `__call` make available on a model and its subclasses.
fn magic_members(class_name) {
    if class_name == "App\\Model" {
        #{
            properties: #{ id: #{ type: "int", read_only: true } },
            methods: #{ find: #{ returns: "?static", params: "int $id", is_static: true } },
        }
    }
}
