mod ast_parser;
mod lexer;
mod token;

use std::fs::File;
use std::io::Read;
use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

use crate::ast_parser::ASTParser;
use crate::lexer::Lexer;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    file_path: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let mut file = File::open(cli.file_path)?;
    let mut file_contents = String::new();
    file.read_to_string(&mut file_contents)?;
    let file_chars: Vec<_> = file_contents.chars().collect();

    let lexer = Lexer::new(&file_chars);
    let tokens: Vec<_> = lexer.collect();

    let mut parser = ASTParser::new(&tokens);
    let ast = parser.parse();
    println!("{:?}", ast);

    Ok(())
}
