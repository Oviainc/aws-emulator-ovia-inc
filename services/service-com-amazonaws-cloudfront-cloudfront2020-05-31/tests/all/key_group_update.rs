// SPDX-License-Identifier: CC0-1.0

//! `UpdateKeyGroup`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront's key group surface states
//! for an update:
//!
//! * the update REPLACES the stored configuration with the one its payload carries, down to
//!   the members the payload leaves out, moves the group's last-modified instant to the
//!   instant it was written, and MINTS a fresh version token, under the id the group was
//!   created with — so a later `GetKeyGroup` answers from that record;
//! * the update is a conditional request on the group's current version: an `If-Match` that
//!   is missing or is not a version token at all is `InvalidIfMatchVersion` (400), one
//!   carrying a well-formed token that is not the current one is `PreconditionFailed` (412),
//!   and a refused update changes nothing — so the token an update hands back is the only
//!   one the NEXT update of that group lands on;
//! * an id no key group holds is `NoSuchResource` (404);
//! * the documented bounds on a key group CONFIGURATION hold on an update exactly as they
//!   hold on a create: at least one public key and a comment no longer than 128 characters
//!   (`InvalidArgument`, 400), and no more public keys than the published quota
//!   (`TooManyPublicKeysInKeyGroup`, 400);
//! * `KeyGroupConfig$Name` identifies ONE group, so a name another stored group holds cannot
//!   be moved onto this one (`KeyGroupAlreadyExists`, 409), while an update keeping the
//!   group's OWN name is not a conflict with itself and still mints a fresh version.
//!
//! The table rows differ along everything these rules generalise over: the configuration
//! updated and the configuration updated TO (every row of the lifecycle suite's table, so
//! every key-list size and every comment form appears on both sides of an update), whether
//! the update changes one member or all of them, whether the key list grows, shrinks or is
//! only reordered, whether the optional `Comment` is added, changed, emptied or dropped,
//! the FORM the version token is presented in (absent, bare, quoted, weak, malformed,
//! another group's) and the key-list and comment sizes at and past each published bound.
//!
//! The harness (the router, the request helpers and the key group table) is the key group
//! lifecycle and distribution lifecycle suites', reused rather than copied.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    comment_too_long, INVALID_IF_MATCH_VERSION_MESSAGE, KEY_GROUP_ALREADY_EXISTS_MESSAGE,
    KEY_GROUP_WITHOUT_A_PUBLIC_KEY_MESSAGE, MAX_KEY_GROUP_COMMENT, MAX_PUBLIC_KEYS_IN_KEY_GROUP,
    NO_SUCH_KEY_GROUP_MESSAGE, PRECONDITION_FAILED_FOR_THIS_OPERATION_MESSAGE,
    TOO_MANY_PUBLIC_KEYS_IN_KEY_GROUP_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, is_version_token, Answer, API_VERSION};
use crate::key_group_lifecycle::{create_group, get_group, mint_public_keys, table, Group};
use crate::public_key_lifecycle::{is_wire_timestamp, wire_instant};

// ---------------------------------------------------------------------------
// Calling the surface under test
// ---------------------------------------------------------------------------

/// `PUT /2020-05-31/key-group/{Id}` with the `KeyGroupConfig` document `body` — the member
/// the operation binds to the HTTP payload — and `If-Match` carried exactly as `if_match`
/// gives it (left off entirely when it is `None`).
async fn update_group(
    router: &MultiServiceRouter,
    id: &str,
    body: &str,
    if_match: Option<&str>,
) -> Answer {
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/key-group/{id}"),
        Some(body),
        if_match,
    )
    .await
}

/// Create one key group through the API and report `(Id, ETag)`.
async fn create(router: &MultiServiceRouter, row: &Group) -> (String, String) {
    let created = create_group(router, &row.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201 from CreateKeyGroup, got {} with {}",
        row.case, created.status, created.raw
    );
    let id = created.root().text_at("Id").unwrap_or_default().to_string();
    let etag = created.header("etag").to_string();
    assert!(
        is_version_token(&etag),
        "case {}: expected a minted version token in ETag, got {etag:?}",
        row.case
    );
    (id, etag)
}

