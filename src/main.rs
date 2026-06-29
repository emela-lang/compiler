mod ast;
mod driver;
mod error;
mod imports;
mod ir;
mod js;
mod lexer;
mod parser;
mod typecheck;

fn main() {
    if let Err(error) = driver::run() {
        eprintln!("{}", error.render());
        std::process::exit(1);
    }
}
