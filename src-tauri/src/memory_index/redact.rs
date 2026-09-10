//! Credential redaction, applied before any transcript text reaches the index.
//!
//! This is the highest-stakes filter in the module. On the measured corpus tool
//! results are 60.6% of pi's records and user text is 1.5%, so nearly all
//! indexed bytes are program output — `.env` dumps, `curl -v` traces,
//! `Authorization` headers, SSH keys. The repository already extends its
//! never-log-secrets rule to anything persisted to the user's disk; an index
//! built for full-text search over that output is the same rule at higher
//! stakes, because it makes the bytes *findable*.
//!
//! Two deliberate choices:
//!
//! * **Redact, don't drop.** A masked value keeps the fact that a credential
//!   appeared, which is itself something worth being able to search for, while
//!   removing the value.
//! * **Name-component matching, not substring matching.** `TOKEN=`,
//!   `GITHUB_TOKEN=` and `api_key:` are credentials; `authentication: failed`
//!   and `sort_key: name` are not. A substring rule for `AUTH` or `KEY` masks
//!   the second pair too, which quietly destroys the usefulness of the index for
//!   a credential that was never there — the same failure mode the existing
//!   `-p<value>` spec was written about.

use std::cell::Cell;
use std::collections::HashSet;
use std::sync::LazyLock;

use regex::{Captures, Regex};

/// Text with credentials masked, plus how many masks were applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redacted {
    pub text: String,
    pub redactions: u32,
}

impl Redacted {
    #[must_use]
    pub fn was_redacted(&self) -> bool {
        self.redactions > 0
    }
}

/// A run of unbroken token characters at least this long is treated as an
/// encoded blob rather than prose, and omitted.
///
/// Images alone are 42.1% of the measured payload. Adapters already keep binary
/// content out of the text, but a base64 body can still arrive inside a tool
/// result, where it is simultaneously useless to search and a place a key can
/// hide.
const BLOB_MIN_LEN: usize = 512;

/// Sentinel wrapper for a mask that has already been applied.
///
/// A mask written as literal `[redacted:credential]` is matchable by the later
/// passes: `ASSIGNMENT`'s bare-value class stops at `]`, so a second run
/// rewrites the mask into `[redacted:credential]]` and keeps growing. Wrapping
/// every mask in control characters gives all passes one cheap, uniform way to
/// recognize their own output and leave it alone. The sentinels are unwrapped
/// into readable text at the very end, so they never reach the index.
const MASK_OPEN: char = '\u{1}';
const MASK_CLOSE: char = '\u{2}';

fn mask(kind: &str) -> String {
    format!("{MASK_OPEN}redacted:{kind}{MASK_CLOSE}")
}

/// Has this span already been masked?
///
/// Checks both forms: the sentinel written by an earlier pass in this same call,
/// and the readable marker left by a previous call. The second check is what
/// makes [`redact`] idempotent — ingest may re-normalize text that was already
/// stored, and a mask that gets re-masked grows a bracket every time.
fn already_masked(span: &str) -> bool {
    span.contains(MASK_OPEN) || span.contains("[redacted:") || span.contains("[omitted:")
}

/// Turn sentinels into the readable markers that get indexed.
fn unwrap_masks(text: &str) -> String {
    text.replace(MASK_OPEN, "[").replace(MASK_CLOSE, "]")
}

static PEM_BLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?-----END [A-Z0-9 ]*PRIVATE KEY-----")
        .expect("PEM_BLOCK")
});

/// Both the header form (`Authorization: Bearer x`) and the JSON body form
/// (`"authorization": "x"`).
///
/// This pass owns `authorization` entirely — the name is deliberately absent
/// from [`SENSITIVE_NAMES`]. Handling it in both places makes the two passes
/// overlap on one input, and the second one mangles what the first produced.
static AUTH_HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)\b(authorization|proxy-authorization)"?\s*:\s*"?(bearer|basic|token|digest)?\s*[^\r\n,;"]+"#,
    )
    .expect("AUTH_HEADER")
});

static BEARER_VALUE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bbearer\s+[A-Za-z0-9._\-+/=]{8,}").expect("BEARER_VALUE"));

