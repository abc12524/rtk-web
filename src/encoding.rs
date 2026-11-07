use encoding_rs::{GBK, UTF_16BE, UTF_16LE};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Charset {
    Auto,
    Utf8,
    Utf16Le,
    Utf16Be,
    Gbk,
}

impl Charset {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "utf-8" | "utf8" => Some(Self::Utf8),
            "utf-16le" | "utf16le" | "unicode" => Some(Self::Utf16Le),
            "utf-16be" | "utf16be" => Some(Self::Utf16Be),
            "gbk" | "cp936" | "gb2312" | "gb18030" => Some(Self::Gbk),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Utf8 => "utf-8",
            Self::Utf16Le => "utf-16le",
            Self::Utf16Be => "utf-16be",
            Self::Gbk => "gbk",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    UnsupportedCharset(String),
    InvalidUtf8,
    InvalidEncoding(&'static str),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedCharset(value) => {
                write!(f, "unsupported charset `{value}` (use auto, utf-8, utf-16le, utf-16be or gbk)")
            }
            Self::InvalidUtf8 => write!(f, "body is not valid UTF-8"),
            Self::InvalidEncoding(name) => write!(f, "body is not valid {name}"),
        }
    }
}

impl std::error::Error for DecodeError {}

pub fn decode(bytes: &[u8], charset: Charset) -> Result<String, DecodeError> {
    let decoded = match charset {
        Charset::Auto => decode_auto(bytes)?,
        Charset::Utf8 => decode_utf8(bytes)?,
        Charset::Utf16Le => decode_with(UTF_16LE, bytes, "utf-16le")?,
        Charset::Utf16Be => decode_with(UTF_16BE, bytes, "utf-16be")?,
        Charset::Gbk => decode_with(GBK, bytes, "gbk")?,
    };
    Ok(normalize_newlines(&decoded))
}

pub fn looks_binary(bytes: &[u8]) -> bool {
    let utf16_bom = bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]);
    if utf16_bom {
        return false;
    }
    bytes.iter().take(8192).any(|byte| *byte == 0)
}

fn decode_auto(bytes: &[u8]) -> Result<String, DecodeError> {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return decode_utf8(&bytes[3..]);
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return decode_with(UTF_16LE, bytes, "utf-16le");
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return decode_with(UTF_16BE, bytes, "utf-16be");
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        return Ok(text.to_string());
    }
    decode_with(GBK, bytes, "gbk")
}

fn decode_utf8(bytes: &[u8]) -> Result<String, DecodeError> {
    std::str::from_utf8(bytes)
        .map(str::to_string)
        .map_err(|_| DecodeError::InvalidUtf8)
}

fn decode_with(
    encoding: &'static encoding_rs::Encoding,
    bytes: &[u8],
    name: &'static str,
) -> Result<String, DecodeError> {
    let (text, _, had_errors) = encoding.decode(bytes);
    if had_errors {
        return Err(DecodeError::InvalidEncoding(name));
    }
    Ok(text.into_owned())
}

fn normalize_newlines(text: &str) -> String {
    if text.contains("\r\n") {
        text.replace("\r\n", "\n")
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_utf8_passes_through() {
        assert_eq!(decode("错误: 编译失败\n".as_bytes(), Charset::Auto).unwrap(), "错误: 编译失败\n");
    }

    #[test]
    fn utf8_bom_is_stripped() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice("hello".as_bytes());
        assert_eq!(decode(&bytes, Charset::Auto).unwrap(), "hello");
    }

    #[test]
    fn utf16le_with_bom_is_decoded() {
        let mut bytes = vec![0xFF, 0xFE];
        for unit in "错误 error".encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(decode(&bytes, Charset::Auto).unwrap(), "错误 error");
    }

    #[test]
    fn gbk_is_the_auto_fallback() {
        let (bytes, _, _) = GBK.encode("下载完成");
        assert_eq!(decode(&bytes, Charset::Auto).unwrap(), "下载完成");
    }

    #[test]
    fn crlf_is_normalized_but_lone_cr_is_kept() {
        assert_eq!(
            decode(b"progress 1%\rprogress 2%\r\nline\r\n", Charset::Auto).unwrap(),
            "progress 1%\rprogress 2%\nline\n"
        );
    }

    #[test]
    fn explicit_utf8_rejects_invalid_bytes() {
        assert_eq!(decode(&[0xFF, 0xFE, 0x00], Charset::Utf8), Err(DecodeError::InvalidUtf8));
    }

    #[test]
    fn binary_detection_excepts_utf16() {
        assert!(looks_binary(&[0x00, 0x01, 0x02]));
        assert!(!looks_binary(b"plain text"));
        assert!(!looks_binary(&[0xFF, 0xFE, 0x00, 0x00]));
    }

    #[test]
    fn unknown_charset_is_rejected() {
        assert_eq!(Charset::parse("latin-1"), None);
    }
}