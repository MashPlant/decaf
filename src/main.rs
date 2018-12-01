extern crate regex;

#[macro_use]
extern crate lazy_static;

extern crate backend;

pub mod ast;
pub mod types;
pub mod errors;
pub mod loc;
pub mod parser;
pub mod print;
pub mod symbol_builder;
pub mod config;
pub mod symbol;
pub mod type_checker;
pub mod jvm_code_gen;

use symbol_builder::SymbolBuilder;
use type_checker::TypeChecker;
use ast::Program;
use errors::Error;
//use print::*;

use std::mem;
use std::env;
use std::fs::File;
use std::io::prelude::*;
//use std::io;

fn string_to_static_str(s: String) -> &'static str {
  unsafe {
    let ret = mem::transmute(&s as &str);
    mem::forget(s);
    ret
  }
}

fn compile(input: &'static str) -> Result<Program, Vec<Error>> {
  let mut parser = parser::Parser::new();
  let program = parser.parse(input)?;
  let symbol_builder = SymbolBuilder::new();
  let program = symbol_builder.build(program)?;
  let type_checker = TypeChecker::new();
  let program = type_checker.check(program)?;
  Ok(program)
}

fn main() {
  let mut input = String::new();
  {
    let filename = env::args().nth(1).unwrap_or_else(|| {
      "RadixSort.decaf".to_string()
//      eprintln!("Please specify input filename");
//      std::process::exit(1);
    });
    let mut f = File::open(filename).unwrap();
    f.read_to_string(&mut input).unwrap();
  }
  let input = string_to_static_str(input);

  match compile(input) {
    Ok(program) => {
//      let mut printer = IndentPrinter::new();
//      program.print_scope(&mut printer);
//      printer.flush(&mut io::stdout());
//      printer.clear();
//      program.print_ast(&mut printer);
//      printer.flush(&mut io::stdout());
      let mut code_gen = jvm_code_gen::JvmCodeGen::new();
      code_gen.gen(program);
    }
    Err(errors) => for error in errors { println!("{}", error); },
  }
}
