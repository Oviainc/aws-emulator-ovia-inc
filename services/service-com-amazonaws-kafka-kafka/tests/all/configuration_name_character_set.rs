// SPDX-License-Identifier: CC0-1.0

//! The character set a configuration name may be built from, driven end to end
//! over the wire through the same `MultiServiceRouter` the emulator serves from
//! (the shared `kafka_harness`).
//!
//! RULE: a caller-supplied name the service turns into a resource identity is
//! admitted only when every character of it is in the service's name character
//! set — ASCII alphanumerics and `.`, `_`, `-`; a name carrying anything else is
//! refused with `BadRequestException` (HTTP 400) and registers nothing.
//!
//! The suites below assert that rule over a TABLE of names rather than one
//! recorded request: the rows differ along which character is out of the set
//! (space, punctuation, an ARN separator, a control character, a non-ASCII
//! letter), where in the name it sits (leading, embedded, trailing) and how long
//! the name is, with legally-named rows of the same lengths as the controls. The
//! second suite walks the printable ASCII range EXHAUSTIVELY, which is strictly
//! stronger than sampling it: for every one of those characters the admission
//! decision is checked against the set written out here, so the gate is pinned to
//! be a character-set gate and nothing narrower or wider.
//!
//! The third suite pins the OTHER half of what a name must satisfy before it can
//! become an identity — its length — on each published bound and on the first
//! value past each of them, so the character gate and the length gate are shown
//! to be two rules and not one.

use serde_json::Value;

// The router harness, the signing region and the account id every suite of this
// crate shares, so this one adds only its own rule.
use super::kafka_harness::{Emulator, Reply, ACCOUNT, REGION};

/// The service's own refusal wordings and name-length bound, so the suite and
/// the handler cannot disagree about what a caller is told or about where the
/// bound sits.
use service_com_amazonaws_kafka_kafka::state::{
    MAX_CONFIGURATION_NAME_LENGTH, MIN_CONFIGURATION_NAME_LENGTH,
    RESOURCE_NAME_INVALID_CHARACTERS_MESSAGE, RESOURCE_NAME_LENGTH_MESSAGE,
};

/// A `server.properties` body every row submits, so the only thing that differs
/// between rows is the name.
const PROPERTIES: &[u8] = b"auto.create.topics.enable=true\n";

/// The characters a name may be built from, written out INDEPENDENTLY of the
/// service's own predicate so this suite is an oracle and not a mirror: Amazon
/// MSK admits a configuration name made of ASCII alphanumerics and the three
/// separators `.`, `_` and `-`.
const NAME_SEPARATORS: &str = "._-";

fn in_name_set(c: char) -> bool {
    c.is_ascii_alphanumeric() || NAME_SEPARATORS.contains(c)
}

/// One row: a name, and whether the service may register it.
#[derive(Debug)]
struct NameRow {
    case: &'static str,
    name: String,
    accepted: bool,
}

impl NameRow {
    fn accepted(case: &'static str, name: &str) -> Self {
        Self {
            case,
            name: name.to_string(),
            accepted: true,
        }
    }

    fn refused(case: &'static str, name: &str) -> Self {
        Self {
            case,
            name: name.to_string(),
            accepted: false,
        }
    }
}

/// `ListConfigurations`, which the model binds to `GET /v1/configurations`.
async fn list(emulator: &Emulator) -> Reply {
    emulator.call("GET", "/v1/configurations", None).await
}

/// Every name `ListConfigurations` reports, in the order it reports them.
async fn listed_names(emulator: &Emulator) -> Vec<String> {
    let reply = list(emulator).await;
    assert_eq!(
        reply.status, 200,
        "expected ListConfigurations to succeed, got {} {}",
        reply.status, reply.body
    );
    reply.body["configurations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            entry["name"]
                .as_str()
                .unwrap_or_else(|| panic!("a listed configuration carries no name: {entry}"))
                .to_string()
        })
        .collect()
}

