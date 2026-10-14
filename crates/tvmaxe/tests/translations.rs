//! Acoperirea traducerilor: fiecare `tr("…")` / `trn("…", …)` din cod are
//! traducere românească, iar catalogul nu păstrează texte care nu mai există.
//! Un text nou fără traducere ar apărea în engleză pe un sistem în română;
//! testul îl prinde înainte.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use tvmaxe_core::i18n::{catalog_entries, plural_entries};

fn sources() -> Vec<PathBuf> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut files = Vec::new();
    let mut dirs = vec![crates.join("tvmaxe/src"), crates.join("tvmaxe-core/src")];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "rs")
                && !path.ends_with("i18n/catalog_ro.rs")
                && !path.ends_with("tvmaxe-core/src/i18n.rs")
            {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// Un literal șir Rust care începe la `s[0] == '"'`: (conținut, lungime).
/// Tratează escape-urile folosite în cod și continuarea de linie (`\` + newline).
fn string_literal(s: &str) -> Option<(String, usize)> {
    let mut out = String::new();
    let mut chars = s.char_indices().skip(1).peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Some((out, i + 1)),
            '\\' => match chars.next()?.1 {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                '\'' => out.push('\''),
                '\n' => {
                    while chars.peek().is_some_and(|(_, c)| c.is_whitespace()) {
                        chars.next();
                    }
                }
                other => panic!("escape neașteptat \\{other}"),
            },
            c => out.push(c),
        }
    }
    None
}

/// Literalele șir de după `name(` (sau `name!(`), separate prin virgule.
fn literal_args(code: &str, at: usize, count: usize) -> Option<Vec<String>> {
    let mut rest = &code[at..];
    let mut args = Vec::new();
    while args.len() < count {
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == ',');
        let (lit, len) = string_literal(rest).filter(|_| rest.starts_with('"'))?;
        args.push(lit);
        rest = &rest[len..];
    }
    Some(args)
}

struct Usage {
    texts: BTreeSet<String>,
    plurals: BTreeSet<String>,
    dynamic: Vec<String>,
}

fn collect() -> Usage {
    let mut u = Usage {
        texts: BTreeSet::new(),
        plurals: BTreeSet::new(),
        dynamic: Vec::new(),
    };
    for file in sources() {
        let code = std::fs::read_to_string(&file).unwrap();
        let name = file.display().to_string();
        for (pat, count) in [("tr(", 1), ("trn(", 1)] {
            for (i, _) in code.match_indices(pat) {
                let line_start = code[..i].rfind('\n').map_or(0, |n| n + 1);
                if code[line_start..i].trim_start().starts_with("//") {
                    continue; // comentariu
                }
                let prev = code[..i].chars().next_back().unwrap_or(' ');
                if prev.is_alphanumeric() || prev == '_' || prev == '.' {
                    continue; // alt identificator, ex. `str(` sau `.tr(`
                }
                let start = i + pat.len();
                match (pat, literal_args(&code, start, count)) {
                    ("tr(", Some(a)) => {
                        u.texts.insert(a[0].clone());
                    }
                    ("trn(", Some(a)) => {
                        u.plurals.insert(a[0].clone());
                    }
                    (_, None) => {
                        let line = code[..i].lines().count();
                        let snippet: String = code[start..].chars().take(30).collect();
                        u.dynamic.push(format!("{name}:{line}: {pat}{snippet}"));
                    }
                    _ => unreachable!(),
                }
            }
        }
    }
    u
}

#[test]
fn every_text_has_a_romanian_translation() {
    let u = collect();
    assert!(
        u.texts.len() > 100,
        "scanarea n-a găsit textele ({})",
        u.texts.len()
    );
    let catalog: BTreeSet<&str> = catalog_entries().map(|(en, _)| en).collect();
    let missing: Vec<&String> = u
        .texts
        .iter()
        .filter(|t| !catalog.contains(t.as_str()))
        .collect();
    assert!(
        missing.is_empty(),
        "fără traducere românească: {missing:#?}"
    );

    let plurals: BTreeSet<&str> = plural_entries().map(|(k, _)| k).collect();
    let missing: Vec<&String> = u
        .plurals
        .iter()
        .filter(|t| !plurals.contains(t.as_str()))
        .collect();
    assert!(missing.is_empty(), "plural fără traducere: {missing:#?}");
}

#[test]
fn catalog_has_no_unused_entries() {
    let u = collect();
    let unused: Vec<&str> = catalog_entries()
        .map(|(en, _)| en)
        .filter(|en| !u.texts.contains(*en))
        .collect();
    assert!(
        unused.is_empty(),
        "texte care nu mai apar în cod: {unused:#?}"
    );
    let unused: Vec<&str> = plural_entries()
        .map(|(k, _)| k)
        .filter(|k| !u.plurals.contains(*k))
        .collect();
    assert!(
        unused.is_empty(),
        "plurale care nu mai apar în cod: {unused:#?}"
    );
}

#[test]
fn all_calls_use_literals() {
    // Un `tr(variabilă)` nu poate fi verificat de testele de mai sus.
    let u = collect();
    assert!(
        u.dynamic.is_empty(),
        "apeluri tr() cu argument care nu e literal: {:#?}",
        u.dynamic
    );
}
