//! English, Russian and Simplified Chinese.
//!
//! Strings are written in English in the code and looked up in a table here,
//! gettext-style, so the code reads plainly and a missing translation falls
//! back to English rather than to a key.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

#[path = "ru.rs"]
mod ru;
#[path = "zh_cn.rs"]
mod zh_cn;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    /// Whatever Windows is set to.
    #[default]
    System,
    En,
    Ru,
    #[serde(rename = "zh-cn", alias = "zh-CN", alias = "zh-Hans", alias = "zh")]
    ZhCn,
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

/// Use `lang` from now on.
pub fn set(lang: Lang) {
    let resolved = match lang {
        Lang::System => system(),
        other => other,
    };
    CURRENT.store(match resolved {
        Lang::Ru => 1,
        Lang::ZhCn => 2,
        _ => 0,
    }, Ordering::Relaxed);
}

pub fn current() -> Lang {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Lang::Ru,
        2 => Lang::ZhCn,
        _ => Lang::En,
    }
}

/// The language Windows shows its own menus in.
pub fn system() -> Lang {
    #[cfg(windows)]
    {
        let id = unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() };
        from_windows_language(id)
    }
    #[cfg(not(windows))]
    Lang::En
}

#[cfg(any(windows, test))]
fn from_windows_language(id: u16) -> Lang {
    if id & 0x3FF == 0x19 {
        Lang::Ru
    } else if matches!(id, 0x0004 | 0x0804 | 0x1004) {
        // Chinese (Simplified), China and Singapore.
        Lang::ZhCn
    } else {
        Lang::En
    }
}

fn translate(english: &'static str, language: Lang) -> &'static str {
    static RU: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    static ZH_CN: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    let table = match language {
        Lang::Ru => RU.get_or_init(|| ru::RU.iter().copied().collect()),
        Lang::ZhCn => ZH_CN.get_or_init(|| zh_cn::ZH_CN.iter().copied().collect()),
        _ => return english,
    };
    table.get(english).copied().unwrap_or(english)
}

/// `english`, in the current language.
pub fn t(english: &'static str) -> &'static str {
    translate(english, current())
}

/// `english` translated, with each `{}` replaced by the next argument.
pub fn tf(english: &'static str, args: &[&dyn std::fmt::Display]) -> String {
    fill(t(english), args)
}

fn fill(template: &str, args: &[&dyn std::fmt::Display]) -> String {
    let mut out = String::new();
    let mut args = args.iter();
    let mut rest = template;
    while let Some(at) = rest.find("{}") {
        out.push_str(&rest[..at]);
        if let Some(arg) = args.next() {
            out.push_str(&arg.to_string());
        }
        rest = &rest[at + 2..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_match_in_every_translation() {
        for (english, translated) in ru::RU.iter().chain(zh_cn::ZH_CN) {
            assert_eq!(english.matches("{}").count(), translated.matches("{}").count(), "{english:?} / {translated:?}");
            assert!(!translated.trim().is_empty(), "empty translation: {english:?}");
        }
    }

    #[test]
    fn no_string_is_translated_twice() {
        for pairs in [ru::RU, zh_cn::ZH_CN] {
            let mut seen = std::collections::HashSet::new();
            for (english, _) in pairs {
                assert!(seen.insert(*english), "translated twice: {english:?}");
            }
        }
    }

    #[test]
    fn arguments_fill_the_placeholders_in_order() {
        assert_eq!(fill("{} of {}", &[&1, &2]), "1 of 2");
        assert_eq!(fill(translate("Stage {} · {}", Lang::ZhCn), &[&2, &1600]), "档位 2 · 1600");
    }

    #[test]
    fn languages_cover_the_same_strings() {
        let keys = |pairs: &[(&'static str, &'static str)]| pairs.iter().map(|(key, _)| *key).collect::<std::collections::HashSet<_>>();
        assert_eq!(keys(ru::RU), keys(zh_cn::ZH_CN));
    }

    #[test]
    fn chinese_selection_and_english_fallback() {
        set(Lang::ZhCn);
        assert_eq!(current(), Lang::ZhCn);
        assert_eq!(t("Settings"), "设置");
        assert_eq!(tf("Stage {}", &[&3]), "档位 3");
        assert_eq!(tf("{} of {}", &[&1, &2]), "1 / 2");
        let unknown = "untranslated fallback";
        assert_eq!(t(unknown), unknown);
        set(Lang::En);
        assert_eq!(t("Settings"), "Settings");
        assert_eq!(translate("Settings", Lang::Ru), "Настройки");
    }

    #[test]
    fn language_preferences_round_trip() {
        for lang in [Lang::System, Lang::En, Lang::Ru, Lang::ZhCn] {
            let json = serde_json::to_string(&lang).unwrap();
            assert_eq!(serde_json::from_str::<Lang>(&json).unwrap(), lang);
        }
        assert_eq!(serde_json::to_string(&Lang::ZhCn).unwrap(), "\"zh-cn\"");
        for alias in ["zh", "zh-CN", "zh-Hans"] {
            assert_eq!(serde_json::from_value::<Lang>(serde_json::json!(alias)).unwrap(), Lang::ZhCn);
        }
    }

    #[test]
    fn windows_ui_language_is_resolved() {
        for id in [0x0004, 0x0804, 0x1004] {
            assert_eq!(from_windows_language(id), Lang::ZhCn);
        }
        assert_eq!(from_windows_language(0x0419), Lang::Ru);
        for id in [0x0409, 0x0407, 0x0404, 0x0c04] {
            assert_eq!(from_windows_language(id), Lang::En);
        }
    }
}
