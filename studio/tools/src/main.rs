//! Development tools; independent of the Windows application and Python.
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use syn::visit::Visit;

#[path = "../../../assets/branding.rs"]
mod branding;

#[path = "../../core/src/ru.rs"]
mod ru;
#[path = "../../core/src/zh_cn.rs"]
mod zh_cn;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const KEY_NAMES: &[&str] = &[
    "Mute",
    "Volume down",
    "Volume up",
    "Next track",
    "Previous track",
    "Play/Pause",
    "Right Ctrl",
    "Right Shift",
    "Right Alt",
];
const UNTRANSLATED_KEYS: &[&str] = &[
    "Enter",
    "Esc",
    "Tab",
    "Backspace",
    "Caps Lock",
    "Insert",
    "End",
    "Page Up",
    "Page Down",
    "Print Screen",
];
const DOWNLOAD: &str = "**Mirage Hub** — download `mirage-hub-setup.exe`. The installer includes the native Rust application and daemon, with English, Simplified Chinese and Russian. No Python installation is required.\n\n**Upgrading from RsRazer:** download and run this installer once. Existing settings are copied to `%LOCALAPPDATA%\\mirage-hub`; the old settings remain intact. Subsequent updates use Mirage Hub's built-in updater.\n\n`mirage-hub-classic.exe` is the standalone classic panel; `mirage-hub-daemon.exe` is the CLI and background service. `SHA256SUMS.txt` lists the file checksums.\n\nBased on [OpenRazer](https://github.com/openrazer/openrazer), [Ar4ikov/openrazer-win](https://github.com/Ar4ikov/openrazer-win) and [OpenInzone](https://github.com/penguinwokrs/openinzone). See the [Mirage Hub documentation](https://imacte.github.io/MirageHub/).\n";

