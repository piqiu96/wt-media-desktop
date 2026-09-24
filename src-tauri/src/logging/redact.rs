//! Masking credentials in a rendered record.
//!
//! The vocabulary and the shapes are the Agent's (`runtime/logging.py`): the two
//! halves of one product must not disagree about what a credential looks like,
//! because a record the Agent masks and Desktop prints is a leak with extra
//! steps. What is deliberately *not* copied is a defect found while mirroring
//! it -- the Agent's keyed pass replaces the whole rest of the line in one
//! match, so a second credential on the same line is never examined:
//!
//! ```text
//! redact("token=aaa password=bbb") -> "token=*** password=bbb"   (measured)
//! ```
//!
//! Here the scan resumes just after the value it masked, so every credential on
//! the line is seen. Registered in the CHG evidence as an Agent-side finding.
//!
//! Two further differences, both stricter and both registered: the boundary in
//! front of a key is ASCII here where the Agent's `\b` is Unicode-aware (so a
//! key written straight after a non-ASCII character is caught here and missed
//! there), and `cookies:` is masked as a cookie header rather than as one value.
//!
//! The mask is applied to the **finished line** on its way to the writer
//! (`backend::LineWriter`), so the message, the fields and whatever a future
//! formatter adds are covered by construction -- the Agent's ruling 十 and the
//! reason its mask lives in a formatter rather than in a `Filter`.
//!
//! Known and registered limits, the Agent's and this one's alike: a bare
//! credential with no key in front of it and no JWT or bearer shape is **not**
//! caught by the shape rules. That is what the `secrets` argument is for -- a
//! value this process holds (the launch token) is masked verbatim wherever it
//! stands.

/// What a masked value becomes. The Agent's three bytes, so one `grep` on one
/// vocabulary covers both sides of the product.
pub const REDACTED: &str = "***";

/// The shortest value that is used as a literal needle.
///
/// A shorter "secret" would blank out ordinary prose wherever those letters
/// happen to appear, which costs the log its purpose and protects nothing. The
/// Agent's `MIN_SECRET_LENGTH`, for the same reason and the same number.
pub const MIN_SECRET_LENGTH: usize = 8;

/// Leaf key names that carry a credential, name for name the Agent's
/// `SENSITIVE_KEY_NAMES`.
///
/// The vocabulary is the config loader's, not a second copy of it: a name that
/// refuses a value in a shipped TOML file must also mask that value in a log
/// line. The rule below accepts a prefix in either spelling, so `proxy_password`,
/// `set-cookie` and `X-Api-Key` are keys too.
pub const SENSITIVE_KEY_NAMES: [&str; 16] = [
    "token",
    "runtime_token",
    "auth_token",
    "password",
    "passwd",
    "secret",
    "client_secret",
    "authorization",
    "bearer",
    "refresh",
    "refresh_token",
    "cookie",
    "cookies",
    "api_key",
    "apikey",
    "private_key",
];

/// The words whose following blob is a credential whatever key it came under.
const BEARER_SCHEMES: [&str; 4] = ["bearer", "basic", "token", "digest"];

/// Where an unquoted value stops: the Agent's `[^\s,;"'&}\]]`.
const VALUE_STOP: [char; 7] = [',', ';', '"', '\'', '&', '}', ']'];

/// Mask credentials in one line of text, keeping everything around them.
///
/// Shape-driven rather than word-driven: `Name: value`, `Name=value`,
/// `"Name": "value"`, a query string, a bare bearer or JWT blob, URL userinfo,
/// and the literal value of a secret this process holds (`secrets` passes in
/// the launch token, which arrives named by nobody).
///
/// Masking is idempotent, because the same line may be handed to more than one
/// sink.
pub fn redact(text: &str, secrets: &[String]) -> String {
    let text = mask_keyed(text);
    let text = mask_userinfo(&text);
    let text = mask_bearer(&text);
    let text = mask_jwt(&text);
    mask_known(&text, secrets)
}

/// The line with its case and its two hyphen spellings levelled.
///
/// Byte for byte as long as the input, so an offset into this is an offset into
/// the original -- which is what lets one scan find the key and another take it
/// away. `to_ascii_lowercase` and `-` -> `_` both keep that property; a Unicode
/// lowercase would not, and nothing here needs one (every name is ASCII).
fn fold(text: &str) -> String {
    text.to_ascii_lowercase().replace('-', "_")
}

