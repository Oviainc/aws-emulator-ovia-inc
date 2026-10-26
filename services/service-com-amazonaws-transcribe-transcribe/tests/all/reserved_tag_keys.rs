// SPDX-License-Identifier: CC0-1.0

//! The reserved tag-key namespace, end to end over the wire.
//!
//! The rule these tests pin: the `aws:` tag-key prefix is reserved for the tags Amazon
//! Web Services applies itself, so no operation that persists caller-supplied tags may
//! store a key that claims it — a request carrying one is refused with this service's
//! client error and writes nothing, while a key that merely LOOKS like it (no colon, the
//! prefix further in, a different prefix) is an ordinary key and is stored.
//!
//! The rows differ along what the rule generalises over: the key's casing, the number of
//! keys a request carries and where in that list the reserved key sits, whether the tags
//! reach the service through `TagResource` or through a create/start operation's `Tags`
//! member, the resource FAMILY the tags belong to (every family this service stores
//! records of, reused from `crate::tagging_resource_families`), and — on the accepted
//! side — keys that share a prefix of `aws:` without claiming it, including one whose
//! first bytes are a multi-byte character.
//!
//! Every call goes through the same `MultiServiceRouter` the production router dispatches
//! on, reusing the transcription-job suite's harness (`crate::transcription_jobs`) rather
//! than standing up a second emulator, so these tests exercise the awsJson1_1 envelope
//! and the `TagKey` boundary constraint too.

use serde_json::{json, Value};

use service_com_amazonaws_transcribe_transcribe::state::DEFAULT_REGION;

use crate::tagging_resource_families::{expected_pairs, families, pairs, tag_list, Family};
use crate::transcription_jobs::Transcribe;

/// The modelled client error this service answers for a request that fails validation,
/// and the status its awsJson1_1 wire protocol puts on every client error (both pinned
/// by `crate::client_error_wire_status`).
const REFUSAL_CODE: &str = "BadRequestException";
/// See [`REFUSAL_CODE`].
const CLIENT_ERROR_STATUS: u16 = 400;

/// One request's `Tags` list, with the key that must be refused when the row is one of
/// the reserved ones ([`reserved_rows`]) and `""` when every key in it is ordinary
/// ([`ordinary_rows`]).
struct Row {
    case: &'static str,
    tags: Vec<(&'static str, &'static str)>,
    refused_key: &'static str,
}

/// Tag lists that claim the reserved namespace, each with the key that must be refused.
///
/// The rows vary the casing of the prefix, the key behind it (including a key that is
/// nothing BUT the prefix, which the modelled `TagKey` `@length(min: 1, max: 128)`
/// admits), how many tags the request carries and where the reserved key sits in the
/// list.
fn reserved_rows() -> Vec<Row> {
    vec![
        Row {
            case: "a reserved key alone",
            tags: vec![("aws:owner", "me")],
            refused_key: "aws:owner",
        },
        Row {
            case: "the prefix in upper case",
            tags: vec![("AWS:owner", "me")],
            refused_key: "AWS:owner",
        },
        Row {
            case: "the prefix in mixed case",
            tags: vec![("Aws:CostCenter", "7")],
            refused_key: "Aws:CostCenter",
        },
        Row {
            case: "the prefix alone",
            tags: vec![("aws:", "me")],
            refused_key: "aws:",
        },
        Row {
            case: "a reserved key behind an ordinary one",
            tags: vec![("team", "speech"), ("aws:created-by", "cloudformation")],
            refused_key: "aws:created-by",
        },
        Row {
            case: "nothing but reserved keys",
            tags: vec![("aws:a", "1"), ("aws:b", "2")],
            refused_key: "aws:a",
        },
    ]
}

