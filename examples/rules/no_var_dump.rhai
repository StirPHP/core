// Runs on every function call, and on nothing else.
fn visit_func_call(node, ctx) {
    if node.name.to_lower() == "var_dump" {
        ctx.error("Don't leave var_dump in", "app.noVarDump");
    }
}
