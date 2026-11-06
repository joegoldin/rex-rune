//! Bundled chrome fonts, a fontconfig-resolved terminal family and ordered fallback faces.
use std::{collections::HashSet, path::PathBuf, process::Command};

pub struct Fonts {
    pub files: &'static [&'static [u8]],
    pub families: (&'static str, &'static str),
    /// The resolved primary family, for Settings; the grid uses the mono slot's full stack.
    pub terminal: Option<&'static str>,
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn pattern(family: &str) -> String {
    family
        .replace('\\', "\\\\")
        .replace(':', "\\:")
        .replace(',', "\\,")
}

/// Resolve four styles without accepting an unrelated substitute for a named family.
/// Keep paths until registration so a collection shared by several families is read once.
fn resolve(family: &str) -> Option<(String, Vec<PathBuf>)> {
    let generic = ["monospace", "sans-serif", "serif"]
        .iter()
        .any(|name| family.eq_ignore_ascii_case(name));
    let mut name = None;
    let mut files = Vec::new();
    for style in ["", ":bold", ":italic", ":bold:italic"] {
        let out = Command::new("fc-match")
            .args([
                "-f",
                "%{family[0]}\n%{file}",
                "--",
                &format!("{}{style}", pattern(family)),
            ])
            .output()
            .ok()?;
        if !out.status.success() {
            continue;
        }
        let out = String::from_utf8_lossy(&out.stdout);
        let Some((found, file)) = out.split_once('\n') else {
            continue;
        };
        if !generic && !found.eq_ignore_ascii_case(family) {
            continue;
        }
        let file = PathBuf::from(file.trim());
        if file.is_file() && !files.contains(&file) {
            name.get_or_insert_with(|| found.to_owned());
            files.push(file);
        }
    }
    Some((name?, files))
}

fn css_family(family: &str) -> String {
    format!("'{}'", family.replace('\\', "\\\\").replace('\'', "\\'"))
}

pub fn load(terminal_family: &str, fallbacks: &[String]) -> Fonts {
    let mut files: Vec<&'static [u8]> = rune_theme_opencode::FONTS.to_vec();
    let mut seen = HashSet::new();
    let mut names = Vec::new();
    let mut terminal = None;
    for (index, family) in std::iter::once(terminal_family)
        .chain(fallbacks.iter().map(String::as_str))
        .enumerate()
    {
        let bundled = [
            rune_theme_opencode::FAMILIES.0,
            rune_theme_opencode::FAMILIES.1,
        ]
        .into_iter()
        .find(|name| name.eq_ignore_ascii_case(family));
        let resolved = bundled.map(str::to_owned).or_else(|| {
            resolve(family).and_then(|(name, paths)| {
                let mut available = false;
                for path in paths {
                    if seen.contains(&path) {
                        available = true;
                    } else if let Ok(bytes) = std::fs::read(&path) {
                        seen.insert(path);
                        files.push(Box::leak(bytes.into_boxed_slice()));
                        available = true;
                    }
                }
                available.then_some(name)
            })
        });
        match resolved {
            Some(name) => {
                if index == 0 {
                    terminal = Some(leak(name.clone()));
                }
                let name = css_family(&name);
                if !names.contains(&name) {
                    names.push(name);
                }
            }
            None => {
                if index == 0 {
                    names.push(css_family("IBM Plex Mono"));
                }
                // IBM Plex Mono is already registered from the kit, not fontconfig.
                if family != "IBM Plex Mono" {
                    eprintln!(
                        "Terminal font {family:?} not found; {}.",
                        if index == 0 {
                            "using IBM Plex Mono"
                        } else {
                            "skipping this fallback"
                        }
                    );
                }
            }
        }
    }
    let bundled = css_family("IBM Plex Mono");
    if !names.contains(&bundled) {
        names.push(bundled);
    }
    Fonts {
        files: Box::leak(files.into_boxed_slice()),
        families: (rune_theme_opencode::FAMILIES.0, leak(names.join(", "))),
        terminal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_stack_keeps_chrome_separate_and_preserves_fallback_order() {
        let fonts = load("IBM Plex Mono", &["Inter".into()]);
        assert_eq!(fonts.families.0, rune_theme_opencode::FAMILIES.0);
        assert!(fonts.families.1.starts_with("'IBM Plex Mono', 'Inter'"));
        assert_eq!(fonts.terminal, Some("IBM Plex Mono"));
        assert_eq!(
            fonts.files.len(),
            rune_theme_opencode::FONTS.len(),
            "bundled faces are not re-read"
        );
        assert_eq!(pattern(r"A:B\C"), r"A\:B\\C");
        assert_eq!(css_family("Joe's Mono"), "'Joe\\'s Mono'");
    }

    #[test]
    #[ignore = "requires fontconfig and a CJK font; exercised by the Nix package check"]
    fn fontconfig_cjk_fallback_has_real_glyphs_and_keeps_ascii_metrics() {
        let fonts = load("IBM Plex Mono", &[]);
        let mut ts = rune::text::TextSystem::new(fonts.files, fonts.families);
        let style = rune::TextStyle::new(15., 400.).mono();
        let text = rune::text::Run::new("世界 日本語 한글");
        let shaped = ts.run_layout(&text, style, 1.);
        let mut count = 0;
        for line in shaped.layout.lines() {
            for run in line.runs() {
                for cluster in run.visual_clusters() {
                    for glyph in cluster.glyphs() {
                        assert_ne!(
                            glyph.id,
                            0,
                            "missing CJK glyph in {:?}",
                            cluster.text_range()
                        );
                        count += 1;
                    }
                }
            }
        }
        assert!(count >= 9);
        let mut bundled =
            rune::text::TextSystem::new(rune_theme_opencode::FONTS, rune_theme_opencode::FAMILIES);
        assert_eq!(
            ts.measure("MMMM 1234", style, 1.),
            bundled.measure("MMMM 1234", style, 1.)
        );
    }
}
