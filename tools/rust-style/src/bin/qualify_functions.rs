//! Qualifies reviewed imported functions without interpreting lowercase names as functions.
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::{env, fs, process};

use proc_macro2::Span;
use syn::punctuated::Punctuated;
use syn::visit::Visit;
use syn::{Expr, Token, UseTree};

type Edits = BTreeMap<usize, (usize, String)>;

fn leaves(tree: &UseTree, prefix: &str, out: &mut Vec<String>) {
    use quote::ToTokens;

    match tree {
        UseTree::Path(path) => leaves(&path.tree, &format!("{prefix}{}::", path.ident), out),
        UseTree::Group(group) => {
            for tree in &group.items {
                leaves(tree, prefix, out);
            }
        }
        tree => out.push(format!("{prefix}{}", tree.to_token_stream())),
    }
}

struct Qualification<'source> {
    source: &'source str,
    mappings: BTreeMap<String, String>,
    edits: Edits,
    conflicts: BTreeSet<String>,
}

impl Qualification<'_> {
    fn replace(&mut self, span: Span, replacement: String) {
        let range = span.byte_range();
        self.edits.insert(range.start, (range.end, replacement));
    }
}

impl<'ast> Visit<'ast> for Qualification<'_> {
    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        use syn::spanned::Spanned;

        let mut paths = Vec::new();
        leaves(
            &item.tree,
            if item.leading_colon.is_some() {
                "::"
            } else {
                ""
            },
            &mut paths,
        );
        let mut changed = false;
        for path in &mut paths {
            let Some((parent, name)) = path.rsplit_once("::") else {
                continue;
            };
            if let Some(module) = self.mappings.get(name) {
                let module_name = parent.rsplit("::").next().expect("nonempty import parent");
                *path = if module_name == module {
                    parent.to_owned()
                } else {
                    format!("{parent} as {module}")
                };
                changed = true;
            }
        }
        if changed {
            if !item.attrs.is_empty()
                || !matches!(item.vis, syn::Visibility::Inherited)
                || self.source[item.tree.span().byte_range()].contains("//")
                || self.source[item.tree.span().byte_range()].contains("/*")
            {
                self.conflicts
                    .insert("attributed or public import".to_owned());
                return;
            }
            let mut seen = BTreeSet::new();
            paths.retain(|path| seen.insert(path.clone()));
            self.replace(item.tree.span(), paths.join("; use "));
        }
    }

    fn visit_pat_ident(&mut self, pattern: &'ast syn::PatIdent) {
        let name = pattern.ident.to_string();
        if self.mappings.contains_key(&name) {
            self.conflicts.insert(name);
        }
        syn::visit::visit_pat_ident(self, pattern);
    }

    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        let name = item.sig.ident.to_string();
        if self.mappings.contains_key(&name) {
            self.conflicts.insert(name);
        }
        syn::visit::visit_item_fn(self, item);
    }

    fn visit_path(&mut self, path: &'ast syn::Path) {
        let names: Vec<_> = path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect();
        for length in (2..=names.len()).rev() {
            let name = names[..length].join("::");
            if let Some(replacement) = self.mappings.get(&name) {
                let start = path.segments[0].ident.span().byte_range().start;
                let end = path.segments[length - 1].ident.span().byte_range().end;
                self.edits.insert(start, (end, replacement.clone()));
                return;
            }
        }
        syn::visit::visit_path(self, path);
    }

    fn visit_expr_path(&mut self, expression: &'ast syn::ExprPath) {
        if expression.qself.is_none()
            && expression.path.leading_colon.is_none()
            && expression.path.segments.len() == 1
        {
            let ident = &expression.path.segments[0].ident;
            if let Some(module) = self.mappings.get(&ident.to_string()) {
                self.replace(ident.span(), format!("{module}::{ident}"));
            }
        }
        syn::visit::visit_expr_path(self, expression);
    }

    fn visit_macro(&mut self, item: &'ast syn::Macro) {
        use syn::parse::Parser;

        if matches!(
            item.path.get_ident().map(ToString::to_string).as_deref(),
            Some(
                "assert"
                    | "assert_eq"
                    | "assert_ne"
                    | "debug_assert"
                    | "debug_assert_eq"
                    | "debug_assert_ne"
                    | "format"
                    | "println"
                    | "eprintln"
            )
        ) {
            let expressions = Punctuated::<Expr, Token![,]>::parse_terminated
                .parse2(item.tokens.clone())
                .expect("reviewed standard macro arguments must parse as expressions");
            for expression in &expressions {
                self.visit_expr(expression);
            }
        }
    }
}