/// Assert that the key group `id` names reads back as `expected` at version `etag`, and
/// report the `LastModifiedTime` the read states.
async fn assert_reads_back(
    router: &MultiServiceRouter,
    id: &str,
    etag: &str,
    expected: &Group,
    context: &str,
) -> String {
    let fetched = get_group(router, id).await;
    assert_eq!(
        fetched.status, 200,
        "case {} ({context}): expected the group to be readable, got {} with {}",
        expected.case, fetched.status, fetched.raw
    );
    assert_eq!(
        fetched.header("etag"),
        etag,
        "case {} ({context}): expected the read to report version {etag}, got {:?}",
        expected.case,
        fetched.header("etag")
    );
    assert_eq!(
        fetched.root().text_at("Id"),
        Some(id),
        "case {} ({context}): expected the read to report Id {id}, got {:?} with {}",
        expected.case,
        fetched.root().text_at("Id"),
        fetched.raw
    );
    expected.assert_matches(
        fetched
            .root()
            .at("KeyGroupConfig")
            .unwrap_or_else(|| {
                panic!(
                    "case {} ({context}): no KeyGroupConfig in {}",
                    expected.case, fetched.raw
                )
            }),
        context,
        &fetched.raw,
    );
    let last_modified = fetched
        .root()
        .text_at("LastModifiedTime")
        .unwrap_or_default()
        .to_string();
    assert!(
        is_wire_timestamp(&last_modified),
        "case {} ({context}): expected an ISO-8601 LastModifiedTime, got {last_modified:?} \
         in {}",
        expected.case,
        fetched.raw
    );
    last_modified
}

// ---------------------------------------------------------------------------
// An update replaces the configuration every later read answers from
// ---------------------------------------------------------------------------

/// The members a key group configuration a single-member row states carries, beside the
/// name the row keeps.
struct Payload {
    /// `Items`, the identifiers of the public keys in the group.
    items: Vec<String>,
    /// `Comment`, or `None` to leave the optional member out entirely.
    comment: Option<String>,
}

/// One single-member row: what the group HOLDS and what the update SUBMITS.
struct Variation {
    /// What this row is a case of, named in every assertion it drives.
    case: &'static str,
    held: Payload,
    submitted: Payload,
}

