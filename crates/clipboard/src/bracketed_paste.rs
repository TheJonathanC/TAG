pub fn wrap_bracketed_paste(text: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(text.len() + 16);
    bytes.extend_from_slice(b"\x1b[200~");
    bytes.extend_from_slice(text.as_bytes());
    bytes.extend_from_slice(b"\x1b[201~");
    bytes
}
