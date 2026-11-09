use std::{
    collections::BTreeMap,
    sync::{
        LazyLock,
        atomic::{AtomicUsize, Ordering},
    },
};

pub const LANGUAGES: [(&str, &str); 6] = [
    ("ru", "Русский"),
    ("en", "English"),
    ("de", "Deutsch"),
    ("fr", "Français"),
    ("es", "Español"),
    ("it", "Italiano"),
];
static ACTIVE: AtomicUsize = AtomicUsize::new(0);
static CATALOGS: LazyLock<Vec<BTreeMap<String, String>>> = LazyLock::new(|| {
    [
        include_str!("../locales/ru.json"),
        include_str!("../locales/en.json"),
        include_str!("../locales/de.json"),
        include_str!("../locales/fr.json"),
        include_str!("../locales/es.json"),
        include_str!("../locales/it.json"),
    ]
    .into_iter()
    .map(|data| serde_json::from_str(data).expect("valid UI catalog"))
    .collect()
});

pub fn set_language(code: &str) {
    ACTIVE.store(
        LANGUAGES
            .iter()
            .position(|(key, _)| *key == code)
            .unwrap_or(1),
        Ordering::Relaxed,
    );
}

pub fn current_code() -> &'static str {
    LANGUAGES[ACTIVE.load(Ordering::Relaxed)].0
}

/// Maps an operating-system locale to a supported UI language.
///
/// The locale may be a bare language (`fr`), a language and region with either
/// separator (`fr_FR`, `fr-FR`), a POSIX locale with a charset (`fr_FR.UTF-8`)
/// or a modifier (`sr_RS@latin`). Anything unknown falls back to English.
fn locale_language(locale: &str) -> &'static str {
    let first = locale.split(':').next().unwrap_or("").trim();
    let code = first
        .split(['.', '@'])
        .next()
        .unwrap_or("")
        .replace('-', "_");
    let primary = code.split('_').next().unwrap_or("").to_ascii_lowercase();
    match primary.as_str() {
        "ru" => "ru",
        "de" => "de",
        "fr" => "fr",
        "es" => "es",
        "it" => "it",
        _ => "en",
    }
}

/// The language to use when no preference has been stored yet.
pub fn system_language() -> &'static str {
    locale_language(&os_locale())
}

#[cfg(windows)]
fn os_locale() -> String {
    // SAFETY: GetUserDefaultUILanguage takes no arguments, returns a plain
    // language identifier and transfers no ownership.
    let id = unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() };
    if id == 0 {
        return String::new();
    }
    // A LANGID packs the primary language in the low ten bits.
    match id & 0x3ff {
        0x19 => "ru-RU",
        0x07 => "de-DE",
        0x0c => "fr-FR",
        0x0a => "es-ES",
        0x10 => "it-IT",
        _ => "en-US",
    }
    .to_string()
}

#[cfg(not(windows))]
fn os_locale() -> String {
    for key in ["LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE"] {
        if let Ok(value) = std::env::var(key) {
            let value = value.trim();
            if !value.is_empty() && value != "C" && value != "POSIX" {
                return value.to_string();
            }
        }
    }
    String::new()
}

pub fn tr_language(code: &str, text: &str) -> String {
    translate(
        LANGUAGES
            .iter()
            .position(|(key, _)| *key == code)
            .unwrap_or(1),
        text,
    )
}

pub fn tr(text: impl AsRef<str>) -> String {
    tr_language(current_code(), text.as_ref())
}

fn translate(language: usize, text: &str) -> String {
    let Some(catalog) = CATALOGS.get(language) else {
        return text.into();
    };
    if let Some(value) = catalog.get(text) {
        return value.clone();
    }
    // Translate formatted statuses without translating user-entered values.
    for (source, target) in catalog.iter().filter(|(key, _)| key.contains('{')) {
        if let Some(value) = interpolate(source, target, text) {
            return value;
        }
    }
    for (source, target) in catalog {
        if let Some(rest) = text
            .strip_prefix(source.as_str())
            .and_then(|rest| rest.strip_prefix(": "))
        {
            return format!("{target}: {rest}");
        }
    }
    text.into()
}

fn parts(template: &str) -> Vec<(&str, bool)> {
    let mut result = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        let Some(end) = rest[start + 1..].find('}') else {
            break;
        };
        result.push((&rest[..start], false));
        result.push((&rest[start + 1..start + 1 + end], true));
        rest = &rest[start + end + 2..];
    }
    result.push((rest, false));
    result
}

