//! Applies reviewed panic explanations to exact Rust token locations.
use std::{env, fs, process};

use proc_macro2::{Delimiter, TokenStream, TokenTree};

fn calls(tokens: TokenStream, found: &mut Vec<(usize, usize, String)>) {
    let tokens: Vec<_> = tokens.into_iter().collect();
    for window in tokens.windows(3) {
        if let [TokenTree::Punct(dot), TokenTree::Ident(method), TokenTree::Group(args)] = window {
            let name = method.to_string();
            if dot.as_char() == '.'
                && matches!(name.as_str(), "unwrap" | "unwrap_err")
                && args.delimiter() == Delimiter::Parenthesis
                && args.stream().is_empty()
            {
                let location = method.span().start();
                found.push((location.line, location.column, name));
            }
        }
    }
    for token in tokens {
        if let TokenTree::Group(group) = token {
            calls(group.stream(), found);
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().collect();
    let path = args.get(1).ok_or("usage: fixture_expects SOURCE [MAP.tsv]")?;
    let mut source = fs::read_to_string(path)?;
    let mut found = Vec::new();
    calls(source.parse()?, &mut found);
    found.sort();
    if let Some(map) = args.get(2) {
        let map = fs::read_to_string(map)?;
        let mut edits = Vec::new();
        for row in map.lines().filter(|line| !line.is_empty()) {
            let fields: Vec<_> = row.splitn(4, '\t').collect();
            if fields.len() != 4 {
                return Err("map requires line, column, method, explanation".into());
            }
            let line: usize = fields[0].parse()?;
            let column: usize = fields[1].parse()?;
            let method = fields[2];
            if !found.contains(&(line, column, method.to_owned())) {
                return Err(format!("no {method} token at {line}:{column}").into());
            }
            let offset = source.split_inclusive('\n').take(line - 1).map(str::len).sum::<usize>() + column;
            let replacement = if method == "unwrap_err" { "expect_err" } else { "expect" };
            // Rust's debug string representation escapes quotes, backslashes and newlines.
            edits.push((offset, method.len() + 2, format!("{replacement}({:?})", fields[3])));
        }
        edits.sort_by_key(|edit| std::cmp::Reverse(edit.0));
        for (offset, length, replacement) in edits {
            source.replace_range(offset..offset + length, &replacement);
        }
        fs::write(path, source)?;
    } else {
        for (line, column, method) in found {
            println!("{line}\t{column}\t{method}");
        }
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn literals_and_comments_are_opaque_but_macro_calls_are_found() {
        let source = r##"fn test() {
            // source.unwrap()
            let text = r#"source.unwrap()"#;
            assert_eq!(source.unwrap(), other.unwrap_err());
        }"##;
        let mut found = Vec::new();
        super::calls(source.parse().expect("the fixture is a complete Rust token stream"), &mut found);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].2, "unwrap");
        assert_eq!(found[1].2, "unwrap_err");
    }
}