/// The table: one row per way a name can sit inside or outside the set.
///
/// The two 29-character rows are the pair that separates this gate from the
/// length gate beside it — they differ only in one character being a space, so a
/// refusal of the second cannot be a length refusal.
fn name_rows() -> Vec<NameRow> {
    let legal_29 = "sim2real-behav-legal-name-29x";
    let space_29 = "sim2real-behav legal-name-29x";
    assert_eq!(
        legal_29.chars().count(),
        29,
        "the length-control pair must be 29 characters long, got {}",
        legal_29.chars().count()
    );
    assert_eq!(
        legal_29.chars().count(),
        space_29.chars().count(),
        "the length-control pair must differ only in one character"
    );
    // 128 characters, every one of them in the set: the longest name MSK
    // creates, so the character-set gate must not be refusing on size.
    let legal_128: String = "a-b_c.9".chars().cycle().take(128).collect();

    vec![
        NameRow::accepted("lowercase", "configuration"),
        NameRow::accepted("mixed-case-and-digits", "Config2Name9"),
        NameRow::accepted("dots", "config.with.dots"),
        NameRow::accepted("underscores", "config_with_underscores"),
        NameRow::accepted("hyphens", "config-with-hyphens"),
        NameRow::accepted("every-separator", "a.b_c-d"),
        NameRow::accepted("single-character", "c"),
        NameRow::accepted("all-separators-only", "._-"),
        NameRow::accepted("legal-29-characters", legal_29),
        NameRow::accepted("legal-128-characters", &legal_128),
        NameRow::refused("embedded-space", "sim2real behav illegal name!"),
        NameRow::refused("space-only-difference-29-characters", space_29),
        NameRow::refused("exclamation-mark", "config!"),
        NameRow::refused("leading-space", " config"),
        NameRow::refused("trailing-space", "config "),
        NameRow::refused("tab", "config\tname"),
        NameRow::refused("newline", "config\nname"),
        // The two ARN separators: a name holding either would split the ARN this
        // service mints from it.
        NameRow::refused("slash", "config/name"),
        NameRow::refused("colon", "config:name"),
        NameRow::refused("at-sign", "config@name"),
        NameRow::refused("plus", "config+name"),
        NameRow::refused("percent", "config%name"),
        NameRow::refused("asterisk", "config*name"),
        NameRow::refused("comma", "config,name"),
        NameRow::refused("quote", "config\"name"),
        NameRow::refused("backslash", "config\\name"),
        NameRow::refused("brace", "config{name}"),
        NameRow::refused("non-ascii-letter", "cönfig"),
        NameRow::refused("non-latin-script", "構成"),
        NameRow::refused("emoji", "config\u{1f600}"),
    ]
}

/// RULE: a configuration name is registered only when every character of it is
/// in the service's name character set; a name carrying anything else is refused
/// 400 `BadRequestException` and nothing about it reaches the store.
#[tokio::test]
async fn a_configuration_name_outside_the_character_set_is_refused() {
    let emulator = Emulator::new();
    let rows = name_rows();

    for row in &rows {
        let reply = emulator
            .create_configuration(&row.name, None, PROPERTIES)
            .await;
        if row.accepted {
            assert_eq!(
                reply.status, 200,
                "case {}: expected CreateConfiguration({:?}) to succeed, got {} {}",
                row.case, row.name, reply.status, reply.body
            );
            // The name is the resource's identity, so it comes back byte
            // identical and the minted ARN is keyed by it.
            assert_eq!(
                reply.at("/name").as_str(),
                Some(row.name.as_str()),
                "case {}: expected the name echoed verbatim, got {}",
                row.case,
                reply.body
            );
            let arn = reply.at("/arn").as_str().unwrap_or_else(|| {
                panic!("case {}: CreateConfiguration returned no arn", row.case)
            });
            let prefix = format!(
                "arn:aws:kafka:{REGION}:{ACCOUNT}:configuration/{}/",
                row.name
            );
            assert!(
                arn.starts_with(&prefix),
                "case {}: expected an ARN starting {prefix:?}, got {arn:?}",
                row.case
            );
            // Read the create back through DescribeConfiguration: an admitted
            // name is a configuration a reader serves, not just a 200.
            let described = emulator.describe_configuration(arn).await;
            assert_eq!(
                described.status, 200,
                "case {}: expected DescribeConfiguration to serve {arn:?}, got {} {}",
                row.case, described.status, described.body
            );
            assert_eq!(
                described.at("/name").as_str(),
                Some(row.name.as_str()),
                "case {}: expected DescribeConfiguration to report the stored name, got {}",
                row.case,
                described.body
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case {}: expected CreateConfiguration({:?}) to be refused with 400, got {} {}",
                row.case, row.name, reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                "BadRequestException",
                "case {}: expected BadRequestException, got {}",
                row.case,
                reply.body
            );
            assert_eq!(
                reply.error_message(),
                RESOURCE_NAME_INVALID_CHARACTERS_MESSAGE,
                "case {}: unexpected refusal wording in {}",
                row.case,
                reply.body
            );
            // A refusal mints nothing: no ARN, no revision, no state.
            assert_eq!(
                reply.at("/arn"),
                &Value::Null,
                "case {}: a refused create returned an ARN: {}",
                row.case,
                reply.body
            );
        }
    }

    // Exactly the admitted names are in the store, each once, and no refused
    // name reached it in any rewritten or sanitised form.
    let listed = listed_names(&emulator).await;
    let mut expected: Vec<String> = rows
        .iter()
        .filter(|row| row.accepted)
        .map(|row| row.name.clone())
        .collect();
    let mut found = listed.clone();
    expected.sort();
    found.sort();
    assert_eq!(
        found, expected,
        "expected ListConfigurations to report exactly the admitted names"
    );
    for row in rows.iter().filter(|row| !row.accepted) {
        let stripped: String = row.name.chars().filter(|c| in_name_set(*c)).collect();
        assert!(
            !listed.iter().any(|name| *name == row.name || *name == stripped),
            "case {}: a refused name reached the store (verbatim or as {stripped:?}): {listed:?}",
            row.case
        );
    }
}

