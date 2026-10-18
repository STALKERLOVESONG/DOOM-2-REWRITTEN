//! ZScript front end against real mods (skipped when absent).

use std::path::PathBuf;

use doom_script::zscript::{ast, parse};

fn wads() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../WADS")
}

/// Every ZSCRIPT lump of a mod, following `#include`s.
fn zscript_sources(pk3: &str) -> Option<Vec<(String, String)>> {
    let (doom2, m) = (wads().join("doom2.wad"), wads().join(pk3));
    if !doom2.exists() || !m.exists() {
        eprintln!("skipping: no doom2.wad / {pk3}");
        return None;
    }
    let stack = doom_content::ResourceStack::load(&doom2, &[m]).unwrap();
    let mut out = Vec::new();
    let mut queue: Vec<(String, String)> = stack
        .all_named("ZSCRIPT")
        .iter()
        .map(|&id| (format!("{}#{}", stack.info(id).name, id.0), String::from_utf8_lossy(stack.data(id)).into_owned()))
        .collect();
    while let Some((name, text)) = queue.pop() {
        let (file, _) = parse(&text);
        for inc in &file.includes {
            if out.iter().any(|(n, _): &(String, String)| n == inc) || queue.iter().any(|(n, _)| n == inc) {
                continue;
            }
            match stack.find_path(inc).or_else(|| stack.check_num_for_name(inc)) {
                Some(id) => queue.push((inc.clone(), String::from_utf8_lossy(stack.data(id)).into_owned())),
                None => panic!("{name}: #include {inc:?} not found"),
            }
        }
        out.push((name, text));
    }
    Some(out)
}

#[test]
fn myhouse_zscript_parses() {
    let Some(sources) = zscript_sources("myhouse.pk3") else { return };
    let (mut classes, mut functions, mut errors) = (0, 0, Vec::new());
    for (name, text) in &sources {
        let (file, errs) = parse(text);
        for d in &file.decls {
            if let ast::Decl::Class(c) = d {
                classes += 1;
                functions += c.members.iter().filter(|m| matches!(m, ast::Member::Function(_))).count();
            }
        }
        errors.extend(errs.into_iter().map(|e| format!("{name}: {e}")));
    }
    eprintln!("{} lumps, {classes} classes, {functions} functions", sources.len());
    assert!(errors.is_empty(), "{} errors:\n{}", errors.len(), errors.join("\n"));
    assert!(classes >= 20, "{classes} classes");
}

/// Every DECORATE lump of a mod with its includes; (actors, errors).
fn parse_mod_decorate(file: &str) -> Option<(usize, Vec<String>)> {
    use doom_script::zscript::parse_decorate;
    let (doom2, m) = (wads().join("doom2.wad"), wads().join(file));
    if !doom2.exists() || !m.exists() {
        return None;
    }
    let stack = doom_content::ResourceStack::load(&doom2, &[m]).unwrap();
    let mut queue: Vec<(String, String)> =
        stack.all_named("DECORATE").iter().map(|&id| (format!("DECORATE#{}", id.0), String::from_utf8_lossy(stack.data(id)).into_owned())).collect();
    let mut seen: Vec<String> = Vec::new();
    let (mut actors, mut errors) = (0, Vec::new());
    while let Some((name, text)) = queue.pop() {
        let (f, errs) = parse_decorate(&text);
        actors += f.actors.len();
        errors.extend(errs.into_iter().map(|e| format!("{name}: {e}")));
        for inc in f.includes {
            if seen.iter().any(|s| s.eq_ignore_ascii_case(&inc)) {
                continue;
            }
            seen.push(inc.clone());
            if let Some(id) = stack.find_path(&inc).or_else(|| stack.check_num_for_name(&inc)) {
                queue.push((inc, String::from_utf8_lossy(stack.data(id)).into_owned()));
            }
        }
    }
    Some((actors, errors))
}

#[test]
fn mod_decorate_parses() {
    for m in ["brutalv21.pk3", "myhouse.pk3", "CC.wad"] {
        let Some((actors, errors)) = parse_mod_decorate(m) else { continue };
        eprintln!("{m}: {actors} actors, {} errors", errors.len());
        for e in errors.iter().take(12) {
            eprintln!("  {e}");
        }
        assert!(errors.is_empty(), "{m}: {} errors", errors.len());
    }
}