#[derive(Default)]
struct Strings(BTreeSet<String>);
impl<'ast> Visit<'ast> for Strings {
    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        self.macro_strings(node.tokens.clone());
    }
    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if let syn::Expr::Path(function) = call.func.as_ref() {
            if function
                .path
                .segments
                .last()
                .is_some_and(|s| s.ident == "t" || s.ident == "tf")
            {
                if let Some(syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(text),
                    ..
                })) = call.args.first()
                {
                    self.0.insert(text.value());
                }
            }
        }
        syn::visit::visit_expr_call(self, call);
    }
}
impl Strings {
    fn macro_strings(&mut self, tokens: proc_macro2::TokenStream) {
        use proc_macro2::{Delimiter, TokenTree};
        let mut tokens = tokens.into_iter().peekable();
        while let Some(token) = tokens.next() {
            match token {
                TokenTree::Ident(name) if name == "t" || name == "tf" => {
                    if let Some(TokenTree::Group(args)) = tokens.peek() {
                        if args.delimiter() == Delimiter::Parenthesis {
                            if let Some(TokenTree::Literal(value)) = args.stream().into_iter().next() {
                                if let Ok(text) = syn::parse_str::<syn::LitStr>(&value.to_string()) {
                                    self.0.insert(text.value());
                                }
                            }
                        }
                    }
                }
                TokenTree::Group(group) => self.macro_strings(group.stream()),
                _ => {}
            }
        }
    }
}
fn rust_files(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            rust_files(&path, files)?;
        } else if path.extension().is_some_and(|e| e == "rs") {
            files.push(path);
        }
    }
    Ok(())
}
fn check_translations(root: &Path) -> Result<()> {
    let mut strings = Strings::default();
    for package in ["app", "core", "setup"] {
        let mut files = Vec::new();
        rust_files(&root.join("studio").join(package).join("src"), &mut files)?;
        for path in files {
            if path
                .file_name()
                .is_some_and(|p| p == "ru.rs" || p == "zh_cn.rs")
            {
                continue;
            }
            strings.visit_file(&syn::parse_file(&fs::read_to_string(path)?)?);
        }
    }
    strings.0.extend(KEY_NAMES.iter().map(|s| s.to_string()));
    let mut failed = false;
    for (language, table) in [("Russian", ru::RU), ("Simplified Chinese", zh_cn::ZH_CN)] {
        let mut translated = BTreeSet::new();
        for &(key, value) in table {
            if !translated.insert(key.to_string()) {
                eprintln!("duplicate {language} key: {key}");
                failed = true;
            }
            if key.matches("{}").count() != value.matches("{}").count() {
                eprintln!("{language} placeholder mismatch: {key}");
                failed = true;
            }
        }
        let missing: Vec<_> = strings.0.difference(&translated).collect();
        let unused: Vec<_> = translated
            .difference(&strings.0)
            .filter(|s| !UNTRANSLATED_KEYS.contains(&s.as_str()))
            .collect();
        for text in &missing {
            eprintln!("no {language} for: {text}");
        }
        for text in &unused {
            eprintln!("{language} translated but not used: {text}");
        }
        println!(
            "{language}: {} strings, {} missing, {} unused",
            strings.0.len(),
            missing.len(),
            unused.len()
        );
        failed |= !missing.is_empty() || !unused.is_empty();
    }
    if failed {
        Err("translation check failed".into())
    } else {
        Ok(())
    }
}
fn docs_data(root: &Path, check: bool) -> Result<()> {
    let database: Value = serde_json::from_slice(&fs::read(root.join("data/devices.json"))?)?;
    let inzone: Value = serde_json::from_slice(&fs::read(root.join("data/inzone-devices.json"))?)?;
    let mut rows = database["devices"].as_array().ok_or("database has no devices")?.iter().chain(inzone["devices"].as_array().ok_or("INZONE database has no devices")?).map(|d| {
        json!({"n":d["name"],"v":format!("{:04x}",d["vid"].as_u64().unwrap_or_default()),"p":format!("{:04x}",d["pid"].as_u64().unwrap_or_default()),"t":d["type"],
            "m":d["matrix_dims"],"d":d["dpi_max"],"b":d["methods"].as_array().is_some_and(|m|m.iter().any(|v|v=="get_battery" || v=="get_battery_level")),
            "i":d["image"].as_str().unwrap_or_default()})
    }).collect::<Vec<_>>();
    rows.sort_by(|a, b| {
        (a["t"].as_str(), a["n"].as_str()).cmp(&(b["t"].as_str(), b["n"].as_str()))
    });
    let version = fs::read_to_string(root.join("data/upstream-version.txt"))?;
    let output = json!({"generated_from":format!("openrazer {}; openinzone {}",version.trim(),inzone["revision"].as_str().unwrap_or_default()),"count":rows.len(),"devices":rows});
    let path = root.join("docs/devices.json");
    if check {
        let saved: Value = serde_json::from_slice(&fs::read(&path)?)?;
        if saved != output {
            return Err("docs/devices.json is stale; run mirage-hub-tools docs-data".into());
        }
    } else {
        fs::write(&path, format!("{}\n", serde_json::to_string(&output)?))?;
    }
    println!("{}: {} devices", path.display(), rows.len());
    Ok(())
}
fn section(changelog: &str, version: &str) -> Option<String> {
    let heading = format!("## [{version}]");
    let mut lines = changelog.lines();
    lines.find(|line| line.starts_with(&heading))?;
    Some(
        lines
            .take_while(|line| !line.starts_with("## ["))
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string(),
    )
}
fn release_body(version: &str, english: &str, russian: &str, chinese: &str) -> Result<String> {
    let notes = section(english, version)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("CHANGELOG.md has no notes for {version}"))?;
    let mut body = format!("<!-- notes:en -->\n{notes}\n<!-- /notes -->\n\n");
    for (language, label, text) in [("ru", "По-русски", russian), ("zh-CN", "简体中文", chinese)]
    {
        if let Some(notes) = section(text, version).filter(|s| !s.is_empty()) {
            body.push_str(&format!("<details>\n<summary>{label}</summary>\n\n<!-- notes:{language} -->\n{notes}\n<!-- /notes -->\n\n</details>\n\n"));
        }
    }
    body.push_str("---\n\n");
    body.push_str(DOWNLOAD);
    Ok(body)
}
fn run() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("icons") if args.len()==1 => icons(&root),
        Some("check-translations") if args.len()==1 => check_translations(&root),
        Some("docs-data") if args.len()==1 || (args.len()==2 && args[1]=="--check") => docs_data(&root,args.len()==2),
        Some("release-notes") if args.len()==2 => {
            let version = args[1].trim_start_matches('v');
            let body = release_body(version,&fs::read_to_string(root.join("CHANGELOG.md"))?,
                &fs::read_to_string(root.join("CHANGELOG.ru.md")).unwrap_or_default(),
                &fs::read_to_string(root.join("CHANGELOG.zh-CN.md")).unwrap_or_default())?;
            std::io::stdout().write_all(body.as_bytes())?; Ok(())
        }
        _ => Err("Usage: mirage-hub-tools check-translations | docs-data [--check] | release-notes <version> | icons".into()),
    }
}
fn icons(root: &Path) -> Result<()> {
    let sizes = [16u32, 24, 32, 48, 64, 128, 256];
    let mut images = Vec::new();
    for size in sizes {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, size, size);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()?
                .write_image_data(&branding::icon_rgba(size))?;
        }
        images.push(bytes);
    }
    let mut ico = vec![0, 0, 1, 0, sizes.len() as u8, 0];
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (size, bytes) in sizes.iter().zip(&images) {
        ico.extend([*size as u8, *size as u8, 0, 0, 1, 0, 32, 0]);
        ico.extend((bytes.len() as u32).to_le_bytes());
        ico.extend(offset.to_le_bytes());
        offset += bytes.len() as u32;
    }
    for bytes in &images {
        ico.extend(bytes);
    }
    fs::write(root.join("assets/mirage-hub.ico"), ico)?;
    fs::write(root.join("assets/mirage-hub.png"), &images[6])?;
    fs::write(root.join("docs/favicon.svg"), icon_svg())?;
    println!("Generated assets/mirage-hub.ico, assets/mirage-hub.png and docs/favicon.svg");
    Ok(())
}
fn icon_svg() -> String {
    let [r, g, b] = branding::BACKGROUND;
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {0} {0}\">\n  <title>Mirage Hub</title>\n  <rect width=\"{0}\" height=\"{0}\" rx=\"{1}\" fill=\"#{r:02x}{g:02x}{b:02x}\"/>\n",
        branding::VIEW_BOX,
        branding::CORNER_RADIUS * branding::VIEW_BOX,
    );
    let [r, g, b] = branding::GREEN;
    svg.push_str(&format!(
        "  <path id=\"loop\" fill=\"none\" stroke=\"#{r:02x}{g:02x}{b:02x}\" stroke-width=\"{}\" stroke-linecap=\"round\" stroke-linejoin=\"round\" d=\"\n",
        branding::WIDTH * branding::VIEW_BOX,
    ));
    for curve in branding::CURVES {
        svg.push_str(&format!("    M {:.2} {:.2}\n", curve[0][0], curve[0][1]));
        for c in curve[1..].chunks_exact(3) {
            svg.push_str(&format!(
                "    C {:.2} {:.2}, {:.2} {:.2}, {:.2} {:.2}\n",
                c[0][0], c[0][1], c[1][0], c[1][1], c[2][0], c[2][1],
            ));
        }
    }
    svg.push_str("  \"/>\n</svg>\n");
    svg
}
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    const NOTES: &str = "# Changes\n## [Unreleased]\n\n## [1.5.3] - 2026-11-03\n\n### Added\n\n- A thing.\n\n## [1.5.2]\n- Older.\n";
    #[test]
    fn changelog_versions_have_exact_boundaries() {
        assert_eq!(
            section(NOTES, "1.5.3").as_deref(),
            Some("### Added\n\n- A thing.")
        );
        assert_eq!(section(NOTES, "1.5.2").as_deref(), Some("- Older."));
        assert_eq!(section(NOTES, "1.5"), None);
        assert!(release_body("9.9.9", NOTES, "", "").is_err());
        assert!(release_body("Unreleased", NOTES, "", "").is_err());
    }
    #[test]
    fn release_notes_keep_language_markers_and_native_downloads() {
        let text = release_body("1.5.3", NOTES, NOTES, NOTES).unwrap();
        for language in ["en", "ru", "zh-CN"] {
            assert!(text.contains(&format!("<!-- notes:{language} -->")));
        }
        assert!(text.contains("mirage-hub-setup.exe"));
        assert!(!text.contains("pip install"));
        let text = release_body("1.5.3", NOTES, "", "").unwrap();
        assert!(!text.contains("<details>"));
    }
    #[test]
    fn translation_extraction_handles_raw_strings_escapes_and_ignores_comments() {
        let mut strings = Strings::default();
        strings.visit_file(&syn::parse_file(r##"fn f() { t("one\nline"); crate::tf(r#"raw "quotes""#, &[]); /* t("unused") */ }"##).unwrap());
        assert_eq!(
            strings.0,
            BTreeSet::from(["one\nline".into(), "raw \"quotes\"".into()])
        );
        strings.visit_file(&syn::parse_file(r#"fn f() { vec![t("inside a macro"), format!("{}", tf("nested {}", &[]))]; }"#).unwrap());
        assert!(strings.0.contains("inside a macro"));
        assert!(strings.0.contains("nested {}"));
    }
}
