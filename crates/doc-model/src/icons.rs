//! WikiCraft's SVG icon set (`assets/icons.json`, shared with the web UI): callout glyphs,
//! space/template symbols, UI marks and coloured emoticons. Server-rendered HTML (exports,
//! e-mails) inlines these instead of emoji.

use std::sync::LazyLock;

use serde_json::Value;

static ICONS: LazyLock<Value> = LazyLock::new(|| serde_json::from_str(include_str!("../assets/icons.json")).expect("valid icons.json"));

fn markup(group: &str, name: &str) -> Option<&'static str> {
    ICONS.get(group)?.get(name)?.as_str()
}

/// A line icon (`callout`, `symbols` or `ui` group) as an inline `<svg>` using `currentColor`.
pub fn line_svg(group: &str, name: &str, class: &str, size: &str) -> Option<String> {
    markup(group, name).map(|inner| {
        format!(
            "<svg class=\"{class}\" viewBox=\"0 0 24 24\" width=\"{size}\" height=\"{size}\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.75\" stroke-linecap=\"round\" stroke-linejoin=\"round\" aria-hidden=\"true\">{inner}</svg>"
        )
    })
}

/// Whether `name` is one of the emoticons we draw (Confluence's set).
pub fn is_emoticon(name: &str) -> bool {
    markup("emoticons", name).is_some()
}

/// A coloured emoticon as an inline `<svg>` sized to the surrounding text.
pub fn emoticon_svg(name: &str) -> Option<String> {
    markup("emoticons", name).map(|inner| {
        format!("<svg class=\"emoticon\" viewBox=\"0 0 24 24\" width=\"1.2em\" height=\"1.2em\" style=\"vertical-align:-0.25em\" role=\"img\" aria-label=\"{name}\">{inner}</svg>")
    })
}

/// Confluence's text shortcut for an emoticon (`(/)`, `:)`, …), used in Markdown.
pub fn emoticon_shortcut(name: &str) -> &'static str {
    markup("emoticonShortcuts", name).unwrap_or("")
}

#[cfg(test)]
mod tests {
    #[test]
    fn icons_load() {
        assert!(super::is_emoticon("tick") && !super::is_emoticon("nope"));
        assert!(super::emoticon_svg("smile").unwrap().starts_with("<svg class=\"emoticon\""));
        assert_eq!(super::emoticon_shortcut("tick"), "(/)");
        assert!(super::line_svg("callout", "warning", "i", "16").is_some());
        assert!(super::line_svg("ui", "paperclip", "i", "16").is_some());
        // no emoji anywhere in the icon set
        let raw = include_str!("../assets/icons.json");
        assert!(!raw.chars().any(|c| matches!(c as u32, 0x1F300..=0x1FAFF | 0x2600..=0x27BF)), "emoji in icons.json");
    }
}