/// RULE, exhaustively: across the WHOLE printable ASCII range, a name is
/// admitted exactly when every character of it is in the service's name
/// character set — so the gate is neither narrower (refusing a legal separator)
/// nor wider (admitting one more punctuation mark) than the set.
#[tokio::test]
async fn every_printable_ascii_character_is_admitted_exactly_when_the_name_set_holds_it() {
    let emulator = Emulator::new();
    let mut admitted = 0usize;

    for (index, byte) in (0x20u8..0x7f).enumerate() {
        let candidate = char::from(byte);
        // A distinct name per row, so an accepted one is never refused as a
        // duplicate: the index digits and the separators are themselves in the
        // set, and `candidate` is the only character under test.
        let name = format!("c{index}x{candidate}end");
        let reply = emulator.create_configuration(&name, None, PROPERTIES).await;
        if in_name_set(candidate) {
            admitted += 1;
            assert_eq!(
                reply.status, 200,
                "case char {candidate:?} (0x{byte:02x}): expected {name:?} to be admitted, got {} {}",
                reply.status, reply.body
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case char {candidate:?} (0x{byte:02x}): expected {name:?} to be refused, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                "BadRequestException",
                "case char {candidate:?} (0x{byte:02x}): expected BadRequestException, got {}",
                reply.body
            );
        }
    }

    // 62 ASCII alphanumerics plus the three separators: the shape of the set,
    // asserted so a sweep that silently stopped admitting anything cannot pass.
    assert_eq!(
        admitted, 65,
        "expected 65 of the printable ASCII characters to be in the name set, got {admitted}"
    );
    assert_eq!(
        listed_names(&emulator).await.len(),
        admitted,
        "expected exactly the admitted names in the store"
    );
}

/// RULE: a configuration name is registered only when its length, counted in
/// characters, is within the bound the service registers; a name outside that
/// bound — at EITHER end — is refused 400 `BadRequestException` with the one
/// length sentence and registers nothing, and is never truncated into a name the
/// caller did not ask for. Each published bound is exercised, as is the first
/// value past each of them.
#[tokio::test]
async fn a_configuration_name_outside_the_length_bound_is_refused() {
    let emulator = Emulator::new();
    let shortest = MIN_CONFIGURATION_NAME_LENGTH;
    let longest = MAX_CONFIGURATION_NAME_LENGTH;

    // Inside the bound, at both ends and one step in from each: a name of a
    // legal length is registered and reported back byte identical. The
    // alphabet is in the character set, so length is the only thing under test.
    let mut admitted: Vec<String> = Vec::new();
    for length in [shortest, shortest + 1, longest - 1, longest] {
        let name: String = "abcdefghij".chars().cycle().take(length).collect();
        let reply = emulator.create_configuration(&name, None, PROPERTIES).await;
        assert_eq!(
            reply.status, 200,
            "case length={length}: expected a name of a legal length to be registered, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.at("/name").as_str(),
            Some(name.as_str()),
            "case length={length}: expected the name echoed verbatim, got {}",
            reply.body
        );
        admitted.push(name);
    }

    // Outside it: nothing at all, one character past the ceiling, and far past
    // it. A different letter, so a name truncated to the ceiling could not be
    // mistaken for one of the admitted names above.
    for length in [0, longest + 1, longest + 2, longest * 8] {
        let name: String = std::iter::repeat_n('z', length).collect();
        let reply = emulator.create_configuration(&name, None, PROPERTIES).await;
        assert_eq!(
            reply.status, 400,
            "case length={length}: expected a name outside the bound to be refused, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "BadRequestException",
            "case length={length}: expected BadRequestException, got {}",
            reply.body
        );
        assert_eq!(
            reply.error_message(),
            RESOURCE_NAME_LENGTH_MESSAGE,
            "case length={length}: unexpected refusal wording in {}",
            reply.body
        );
        assert_eq!(
            reply.at("/arn"),
            &Value::Null,
            "case length={length}: a refused create returned an ARN: {}",
            reply.body
        );
    }

    // Exactly the names of a legal length are in the store: no refused name
    // reached it, and none of them was truncated to the ceiling and registered.
    let mut listed = listed_names(&emulator).await;
    listed.sort();
    admitted.sort();
    assert_eq!(
        listed, admitted,
        "expected ListConfigurations to report exactly the names of a legal length"
    );
}