/// Provider key shapes. Each alternative is a vendor-documented prefix, so a
/// match is a credential by construction rather than by heuristic.
static PROVIDER_KEY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?x)",
        r"sk-ant-[A-Za-z0-9_\-]{16,}",
        r"|sk-[A-Za-z0-9_\-]{20,}",
        r"|gh[pousr]_[A-Za-z0-9]{20,}",
        r"|github_pat_[A-Za-z0-9_]{20,}",
        r"|glpat-[A-Za-z0-9_\-]{16,}",
        r"|AKIA[0-9A-Z]{16}",
        r"|ASIA[0-9A-Z]{16}",
        r"|xox[baprse]-[A-Za-z0-9\-]{10,}",
        r"|AIza[0-9A-Za-z_\-]{35}",
        r"|eyJ[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}",
    ))
    .expect("PROVIDER_KEY")
});

/// `NAME = value` / `NAME: value`, with the name captured for component
/// matching and the value captured in quoted or bare form.
/// `NAME = value`, `NAME: value`, and the quoted JSON/YAML forms of both. The
/// optional quote after the name is what lets `{"api_key": "x"}` match.
static ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)([A-Za-z][A-Za-z0-9]*(?:[_\-][A-Za-z0-9]+)*)("?\s*[:=]\s*)(?:"([^"\r\n]*)"|'([^'\r\n]*)'|([^\s,;}\]\)"']+))"#,
    )
    .expect("ASSIGNMENT")
});

static URL_USERINFO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"([a-zA-Z][a-zA-Z0-9+.\-]*://)([^\s/@:]+):([^\s/@]+)@").expect("URL_USERINFO")
});

static BLOB: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r"[A-Za-z0-9+/=_\-]{{{BLOB_MIN_LEN},}}")).expect("BLOB")
});

/// Short options that take a password glued to the flag.
///
/// Command-family limited on purpose. `-p` means `--parents` to `mkdir`,
/// `--publish` to `docker` and `--preserve` to `cp`; masking it everywhere
/// destroys legitimate history for a password that does not exist.
static PASSWORD_OPTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*(?:\S*/)?(mysql|mysqldump|mysqladmin|psql|redis-cli|smbclient|mongosh|mongodump)\b[^\r\n]*")
        .expect("PASSWORD_OPTION")
});

static GLUED_PASSWORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(-p|--password=)([^\s]+)").expect("GLUED_PASSWORD"));

/// Whole names that are credentials regardless of context.
static SENSITIVE_NAMES: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "api_key",
        "apikey",
        "api_secret",
        "access_key",
        "access_key_id",
        "access_token",
        "auth",
        "auth_token",
        "aws_access_key_id",
        "aws_secret_access_key",
        "bearer",
        "client_secret",
        "credential",
        "credentials",
        "id_token",
        "passwd",
        "password",
        "private_key",
        "pwd",
        "refresh_token",
        "secret",
        "secret_key",
        "session_token",
        "signing_key",
        "token",
    ]
    .into_iter()
    .collect()
});

/// Trailing components that make any name a credential.
///
/// `key` is absent on purpose: `sort_key`, `map_key` and `cache_key` are
/// ordinary field names. A `*_key` credential is caught by the pair rule below.
static SENSITIVE_TAILS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "credential",
        "credentials",
        "passwd",
        "password",
        "pwd",
        "secret",
        "token",
    ]
    .into_iter()
    .collect()
});

/// Two-component tails that are credentials even though neither half is.
const SENSITIVE_PAIRS: &[(&str, &str)] = &[
    ("access", "key"),
    ("api", "key"),
    ("client", "key"),
    ("encryption", "key"),
    ("private", "key"),
    ("secret", "key"),
    ("signing", "key"),
];

/// Does this identifier name a credential?
#[must_use]
pub fn is_sensitive_name(raw: &str) -> bool {
    let normalized = raw.to_ascii_lowercase().replace('-', "_");
    if SENSITIVE_NAMES.contains(normalized.as_str()) {
        return true;
    }
    let parts: Vec<&str> = normalized.split('_').filter(|part| !part.is_empty()).collect();
    let Some(last) = parts.last() else {
        return false;
    };
    if SENSITIVE_TAILS.contains(*last) {
        return true;
    }
    if parts.len() >= 2 {
        let penultimate = parts[parts.len() - 2];
        if SENSITIVE_PAIRS
            .iter()
            .any(|(first, second)| *first == penultimate && *second == *last)
        {
            return true;
        }
    }
    false
}