/// One credential key and the value it owns.
enum Family {
    /// `Cookie: a=1; b=2` -- every pair's value is a credential of its own.
    Cookie,
    /// `Authorization: …` -- the whole tail is the credential, separators and
    /// all, so which part of it is "the value" cannot be decided here.
    Opaque,
    /// A key whose leading value is the credential.
    Value,
    /// A word that only contains a credential name (`mytoken`, `tokenizer`):
    /// neither it nor its value is masked, as on the Agent's side.
    Plain,
}

fn family(key: &str) -> Family {
    if key.ends_with("cookie") || key.ends_with("cookies") {
        Family::Cookie
    } else if key.ends_with("authorization") {
        Family::Opaque
    } else if is_sensitive_key(key) {
        Family::Value
    } else {
        Family::Plain
    }
}

fn is_sensitive_key(key: &str) -> bool {
    SENSITIVE_KEY_NAMES.iter().any(|name| {
        key == *name
            || (key.len() > name.len()
                && key.ends_with(name)
                && key.as_bytes()[key.len() - name.len() - 1] == b'_')
    })
}

/// Mask every `key: value` / `key=value` credential in the line.
fn mask_keyed(text: &str) -> String {
    let folded = fold(text);
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut search = 0;

    while let Some((leaf, at)) = find_leaf(&folded, search) {
        let key_end = at + leaf.len();
        let Some(tail_start) = separator_end(text, key_end) else {
            search = key_end;
            continue;
        };
        let key = &folded[key_start(text, at)..key_end];
        let tail = &text[tail_start..];

        // What the credential reaches, and where the line may be scanned again
        // from. `Plain` and a value that is not there leave the line alone.
        let masked = match family(key) {
            Family::Cookie => Some((mask_cookie_pairs(tail), text.len())),
            Family::Opaque => Some((REDACTED.to_owned(), text.len())),
            // Only the value's own span: the rest of the tail is copied by the
            // next turn of the loop, and pushing it here would print it twice.
            Family::Value => leading_value(tail)
                .map(|value_end| (quote_like(&tail[..value_end]), tail_start + value_end)),
            Family::Plain => None,
        };
        let Some((masked, resumed_at)) = masked else {
            search = key_end;
            continue;
        };

        // The key and its separator are echoed as they arrived; only the value
        // is replaced. Resuming at the end of the value rather than at the end
        // of the line is what catches the second credential in
        // `token=aaa password=bbb`.
        out.push_str(&text[copied..key_end]);
        out.push_str(&text[key_end..tail_start]);
        out.push_str(&masked);
        copied = resumed_at;
        search = resumed_at;
    }

    out.push_str(&text[copied..]);
    out
}

/// The first credential name at or after `from`, longest first so
/// `runtime_token` wins over the `token` inside it.
fn find_leaf(folded: &str, from: usize) -> Option<(&'static str, usize)> {
    let bytes = folded.as_bytes();
    let mut at = from;
    while at < bytes.len() {
        let byte = bytes[at];
        // A name starts with an ASCII letter, and an ASCII byte is always a
        // character boundary, so slicing from here is sound whatever the line
        // holds.
        if byte.is_ascii_alphabetic() {
            let mut best = "";
            for name in SENSITIVE_KEY_NAMES {
                if name.len() > best.len() && folded[at..].starts_with(name) {
                    best = name;
                }
            }
            if !best.is_empty() {
                return Some((best, at));
            }
        }
        at += 1;
    }
    None
}

/// Where the key a name occurrence ends begins.
///
/// The key is the run of key characters the name ends, less any leading `-` or
/// `.`: the Agent's `\b` anchor has the same effect, because those two are key
/// characters that no word starts with.
fn key_start(text: &str, name_at: usize) -> usize {
    let bytes = text.as_bytes();
    let mut start = name_at;
    while start > 0 && is_key_char(bytes[start - 1]) {
        start -= 1;
    }
    while start < name_at && !is_word_char(bytes[start]) {
        start += 1;
    }
    start
}

/// Where the value begins: past an optional quote, then `:` or `=`, then any
/// spaces. `None` when what follows is not a separator at all.
///
/// The Agent's `["']?(\s*[:=]\s*)`, with the whitespace read as spaces and tabs:
/// a record is one line, so the newline `\s` also covers cannot be here.
fn separator_end(text: &str, from: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut at = from;
    if matches!(bytes.get(at), Some(b'"') | Some(b'\'')) {
        at += 1;
    }
    at = skip_spaces(bytes, at);
    if !matches!(bytes.get(at), Some(b':') | Some(b'=')) {
        return None;
    }
    Some(skip_spaces(bytes, at + 1))
}

fn skip_spaces(bytes: &[u8], from: usize) -> usize {
    let mut at = from;
    while matches!(bytes.get(at), Some(b' ') | Some(b'\t')) {
        at += 1;
    }
    at
}