/// The configurations an update is run FROM and TO.
///
/// Every row of the lifecycle table updated to the next row's configuration — so each
/// key-list size and each comment form appears on both sides of an update — plus the
/// single-member variations of the first row (a comment added, changed, emptied and
/// dropped; a key list grown, shrunk and only reordered), which keep the group's own name
/// so that an update that renames nothing is exercised too.
///
/// Every name is unique across the pairs, so one store can hold every case.
fn transitions(rows: &[Group]) -> Vec<(Group, Group)> {
    let mut cases: Vec<(Group, Group)> = Vec::new();
    for (index, from) in rows.iter().enumerate() {
        let to = &rows[(index + 1) % rows.len()];
        cases.push((
            Group {
                case: format!("{} (before)", from.case),
                name: format!("kg-whole-{index}"),
                ..from.clone()
            },
            Group {
                case: format!("{} -> {}", from.case, to.case),
                // A name no stored group holds: what this row is about is the REPLACEMENT,
                // not the conflict.
                name: format!("kg-whole-renamed-{index}"),
                ..to.clone()
            },
        ));
    }

    // The widest key list the table carries, which is the published quota's worth.
    let widest = rows
        .iter()
        .max_by_key(|row| row.items.len())
        .expect("the lifecycle table is not empty");
    let one_key = widest.items[..1].to_vec();
    let every_key = widest.items.clone();
    let reversed: Vec<String> = widest.items.iter().rev().cloned().collect();
    let held_comment = Some("the comment the create stored".to_string());

    // Each row states the configuration HELD and the one SUBMITTED: one member moves and
    // the rest stay, and the name stays the group's own, so an update that renames nothing
    // is exercised too.
    let variations: Vec<Variation> = vec![
        Variation {
            case: "a comment added",
            held: Payload {
                items: one_key.clone(),
                comment: None,
            },
            submitted: Payload {
                items: one_key.clone(),
                comment: Some("a comment the create left out".to_string()),
            },
        },
        Variation {
            case: "a comment changed",
            held: Payload {
                items: one_key.clone(),
                comment: held_comment.clone(),
            },
            submitted: Payload {
                items: one_key.clone(),
                comment: Some("a comment that replaces another".to_string()),
            },
        },
        Variation {
            case: "a comment emptied",
            held: Payload {
                items: one_key.clone(),
                comment: held_comment.clone(),
            },
            submitted: Payload {
                items: one_key.clone(),
                comment: Some(String::new()),
            },
        },
        Variation {
            case: "a comment dropped",
            held: Payload {
                items: one_key.clone(),
                comment: held_comment.clone(),
            },
            submitted: Payload {
                items: one_key.clone(),
                comment: None,
            },
        },
        Variation {
            // "The comment cannot be longer than 128 characters": the bound itself is
            // admissible.
            case: "a comment grown to the published bound",
            held: Payload {
                items: one_key.clone(),
                comment: held_comment.clone(),
            },
            submitted: Payload {
                items: one_key.clone(),
                comment: Some("c".repeat(MAX_KEY_GROUP_COMMENT)),
            },
        },
        Variation {
            // The published quota on one group's key list, which is admissible.
            case: "a key list grown to the published quota",
            held: Payload {
                items: one_key.clone(),
                comment: held_comment.clone(),
            },
            submitted: Payload {
                items: every_key.clone(),
                comment: held_comment.clone(),
            },
        },
        Variation {
            case: "a key list shrunk to one key",
            held: Payload {
                items: every_key.clone(),
                comment: held_comment.clone(),
            },
            submitted: Payload {
                items: one_key.clone(),
                comment: held_comment.clone(),
            },
        },
        Variation {
            case: "a key list only reordered",
            held: Payload {
                items: every_key.clone(),
                comment: held_comment.clone(),
            },
            submitted: Payload {
                items: reversed,
                comment: held_comment.clone(),
            },
        },
    ];

    for (index, variation) in variations.into_iter().enumerate() {
        let name = format!("kg-member-{index}");
        let case = variation.case;
        cases.push((
            Group {
                case: format!("{case} (before)"),
                name: name.clone(),
                items: variation.held.items,
                comment: variation.held.comment,
            },
            Group {
                case: case.to_string(),
                name,
                items: variation.submitted.items,
                comment: variation.submitted.comment,
            },
        ));
    }
    cases
}

