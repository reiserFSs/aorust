//! Login challenge-response: half Diffie-Hellman + TEA-CBC.
//!
//! Client side = `Client_t::MakeChallengeResponse` -> Interfaces.dll 0x100129a1 (DH + string
//! assembly), 0x10012adc (TEA-CBC, hex output), 0x10012c53 (TEA block). Server side =
//! CellAO `LoginEncryption.DecryptLoginKey`. Pure functions: randomness is passed in.

use anyhow::{anyhow, bail, Result};
use num_bigint::BigUint;

/// 1024-bit prime (Interfaces.dll file offset 0x16988, identical to AOChat `$dhN`).
pub const DH_PRIME_HEX: &str = "eca2e8c85d863dcdc26a429a71a9815ad052f6139669dd659f98ae159d313d13c6bf2838e10a69b6478b64a24bd054ba8248e8fa778703b418408249440b2c1edd28853e240d8a7e49540b76d120d3b1ad2878b1b99490eb4a2a5e84caa8a91cecbdb1aa7c816e8be343246f80c637abc653b893fd91686cf8d32d6cfe5f2a6f";
/// Login server's DH public key as hard-coded in this client (Interfaces.dll file offset
/// 0x16a90). NOT the AOChat `$dhY` (9c32cc23...) — a PRK/retail server must hold the private half.
pub const LOGIN_SERVER_PUB_HEX: &str = "90b8ce5fe64f466678a7a8589023be89c64e34358b0e0165cd5e381c75f3f5e7bc34e8e1a6ca50c3ba982726752b3a66d6b2ccbdee90cd0f8e17237e218b3a2c41cf8a2988935d797a9d777cfac141ab887b0532432f3628fa1f312edabe13eb1ab7840eeb485e31b05df8b36aa5c1286f215926eb2ec7ffa5fba24ecbbb8ae1";
/// Generator, `.data` 0x100328e4 in Interfaces.dll.
pub const DH_GENERATOR: u32 = 5;

pub fn dh_prime() -> BigUint {
    BigUint::parse_bytes(DH_PRIME_HEX.as_bytes(), 16).unwrap()
}
pub fn login_server_pub() -> BigUint {
    BigUint::parse_bytes(LOGIN_SERVER_PUB_HEX.as_bytes(), 16).unwrap()
}

const DELTA: u32 = 0x9E37_79B9;

/// One TEA encrypt block, 32 rounds, key words as in Interfaces.dll 0x10012c53.
pub fn tea_encrypt_block([mut v0, mut v1]: [u32; 2], k: [u32; 4]) -> [u32; 2] {
    let mut sum = 0u32;
    for _ in 0..32 {
        sum = sum.wrapping_add(DELTA);
        v0 = v0.wrapping_add(
            (v1 << 4).wrapping_add(k[0]) ^ v1.wrapping_add(sum) ^ (v1 >> 5).wrapping_add(k[1]),
        );
        v1 = v1.wrapping_add(
            (v0 << 4).wrapping_add(k[2]) ^ v0.wrapping_add(sum) ^ (v0 >> 5).wrapping_add(k[3]),
        );
    }
    [v0, v1]
}

/// Inverse (CellAO `DecryptTeaRound`).
pub fn tea_decrypt_block([mut v0, mut v1]: [u32; 2], k: [u32; 4]) -> [u32; 2] {
    let mut sum = DELTA.wrapping_mul(32);
    for _ in 0..32 {
        v1 = v1.wrapping_sub(
            (v0 << 4).wrapping_add(k[2]) ^ v0.wrapping_add(sum) ^ (v0 >> 5).wrapping_add(k[3]),
        );
        v0 = v0.wrapping_sub(
            (v1 << 4).wrapping_add(k[0]) ^ v1.wrapping_add(sum) ^ (v1 >> 5).wrapping_add(k[1]),
        );
        sum = sum.wrapping_sub(DELTA);
    }
    [v0, v1]
}

/// Shared secret -> 16-byte key. The client takes the lowercase hex of the secret (no leading
/// zeros, GMP `mpz_get_str`), requires >= 32 chars ("input key too short." otherwise), and
/// parses the first 32 chars as 16 bytes.
fn key_from_secret(secret: &BigUint) -> Result<[u8; 16]> {
    let h = secret.to_str_radix(16);
    if h.len() < 32 {
        bail!("input key too short");
    }
    let mut k = [0u8; 16];
    for (i, b) in k.iter_mut().enumerate() {
        *b = u8::from_str_radix(&h[2 * i..2 * i + 2], 16)?;
    }
    Ok(k)
}

fn key_words(k: &[u8; 16]) -> [u32; 4] {
    std::array::from_fn(|i| u32::from_le_bytes(k[4 * i..4 * i + 4].try_into().unwrap()))
}

