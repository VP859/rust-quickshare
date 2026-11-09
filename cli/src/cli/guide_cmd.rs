//! `ferry guide [TOPIC]`: short answers to the usual questions, the command
//! line counterpart of the GUI's help window (the same topics and the same
//! documentation sections, see `src-tauri/ui/js/help-topics.js`).

use anyhow::Result;
use rust_i18n::t;

/// The project website (docs built from the repository).
const SITE: &str = "https://vp859.github.io/rust-quickshare/";

/// The topics: their name and the page and section of the documentation.
const TOPICS: [(&str, &str); 7] = [
    ("send", "index.html#quick-start"),
    ("pair", "index.html#local-network-and-pairing"),
    (
        "browser",
        "index.html#browser-sessions-no-app-on-the-other-device",
    ),
    ("apple", "index.html#localsend-compatibility-mode"),
    ("internet", "index.html#network-modes"),
    ("trouble", "index.html#faq"),
    ("privacy", "SECURITY-MODEL.html#what-each-party-can-see"),
];

/// Prints one topic or all of them.
///
/// # Errors
///
/// Fails for a topic that does not exist.
pub fn run(topic: Option<&str>) -> Result<()> {
    let chosen: Vec<_> = match topic {
        None => {
            println!("{}\n", t!("guide.intro"));
            TOPICS.to_vec()
        }
        Some(name) => {
            let found: Vec<_> = TOPICS
                .iter()
                .filter(|(id, _)| id.eq_ignore_ascii_case(name.trim()))
                .copied()
                .collect();
            if found.is_empty() {
                let topics = TOPICS
                    .iter()
                    .map(|(id, _)| *id)
                    .collect::<Vec<_>>()
                    .join(", ");
                anyhow::bail!(t!("guide.unknown", topic = name, topics = topics));
            }
            found
        }
    };
    for (id, page) in chosen {
        println!("{}  ({id})", t!(format!("guide.{id}_q")));
        println!("  {}", t!(format!("guide.{id}_a")));
        println!("  {} {SITE}{page}\n", t!("guide.more"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_topic_has_a_question_and_an_answer_in_every_language() {
        for lang in ["en", "pl"] {
            for (id, _) in TOPICS {
                for part in ["q", "a"] {
                    let key = format!("guide.{id}_{part}");
                    let text = t!(&key, locale = lang);
                    assert_ne!(text, key, "{key} in {lang}");
                }
            }
        }
    }

    #[test]
    fn topics_are_the_ones_of_the_gui() {
        let gui = include_str!("../../../src-tauri/ui/js/help-topics.js");
        for (id, page) in TOPICS {
            assert!(gui.contains(&format!("id: '{id}'")), "{id} is a GUI topic");
            let anchor = page.split('#').nth(1).unwrap();
            assert!(
                gui.contains(&format!("#{anchor}'")),
                "{id} links where the GUI does"
            );
        }
        assert!(super::run(Some("nope")).is_err());
        assert!(super::run(Some("PAIR")).is_ok());
    }
}
