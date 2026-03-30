pub fn encode_lower_hex(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::encode_lower_hex;

    #[test]
    fn encodes_empty_input() {
        assert_eq!(encode_lower_hex([]), "");
    }

    #[test]
    fn encodes_mixed_bytes() {
        assert_eq!(
            encode_lower_hex([0x00, 0x0f, 0x10, 0xab, 0xff]),
            "000f10abff"
        );
    }

    #[test]
    fn encodes_ascii_bytes() {
        assert_eq!(encode_lower_hex("abc"), "616263");
    }
}
