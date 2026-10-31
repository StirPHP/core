fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dialect = if args[1] == "php" {
        stir_syntax::Dialect::Php
    } else {
        stir_syntax::Dialect::Stir
    };
    let src = std::fs::read_to_string(&args[2]).unwrap();
    print!("{}", stir_syntax::parse(&src, dialect).debug_tree());
}