/// Tag keys that share a prefix of `aws:` without claiming the namespace, so they are
/// ordinary keys: the prefix needs its colon, and a key that merely CONTAINS it, or
/// starts with other bytes entirely, is the caller's to name. The last row's key starts
/// with a multi-byte character, so a byte-wise prefix test must answer "not reserved"
/// rather than slicing inside it.
fn ordinary_rows() -> Vec<Row> {
    [
        ("the bare word", ("aws", "yes")),
        ("a longer word starting with it", ("awsome", "yes")),
        ("the prefix further in", ("my-aws:owner", "me")),
        ("a shorter prefix", ("ws:owner", "me")),
        ("no separator", ("AWSowner", "me")),
        ("a multi-byte first character", ("日aws:owner", "me")),
    ]
    .into_iter()
    .map(|(case, tag)| Row {
        case,
        tags: vec![tag],
        // Not a refused key: these rows are the accepted side of the rule.
        refused_key: "",
    })
    .collect()
}

/// `ListTagsForResource` for `arn`, as the tag set it reports.
async fn listed(transcribe: &Transcribe, arn: &str, case: &str) -> Vec<(String, String)> {
    let reply = transcribe
        .call("ListTagsForResource", json!({"ResourceArn": arn}))
        .await;
    let body = reply.ok(case);
    let tags = body
        .get("Tags")
        .unwrap_or_else(|| panic!("case {case:?}: expected a Tags member, got {body:?}"));
    pairs(tags, case)
}

/// The ARN of `family`'s resource named `name`, as the tagging operations take it for a
/// request served in the emulator's default Region.
fn arn_of(family: &Family, name: &str) -> String {
    family.arn_in(DEFAULT_REGION, name)
}

/// RULE: `TagResource` refuses a request carrying a key in the reserved `aws:`
/// namespace, and the resource's stored tag set is left exactly as it was.
#[tokio::test]
async fn tag_resource_refuses_reserved_keys_and_writes_nothing() {
    let transcribe = Transcribe::new();

    for family in families() {
        let created = [("team", "speech"), ("env", "test")];
        for (index, row) in reserved_rows().into_iter().enumerate() {
            let case = format!("{} / TagResource / {}", family.case, row.case);
            let name = family.resource_name(&format!("reserved{index}"));
            transcribe
                .call(family.create, family.create_request(&name, &created))
                .await
                .ok(&case);
            let arn = arn_of(&family, &name);

            let reply = transcribe
                .call(
                    "TagResource",
                    json!({"ResourceArn": arn, "Tags": tag_list(&row.tags)}),
                )
                .await;
            reply.refused(&case, CLIENT_ERROR_STATUS, REFUSAL_CODE);
            let message = reply.error_message(&case);
            assert!(
                message.contains(row.refused_key),
                "case {case:?}: the refusal must name the key it refused, got {message:?}"
            );

            let found = listed(&transcribe, &arn, &case).await;
            assert_eq!(
                found,
                expected_pairs(&created),
                "case {case:?}: a refused TagResource must leave the stored set untouched, \
                 got {found:?}"
            );
        }
    }
}

/// RULE, the other side: a key that does not claim the namespace is an ordinary key, so
/// `TagResource` stores it and `ListTagsForResource` reports it back.
#[tokio::test]
async fn tag_resource_stores_keys_that_only_look_reserved() {
    let transcribe = Transcribe::new();

    for family in families() {
        for (index, row) in ordinary_rows().into_iter().enumerate() {
            let case = format!("{} / TagResource / {}", family.case, row.case);
            let name = family.resource_name(&format!("ordinary{index}"));
            transcribe
                .call(family.create, family.create_request(&name, &[]))
                .await
                .ok(&case);
            let arn = arn_of(&family, &name);

            transcribe
                .call(
                    "TagResource",
                    json!({"ResourceArn": arn, "Tags": tag_list(&row.tags)}),
                )
                .await
                .ok(&case);

            let found = listed(&transcribe, &arn, &case).await;
            assert_eq!(
                found,
                expected_pairs(&row.tags),
                "case {case:?}: an ordinary key must be stored and reported, got {found:?}"
            );
        }
    }
}

