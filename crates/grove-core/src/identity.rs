//! Random identities for persisted native agent conversations.
pub fn random_id() -> Result<String, String> {
    use ring::rand::{SecureRandom, SystemRandom};
    const HEX: &[u8] = b"0123456789abcdef";
    let mut bytes = [0u8; 16];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| "Secure random generator failed".to_owned())?;
    let mut id = String::with_capacity(32);
    for byte in bytes {
        id.push(char::from(HEX[usize::from(byte >> 4)]));
        id.push(char::from(HEX[usize::from(byte & 15)]));
    }
    Ok(id)
}
