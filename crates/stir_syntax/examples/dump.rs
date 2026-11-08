fn main() {
    let args: Vec<String> = std::env::args().collect();
    let src = std::fs::read_to_string(&args[1]).unwrap();
    let parse = if stir_syntax::is_blade_path(std::path::Path::new(&args[1])) {
        stir_syntax::parse_blade(&src)
    } else {
        stir_syntax::parse(&src)
    };
    print!("{}", parse.debug_tree());
}