/// An update replaces the whole stored configuration under the group's own id, moves its
/// last-modified instant, mints a fresh version token, and leaves every other stored group
/// alone.
#[tokio::test]
async fn an_update_replaces_the_whole_configuration() {
    let router = emulator();
    let rows = table(&router).await;
    for (from, to) in transitions(&rows) {
        let case = to.case.clone();
        let (id, created_etag) = create(&router, &from).await;
        let created_last_modified =
            assert_reads_back(&router, &id, &created_etag, &from, "GetKeyGroup before the update")
                .await;

        let updated = update_group(&router, &id, &to.xml(), Some(&created_etag)).await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected HTTP 200 from UpdateKeyGroup, got {} with {}",
            updated.status, updated.raw
        );

        // The answer reports the record just written: the same identity, the submitted
        // configuration and a NEW version token.
        assert_eq!(
            updated.root().text_at("Id"),
            Some(id.as_str()),
            "case {case}: expected the update to report Id {id}, got {:?} with {}",
            updated.root().text_at("Id"),
            updated.raw
        );
        let etag = updated.header("etag").to_string();
        assert!(
            is_version_token(&etag),
            "case {case}: expected a minted version token in ETag, got {etag:?}"
        );
        assert_ne!(
            etag, created_etag,
            "case {case}: expected a fresh version token, got {created_etag} again"
        );
        to.assert_matches(
            updated
                .root()
                .at("KeyGroupConfig")
                .unwrap_or_else(|| panic!("case {case}: no KeyGroupConfig in {}", updated.raw)),
            "UpdateKeyGroup",
            &updated.raw,
        );
        let written_last_modified = updated
            .root()
            .text_at("LastModifiedTime")
            .unwrap_or_default()
            .to_string();
        assert!(
            is_wire_timestamp(&written_last_modified),
            "case {case}: expected an ISO-8601 LastModifiedTime, got \
             {written_last_modified:?} in {}",
            updated.raw
        );
        // The ordering is decided on the INSTANT each text names, never on the two texts:
        // the wire form carries only the significant digits of its subsecond fraction, so
        // the strings do not sort the way the instants they name do.
        let (written_at, created_at) = (
            wire_instant(&written_last_modified, &case),
            wire_instant(&created_last_modified, &case),
        );
        assert!(
            written_at >= created_at,
            "case {case}: expected the last-modified instant not to move backwards from \
             {created_last_modified} ({created_at:?}), got {written_last_modified} \
             ({written_at:?})"
        );

        // A later read answers from the stored record, at the version the update minted and
        // with the instant the update wrote.
        let read_last_modified =
            assert_reads_back(&router, &id, &etag, &to, "GetKeyGroup after an update").await;
        assert_eq!(
            read_last_modified, written_last_modified,
            "case {case}: expected the read to report the instant the update wrote \
             ({written_last_modified}), got {read_last_modified}"
        );

        // The version the update replaced no longer lands: the token it handed back is the
        // only one the next update of this group matches on.
        let stale = update_group(&router, &id, &from.xml(), Some(&created_etag)).await;
        assert_eq!(
            stale.status, 412,
            "case {case}: expected HTTP 412 for the version the update replaced, got {} \
             with {}",
            stale.status, stale.raw
        );
        let stale_message = stale.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            stale_message, PRECONDITION_FAILED_FOR_THIS_OPERATION_MESSAGE,
            "case {case}: expected the unmet-precondition sentence, got {stale_message:?} \
             in {}",
            stale.raw
        );
        let again = update_group(&router, &id, &from.xml(), Some(&etag)).await;
        assert_eq!(
            again.status, 200,
            "case {case}: expected the token the update minted to land, got {} with {}",
            again.status, again.raw
        );
        assert_reads_back(
            &router,
            &id,
            again.header("etag"),
            &from,
            "GetKeyGroup after updating back",
        )
        .await;
    }
}

// ---------------------------------------------------------------------------
// The conditional request
// ---------------------------------------------------------------------------

/// How a row presents the version token to the update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Version {
    /// No `If-Match` header at all.
    Absent,
    /// The group's current version, exactly as the `ETag` header reported it.
    Current,
    /// The current version in HTTP entity-tag quotes.
    QuotedCurrent,
    /// The current version as a weak entity tag.
    WeakCurrent,
    /// A literal that is not a version token at all.
    Malformed(&'static str),
    /// A well-formed version token that is another group's — so not this one's.
    OtherGroup,
}

/// One row of the precondition table.
#[derive(Debug, Clone, Copy)]
struct Precondition {
    case: &'static str,
    version: Version,
    /// The status the update is expected to answer with.
    status: u16,
    /// The error code the update is expected to answer with, or `None` when it lands.
    code: Option<&'static str>,
}

/// Every form the `If-Match` value takes, and what each makes of the update.
const PRECONDITIONS: &[Precondition] = &[
    Precondition {
        case: "no-if-match",
        version: Version::Absent,
        status: 400,
        code: Some("InvalidIfMatchVersion"),
    },
    Precondition {
        case: "empty-if-match",
        version: Version::Malformed(""),
        status: 400,
        code: Some("InvalidIfMatchVersion"),
    },
    Precondition {
        case: "if-match-that-is-not-a-token",
        version: Version::Malformed("not-a-version"),
        status: 400,
        code: Some("InvalidIfMatchVersion"),
    },
    Precondition {
        case: "if-match-token-of-the-wrong-length",
        version: Version::Malformed("E123"),
        status: 400,
        code: Some("InvalidIfMatchVersion"),
    },
    Precondition {
        case: "if-match-naming-another-group",
        version: Version::OtherGroup,
        status: 412,
        code: Some("PreconditionFailed"),
    },
    Precondition {
        case: "if-match-current",
        version: Version::Current,
        status: 200,
        code: None,
    },
    Precondition {
        case: "if-match-current-quoted",
        version: Version::QuotedCurrent,
        status: 200,
        code: None,
    },
    Precondition {
        case: "if-match-current-weak",
        version: Version::WeakCurrent,
        status: 200,
        code: None,
    },
];

