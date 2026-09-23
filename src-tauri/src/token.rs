//! The per-launch secret the Local Agent requires on every local call.
//!
//! The Agent enforces this token as soon as it is given one (`local_api/server.py`
//! `_check_auth`: an empty token disables the check, a non-empty one requires
//! `Bearer <token>` on every GET and POST). Desktop generates it at startup,
//! passes it to the sidecar through the environment, and attaches it to every
//! request it makes — so the local API stops being reachable by any other process
//! on the machine that can open a loopback socket.
//!
//! The type has **no** `Debug`, `Display` or `Serialize`, and that is the whole
//! point of it existing as a type rather than as a `String`. Those three impls
//! are what put a secret into a log line, and they are one `#[derive]` away: a
//! `String` field gets them for free, and a token that can be printed will
//! eventually be printed. `expose` is the only way the bytes come back out, so
//! `grep -rn expose src-tauri/src` is a complete list of the places the value is
//! read.
//!
//! This is a compile-time guarantee, not a tested one — see the evidence record
//! for how it was checked (by a deliberate compile failure) and why it is not a
//! permanent test.

use uuid::Uuid;

/// A fresh random token, generated once per Desktop launch.
#[derive(Clone)]
pub struct RuntimeToken(String);

impl RuntimeToken {
    /// Generate a token for this launch.
    ///
    /// v4 (random) rather than v1/v7 (time-ordered): the token is a bearer
    /// credential for a loopback service, and a time-ordered one leaks the
    /// launch time and can collide across a process restart within the same tick.
    pub fn generate() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    /// The token's bytes. The only accessor.
    ///
    /// Callers are limited to the two places the value is allowed to appear: the
    /// sidecar's environment, and the `Authorization` header of a local-agent
    /// request.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// A v4 UUID, in the canonical hyphenated lower-case form the Agent compares
    /// against byte for byte.
    #[test]
    fn a_token_is_a_canonical_v4_uuid() {
        let token = RuntimeToken::generate();
        let value = token.expose();

        assert_eq!(value.len(), 36, "{value}");
        assert_eq!(
            value.chars().filter(|c| *c == '-').count(),
            4,
            "every UUID has exactly four hyphens: {value}"
        );
        assert!(
            value.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
            "a token must be hex so it survives an environment variable and a header: {value}"
        );
        // Version 4 in the first nibble of the third group; variant 10xx in the
        // first nibble of the fourth. Pinning both is what distinguishes a
        // random v4 from a constant that happens to be shaped like a UUID.
        let groups: Vec<&str> = value.split('-').collect();
        assert_eq!(groups.len(), 5, "{value}");
        assert!(groups[2].starts_with('4'), "must be version 4: {value}");
        assert!(
            matches!(groups[3].chars().next(), Some('8' | '9' | 'a' | 'b')),
            "must be RFC 4122 variant 8/9/a/b: {value}"
        );
    }

    /// Two launches must not share a token.
    ///
    /// 1000 draws across a 122-bit space: a birthday collision is not a
    /// possibility worth reasoning about, so any duplicate here means the value
    /// is not random — a constant, a counter, or a fixed prefix. This is the
    /// test that dies if `Uuid::new_v4()` is replaced by anything deterministic.
    #[test]
    fn tokens_do_not_repeat_across_launches() {
        const DRAWS: usize = 1000;
        let seen: HashSet<String> = (0..DRAWS)
            .map(|_| RuntimeToken::generate().expose().to_string())
            .collect();
        assert_eq!(seen.len(), DRAWS, "a token repeated across draws");
    }

    /// `expose` is stable: the same token yields the same bytes every time, so a
    /// caller may call it once for the environment and once for a header and be
    /// sure the Agent sees one value.
    #[test]
    fn expose_is_stable_for_a_given_token() {
        let token = RuntimeToken::generate();
        assert_eq!(token.expose(), token.expose());
    }
}