/// RULE: the same reservation holds on the `Tags` member of every create/start
/// operation — the request is refused and no resource is recorded, so the name is still
/// free afterwards for a create whose tags are the caller's to name.
#[tokio::test]
async fn creates_refuse_reserved_keys_and_record_nothing() {
    let transcribe = Transcribe::new();

    for family in families() {
        for (index, row) in reserved_rows().into_iter().enumerate() {
            let case = format!("{} / {} / {}", family.case, family.create, row.case);
            let name = family.resource_name(&format!("createreserved{index}"));

            let reply = transcribe
                .call(family.create, family.create_request(&name, &row.tags))
                .await;
            reply.refused(&case, CLIENT_ERROR_STATUS, REFUSAL_CODE);
            let message = reply.error_message(&case);
            assert!(
                message.contains(row.refused_key),
                "case {case:?}: the refusal must name the key it refused, got {message:?}"
            );

            // Nothing was recorded: the ARN the refused create would have minted names
            // no resource, so the tagging API refuses it too.
            let arn = arn_of(&family, &name);
            transcribe
                .call("ListTagsForResource", json!({"ResourceArn": arn}))
                .await
                .refused(&case, CLIENT_ERROR_STATUS, REFUSAL_CODE);

            // And the name is still free: the same create, with keys the caller may
            // name, succeeds and its tags round-trip through the tagging API.
            let allowed = [("owner", "me"), ("env", "test")];
            transcribe
                .call(family.create, family.create_request(&name, &allowed))
                .await
                .ok(&case);
            let found = listed(&transcribe, &arn, &case).await;
            assert_eq!(
                found,
                expected_pairs(&allowed),
                "case {case:?}: the create must store the tags it was given, got {found:?}"
            );
        }
    }
}

/// RULE, the other side at create time: a create whose `Tags` claim no reserved key
/// records the resource with exactly those tags.
#[tokio::test]
async fn creates_store_keys_that_only_look_reserved() {
    let transcribe = Transcribe::new();

    for family in families() {
        for (index, row) in ordinary_rows().into_iter().enumerate() {
            let case = format!("{} / {} / {}", family.case, family.create, row.case);
            let name = family.resource_name(&format!("createordinary{index}"));
            transcribe
                .call(family.create, family.create_request(&name, &row.tags))
                .await
                .ok(&case);

            let found = listed(&transcribe, &arn_of(&family, &name), &case).await;
            assert_eq!(
                found,
                expected_pairs(&row.tags),
                "case {case:?}: a create must store an ordinary key, got {found:?}"
            );
        }
    }
}

/// The modelled `TagList` is `@length(min: 1, max: 200)` and `TagKey`
/// `@length(min: 1, max: 128)` (`smithy_query(query="shape", shape="TagList,TagKey")`),
/// so the boundary — not this rule — is what refuses an empty list or an over-long key.
/// Pinned here so the reservation is not mistaken for the constraint that already holds.
#[tokio::test]
async fn the_modelled_tag_constraints_still_hold() {
    let transcribe = Transcribe::new();
    let family = &families()[0];
    let name = family.resource_name("constraints");
    let case = "modelled tag constraints";
    transcribe
        .call(family.create, family.create_request(&name, &[]))
        .await
        .ok(case);
    let arn = arn_of(family, &name);

    let over_long = "k".repeat(129);
    let rows: Vec<(&str, Value)> = vec![
        ("an empty Tags list", json!([])),
        (
            "an over-long key",
            tag_list(&[(over_long.as_str(), "value")]),
        ),
    ];
    for (row, tags) in rows {
        let case = format!("{case} / {row}");
        transcribe
            .call(
                "TagResource",
                json!({"ResourceArn": arn, "Tags": tags}),
            )
            .await
            .refused(&case, CLIENT_ERROR_STATUS, "ValidationException");
    }

    // The resource still carries nothing: a refused call wrote no tag.
    let found = listed(&transcribe, &arn, case).await;
    assert_eq!(
        found,
        Vec::<(String, String)>::new(),
        "case {case:?}: a refused call must store no tag, got {found:?}"
    );
}