fn is_key_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.')
}

fn is_word_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// How much of `tail` the credential reaches.
fn leading_value(tail: &str) -> Option<usize> {
    let start = scheme_prefix(tail).unwrap_or(0);
    value_end(tail.get(start..)?).map(|end| start + end)
}

/// Past a `Bearer`-style scheme word, if the value opens with one.
///
/// The Agent's `(?:(?:SCHEMES)\s+)?`: without this, `Authorization: Bearer eyJ…`
/// would keep everything after the first word.
fn scheme_prefix(tail: &str) -> Option<usize> {
    let word_end = tail.find(char::is_whitespace)?;
    let word = tail.get(..word_end)?;
    if !BEARER_SCHEMES
        .iter()
        .any(|scheme| word.eq_ignore_ascii_case(scheme))
    {
        return None;
    }
    let mut at = word_end;
    while tail[at..].starts_with(char::is_whitespace) {
        at += tail[at..].chars().next()?.len_utf8();
    }
    Some(at)
}

/// The end of the value at the start of `value`: a quoted string up to its
/// closing quote, or a run of non-separator characters.
fn value_end(value: &str) -> Option<usize> {
    let first = value.chars().next()?;
    match first {
        '"' | '\'' => {
            let rest = &value[first.len_utf8()..];
            let close = rest.find(first)?;
            Some(first.len_utf8() + close + first.len_utf8())
        }
        _ => {
            let mut end = 0;
            for (at, character) in value.char_indices() {
                if character.is_whitespace() || VALUE_STOP.contains(&character) {
                    break;
                }
                end = at + character.len_utf8();
            }
            (end > 0).then_some(end)
        }
    }
}

/// Mask a value, keeping the quotes it arrived in (so JSON stays JSON).
fn quote_like(value: &str) -> String {
    match (value.chars().next(), value.chars().next_back()) {
        (Some(first), Some(last))
            if first == last && (first == '"' || first == '\'') && value.len() >= 2 =>
        {
            format!("{first}{REDACTED}{last}")
        }
        _ => REDACTED.to_owned(),
    }
}

/// Mask each `name=value`'s value, keeping the names and the separators.
///
/// A cookie header holds several credentials at once (`a=1; session=xyz`), so
/// masking only the first would leak the session. The names are kept: they are
/// the part that tells a reader *which* cookie was in play.
fn mask_cookie_pairs(tail: &str) -> String {
    tail.split(';')
        .map(|part| match part.split_once('=') {
            Some((name, _)) => format!("{name}={REDACTED}"),
            None if part.trim().is_empty() => part.to_owned(),
            None => REDACTED.to_owned(),
        })
        .collect::<Vec<String>>()
        .join(";")
}

/// Mask the password in `scheme://user:password@host`, keeping the user.
fn mask_userinfo(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut search = 0;
    while let Some(found) = text[search..].find("://") {
        let after_scheme = search + found + 3;
        let user_end = scan_while(text, after_scheme, &['/', ':', '@']);
        if user_end == after_scheme || text.as_bytes().get(user_end) != Some(&b':') {
            search = after_scheme;
            continue;
        }
        let password_start = user_end + 1;
        let password_end = scan_while(text, password_start, &['/', '@']);
        if password_end == password_start || text.as_bytes().get(password_end) != Some(&b'@') {
            search = after_scheme;
            continue;
        }
        out.push_str(&text[copied..password_start]);
        out.push_str(REDACTED);
        copied = password_end;
        search = password_end;
    }
    out.push_str(&text[copied..]);
    out
}

/// Mask a bare `Bearer <token>`, with no key in front of it.
fn mask_bearer(text: &str) -> String {
    mask_spans(text, |text, at| {
        let rest = text.get(at..)?;
        let scheme = BEARER_SCHEMES.iter().find(|scheme| {
            rest.get(..scheme.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(scheme))
                && rest
                    .as_bytes()
                    .get(scheme.len())
                    .is_some_and(u8::is_ascii_whitespace)
        })?;
        let token_at = skip_spaces(rest.as_bytes(), scheme.len());
        let token_end = scan_while_fn(rest, token_at, is_token_char);
        // `[A-Za-z0-9\-._~+/=]{8,}`: a shorter run is not a token.
        (token_end - token_at >= MIN_SECRET_LENGTH).then_some((at + token_at, at + token_end))
    })
}

