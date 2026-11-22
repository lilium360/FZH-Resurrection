//! The recovery code of an account and the password derived from it (docs/launcher-contract.md,
//! "Backend account API", Scheme). The launcher shows the code once and never stores it.

use crate::error::{BackendError, Res};

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const SYMBOLS: usize = 25;
const GROUP: usize = 5;
const CONTEXT: &str = "fzh-account-v1\n";

/// 25 symbols from the OS's secure random source, shown as five groups of five.
pub fn generate() -> Res<String> {
    let mut raw = [0u8; SYMBOLS];
    getrandom::fill(&mut raw).map_err(|e| BackendError::internal(format!("no secure random numbers: {e}")))?;
    // 256 is a multiple of 32, so the low five bits of a uniform byte are uniform.
    let code: Vec<u8> = raw.iter().map(|b| ALPHABET[usize::from(b & 31)]).collect();
    Ok(code.chunks(GROUP).map(|g| String::from_utf8_lossy(g)).collect::<Vec<_>>().join("-"))
}

/// The code as typed (any case, spaces and dashes anywhere, `O` for `0`, `I` or `L` for `1`), or
/// `None` when it is not a recovery code.
pub fn canonical(text: &str) -> Option<String> {
    let code: String = text
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        })
        .collect();
    (code.len() == SYMBOLS && code.bytes().all(|b| ALPHABET.contains(&b))).then_some(code)
}

/// base64url without padding of HMAC-SHA256(canonical code, "fzh-account-v1\n" + lower-case Online ID).
pub fn password(canonical_code: &str, online_id: &str) -> String {
    let message = format!("{CONTEXT}{}", online_id.to_ascii_lowercase());
    let mac = crate::pfs::hmac_sha256(canonical_code.as_bytes(), message.as_bytes());
    crate::avatar::base64_encode(&mac).trim_end_matches('=').replace('+', "-").replace('/', "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    const VECTOR: &str = "jJXhJrVT4CM79WTqRKwHHabrAZAfDbx8wWs1nCbXaCc";

    #[test]
    fn the_contract_test_vector() {
        let code = canonical("ABCDE-FGHJK-MNPQR-STVWX-YZ012").unwrap();
        assert_eq!(code, "ABCDEFGHJKMNPQRSTVWXYZ012");
        assert_eq!(password(&code, "Player_One"), VECTOR);
        assert_eq!(password(&code, "player_one"), VECTOR);
        assert_eq!(password(&canonical("abcde fghjk mnpqr stvwx yz012").unwrap(), "Player_One"), VECTOR);
    }

    #[test]
    fn typed_forms_of_the_same_code() {
        let code = "ABCDEFGHJKMNPQRSTVWXYZ012";
        for typed in ["abcdefghjkmnpqrstvwxyz012", " ABCDE-FGHJK MNPQR-STVWX-YZOI2 ", "abcde-fghjk-mnpqr-stvwx-yzol2", "A-B-C-D-E-FGHJKMNPQRSTVWXYZ012\n"] {
            assert_eq!(canonical(typed).as_deref(), Some(code), "{typed:?}");
        }
        for bad in ["", "ABCDE-FGHJK-MNPQR-STVWX-YZ01", "ABCDE-FGHJK-MNPQR-STVWX-YZ0123", "ABCDE-FGHJK-MNPQR-STVWX-YZ01U", "ABCDE-FGHJK-MNPQR-STVWX-YZ01é", "ABCDE_FGHJK_MNPQR_STVWX_YZ012"] {
            assert_eq!(canonical(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn generated_codes() {
        let a = generate().unwrap();
        let b = generate().unwrap();
        assert_ne!(a, b);
        for code in [&a, &b] {
            assert_eq!(code.len(), 29);
            assert_eq!(code.split('-').map(str::len).collect::<Vec<_>>(), [5; 5]);
            assert_eq!(canonical(code), Some(code.replace('-', "")));
        }
        let p = password(&canonical(&a).unwrap(), "Someone_1");
        assert_eq!(p.len(), 43);
        assert!(p.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-'));
    }
}
