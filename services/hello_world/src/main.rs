use std::process;

const IDENTITY: &str = "hello";

fn main() {
    let endpoint = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("expected endpoint argument");
        process::exit(1);
    });

    let message = match endpoint.as_str() {
        "hello" => format!("Soy {IDENTITY} y digo hello"),
        "bye" => format!("Soy {IDENTITY} y digo bye"),
        other => {
            eprintln!("unsupported endpoint '{other}'");
            process::exit(1);
        }
    };

    println!("{}", message);
}