/// CBC with zero IV over little-endian u32 pairs; `plain.len()` must be a multiple of 8.
/// ponytail: the original skips the chaining XOR when the previous block's first word is 0
/// (2^-32 per block, produces ciphertext no server can decrypt); we always XOR.
fn cbc_encrypt(k: &[u8; 16], plain: &[u8]) -> Vec<u8> {
    let kw = key_words(k);
    let mut prev = [0u32; 2];
    let mut out = Vec::with_capacity(plain.len());
    for c in plain.chunks_exact(8) {
        let w = [
            u32::from_le_bytes(c[..4].try_into().unwrap()) ^ prev[0],
            u32::from_le_bytes(c[4..].try_into().unwrap()) ^ prev[1],
        ];
        prev = tea_encrypt_block(w, kw);
        out.extend_from_slice(&prev[0].to_le_bytes());
        out.extend_from_slice(&prev[1].to_le_bytes());
    }
    out
}

fn cbc_decrypt(k: &[u8; 16], ct: &[u8]) -> Result<Vec<u8>> {
    if ct.len() % 8 != 0 {
        bail!("ciphertext not a multiple of 8");
    }
    let kw = key_words(k);
    let mut prev = [0u32; 2];
    let mut out = Vec::with_capacity(ct.len());
    for c in ct.chunks_exact(8) {
        let cw = [
            u32::from_le_bytes(c[..4].try_into().unwrap()),
            u32::from_le_bytes(c[4..].try_into().unwrap()),
        ];
        let p = tea_decrypt_block(cw, kw);
        out.extend_from_slice(&(p[0] ^ prev[0]).to_le_bytes());
        out.extend_from_slice(&(p[1] ^ prev[1]).to_le_bytes());
        prev = cw;
    }
    Ok(out)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Result<Vec<u8>> {
    if s.len() % 2 != 0 {
        bail!("odd hex length");
    }
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).map_err(|e| anyhow!(e)))
        .collect()
}

/// Build the `UserCredentials` response string `"<dhX hex>-<ciphertext hex>"`.
///
/// * `salt` – the 32 raw bytes of `ServerSalt`; the client treats them as a C string, so
///   everything from the first NUL on is dropped (`Client_t::ProcessMessage` case 0x24).
/// * `exponent` – the client's 128-bit random private exponent (`random_integer(0x80)`).
/// * `prefix` – 8 random bytes (`rand()`) that start the plaintext.
pub fn make_challenge_response_with(
    server_pub: &BigUint,
    name: &str,
    salt: &[u8],
    password: &str,
    exponent: &BigUint,
    prefix: [u8; 8],
) -> Result<String> {
    let n = dh_prime();
    let x_pub = BigUint::from(DH_GENERATOR).modpow(exponent, &n);
    let key = key_from_secret(&server_pub.modpow(exponent, &n))?;

    let salt = &salt[..salt.iter().position(|&b| b == 0).unwrap_or(salt.len())];
    let mut s = Vec::new();
    s.extend_from_slice(name.as_bytes());
    s.push(b'|');
    s.extend_from_slice(salt);
    s.push(b'|');
    s.extend_from_slice(password.as_bytes());

    // 8 prefix + 4 BE length + string, then pad with spaces by `8 - (t % 8)` (a full block
    // when t % 8 == 0, unlike AOChat's PHP).
    let t = 12 + s.len();
    let mut plain = Vec::with_capacity(t + 8);
    plain.extend_from_slice(&prefix);
    plain.extend_from_slice(&(s.len() as u32).to_be_bytes());
    plain.extend_from_slice(&s);
    plain.resize(t + (8 - t % 8), b' ');

    Ok(format!("{}-{}", x_pub.to_str_radix(16), hex(&cbc_encrypt(&key, &plain))))
}

/// [`make_challenge_response_with`] against this client's hard-coded login-server key.
pub fn make_challenge_response(
    name: &str,
    salt: &[u8],
    password: &str,
    exponent: &BigUint,
    prefix: [u8; 8],
) -> Result<String> {
    make_challenge_response_with(&login_server_pub(), name, salt, password, exponent, prefix)
}