/// An update lands only when the caller holds the group's current version: every other form
/// of `If-Match` is the refusal the operation models for it, and leaves the stored
/// configuration and its version exactly as they were.
#[tokio::test]
async fn an_update_lands_only_on_the_current_version() {
    for precondition in PRECONDITIONS {
        let case = precondition.case;
        let router = emulator();
        let rows = table(&router).await;
        // Two groups, so a well-formed token that is not the target's is available without
        // inventing one.
        let target = Group {
            case: format!("{case} target"),
            name: format!("kg-precondition-{case}"),
            ..rows[0].clone()
        };
        let other = Group {
            case: format!("{case} other"),
            name: format!("kg-precondition-other-{case}"),
            ..rows[1].clone()
        };
        let (id, etag) = create(&router, &target).await;
        let (other_id, other_etag) = create(&router, &other).await;
        let submitted = Group {
            case: case.to_string(),
            comment: Some(format!("the configuration {case} submits")),
            ..target.clone()
        };

        let quoted = format!("\"{etag}\"");
        let weak = format!("W/\"{etag}\"");
        let if_match: Option<&str> = match precondition.version {
            Version::Absent => None,
            Version::Current => Some(&etag),
            Version::QuotedCurrent => Some(&quoted),
            Version::WeakCurrent => Some(&weak),
            Version::Malformed(literal) => Some(literal),
            Version::OtherGroup => Some(&other_etag),
        };

        let updated = update_group(&router, &id, &submitted.xml(), if_match).await;
        assert_eq!(
            updated.status, precondition.status,
            "case {case}: expected HTTP {} from UpdateKeyGroup, got {} with {}",
            precondition.status, updated.status, updated.raw
        );
        match precondition.code {
            None => {
                let minted = updated.header("etag").to_string();
                assert_ne!(
                    minted, etag,
                    "case {case}: expected a fresh version token, got {etag} again"
                );
                assert_reads_back(
                    &router,
                    &id,
                    &minted,
                    &submitted,
                    "GetKeyGroup after an accepted update",
                )
                .await;
            }
            Some(code) => {
                assert_eq!(
                    updated.error_code(),
                    code,
                    "case {case}: expected {code}, got {:?} with {}",
                    updated.error_code(),
                    updated.raw
                );
                // Each refusal carries the fixed sentence CloudFront words it with: the
                // unmet precondition is stated of the OPERATION on this surface, and an
                // unusable `If-Match` is stated of the header the request carried.
                let expected_message = match code {
                    "PreconditionFailed" => PRECONDITION_FAILED_FOR_THIS_OPERATION_MESSAGE,
                    _ => INVALID_IF_MATCH_VERSION_MESSAGE,
                };
                let message = updated.root().find_text("Message").unwrap_or_default();
                assert_eq!(
                    message, expected_message,
                    "case {case}: expected the {code} sentence, got {message:?} in {}",
                    updated.raw
                );
                // A refused update writes nothing: the group is still readable, at the same
                // version and with the configuration it held.
                assert_reads_back(
                    &router,
                    &id,
                    &etag,
                    &target,
                    "GetKeyGroup after a refused update",
                )
                .await;
            }
        }

        // The group the row did not target is untouched either way.
        assert_reads_back(
            &router,
            &other_id,
            &other_etag,
            &other,
            "GetKeyGroup of the group the update did not name",
        )
        .await;
    }
}

// ---------------------------------------------------------------------------
// An id no key group holds
// ---------------------------------------------------------------------------