/// Mask a bare JWT: three base64url segments, the first starting `eyJ`
/// (`{"` in base64, which is what a JOSE header opens with).
///
/// Narrow on purpose, as on the Agent's side: a blanket "long random string"
/// rule would redact ids and hashes, so a bare 64-hex secret with no key and no
/// JWT shape is not caught here.
fn mask_jwt(text: &str) -> String {
    mask_spans(text, |text, at| {
        let rest = text.get(at..)?;
        if !rest.starts_with("eyJ") {
            return None;
        }
        let mut end = 3;
        for _ in 0..2 {
            let segment = scan_while_fn(rest, end, is_base64url);
            if segment - end < MIN_SECRET_LENGTH || rest.as_bytes().get(segment) != Some(&b'.') {
                return None;
            }
            end = segment + 1;
        }
        let last = scan_while_fn(rest, end, is_base64url);
        (last > end).then_some((at, at + last))
    })
}

/// Replace what `find` reports, left to right.
///
/// `find(text, at)` is offered every position a word could start at -- the
/// Agent's `\b`, read with ASCII letters and digits -- and returns the span to
/// replace as `(keep_from, resume_at)`: everything before `keep_from` is echoed,
/// [`REDACTED`] goes in, and the scan continues at `resume_at`.
fn mask_spans(text: &str, find: impl Fn(&str, usize) -> Option<(usize, usize)>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut at = 0;
    let mut previous: Option<char> = None;
    while at < text.len() {
        let character = text[at..].chars().next().expect("`at` is a boundary");
        if !previous.is_some_and(is_word_char_char) {
            if let Some((keep_from, resume_at)) = find(text, at) {
                out.push_str(&text[copied..keep_from]);
                out.push_str(REDACTED);
                copied = resume_at;
                at = resume_at;
                previous = text[..at].chars().next_back();
                continue;
            }
        }
        previous = Some(character);
        at += character.len_utf8();
    }
    out.push_str(&text[copied..]);
    out
}

fn is_word_char_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_'
}

fn is_token_char(character: char) -> bool {
    character.is_ascii_alphanumeric()
        || matches!(character, '-' | '.' | '_' | '~' | '+' | '/' | '=')
}

fn is_base64url(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
}

/// The end of the run of characters that are not in `stop`, from `from` on.
fn scan_while(text: &str, from: usize, stop: &[char]) -> usize {
    scan_while_fn(text, from, |character| !stop.contains(&character))
}

/// The end of the run of allowed characters, from `from` on.
///
/// Whitespace ends the run whatever `allowed` says: every character class this
/// module reads a value out of is whitespace-delimited, and the Agent's `[...]`
/// classes say `\s` too.
fn scan_while_fn(text: &str, from: usize, allowed: impl Fn(char) -> bool) -> usize {
    let mut end = from;
    for (at, character) in text[from..].char_indices() {
        if character.is_whitespace() || !allowed(character) {
            break;
        }
        end = from + at + character.len_utf8();
    }
    end
}

