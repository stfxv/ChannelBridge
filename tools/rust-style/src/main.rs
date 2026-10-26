use std::path::PathBuf;
use std::{env, fs, process};

use proc_macro2::{Delimiter, TokenStream, TokenTree};
use quote::ToTokens;
use syn::UseTree;
use syn::visit::Visit;

#[derive(Debug)]
struct Edit {
    start: usize,
    end: usize,
    replacement: String,
}

struct Imports<'a> {
    source: &'a str,
    edits: Vec<Edit>,
    findings: Vec<(usize, &'static str)>,
}

fn nested_group(tree: &UseTree, in_group: bool) -> bool {
    match tree {
        UseTree::Path(path) => in_group || nested_group(&path.tree, false),
        UseTree::Group(group) => group.items.iter().any(|item| nested_group(item, true)),
        _ => false,
    }
}

fn leaves(tree: &UseTree, prefix: &str, out: &mut Vec<String>) {
    match tree {
        UseTree::Path(path) => leaves(&path.tree, &format!("{prefix}{}::", path.ident), out),
        UseTree::Group(group) => {
            for item in &group.items {
                leaves(item, prefix, out);
            }
        }
        UseTree::Name(name) if name.ident == "self" => {
            out.push(prefix.trim_end_matches("::").to_owned());
        }
        UseTree::Rename(rename) if rename.ident == "self" => {
            out.push(format!(
                "{} as {}",
                prefix.trim_end_matches("::"),
                rename.rename
            ));
        }
        leaf => out.push(format!("{prefix}{}", leaf.to_token_stream())),
    }
}

impl<'ast> Visit<'ast> for Imports<'_> {
    fn visit_expr_path(&mut self, expression: &'ast syn::ExprPath) {
        let names: Vec<_> = expression
            .path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect();
        if names.ends_with(&["PoisonError".to_owned(), "into_inner".to_owned()]) {
            self.findings.push((
                expression
                    .path
                    .segments
                    .last()
                    .expect("matched two path segments")
                    .ident
                    .span()
                    .start()
                    .line,
                "poison-into-inner",
            ));
        }
        syn::visit::visit_expr_path(self, expression);
    }

    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        if !nested_group(&item.tree, false) {
            return;
        }
        let start = item.use_token.span.start();
        self.findings.push((start.line, "import-group"));
        let range = item.use_token.span.byte_range().start..item.semi_token.span.byte_range().end;
        let original = &self.source[range.clone()];
        if original.contains("//") || original.contains("/*") || !item.attrs.is_empty() {
            return;
        }
        let mut imports = Vec::new();
        leaves(
            &item.tree,
            if item.leading_colon.is_some() {
                "::"
            } else {
                ""
            },
            &mut imports,
        );
        let visibility = item.vis.to_token_stream().to_string();
        let prefix = if visibility.is_empty() {
            String::new()
        } else {
            format!("{visibility} ")
        };
        let replacement = imports
            .iter()
            .enumerate()
            .map(|(index, path)| {
                if index == 0 {
                    format!("use {path};")
                } else {
                    format!("\n{prefix}use {path};")
                }
            })
            .collect::<String>();
        self.edits.push(Edit {
            start: range.start,
            end: range.end,
            replacement,
        });
    }
}

fn unwrap_lines(tokens: TokenStream, out: &mut Vec<usize>) {
    let tokens: Vec<_> = tokens.into_iter().collect();
    for window in tokens.windows(3) {
        if matches!(&window[0], TokenTree::Punct(p) if p.as_char() == '.')
            && matches!(&window[1], TokenTree::Ident(i) if i == "unwrap")
            && matches!(&window[2], TokenTree::Group(g) if g.delimiter() == Delimiter::Parenthesis && g.stream().is_empty())
        {
            out.push(window[1].span().start().line);
        }
    }
    for token in tokens {
        if let TokenTree::Group(group) = token {
            unwrap_lines(group.stream(), out);
        }
    }
}

fn inspect(source: &str) -> Result<Imports<'_>, syn::Error> {
    let file = syn::parse_file(source)?;
    let mut imports = Imports {
        source,
        edits: Vec::new(),
        findings: Vec::new(),
    };
    imports.visit_file(&file);
    Ok(imports)
}

