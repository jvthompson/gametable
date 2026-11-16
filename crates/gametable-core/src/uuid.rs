//! Random version-4 UUIDs, formatted exactly like `src/core/uuid.lua` (lowercase hex).

/// Returns a new random UUID such as `"3f2b8c1e-9a4d-4e7f-b1c2-0d9e8f7a6b5c"`.
pub fn new_v4_uuid() -> String {
    let mut bytes: [u8; 16] = rand::random();
    bytes[6] = (bytes[6] & 0x0F) | 0x40; // version 4
    bytes[8] = (bytes[8] & 0x3F) | 0x80; // RFC 4122 variant
    let hex: Vec<String> = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        hex[0..4].concat(),
        hex[4..6].concat(),
        hex[6..8].concat(),
        hex[8..10].concat(),
        hex[10..16].concat()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_version_and_variant_bits_and_lowercase_layout() {
        for _ in 0..100 {
            let uuid = new_v4_uuid();
            assert_eq!(uuid.len(), 36);
            let groups: Vec<&str> = uuid.split('-').collect();
            assert_eq!(groups.iter().map(|g| g.len()).collect::<Vec<_>>(), vec![8, 4, 4, 4, 12]);
            assert!(groups[2].starts_with('4'));
            assert!(matches!(groups[3].as_bytes()[0], b'8' | b'9' | b'a' | b'b'));
            assert_eq!(uuid, uuid.to_lowercase());
        }
    }
}
