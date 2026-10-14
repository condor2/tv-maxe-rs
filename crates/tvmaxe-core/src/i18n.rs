//! Textele interfeței: engleză (implicit) și română.
//!
//! Textul englezesc din cod e și cheia de căutare, iar catalogul românesc e
//! compilat în binar ([`catalog_ro`]), deci nu trebuie instalate fișiere de
//! traducere. Limba se alege din mediu, ca la orice program Unix:
//! `LANGUAGE`, apoi `LC_ALL`, `LC_MESSAGES`, `LANG`.
//!
//! ```text
//! tr("Subscriptions")                       // „Abonamente” pe un sistem în română
//! fill(tr("Playing: %1"), &[&name])         // %1…%9 înlocuite într-o singură trecere
//! trn("%n channel", "%n channels", n)       // plural (în română: 1 / 2–19 / 20+ „de”)
//! ```

mod catalog_ro;

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    English,
    Romanian,
}

static LANGUAGE: OnceLock<Language> = OnceLock::new();

/// Limba aleasă din mediul procesului (citită o singură dată).
pub fn language() -> Language {
    *LANGUAGE.get_or_init(|| detect_language(|name| std::env::var(name).ok()))
}

/// Fixează limba înainte de prima traducere (testele de integrare compară
/// texte, deci nu pot depinde de limba sistemului). Întoarce `false` dacă
/// limba fusese deja aleasă altfel.
#[doc(hidden)]
pub fn force_language(lang: Language) -> bool {
    *LANGUAGE.get_or_init(|| lang) == lang
}

/// Ordinea obișnuită pe Unix: fiecare intrare din `LANGUAGE`, apoi locale-ul
/// efectiv din `LC_ALL`, `LC_MESSAGES` sau `LANG`. O preferință explicită
/// pentru engleză (sau `C` / `POSIX`) oprește căutarea.
pub fn detect_language(var: impl Fn(&str) -> Option<String>) -> Language {
    let mut candidates: Vec<String> = Vec::new();
    if let Some(list) = var("LANGUAGE") {
        candidates.extend(list.split(':').filter(|s| !s.is_empty()).map(str::to_owned));
    }
    if let Some(locale) = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|name| var(name))
        .find(|v| !v.is_empty())
    {
        candidates.push(locale);
    }
    for candidate in candidates {
        let lower = candidate.to_ascii_lowercase();
        if lower.starts_with("ro") {
            return Language::Romanian;
        }
        if lower.starts_with("en") || lower == "c" || lower.starts_with("c.") || lower == "posix" {
            return Language::English;
        }
    }
    Language::English
}

fn catalog() -> &'static HashMap<&'static str, &'static str> {
    static CATALOG: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    CATALOG.get_or_init(|| catalog_ro::RO.iter().copied().collect())
}

/// Traduce un text. Textele necunoscute rămân neschimbate (în engleză).
pub fn tr(source: &'static str) -> &'static str {
    match language() {
        Language::English => source,
        Language::Romanian => catalog().get(source).copied().unwrap_or(source),
    }
}

/// Forma de plural românească (aceeași regulă ca gettext pentru `ro`):
/// 0 = „1 canal”, 1 = „2 canale”, 2 = „20 de canale”.
pub fn romanian_plural_form(n: u64) -> usize {
    if n == 1 {
        0
    } else if n == 0 || (1..=19).contains(&(n % 100)) {
        1
    } else {
        2
    }
}

/// Text cu plural. `one` e și cheia în catalog; `%n` devine numărul.
pub fn trn(one: &'static str, other: &'static str, n: u64) -> String {
    let template = match language() {
        Language::English => {
            if n == 1 {
                one
            } else {
                other
            }
        }
        Language::Romanian => catalog_ro::RO_PLURALS
            .iter()
            .find(|(key, _)| *key == one)
            .map(|(_, forms)| forms[romanian_plural_form(n)])
            .unwrap_or(if n == 1 { one } else { other }),
    };
    template.replace("%n", &n.to_string())
}

/// Înlocuiește `%1`…`%9` într-o singură trecere: textul pus în locul unui
/// marcaj nu e reinterpretat (un nume de canal care conține „%2” rămâne așa).
pub fn fill(template: &str, args: &[&dyn Display]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut chars = template.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '%'
            && let Some(d) = chars.peek().and_then(|c| c.to_digit(10))
            && d >= 1
            && (d as usize) <= args.len()
        {
            chars.next();
            out.push_str(&args[d as usize - 1].to_string());
            continue;
        }
        out.push(ch);
    }
    out
}

/// Cheile catalogului (pentru testul care verifică acoperirea traducerilor).
#[doc(hidden)]
pub fn catalog_entries() -> impl Iterator<Item = (&'static str, &'static str)> {
    catalog_ro::RO.iter().copied()
}

#[doc(hidden)]
pub fn plural_entries() -> impl Iterator<Item = (&'static str, [&'static str; 3])> {
    catalog_ro::RO_PLURALS.iter().copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn detects_language_like_gettext() {
        use Language::*;
        assert_eq!(detect_language(env(&[])), English);
        assert_eq!(detect_language(env(&[("LANG", "ro_RO.UTF-8")])), Romanian);
        assert_eq!(detect_language(env(&[("LANG", "de_DE.UTF-8")])), English);
        assert_eq!(
            detect_language(env(&[("LANG", "ro_RO.UTF-8"), ("LC_ALL", "C")])),
            English,
            "LC_ALL are prioritate"
        );
        assert_eq!(
            detect_language(env(&[
                ("LANG", "en_US.UTF-8"),
                ("LC_MESSAGES", "ro_RO.UTF-8")
            ])),
            Romanian
        );
        assert_eq!(
            detect_language(env(&[("LANGUAGE", "de:ro"), ("LANG", "de_DE.UTF-8")])),
            Romanian,
            "a doua preferință din LANGUAGE"
        );
        assert_eq!(
            detect_language(env(&[("LANGUAGE", "en_GB:ro"), ("LANG", "ro_RO.UTF-8")])),
            English
        );
    }

    #[test]
    fn plural_forms() {
        let forms: Vec<usize> = [0, 1, 2, 19, 20, 101, 102, 119, 120, 1000]
            .iter()
            .map(|&n| romanian_plural_form(n))
            .collect();
        assert_eq!(forms, [1, 0, 1, 1, 2, 1, 1, 1, 2, 2]);
    }

    #[test]
    fn fill_is_single_pass() {
        assert_eq!(fill("%1 · %2", &[&"a %2", &"b"]), "a %2 · b");
        assert_eq!(fill("100% %1", &[&5]), "100% 5");
        assert_eq!(fill("%3 lipsă", &[&1]), "%3 lipsă");
    }

    #[test]
    fn catalog_is_well_formed() {
        let mut seen = std::collections::HashSet::new();
        for (en, ro) in catalog_entries() {
            assert!(seen.insert(en), "cheie duplicată: {en:?}");
            assert!(!ro.trim().is_empty(), "traducere goală: {en:?}");
            assert_eq!(
                placeholders(en),
                placeholders(ro),
                "marcaje diferite: {en:?}"
            );
        }
        for (one, forms) in plural_entries() {
            for f in forms {
                assert!(f.contains("%n"), "{one:?} → {f:?} fără %n");
            }
        }
    }

    fn placeholders(s: &str) -> Vec<char> {
        let mut v: Vec<char> = s
            .match_indices('%')
            .filter_map(|(i, _)| s[i + 1..].chars().next())
            .filter(|c| c.is_ascii_digit() || *c == 'n')
            .collect();
        v.sort_unstable();
        v
    }
}
