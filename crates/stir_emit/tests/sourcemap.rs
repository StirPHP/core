//! Source-map hook: output ranges recorded for nodes with an origin.

mod common;

use common::*;
use stir_emit::ast::*;
use stir_emit::print::{print_file, print_file_with_map};

fn o(start: u32, end: u32) -> Option<Origin> {
    Some(Origin::new(7, start, end))
}

fn sample() -> File {
    let greet = FunctionDecl {
        params: vec![Param {
            origin: o(13, 24),
            ..Param::new("name").ty(Type::string())
        }],
        origin: o(0, 80),
        ..FunctionDecl::new("greet")
    }
    .returns(Type::string())
    .body(vec![
        Stmt::ret(Some(
            Expr::interp(vec![
                InterpPart::Lit("Hello, ".into()),
                InterpPart::Expr(v("name").with_origin(o(50, 54))),
            ])
            .with_origin(o(40, 56)),
        ))
        .with_origin(o(33, 56)),
    ]);
    let class = ClassDecl {
        origin: o(90, 200),
        ..ClassDecl::new("Counter")
    }
    .member(PropertyDecl {
        origin: o(110, 125),
        ..PropertyDecl::new(Visibility::Private, "n")
            .ty(Type::int())
            .default_value(int(0))
    })
    .member(MethodDecl {
        origin: o(130, 190),
        ..MethodDecl::new(Visibility::Public, "inc")
            .returns(Type::int())
            .body(vec![
                Stmt::ret(Some(
                    un(
                        UnaryOp::PreInc,
                        Expr::this().prop("n").with_origin(o(160, 167)),
                    )
                    .with_origin(o(158, 167)),
                ))
                .with_origin(o(151, 168)),
            ])
    });
    let mut f = File::new().namespace("App");
    f.stmts = vec![
        greet.into(),
        class.into(),
        Stmt::echo(vec![
            Expr::call_fn("greet", args(vec![s("wörld").with_origin(o(215, 222))]))
                .with_origin(o(209, 223)),
        ])
        .with_origin(o(204, 224)),
    ];
    f
}

#[test]
fn mappings() {
    let f = sample();
    let (text, maps) = print_file_with_map(&f, &opts());
    assert_eq!(
        text,
        print_file(&f, &opts()),
        "mapping must not change output"
    );

    // Consistency: offsets, lines and columns agree with the text; ranges are ordered.
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let mut prev = 0;
    for m in &maps {
        for p in [m.start, m.end] {
            let ls = line_starts[p.line as usize];
            assert_eq!(ls + p.col as usize, p.offset as usize);
        }
        assert!(m.start.offset <= m.end.offset);
        assert!(m.start.offset >= prev);
        prev = m.start.offset;
    }

    let mut report = String::new();
    for m in &maps {
        let slice = &text[m.start.offset as usize..m.end.offset as usize];
        let first = slice.lines().next().unwrap_or("");
        report.push_str(&format!(
            "{}:{}-{}:{} <- {}..{}  `{}`{}\n",
            m.start.line,
            m.start.col,
            m.end.line,
            m.end.col,
            m.origin.start,
            m.origin.end,
            first,
            if slice.contains('\n') { " ..." } else { "" },
        ));
    }
    insta::assert_snapshot!(format!("{text}\n=== mappings ===\n{report}"));
}