fn interpolate(source: &str, target: &str, text: &str) -> Option<String> {
    let source = parts(source);
    let mut captures = Vec::new();
    let mut rest = text;
    for (index, (part, field)) in source.iter().enumerate() {
        if *field {
            let separator = source.get(index + 1)?.0;
            let end = if separator.is_empty() {
                rest.len()
            } else {
                rest.find(separator)?
            };
            captures.push((*part, &rest[..end]));
            rest = &rest[end..];
        } else {
            rest = rest.strip_prefix(part)?;
        }
    }
    if !rest.is_empty() {
        return None;
    }
    let mut result = String::new();
    let mut position = 0;
    for (part, field) in parts(target) {
        if field {
            let value = if part.is_empty() {
                let value = captures.get(position)?.1;
                position += 1;
                value
            } else {
                captures.iter().find(|(name, _)| *name == part)?.1
            };
            result.push_str(value);
        } else {
            result.push_str(part);
        }
    }
    Some(result)
}

pub trait Localized {
    type Output;
    fn localized(self) -> Self::Output;
}
impl Localized for &str {
    type Output = String;
    fn localized(self) -> String {
        tr(self)
    }
}
impl Localized for String {
    type Output = String;
    fn localized(self) -> String {
        tr(self)
    }
}
impl Localized for &String {
    type Output = String;
    fn localized(self) -> String {
        tr(self)
    }
}
impl Localized for &&str {
    type Output = String;
    fn localized(self) -> String {
        tr(self)
    }
}
impl Localized for eframe::egui::RichText {
    type Output = Self;
    fn localized(self) -> Self {
        self
    }
}
impl Localized for eframe::egui::WidgetText {
    type Output = Self;
    fn localized(self) -> Self {
        self
    }
}
pub fn ui_text<T: Localized>(text: T) -> T::Output {
    text.localized()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interpolation_keeps_values_and_allows_translated_order() {
        assert_eq!(
            interpolate(
                "Версия {version}, код {code:02X}",
                "Code {code:02X}, version {version}",
                "Версия 0.3.0, код 0F"
            ),
            Some("Code 0F, version 0.3.0".into())
        );
        assert_eq!(
            interpolate(
                "Этот ПК: {} · протокол {}",
                "This PC: {} · protocol {}",
                "Этот ПК: 0.3.0 · протокол 2"
            ),
            Some("This PC: 0.3.0 · protocol 2".into())
        );
        assert!(
            interpolate("Версия {v} готова", "Version {v} ready", "Произвольное имя").is_none()
        );
    }
    #[test]
    fn translated_catalogs_do_not_fall_back_to_english_sentences() {
        // Proper nouns, platform names, and short words that are genuinely
        // identical in a target language are allowed to stay unchanged.
        const SHARED: &[&str] = &[
            "Application",
            "Audio",
            "Computer:",
            "Details",
            "Microphone",
            "Mouse",
            "Offline",
            "PC",
            "Updates",
        ];
        for (index, catalog) in CATALOGS.iter().enumerate().skip(2) {
            let language = LANGUAGES[index].0;
            for (key, value) in catalog {
                let english = &CATALOGS[1][key];
                if value == english
                    && english.split_whitespace().count() >= 3
                    && !SHARED.contains(&value.as_str())
                {
                    panic!("{language} still uses the English text for {key:?}");
                }
            }
        }
    }

    #[test]
    fn the_system_locale_selects_a_supported_language_or_english() {
        for (locale, expected) in [
            ("ru", "ru"),
            ("ru_RU", "ru"),
            ("ru-RU", "ru"),
            ("ru_RU.UTF-8", "ru"),
            ("de_DE.UTF-8", "de"),
            ("de-DE", "de"),
            ("fr", "fr"),
            ("fr_CA", "fr"),
            ("es_ES.UTF-8", "es"),
            ("it_IT", "it"),
            ("en_US.UTF-8", "en"),
            ("pt_BR", "en"),
            ("C", "en"),
            ("", "en"),
            ("C.UTF-8", "en"),
        ] {
            assert_eq!(locale_language(locale), expected, "locale {locale}");
        }
        // LANGUAGE may list several locales; the first one decides.
        assert_eq!(locale_language("de_DE:en_US:ru_RU"), "de");
        // Every produced code must exist in the catalog set.
        for (code, _) in LANGUAGES {
            assert!(CATALOGS.iter().all(|catalog| !catalog.is_empty()));
            assert!(LANGUAGES.iter().any(|(key, _)| *key == code));
        }
    }

    #[test]
    fn every_catalog_has_the_same_complete_set_of_keys() {
        let keys = CATALOGS[0].keys().collect::<Vec<_>>();
        assert!(keys.len() > 150, "translate the entire interface");
        for catalog in CATALOGS.iter() {
            assert_eq!(catalog.keys().collect::<Vec<_>>(), keys);
            assert!(catalog.values().all(|value| !value.trim().is_empty()));
            for (key, value) in catalog {
                let mut source = parts(key)
                    .into_iter()
                    .filter(|(_, field)| *field)
                    .map(|(name, _)| name)
                    .collect::<Vec<_>>();
                let mut target = parts(value)
                    .into_iter()
                    .filter(|(_, field)| *field)
                    .map(|(name, _)| name)
                    .collect::<Vec<_>>();
                source.sort();
                target.sort();
                assert_eq!(source, target, "placeholder mismatch: {key}");
            }
        }
    }
}
