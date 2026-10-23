//! 输入来源标识，不持有 macOS 原生输入来源对象。

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputSource {
    pub id: String,
}

#[derive(Clone, Debug)]
pub struct SourceSnapshot {
    pub id: String,
    pub selected: bool,
}

pub fn is_apple_pinyin(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    id == "com.apple.inputmethod.scim.itabc"
        || (id.starts_with("com.apple.inputmethod.")
            && (id.contains("pinyin") || id.ends_with(".itabc")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_apple_pinyin_without_accepting_zhuyin_or_third_party() {
        assert!(is_apple_pinyin("com.apple.inputmethod.SCIM.ITABC"));
        assert!(is_apple_pinyin("com.apple.inputmethod.TCIM.Pinyin"));
        assert!(!is_apple_pinyin("com.apple.inputmethod.TCIM.Zhuyin"));
        assert!(!is_apple_pinyin("im.thirdparty.pinyin"));
        assert!(!is_apple_pinyin("com.apple.keylayout.ABC"));
    }
}