/// An id no key group holds is `NoSuchResource`, whatever the request says about the
/// version, with the absent-group sentence whatever the id's shape — this operation's own
/// recorded answer, which differs from `GetKeyGroup`'s. Nothing the account holds is touched.
#[tokio::test]
async fn an_id_no_key_group_holds_is_refused() {
    let router = emulator();
    let rows = table(&router).await;
    let stored = Group {
        case: "the one stored group".to_string(),
        name: "kg-missing-id-stored".to_string(),
        ..rows[0].clone()
    };
    let (id, etag) = create(&router, &stored).await;
    let submitted = Group {
        case: "the configuration the refused updates submit".to_string(),
        name: "kg-missing-id-submitted".to_string(),
        ..rows[1].clone()
    };

    // Ids no key group holds: a UUID of the shape this service mints key group ids in, one
    // from another entity family, and one that is no id at all.
    for (case, missing, expected) in [
        (
            "a UUID no group holds",
            "f9a6a0b4-8d39-4a1e-9f67-1b0f7f5d2c3e",
            NO_SUCH_KEY_GROUP_MESSAGE,
        ),
        (
            "an id from the distribution namespace",
            "E1AAAAAAAAAAAA",
            NO_SUCH_KEY_GROUP_MESSAGE,
        ),
        (
            "a value that is no id at all",
            "no-such-key-group",
            NO_SUCH_KEY_GROUP_MESSAGE,
        ),
    ] {
        for (version_case, if_match) in [
            ("no-if-match", None),
            ("current-of-another", Some(etag.as_str())),
        ] {
            let answer = update_group(&router, missing, &submitted.xml(), if_match).await;
            assert_eq!(
                answer.status, 404,
                "case {case}/{version_case}: expected HTTP 404 from UpdateKeyGroup, got {} \
                 with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "NoSuchResource",
                "case {case}/{version_case}: expected NoSuchResource, got {:?} with {}",
                answer.error_code(),
                answer.raw
            );
            let message = answer.root().find_text("Message").unwrap_or_default();
            assert_eq!(
                message, expected,
                "case {case}/{version_case}: expected {expected:?}, got {message:?} in {}",
                answer.raw
            );
        }
    }

    // The group the account does hold is exactly as it was.
    assert_reads_back(
        &router,
        &id,
        &etag,
        &stored,
        "GetKeyGroup after updates of ids no group holds",
    )
    .await;
}

// ---------------------------------------------------------------------------
// The configuration the update submits
// ---------------------------------------------------------------------------