fn qualify(
    source: &str,
    mappings: BTreeMap<String, String>,
    module: Option<&str>,
) -> Result<String, Box<dyn Error>> {
    let file = syn::parse_file(source)?;
    let mut qualification = Qualification {
        source,
        mappings,
        edits: BTreeMap::new(),
        conflicts: BTreeSet::new(),
    };
    if let Some(module) = module {
        for item in &file.items {
            if let syn::Item::Mod(item) = item {
                if item.ident == module {
                    qualification.visit_item_mod(item);
                }
            }
        }
    } else {
        qualification.visit_file(&file);
    }
    if !qualification.conflicts.is_empty() {
        return Err(format!(
            "bindings require manual scope review: {:?}",
            qualification.conflicts
        )
        .into());
    }
    let mut result = source.to_owned();
    for (start, (end, replacement)) in qualification.edits.into_iter().rev() {
        result.replace_range(start..end, &replacement);
    }
    Ok(result)
}

fn qualify_parent_functions(source: &str) -> Result<String, Box<dyn Error>> {
    let file = syn::parse_file(source)?;
    let mappings: BTreeMap<_, _> = file
        .items
        .iter()
        .filter_map(|item| {
            if let syn::Item::Fn(item) = item {
                Some((item.sig.ident.to_string(), "super".to_owned()))
            } else {
                None
            }
        })
        .collect();
    let mut result = source.to_owned();
    if mappings.is_empty() {
        return Ok(result);
    }
    for item in &file.items {
        if let syn::Item::Mod(module) = item {
            let Some((_, items)) = &module.content else {
                continue;
            };
            let imports_parent = items.iter().any(|item| {
                let syn::Item::Use(item) = item else {
                    return false;
                };
                let UseTree::Path(path) = &item.tree else {
                    return false;
                };
                path.ident == "super" && matches!(*path.tree, UseTree::Glob(_))
            });
            if imports_parent {
                result = qualify(&result, mappings.clone(), Some(&module.ident.to_string()))?;
            }
        }
    }
    Ok(result)
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut args = env::args().skip(1);
    let path = args
        .next()
        .ok_or("usage: qualify_functions SOURCE NAME=MODULE ...")?;
    let args: Vec<_> = args.collect();
    if args == ["--parent-functions"] {
        let source = fs::read_to_string(&path)?;
        fs::write(path, qualify_parent_functions(&source)?)?;
        return Ok(());
    }
    let module = args
        .iter()
        .find_map(|argument| argument.strip_prefix("--module=").map(str::to_owned));
    let mappings = args
        .into_iter()
        .filter(|argument| !argument.starts_with("--module="))
        .map(|argument| {
            let (name, module) = argument.split_once('=').ok_or("expected NAME=MODULE")?;
            Ok((name.to_owned(), module.to_owned()))
        })
        .collect::<Result<BTreeMap<_, _>, Box<dyn Error>>>()?;
    if mappings.is_empty() {
        return Err("at least one reviewed mapping is required".into());
    }
    let source = fs::read_to_string(&path)?;
    fs::write(path, qualify(&source, mappings, module.as_deref())?)?;
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    #[test]
    fn rewrites_calls_references_and_assertions_but_preserves_other_tokens() {
        let source = r#"use crate::frame::finite;
fn f() { finite(1.); let _ = xs.map(finite); assert_eq!(finite(2.), 2.);
let _ = other::finite(); let _ = x.finite(); let _ = "finite(3.)";
// finite(4.)
sel!(finite); }"#;
        let mappings = BTreeMap::from([("finite".to_owned(), "frame".to_owned())]);
        let changed = super::qualify(source, mappings.clone(), None).expect("unshadowed fixture");
        assert!(changed.contains("frame::finite(1.)"));
        assert!(changed.contains("map(frame::finite)"));
        assert!(changed.contains("assert_eq!(frame::finite(2.)"));
        for untouched in [
            "use crate::frame;",
            "other::finite()",
            "x.finite()",
            "\"finite(3.)\"",
            "// finite(4.)",
            "sel!(finite)",
        ] {
            assert!(changed.contains(untouched));
        }
        assert_eq!(
            super::qualify(&changed, mappings, None).expect("repeat succeeds"),
            changed
        );
    }

    #[test]
    fn selected_module_does_not_rewrite_parent_function_or_call() {
        let source = "fn stereo_queue() {} fn parent() { stereo_queue(); } mod tests { fn f() { stereo_queue(); } }";
        let changed = super::qualify(
            source,
            BTreeMap::from([("stereo_queue".to_owned(), "super".to_owned())]),
            Some("tests"),
        )
        .expect("selected test module has no shadows");
        assert_eq!(
            changed,
            "fn stereo_queue() {} fn parent() { stereo_queue(); } mod tests { fn f() { super::stereo_queue(); } }"
        );
    }

    #[test]
    fn shortens_only_reviewed_type_prefixes_preserving_generics_and_variants() {
        let source = "use std::cell::Cell; fn f(_: std::cell::Cell<Option<u8>>) { let _ = core_api::State::Ready; }";
        let changed = super::qualify(
            source,
            BTreeMap::from([
                ("std::cell::Cell".to_owned(), "Cell".to_owned()),
                ("core_api::State".to_owned(), "State".to_owned()),
            ]),
            None,
        )
        .expect("reviewed paths parse");
        assert_eq!(
            changed,
            "use std::cell::Cell; fn f(_: Cell<Option<u8>>) { let _ = State::Ready; }"
        );
    }

    #[test]
    fn refuses_import_edits_that_would_discard_comments_or_attributes() {
        for source in [
            "use frame::{/* important */ finite};",
            "#[cfg(test)] use frame::finite;",
        ] {
            assert!(
                super::qualify(
                    source,
                    BTreeMap::from([("finite".to_owned(), "frame".to_owned())]),
                    None
                )
                .is_err()
            );
        }
    }

    #[test]
    fn parent_inventory_handles_tests_and_model_tests_without_changing_parent_calls() {
        let source = "fn queue() {} fn parent() { queue(); } mod tests { use super::*; fn f() { assert!(queue()); } } mod model_tests { use super::*; fn f() { let _ = queue; } }";
        let changed = super::qualify_parent_functions(source)
            .expect("parent declarations resolve both child calls");
        assert!(changed.contains("fn parent() { queue(); }"));
        assert!(changed.contains("assert!(super::queue())"));
        assert!(changed.contains("let _ = super::queue;"));
        assert_eq!(
            super::qualify_parent_functions(&changed).expect("repeat succeeds"),
            changed
        );
        assert!(
            super::qualify_parent_functions(
                "fn queue() {} mod tests { use super::*; fn f(queue: fn()) { queue(); } }"
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_shadowing_before_writing() {
        for source in [
            "fn f(finite: fn()) { finite(); }",
            "fn f() { let finite = || {}; finite(); }",
            "fn finite() {}",
        ] {
            assert!(
                super::qualify(
                    source,
                    BTreeMap::from([("finite".to_owned(), "frame".to_owned())]),
                    None
                )
                .is_err()
            );
        }
    }
}