fn rewrite(source: &str, edits: &mut [Edit]) -> String {
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.start));
    let mut result = source.to_owned();
    for edit in edits {
        result.replace_range(edit.start..edit.end, &edit.replacement);
    }
    result
}

fn main() {
    let mut args = env::args().skip(1);
    let mode = args.next().unwrap_or_default();
    if !matches!(mode.as_str(), "check" | "fix-imports" | "inventory") {
        eprintln!("usage: channelbridge-rust-style <check|fix-imports|inventory> FILE.rs ...");
        process::exit(2);
    }
    let paths: Vec<PathBuf> = args.map(PathBuf::from).collect();
    if paths.is_empty() {
        eprintln!("at least one Rust source file is required");
        process::exit(2);
    }
    let mut failed = false;
    for path in paths {
        let result = (|| -> Result<(), Box<dyn std::error::Error>> {
            let source = fs::read_to_string(&path)?;
            let mut imports = inspect(&source)?;
            if mode == "fix-imports" {
                fs::write(&path, rewrite(&source, &mut imports.edits))?;
                return Ok(());
            }
            let mut findings = imports.findings;
            let mut unwraps = Vec::new();
            unwrap_lines(source.parse()?, &mut unwraps);
            findings.extend(unwraps.into_iter().map(|line| (line, "unwrap")));
            findings.sort_unstable();
            for (line, rule) in findings {
                println!("{}:{line}: {rule}", path.display());
                failed = true;
            }
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("{}: {error}", path.display());
            process::exit(2);
        }
    }
    if failed && mode == "check" {
        process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::{inspect, rewrite, unwrap_lines};

    #[test]
    fn splits_nested_imports_preserving_visibility_aliases_and_self() {
        let source = "pub(super) use ::foo::{self, bar::{self as b, Baz as Q}, *};";
        let mut imports = inspect(source).expect("fixture parses");
        assert_eq!(imports.findings.len(), 1);
        let changed = rewrite(source, &mut imports.edits);
        assert_eq!(
            changed,
            "pub(super) use ::foo;\npub (super) use ::foo::bar as b;\npub (super) use ::foo::bar::Baz as Q;\npub (super) use ::foo::*;"
        );
        let mut second = inspect(&changed).expect("rewritten imports parse");
        assert!(second.findings.is_empty());
        assert_eq!(rewrite(&changed, &mut second.edits), changed);
    }

    #[test]
    fn leaves_comments_and_attributes_for_manual_review() {
        for source in ["use a::{/* keep */ b::C};", "#[cfg(test)] use a::{b::C};"] {
            let imports = inspect(source).expect("fixture parses");
            assert_eq!(imports.findings.len(), 1);
            assert!(imports.edits.is_empty());
        }
    }

    #[test]
    fn accepts_module_groups_and_inline_super_glob() {
        let imports = inspect("use foo::{self, Bar as B}; mod tests { use super::*; }")
            .expect("fixture parses");
        assert!(imports.findings.is_empty());
    }

    #[test]
    fn poison_rule_requires_an_explicit_poison_error_path() {
        let imports = inspect("fn f() { let _ = Mutex::into_inner; let _ = value.into_inner(); let _ = std::sync::PoisonError::into_inner; }").expect("fixture parses");
        assert_eq!(imports.findings, vec![(1, "poison-into-inner")]);
    }

    #[test]
    fn finds_unwrap_inside_macros_without_touching_strings_or_comments() {
        let source = r###"
            /// x.unwrap()
            fn f() {
                let s = r#"x.unwrap()"#;
                // x.unwrap()
                assert_eq!(Some(1).unwrap(), 1);
                outer!(inner!(value.unwrap()));
                value.unwrap_or(1);
            }
        "###;
        let mut lines = Vec::new();
        unwrap_lines(source.parse().expect("fixture tokenizes"), &mut lines);
        assert_eq!(lines, vec![6, 7]);
        let mut imports = inspect(source).expect("fixture parses");
        assert_eq!(rewrite(source, &mut imports.edits), source);
    }
}