/// One row of the bounds table: a configuration an update submits, and whether it is
/// accepted or the refusal it draws.
struct Bound {
    case: String,
    /// The `Items` the update submits.
    items: Vec<String>,
    /// The `Comment` the update submits.
    comment: Option<String>,
    /// `None` when the configuration is accepted, else `(code, message)`.
    refusal: Option<(&'static str, String)>,
}

/// The sentence an over-long comment is refused with, for a comment of `length`
/// characters: the service's own, which states the constraint and the length submitted, so
/// a row expecting it pins the variable part without restating the text.
fn over_long_comment(length: usize) -> String {
    comment_too_long(length).message.unwrap_or_default()
}

/// The documented bounds on a key group CONFIGURATION hold on an update exactly as they
/// hold on a create: one row at each published bound, one past it, and one far past it.
#[tokio::test]
async fn a_configuration_outside_the_documented_bounds_is_refused() {
    let router = emulator();
    let keys = mint_public_keys(&router, "kg-bounds", MAX_PUBLIC_KEYS_IN_KEY_GROUP).await;
    // A key list is a list of opaque identifiers, so a row past the published quota is
    // written from identifiers of that same form rather than from minted keys.
    let past_quota: Vec<String> = (0..=MAX_PUBLIC_KEYS_IN_KEY_GROUP)
        .map(|index| format!("K{index}QUOTAIDAAAAA"))
        .collect();
    let rows = vec![
        Bound {
            case: "one public key".to_string(),
            items: keys[..1].to_vec(),
            comment: None,
            refusal: None,
        },
        Bound {
            case: "the published quota of public keys".to_string(),
            items: keys.clone(),
            comment: None,
            refusal: None,
        },
        Bound {
            case: "one public key past the published quota".to_string(),
            items: past_quota.clone(),
            comment: None,
            refusal: Some((
                "TooManyPublicKeysInKeyGroup",
                TOO_MANY_PUBLIC_KEYS_IN_KEY_GROUP_MESSAGE.to_string(),
            )),
        },
        Bound {
            case: "far past the published quota of public keys".to_string(),
            items: (0..MAX_PUBLIC_KEYS_IN_KEY_GROUP * 4)
                .map(|index| format!("K{index}FARPASTIDAAA"))
                .collect(),
            comment: None,
            refusal: Some((
                "TooManyPublicKeysInKeyGroup",
                TOO_MANY_PUBLIC_KEYS_IN_KEY_GROUP_MESSAGE.to_string(),
            )),
        },
        Bound {
            case: "no public key at all".to_string(),
            items: Vec::new(),
            comment: None,
            refusal: Some((
                "InvalidArgument",
                KEY_GROUP_WITHOUT_A_PUBLIC_KEY_MESSAGE.to_string(),
            )),
        },
        Bound {
            case: "no public key at all and a comment".to_string(),
            items: Vec::new(),
            comment: Some("a group that names nothing".to_string()),
            refusal: Some((
                "InvalidArgument",
                KEY_GROUP_WITHOUT_A_PUBLIC_KEY_MESSAGE.to_string(),
            )),
        },
        Bound {
            case: "a comment one character inside the published bound".to_string(),
            items: keys[..1].to_vec(),
            comment: Some("c".repeat(MAX_KEY_GROUP_COMMENT - 1)),
            refusal: None,
        },
        Bound {
            case: "a comment at the published bound".to_string(),
            items: keys[..1].to_vec(),
            comment: Some("c".repeat(MAX_KEY_GROUP_COMMENT)),
            refusal: None,
        },
        Bound {
            case: "a comment one character past the published bound".to_string(),
            items: keys[..1].to_vec(),
            comment: Some("c".repeat(MAX_KEY_GROUP_COMMENT + 1)),
            // The refusal states the constraint and the length submitted, so the sentence
            // The refusal states the constraint and the LENGTH submitted, so the sentence
            // this row expects is read off that length rather than written out here.
            refusal: Some(("InvalidArgument", over_long_comment(MAX_KEY_GROUP_COMMENT + 1))),
        },
        Bound {
            case: "a comment far past the published bound".to_string(),
            items: keys[..1].to_vec(),
            comment: Some("c".repeat(MAX_KEY_GROUP_COMMENT * 4)),
            refusal: Some(("InvalidArgument", over_long_comment(MAX_KEY_GROUP_COMMENT * 4))),
        },
        Bound {
            // The bound counts CHARACTERS, so a comment of that many multi-byte characters
            // is inside it.
            case: "a multi-byte comment at the published bound".to_string(),
            items: keys[..1].to_vec(),
            comment: Some("é".repeat(MAX_KEY_GROUP_COMMENT)),
            refusal: None,
        },
        Bound {
            case: "a multi-byte comment one character past the published bound".to_string(),
            items: keys[..1].to_vec(),
            comment: Some("é".repeat(MAX_KEY_GROUP_COMMENT + 1)),
            // The length the refusal states is a CHARACTER count, so a multi-byte comment
            // one character past the bound draws the same sentence as a single-byte one.
            refusal: Some(("InvalidArgument", over_long_comment(MAX_KEY_GROUP_COMMENT + 1))),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        // Each row updates a group of its own, created from a configuration every bound
        // admits, so what the row tests is the UPDATE's payload.
        let held = Group {
            case: format!("{} (before)", row.case),
            name: format!("kg-bound-{index}"),
            items: keys[..1].to_vec(),
            comment: Some("the configuration the create stored".to_string()),
        };
        let (id, etag) = create(&router, &held).await;
        let submitted = Group {
            case: row.case.clone(),
            name: held.name.clone(),
            items: row.items.clone(),
            comment: row.comment.clone(),
        };

        let updated = update_group(&router, &id, &submitted.xml(), Some(&etag)).await;
        match &row.refusal {
            None => {
                assert_eq!(
                    updated.status, 200,
                    "case {}: expected HTTP 200 from UpdateKeyGroup, got {} with {}",
                    row.case, updated.status, updated.raw
                );
                assert_reads_back(
                    &router,
                    &id,
                    updated.header("etag"),
                    &submitted,
                    "GetKeyGroup after a configuration inside every bound",
                )
                .await;
            }
            Some((code, message)) => {
                assert_eq!(
                    updated.status, 400,
                    "case {}: expected HTTP 400 from UpdateKeyGroup, got {} with {}",
                    row.case, updated.status, updated.raw
                );
                assert_eq!(
                    updated.error_code(),
                    *code,
                    "case {}: expected {code}, got {:?} with {}",
                    row.case,
                    updated.error_code(),
                    updated.raw
                );
                let actual = updated.root().find_text("Message").unwrap_or_default();
                assert_eq!(
                    actual, *message,
                    "case {}: expected the sentence {message:?}, got {actual:?} in {}",
                    row.case, updated.raw
                );
                // A refused update writes nothing: the group still holds what the create
                // stored, at the version the create minted.
                assert_reads_back(
                    &router,
                    &id,
                    &etag,
                    &held,
                    "GetKeyGroup after a refused configuration",
                )
                .await;
            }
        }
    }
}

/// `KeyGroupConfig$Name` identifies ONE group: the name ANOTHER stored group holds cannot be
/// moved onto this one, while an update keeping the group's own name is not a conflict with
/// itself and still mints a fresh version.
#[tokio::test]
async fn a_name_another_key_group_holds_cannot_be_moved_onto_this_one() {
    let router = emulator();
    let rows = table(&router).await;
    for (index, pair) in rows.chunks(2).enumerate() {
        if pair.len() < 2 {
            continue;
        }
        let target = Group {
            case: format!("{} taking over a taken name", pair[0].case),
            name: format!("kg-conflict-target-{index}"),
            ..pair[0].clone()
        };
        let other = Group {
            case: format!("{} holding the name", pair[1].case),
            name: format!("kg-conflict-holder-{index}"),
            ..pair[1].clone()
        };
        let case = target.case.clone();
        let (id, etag) = create(&router, &target).await;
        let (other_id, other_etag) = create(&router, &other).await;

        // Every configuration under the taken name is refused, whatever else it says: the
        // name is what identifies the group, so the request names no group of its own.
        for (variant_case, submitted) in [
            (
                "the whole configuration of the group that holds the name",
                Group {
                    name: other.name.clone(),
                    ..other.clone()
                },
            ),
            (
                "this group's own keys under the taken name",
                Group {
                    name: other.name.clone(),
                    ..target.clone()
                },
            ),
        ] {
            let conflict = update_group(&router, &id, &submitted.xml(), Some(&etag)).await;
            assert_eq!(
                conflict.status, 409,
                "case {case}/{variant_case}: expected HTTP 409 from UpdateKeyGroup, got {} \
                 with {}",
                conflict.status, conflict.raw
            );
            assert_eq!(
                conflict.error_code(),
                "KeyGroupAlreadyExists",
                "case {case}/{variant_case}: expected KeyGroupAlreadyExists, got {:?} with {}",
                conflict.error_code(),
                conflict.raw
            );
            let message = conflict.root().find_text("Message").unwrap_or_default();
            assert_eq!(
                message, KEY_GROUP_ALREADY_EXISTS_MESSAGE,
                "case {case}/{variant_case}: expected the taken-name sentence, got \
                 {message:?} in {}",
                conflict.raw
            );
        }

        // Both groups are exactly as they were.
        assert_reads_back(
            &router,
            &id,
            &etag,
            &target,
            "GetKeyGroup after a refused take-over",
        )
        .await;
        assert_reads_back(
            &router,
            &other_id,
            &other_etag,
            &other,
            "GetKeyGroup of the group that holds the name",
        )
        .await;

        // The group's own name is not a conflict with itself: the update lands and mints a
        // fresh version, which is the one the next update matches on.
        let itself = Group {
            case: format!("{case} keeping its own name"),
            comment: Some("a comment under the group's own name".to_string()),
            ..target.clone()
        };
        let accepted = update_group(&router, &id, &itself.xml(), Some(&etag)).await;
        assert_eq!(
            accepted.status, 200,
            "case {case}: expected an update keeping the group's own name to land, got {} \
             with {}",
            accepted.status, accepted.raw
        );
        let minted = accepted.header("etag").to_string();
        assert_ne!(
            minted, etag,
            "case {case}: expected a fresh version token, got {etag} again"
        );
        assert_reads_back(
            &router,
            &id,
            &minted,
            &itself,
            "GetKeyGroup after an update keeping its own name",
        )
        .await;
    }
}