/// Mask a literal secret this process holds, wherever it stands.
fn mask_known(text: &str, secrets: &[String]) -> String {
    let mut out = text.to_owned();
    for secret in secrets {
        if secret.len() >= MIN_SECRET_LENGTH {
            out = out.replace(secret.as_str(), REDACTED);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape the launch token arrives in (T-17): a value nobody names.
    const LAUNCH_TOKEN: &str = "Zq7Yk3Nv1PdA8sXm";

    fn masked(text: &str) -> String {
        redact(text, &[])
    }

    /// The list the ruling names (七, 十, 十三·7), one line per shape.
    ///
    /// Every expectation keeps the key and whatever followed the value: a mask
    /// that swallowed the line would hide the context that explains the leak.
    #[test]
    fn every_credential_shape_is_masked_and_the_line_around_it_survives() {
        let cases = [
            (
                "Cookie: session=abc123def456; theme=dark",
                "Cookie: session=***; theme=***",
            ),
            (
                "Set-Cookie: session=abc123def456; Path=/",
                "Set-Cookie: session=***; Path=***",
            ),
            (
                "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.sig",
                "Authorization: ***",
            ),
            ("authorization=Basic dXNlcjpwYXNz", "authorization=***"),
            (
                "sent with Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.sig to the Agent",
                "sent with Bearer *** to the Agent",
            ),
            // The row above is a JWT, so the JWT rule masks it whether or not
            // the bearer rule works. This one is not: an opaque token of the
            // right shape under a scheme word is only reachable by that rule
            // (`a_bearer_word_with_nothing_token_shaped_after_it_is_left_alone`
            // is the other half, and it is named for the half that holds back).
            (
                "handed bearer aaabbbcccddd to the Agent",
                "handed bearer *** to the Agent",
            ),
            // The plural is in the vocabulary but not in `endswith("cookie")`,
            // so it takes the cookie family only if the family rule says so:
            // the generic key rule would mask the first pair and leak the rest.
            ("cookies: a=1; b=2", "cookies: a=***; b=***"),
            ("proxy_password=hunter2hunter2", "proxy_password=***"),
            ("password: hunter2", "password: ***"),
            (
                "refresh_token=\"rt-abcdefghijklmnop\"",
                "refresh_token=\"***\"",
            ),
            (
                "{\"client_secret\": \"cs-abcdefghijklmnop\", \"port\": 8765}",
                "{\"client_secret\": \"***\", \"port\": 8765}",
            ),
            ("X-Api-Key: ak-abcdefghijkl", "X-Api-Key: ***"),
            (
                "GET https://api.example/v1/tasks?token=abcdefghijkl&page=2",
                "GET https://api.example/v1/tasks?token=***&page=2",
            ),
            (
                "dial https://alice:s3cretpw@agent.local:8765/healthz",
                "dial https://alice:***@agent.local:8765/healthz",
            ),
            (
                "payload eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.c2lnbmF0dXJl",
                "payload ***",
            ),
            // A short value is masked all the same: the minimum length is a
            // rule about needles, not about values.
            ("token=a", "token=***"),
        ];
        for (line, expected) in cases {
            assert_eq!(masked(line), expected, "input: {line}");
        }
    }

    /// The line the Agent's shipped redactor gets wrong, measured there first:
    /// its keyed pass replaces the whole tail in one match, so the second
    /// credential is never looked at.
    #[test]
    fn every_credential_on_a_line_is_masked_and_not_just_the_first() {
        assert_eq!(
            masked("token=aaa password=bbb secret=ccc"),
            "token=*** password=*** secret=***"
        );
        assert_eq!(
            masked("Authorization: Bearer aaa cookie=sess=xyz"),
            "Authorization: ***"
        );
    }

    #[test]
    fn a_name_that_merely_contains_a_credential_name_is_left_alone() {
        // The Agent leaves these too: `_KEY` is broad, and the family check is
        // what decides. The values here are not credentials.
        for line in [
            "mytoken=abcdefghijkl",
            "the tokenizer finished in 12ms",
            "token_count=7",
        ] {
            assert_eq!(masked(line), line);
        }
    }

    #[test]
    fn masking_a_line_twice_changes_nothing_more() {
        // Two sinks see the same line (the file and stderr), and a record that
        // already carries a mask must not be mangled by the second pass.
        for line in [
            "Cookie: session=abc123def456; theme=dark",
            "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.sig",
            "dial https://alice:s3cretpw@agent.local:8765/healthz",
            "token=aaa password=bbb",
            "Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.c2ln",
        ] {
            let once = masked(line);
            assert_eq!(masked(&once), once, "input: {line}");
        }
    }

    #[test]
    fn a_secret_this_process_holds_is_masked_without_a_key_to_name_it() {
        // The control arm first: with no shapes to catch it, the line really
        // does carry the token, so "the masked one does not" means something.
        let line = format!("the sidecar was handed {LAUNCH_TOKEN} at launch");
        assert!(masked(&line).contains(LAUNCH_TOKEN));

        let held = redact(&line, &[LAUNCH_TOKEN.to_owned()]);
        assert_eq!(held, "the sidecar was handed *** at launch");
    }

    #[test]
    fn a_value_too_short_to_be_a_secret_is_not_used_as_a_needle() {
        // Both arms, because "8 characters" is the whole rule: a 7-character
        // needle would blank out ordinary prose wherever those letters fall.
        let short = "abc1234";
        let long = "abc12345";
        let line = format!("{short} and {long} are both here");
        let held = redact(&line, &[short.to_owned(), long.to_owned()]);
        assert!(held.contains(short), "{held}");
        assert!(!held.contains(long), "{held}");
        assert_eq!(MIN_SECRET_LENGTH, long.len());
    }

    #[test]
    fn a_bearer_word_with_nothing_token_shaped_after_it_is_left_alone() {
        // The `{8,}` in the token class, and its control: the same sentence
        // with a real token is masked.
        let prose = "basic auth is described in the manual";
        assert_eq!(masked(prose), prose);
        assert_eq!(masked("basic dXNlcjpwYXNz"), "basic ***");
    }

    #[test]
    fn a_url_with_a_user_but_no_password_keeps_its_user() {
        // The rule is `user:password@`, so `user@host` is not a credential.
        let line = "dial https://alice@agent.local:8765/healthz";
        assert_eq!(masked(line), line);
    }
}
