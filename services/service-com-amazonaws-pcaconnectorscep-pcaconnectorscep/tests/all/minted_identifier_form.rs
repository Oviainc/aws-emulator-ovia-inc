// SPDX-License-Identifier: CC0-1.0

//! Every identifier this service mints is a well-formed UUID, whichever operation minted it.
//!
//! The rule under test: a minted connector id, challenge id and challenge `Password` is a
//! canonical hyphenated lowercase UUID — five groups of `8-4-4-4-12` hexadecimal digits with
//! the version nibble of a random-bytes UUID and one of that form's four variant nibbles —
//! and it is unique per mint and stable once stored. The id form is the one
//! `ConnectorArn`/`ChallengeArn` state (`smithy_query: shape ConnectorArn,ChallengeArn`:
//! `[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}`) and the password form is the "in UUID format"
//! its `Password` member documents (`smithy_query: shape Challenge`).
//!
//! The three mints go through ONE identifier mint, reached from two operations: `CreateConnector`
//! for a connector id and `CreateChallenge` for a challenge id and its password. This suite
//! drives both, so neither call site can drift from the form the other is held to. The rows
//! differ along what the rule generalises over: the CA the connector is created from and how
//! many challenges — hence how many mints — are taken from one connector.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{self, label, new_router, Answer};

/// A region that is NOT the emulator default, so an ARN built from the request's own SigV4
/// credential scope is distinguishable from a hardcoded one.
const REGION: &str = "eu-west-2";

/// The group lengths of the canonical hyphenated UUID form, from the id pattern
/// `ConnectorArn` and `ChallengeArn` embed (`[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}`).
const GROUP_LENGTHS: [usize; 5] = [8, 4, 4, 4, 12];

/// The version nibble a UUID built from random bytes carries: the first character of the
/// third group.
const RANDOM_VERSION_NIBBLE: char = '4';

/// The variant nibbles that form allows, as the first character of the fourth group.
const VARIANT_NIBBLES: [char; 4] = ['8', '9', 'a', 'b'];

/// Private-CA ARNs matching `CertificateAuthorityArn`'s own pattern
/// (`^arn:aws(-[a-z]+)*:acm-pca:<region>:<account>:certificate-authority/<uuid>$`). Rows name
/// different CAs, so no assertion can pass by echoing one value.
const CA_ARNS: &[&str] = &[
    "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/11111111-2222-3333-4444-555555555555",
    "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/0123456f-89ab-cdef-0123-456789abcdef",
    "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/99999999-8888-7777-6666-555544443333",
];

/// The segment a challenge ARN inserts between its connector's ARN and the challenge id.
const CHALLENGE_SEGMENT: &str = "/challenge/";

/// The segment a connector ARN inserts before the connector id.
const CONNECTOR_SEGMENT: &str = ":connector/";

async fn create_connector(router: &MultiServiceRouter, request: Value) -> Answer {
    harness::call(router, REGION, "POST", "/connectors", Some(request)).await
}

async fn get_connector(router: &MultiServiceRouter, arn: &str) -> Answer {
    harness::call(
        router,
        REGION,
        "GET",
        &format!("/connectors/{}", label(arn)),
        None,
    )
    .await
}

async fn create_challenge(router: &MultiServiceRouter, request: Value) -> Answer {
    harness::call(router, REGION, "POST", "/challenges", Some(request)).await
}

async fn get_challenge_password(router: &MultiServiceRouter, arn: &str) -> Answer {
    harness::call(
        router,
        REGION,
        "GET",
        &format!("/challengePasswords/{}", label(arn)),
        None,
    )
    .await
}