/// Mask every credential shape in `text`.
///
/// Ordering is load-bearing: multi-line PEM blocks collapse first so their
/// base64 body is never seen by the blob or provider-key passes, and the blob
/// pass runs last so it only sees what survived.
#[must_use]
pub fn redact(text: &str) -> Redacted {
    let count = Cell::new(0_u32);
    let bump = || count.set(count.get() + 1);

    let out = PEM_BLOCK.replace_all(text, |block: &Captures<'_>| {
        if already_masked(&block[0]) {
            return block[0].to_string();
        }
        bump();
        mask("private-key")
    });

    let out = PASSWORD_OPTION.replace_all(&out, |command: &Captures<'_>| {
        GLUED_PASSWORD
            .replace_all(&command[0], |option: &Captures<'_>| {
                if already_masked(&option[0]) {
                    return option[0].to_string();
                }
                bump();
                format!("{}{}", &option[1], mask("password"))
            })
            .into_owned()
    });

    let out = AUTH_HEADER.replace_all(&out, |header: &Captures<'_>| {
        if already_masked(&header[0]) {
            return header[0].to_string();
        }
        bump();
        match header.get(2) {
            Some(scheme) => format!(
                "{}: {} {}",
                &header[1],
                scheme.as_str(),
                mask("credential")
            ),
            None => format!("{}: {}", &header[1], mask("credential")),
        }
    });

    let out = BEARER_VALUE.replace_all(&out, |bearer: &Captures<'_>| {
        if already_masked(&bearer[0]) {
            return bearer[0].to_string();
        }
        bump();
        format!("Bearer {}", mask("credential"))
    });

    let out = ASSIGNMENT.replace_all(&out, |assignment: &Captures<'_>| {
        let name = &assignment[1];
        if already_masked(&assignment[0]) || !is_sensitive_name(name) {
            return assignment[0].to_string();
        }
        bump();
        let separator = &assignment[2];
        let masked = mask("credential");
        if assignment.get(3).is_some() {
            format!("{name}{separator}\"{masked}\"")
        } else if assignment.get(4).is_some() {
            format!("{name}{separator}'{masked}'")
        } else {
            format!("{name}{separator}{masked}")
        }
    });

    let out = PROVIDER_KEY.replace_all(&out, |key: &Captures<'_>| {
        if already_masked(&key[0]) {
            return key[0].to_string();
        }
        bump();
        mask("api-key")
    });

    let out = URL_USERINFO.replace_all(&out, |url: &Captures<'_>| {
        if already_masked(&url[0]) {
            return url[0].to_string();
        }
        bump();
        format!("{}{}:{}@", &url[1], &url[2], mask("credential"))
    });

    let out = BLOB.replace_all(&out, |blob: &Captures<'_>| {
        if already_masked(&blob[0]) {
            return blob[0].to_string();
        }
        bump();
        format!("{MASK_OPEN}omitted:blob:{}b{MASK_CLOSE}", blob[0].len())
    });

    Redacted {
        text: unwrap_masks(&out),
        redactions: count.get(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_gone(secret: &str, input: &str) {
        let result = redact(input);
        assert!(
            !result.text.contains(secret),
            "secret survived redaction\n  input: {input}\n  output: {}",
            result.text
        );
        assert!(
            result.was_redacted(),
            "redaction was not counted for: {input}"
        );
    }

    /// AC7, the whole point. Each of these is a shape actually present in the
    /// measured corpus.
    #[test]
    fn every_credential_shape_is_masked() {
        assert_gone(
            "abc123SECRETVALUE456xyz",
            "Authorization: Bearer abc123SECRETVALUE456xyz",
        );
        assert_gone(
            "abc123SECRETVALUE456xyz",
            "curl -H 'authorization: token abc123SECRETVALUE456xyz' https://api",
        );
        assert_gone(
            "sk-ant-api03-AAAAAAAAAAAAAAAAAAAAAAAA",
            "key is sk-ant-api03-AAAAAAAAAAAAAAAAAAAAAAAA today",
        );
        assert_gone(
            "ghp_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "remote uses ghp_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        );
        assert_gone("AKIAIOSFODNN7EXAMPLE", "aws id AKIAIOSFODNN7EXAMPLE");
        assert_gone(
            "hunter2",
            "GITHUB_TOKEN=hunter2",
        );
        assert_gone("hunter2", r#"{"api_key": "hunter2"}"#);
        assert_gone("hunter2", "export AWS_SECRET_ACCESS_KEY='hunter2'");
        assert_gone("hunter2", "PASSWORD: hunter2");
        assert_gone(
            "hunter2",
            "postgres://appuser:hunter2@db.internal:5432/app",
        );
        assert_gone(
            "MIIEvQIBADANBgkq",
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEvQIBADANBgkq\naaaa\n-----END RSA PRIVATE KEY-----",
        );
        assert_gone(
            "hunter2",
            "mysql -uroot -phunter2 mydb",
        );
    }

    /// The other half of the contract. Over-masking is not a safe default: it
    /// destroys the index's usefulness for content that never held a secret.
    #[test]
    fn ordinary_text_that_merely_looks_credential_ish_is_left_alone() {
        for benign in [
            "authentication: failed",
            "sort_key: name",
            "map_key = user_id",
            "cache_key: v2:users:7",
            "the token bucket refills every 5s",
            "PASSWORD_MIN_LENGTH = 12",
        ] {
            let result = redact(benign);
            assert_eq!(
                result.text, benign,
                "benign text was modified: {benign} -> {}",
                result.text
            );
            assert_eq!(result.redactions, 0, "spurious redaction on: {benign}");
        }
    }

    /// `PASSWORD_MIN_LENGTH` ends in `length`, not a credential tail — the case
    /// a substring rule for `PASSWORD` would get wrong.
    #[test]
    fn name_matching_uses_components_not_substrings() {
        assert!(is_sensitive_name("TOKEN"));
        assert!(is_sensitive_name("GITHUB_TOKEN"));
        assert!(is_sensitive_name("api-key"));
        assert!(is_sensitive_name("aws_secret_access_key"));
        assert!(is_sensitive_name("client_secret"));
        assert!(!is_sensitive_name("authentication"));
        assert!(!is_sensitive_name("sort_key"));
        assert!(!is_sensitive_name("password_min_length"));
        assert!(!is_sensitive_name("tokenizer"));
    }

    /// `-p` is `--parents` to `mkdir` and `--publish` to `docker`. Masking it
    /// there would corrupt real history to hide a password that is not present.
    #[test]
    fn glued_password_masking_is_limited_to_password_taking_commands() {
        for other_family in [
            "mkdir -p /tmp/nested/dirs",
            "docker run -p8080:80 nginx",
            "cp -pr src dst",
        ] {
            let result = redact(other_family);
            assert_eq!(
                result.text, other_family,
                "{other_family} must be untouched"
            );
        }
        let masked = redact("psql -phunter2 -h db");
        assert!(!masked.text.contains("hunter2"), "{}", masked.text);
    }

    #[test]
    fn long_encoded_blobs_are_omitted_rather_than_indexed() {
        let blob = "A".repeat(BLOB_MIN_LEN + 40);
        let result = redact(&format!("data:image/png;base64,{blob}"));
        assert!(!result.text.contains(&blob));
        assert!(result.text.contains("[omitted:blob:"), "{}", result.text);
    }

    #[test]
    fn a_blob_just_under_the_threshold_is_kept() {
        let almost = "A".repeat(BLOB_MIN_LEN - 1);
        let result = redact(&almost);
        assert_eq!(result.text, almost);
        assert_eq!(result.redactions, 0);
    }

    /// Redaction has to be a fixed point: running it twice must not mangle the
    /// markers it just wrote, because ingest may re-normalize stored text.
    #[test]
    fn redaction_is_idempotent() {
        let once = redact("Authorization: Bearer abc123SECRETVALUE456xyz\nTOKEN=hunter2");
        let twice = redact(&once.text);
        assert_eq!(twice.text, once.text);
        assert_eq!(twice.redactions, 0, "second pass found: {}", twice.text);
    }

    #[test]
    fn empty_and_plain_text_are_cheap_and_unchanged() {
        assert_eq!(redact("").text, "");
        assert_eq!(redact("fix the login bug").redactions, 0);
    }
}
