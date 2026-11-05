fn main() {
    let args: Vec<String> = std::env::args().collect();
    let src = std::fs::read_to_string(&args[1]).unwrap();
    print!("{}", stir_syntax::parse(&src).debug_tree());
}