/// Server side (CellAO `DecryptLoginKey`): returns `(name, salt, password)`.
pub fn open_challenge_response(
    server_priv: &BigUint,
    response: &str,
) -> Result<(String, Vec<u8>, String)> {
    let (x_hex, ct_hex) = response.split_once('-').ok_or_else(|| anyhow!("missing '-'"))?;
    let x = BigUint::parse_bytes(x_hex.as_bytes(), 16).ok_or_else(|| anyhow!("bad dhX"))?;
    let key = key_from_secret(&x.modpow(server_priv, &dh_prime()))?;
    let plain = cbc_decrypt(&key, &unhex(ct_hex)?)?;
    if plain.len() < 12 {
        bail!("plaintext too short");
    }
    let len = u32::from_be_bytes(plain[8..12].try_into()?) as usize;
    let body = plain.get(12..12 + len).ok_or_else(|| anyhow!("bad inner length"))?;
    let p1 = body.iter().position(|&b| b == b'|').ok_or_else(|| anyhow!("no name separator"))?;
    let rest = &body[p1 + 1..];
    if rest.len() < 33 || rest[32] != b'|' {
        bail!("bad salt field");
    }
    Ok((
        String::from_utf8_lossy(&body[..p1]).into_owned(),
        rest[..32].to_vec(),
        String::from_utf8_lossy(&rest[33..]).into_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Known answers below were produced by an independent Python re-implementation of the
    // RE'd client algorithm (Interfaces.dll 0x10012adc / 0x10012c53, GMP powm) plus a port
    // of CellAO's LoginEncryption.DecryptTea for the reverse direction. No real-server capture.
    fn x() -> BigUint {
        BigUint::parse_bytes(b"0123456789abcdef0123456789abcdef", 16).unwrap()
    }
    const PREFIX: [u8; 8] = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
    const X_PUB: &str = "8cc3b8c63c69d7cf3e7ae98bcfa5607e6400f5c6a7974b64f11110cfce532e84decbc88f31c95811b2e13b74b902cd8bfb2a04b6c9f6d91830a10dd35bcf786d74274c07625d8d609574d91c9e2050ffa222ec934f354c6323e5781c3b770656f5ef616f39d926f0362477d741d0f4c9ca8148e7438a3ecede4da173c25a9c45";
    const K_HEX: &str = "24d742e39b1367d4699a636dd3911f35";

    #[test]
    fn tea_known_answer() {
        let k = key_words(&unhex("000102030405060708090a0b0c0d0e0f").unwrap().try_into().unwrap());
        let c = tea_encrypt_block([0x01234567, 0x89abcdef], k);
        assert_eq!(c, [0x6847d0b4, 0xd158787c]);
        assert_eq!(tea_decrypt_block(c, k), [0x01234567, 0x89abcdef]);
    }

    #[test]
    fn dh_known_answer() {
        let n = dh_prime();
        assert_eq!(BigUint::from(5u32).modpow(&x(), &n).to_str_radix(16), X_PUB);
        let k = login_server_pub().modpow(&x(), &n).to_str_radix(16);
        assert!(k.starts_with(K_HEX));
    }

    #[test]
    fn response_known_answer() {
        let salt: Vec<u8> = (0x21..0x41).collect();
        let got = make_challenge_response("testuser", &salt, "hunter2", &x(), PREFIX).unwrap();
        let want = format!("{X_PUB}-42b54b2355435fc3e8df642f8885b26998f1bd97c04625122be058aa91315ce8a2c7304eda6f491a15ae979eaa41bb95143c08b448618ef456c184c303634ba3");
        assert_eq!(got, want);
        // 12 + len(37) = 49 -> 7 pad bytes
        let got2 = make_challenge_response("ab", &salt, "c", &x(), PREFIX).unwrap();
        assert!(got2.ends_with("42b54b2355435fc37e04652329e2d8ab4a24d245b3b6d649db084dbc7d33d9c9d4fbe3a36913ace3a0038022fc340be31463e536977fd84f"));
        // 12 + len(36) = 48 -> client appends a FULL 8-byte pad block (56 bytes total)
        let got3 = make_challenge_response("ab", &salt, "", &x(), PREFIX).unwrap();
        assert!(got3.ends_with("42b54b2355435fc3b90507969864e28854407eb59c50acee1149977cbfa57753d6c4e65298a658a8add007085cd5503dede0955d14107b68"));
    }

    #[test]
    fn server_roundtrip_with_own_keypair() {
        let n = dh_prime();
        let sp = BigUint::parse_bytes(b"7ad852c6494f664e8df21446285ecd6f400cf20e1d872ee96136d7744887424b", 16).unwrap();
        let spub = BigUint::from(5u32).modpow(&sp, &n);
        let salt: Vec<u8> = (1..=32).collect();
        let r = make_challenge_response_with(&spub, "Me", &salt, "p|w", &x(), PREFIX).unwrap();
        let (u, s, p) = open_challenge_response(&sp, &r).unwrap();
        assert_eq!((u.as_str(), s, p.as_str()), ("Me", salt, "p|w"));
    }

    #[test]
    fn salt_truncates_at_nul() {
        let n = dh_prime();
        let sp = BigUint::from(0x1234_5678_9abc_def0u64);
        let spub = BigUint::from(5u32).modpow(&sp, &n);
        let mut salt = [7u8; 32];
        salt[3] = 0;
        let r = make_challenge_response_with(&spub, "a", &salt, "b", &x(), PREFIX).unwrap();
        // Short salt breaks the server's fixed 32-byte parse — that is the original's behaviour
        // (CellAO avoids it by never generating NUL salt bytes).
        assert!(open_challenge_response(&sp, &r).is_err());
    }
}
