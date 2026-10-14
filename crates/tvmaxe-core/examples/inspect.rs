//! Afișează conținutul unei liste: `cargo run -p tvmaxe-core --example inspect -- lista.db`
use tvmaxe_core::{Kind, Logo, formats};

fn main() {
    for path in std::env::args().skip(1) {
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("{path}: {e}");
                continue;
            }
        };
        match formats::parse(&data) {
            Ok(l) => {
                let tv = l.channels.iter().filter(|c| c.kind == Kind::Tv).count();
                let logos = l.channels.iter().filter(|c| c.logo != Logo::None).count();
                println!(
                    "{path}: {:?} · {:?} · {tv} TV, {} radio, {logos} logo-uri",
                    formats::detect(&data).unwrap(),
                    l.name.as_deref().unwrap_or("-"),
                    l.channels.len() - tv
                );
                if let Some(c) = l.channels.first() {
                    println!("  ex: {} → {}", c.name, c.urls.join(" | "));
                }
            }
            Err(e) => println!("{path}: {e}"),
        }
    }
}