/// Why `value` is not a canonical hyphenated UUID, or `None` when it is.
///
/// Stated here as the wire form, independently of how the service builds it: the group count
/// and lengths, the lowercase hexadecimal alphabet, the version nibble of a random-bytes UUID
/// and one of that form's variant nibbles.
fn uuid_form_violation(value: &str) -> Option<String> {
    let groups: Vec<&str> = value.split('-').collect();
    if groups.len() != GROUP_LENGTHS.len() {
        return Some(format!(
            "expected {} hyphen-separated groups, got {}",
            GROUP_LENGTHS.len(),
            groups.len()
        ));
    }
    for (index, (group, expected)) in groups.iter().zip(GROUP_LENGTHS).enumerate() {
        if group.len() != expected {
            return Some(format!(
                "group {index} is {} characters, expected {expected}",
                group.len()
            ));
        }
        if let Some(character) = group
            .chars()
            .find(|character| !character.is_ascii_digit() && !('a'..='f').contains(character))
        {
            return Some(format!(
                "group {index} carries {character:?}, which is not a lowercase hexadecimal digit"
            ));
        }
    }
    // The group lengths are checked above, so each group has a first character; an absent one
    // is reported as a violation rather than falling through to "well formed".
    let version = groups[2].chars().next().unwrap_or_default();
    if version != RANDOM_VERSION_NIBBLE {
        return Some(format!(
            "the version nibble is {version:?}, expected {RANDOM_VERSION_NIBBLE:?}"
        ));
    }
    let variant = groups[3].chars().next().unwrap_or_default();
    if !VARIANT_NIBBLES.contains(&variant) {
        return Some(format!(
            "the variant nibble is {variant:?}, expected one of {VARIANT_NIBBLES:?}"
        ));
    }
    None
}

/// Assert `value` is a canonical UUID, and that `minted` has not seen it before.
fn check_minted(case: &str, what: &str, value: &str, minted: &mut BTreeSet<String>) {
    let violation = uuid_form_violation(value);
    assert!(
        violation.is_none(),
        "case {case:?}: the {what} must be a canonical UUID, got {value:?}: {}",
        violation.unwrap_or_default()
    );
    assert!(
        minted.insert(value.to_string()),
        "case {case:?}: the {what} {value:?} was already minted; every mint must be unique"
    );
}

/// The id segment of a connector ARN.
fn connector_id(case: &str, connector_arn: &str) -> String {
    connector_arn
        .rsplit_once(CONNECTOR_SEGMENT)
        .map(|(_, id)| id.to_string())
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a connector segment in {connector_arn:?}")
        })
}

/// The challenge id segment of a challenge ARN.
fn challenge_id(case: &str, challenge_arn: &str) -> String {
    challenge_arn
        .rsplit_once(CHALLENGE_SEGMENT)
        .map(|(_, id)| id.to_string())
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a challenge segment in {challenge_arn:?}")
        })
}

/// One row of the table: a connector created from `ca_arn`, and `challenges` challenges taken
/// from it.
#[derive(Debug)]
struct Case {
    name: &'static str,
    ca_arn: &'static str,
    challenges: usize,
}

/// The whole rule, row by row, over both operations that mint an identifier.
#[tokio::test]
async fn every_minted_identifier_is_a_canonical_uuid() {
    let cases = [
        Case {
            name: "one challenge",
            ca_arn: CA_ARNS[0],
            challenges: 1,
        },
        Case {
            name: "two challenges",
            ca_arn: CA_ARNS[1],
            challenges: 2,
        },
        Case {
            name: "three challenges",
            ca_arn: CA_ARNS[2],
            challenges: 3,
        },
    ];

    // Every identifier any row minted, so uniqueness is checked across the whole table and
    // not merely within one connector.
    let mut minted: BTreeSet<String> = BTreeSet::new();
    let (router, engine) = new_router();

    for case in &cases {
        let name = case.name;

        // CreateConnector: the first of the two call sites that take an identifier.
        let created =
            create_connector(&router, json!({ "CertificateAuthorityArn": case.ca_arn })).await;
        assert_eq!(
            created.status, 202,
            "case {name:?}: expected 202 from CreateConnector, got {} / {:?}",
            created.status, created.body
        );
        let connector_arn = created.text("ConnectorArn");
        check_minted(
            name,
            "connector id",
            &connector_id(name, &connector_arn),
            &mut minted,
        );

        // The stored record carries the same ARN the create answered with.
        harness::release_pending(&engine, &connector_arn);
        let read = get_connector(&router, &connector_arn).await;
        assert_eq!(
            read.connector("Arn"),
            &json!(connector_arn),
            "case {name:?}: GetConnector must read back the ARN CreateConnector minted, got {:?}",
            read.body
        );

        // CreateChallenge: the second call site, minting a challenge id AND a password.
        for round in 0..case.challenges {
            let challenge =
                create_challenge(&router, json!({ "ConnectorArn": connector_arn.clone() })).await;
            assert_eq!(
                challenge.status, 202,
                "case {name:?} round {round}: expected 202 from CreateChallenge, got {} / {:?}",
                challenge.status, challenge.body
            );
            let challenge_arn = challenge
                .challenge("Arn")
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {name:?} round {round}: expected a challenge Arn in {:?}",
                        challenge.body
                    )
                })
                .to_string();
            let password = challenge
                .challenge("Password")
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {name:?} round {round}: expected a challenge Password in {:?}",
                        challenge.body
                    )
                })
                .to_string();

            check_minted(
                name,
                "challenge id",
                &challenge_id(name, &challenge_arn),
                &mut minted,
            );
            check_minted(name, "challenge password", &password, &mut minted);

            // The minted password is what the reader answers with, unchanged.
            let reread = get_challenge_password(&router, &challenge_arn).await;
            assert_eq!(
                (reread.status, reread.body["Password"].clone()),
                (200, json!(password)),
                "case {name:?} round {round}: GetChallengePassword must read back the minted \
                 password, got {} / {:?}",
                reread.status,
                reread.body
            );
        }
    }

    // Three rows mint three connector ids plus six challenge ids and six passwords.
    let expected_mints = cases.len() + 2 * cases.iter().map(|case| case.challenges).sum::<usize>();
    assert_eq!(
        minted.len(),
        expected_mints,
        "expected {expected_mints} distinct identifiers across the table, got {}: {minted:?}",
        minted.len()
    );
}

/// The form check itself refuses what the mint must never produce, so a green suite above is
/// not green because the check accepts anything.
#[test]
fn the_form_check_refuses_a_value_that_is_not_a_canonical_uuid() {
    let rejected = [
        ("32 hex characters, no hyphens", "4f3c2b1a4f3c4b1a8f3c2b1a4f3c2b1a"),
        ("four groups", "4f3c2b1a-4f3c-4b1a-8f3c2b1a4f3c"),
        ("a group of the wrong length", "4f3c2b1a-4f3c-4b1a-8f3c-2b1a4f3c2b1"),
        ("uppercase hexadecimal", "4F3C2B1A-4F3C-4B1A-8F3C-2B1A4F3C2B1A"),
        ("a non-hexadecimal digit", "4f3c2b1g-4f3c-4b1a-8f3c-2b1a4f3c2b1a"),
        ("the wrong version nibble", "4f3c2b1a-4f3c-1b1a-8f3c-2b1a4f3c2b1a"),
        ("the wrong variant nibble", "4f3c2b1a-4f3c-4b1a-0f3c-2b1a4f3c2b1a"),
    ];
    for (name, value) in rejected {
        assert!(
            uuid_form_violation(value).is_some(),
            "case {name:?}: {value:?} must be refused as a canonical UUID"
        );
    }
    // And it accepts the canonical form, at both ends of the variant range.
    for value in [
        "00000000-0000-4000-8000-000000000000",
        "ffffffff-ffff-4fff-bfff-ffffffffffff",
    ] {
        assert!(
            uuid_form_violation(value).is_none(),
            "case {value:?}: a canonical UUID must be accepted, got {:?}",
            uuid_form_violation(value)
        );
    }
}
