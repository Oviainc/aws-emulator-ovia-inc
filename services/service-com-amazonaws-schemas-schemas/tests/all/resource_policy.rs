// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for a registry's resource-based policy:
//! `PutResourcePolicy` and `GetResourcePolicy`.
//!
//! Every case drives the real wire path: a signed `restJson1` request through
//! `MultiServiceRouter::dispatch`, exactly as the production router serves it. Nothing
//! here touches the state struct directly, so a handler that answered from its input
//! instead of from the store would still have to make a later read agree.
//!
//! The HTTP harness is the crate's one harness, imported from
//! `registry_schema_lifecycle`; only the policy-specific helpers live here.

use std::collections::BTreeSet;

use serde_json::json;
use service_com_amazonaws_schemas_schemas::state::MAX_POLICY_BYTES;

use crate::registry_schema_lifecycle::{create_registry, Answer, Emulator};

/// `/v1/policy`, with the `registryName` query parameter the model binds
/// `RegistryName` to; `None` omits it.
///
/// Shared with the sibling policy suite in `malformed_content_and_policy_deletion.rs`,
/// so the binding is encoded once.
pub(crate) fn policy_uri(registry: Option<&str>) -> String {
    match registry {
        Some(registry) => format!("/v1/policy?registryName={registry}"),
        None => "/v1/policy".to_string(),
    }
}

/// `PutResourcePolicy` for `registry`, with an optional `RevisionId` precondition.
async fn put_policy(
    emulator: &Emulator,
    registry: Option<&str>,
    document: &str,
    revision: Option<&str>,
) -> Answer {
    let mut body = json!({ "Policy": document });
    if let Some(revision) = revision {
        body["RevisionId"] = json!(revision);
    }
    emulator
        .call("PUT", &policy_uri(registry), Some(body))
        .await
}

/// `GetResourcePolicy` for `registry`.
async fn get_policy(emulator: &Emulator, registry: Option<&str>) -> Answer {
    emulator.call("GET", &policy_uri(registry), None).await
}

/// The policy document a response reports.
fn policy_of(answer: &Answer) -> String {
    answer.str_at("Policy")
}

/// The revision id a response reports.
fn revision_of(answer: &Answer) -> String {
    answer.str_at("RevisionId")
}

/// `8-4-4-4-12` lowercase hex: the shape this emulator mints a revision id in.
fn is_opaque_id_shaped(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    let lengths: Vec<usize> = groups.iter().map(|group| group.len()).collect();
    lengths == vec![8, 4, 4, 4, 12]
        && groups.iter().all(|group| {
            group
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        })
}

/// One policy document the table below stores and reads back.
#[derive(Debug)]
struct PolicyCase {
    case: &'static str,
    document: String,
}

/// The table: rows differ along what the round-trip rule generalises over — the size,
/// nesting, member count, key order, whitespace and alphabet of the JSON document.
/// Every row is a JSON object, which is what a resource-based policy document is.
fn policy_cases() -> Vec<PolicyCase> {
    vec![
        PolicyCase {
            case: "the smallest JSON object there is",
            document: "{}".to_string(),
        },
        PolicyCase {
            case: "a single-statement policy",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "AllowDescribe",
                    "Effect": "Allow",
                    "Principal": { "AWS": "000000000000" },
                    "Action": ["schemas:DescribeRegistry"],
                    "Resource": "arn:aws:schemas:us-west-2:000000000000:registry/demo"
                }]
            })
            .to_string(),
        },
        PolicyCase {
            case: "two statements, a list principal and a condition",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Effect": "Allow",
                        "Principal": { "AWS": ["000000000000", "000000000001"] },
                        "Action": "schemas:*",
                        "Resource": "*",
                        "Condition": { "StringEquals": { "aws:PrincipalOrgID": "o-abc123" } }
                    },
                    {
                        "Effect": "Deny",
                        "Principal": "*",
                        "Action": ["schemas:DeleteRegistry", "schemas:DeleteSchema"],
                        "Resource": "*"
                    }
                ]
            })
            .to_string(),
        },
        PolicyCase {
            case: "pretty-printed, with newlines and tabs between tokens",
            document: "{\n\t\"Version\" : \"2012-10-17\",\n\t\"Statement\" : [ ]\n}".to_string(),
        },
        PolicyCase {
            case: "keys in an order no renderer would choose, with non-ASCII text",
            document: "{\"Statement\":[],\"Sid\":\"caf\u{e9} \u{2014} \u{30b9}\u{30ad}\u{30fc}\u{30de}\",\"Version\":\"2012-10-17\"}"
                .to_string(),
        },
        PolicyCase {
            case: "a document of several kilobytes",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "AWS": "000000000000" },
                    "Action": "schemas:DescribeRegistry",
                    "Resource": "*",
                    "Sid": "x".repeat(8_000)
                }]
            })
            .to_string(),
        },
    ]
}

/// RULE: a successful `PutResourcePolicy` persists the document it was given
/// verbatim under the named registry and reports it with a revision id; every later
/// `GetResourcePolicy` of that registry reports the same document and the same
/// revision id, for as long as nothing writes the policy again.
#[tokio::test]
async fn a_stored_policy_reads_back_verbatim_with_a_stable_revision() {
    let emulator = Emulator::new();
    let registry = "policy-registry";
    create_registry(&emulator, registry).await;

    let mut revisions = BTreeSet::new();
    for case in policy_cases() {
        let put = put_policy(&emulator, Some(registry), &case.document, None).await;
        assert_eq!(
            put.status, 200,
            "case {:?}: expected 200 from PutResourcePolicy, got {} {:?}",
            case.case, put.status, put.body
        );
        assert_eq!(
            policy_of(&put),
            case.document,
            "case {:?}: PutResourcePolicy must report the document it stored",
            case.case
        );
        let revision = revision_of(&put);
        assert!(
            is_opaque_id_shaped(&revision),
            "case {:?}: expected an 8-4-4-4-12 hex revision id, got {revision:?}",
            case.case
        );
        assert!(
            revisions.insert(revision.clone()),
            "case {:?}: every write mints a revision id no earlier write used, \
             but {revision:?} was already seen in {revisions:?}",
            case.case
        );

        // Two reads in a row: the policy is reported from the store, so a read does
        // not move the revision on.
        for read in 0..2 {
            let got = get_policy(&emulator, Some(registry)).await;
            assert_eq!(
                got.status, 200,
                "case {:?} (read {read}): expected 200 from GetResourcePolicy, got {} {:?}",
                case.case, got.status, got.body
            );
            assert_eq!(
                policy_of(&got),
                case.document,
                "case {:?} (read {read}): GetResourcePolicy must report the stored document",
                case.case
            );
            assert_eq!(
                revision_of(&got),
                revision,
                "case {:?} (read {read}): the revision id must not change between reads",
                case.case
            );
        }
    }
}

/// RULE: a `RevisionId` on a write is a precondition — the write happens only while
/// the stored policy is still at that revision, and is otherwise refused with HTTP 412
/// `PreconditionFailedException`, leaving the stored policy untouched.
#[tokio::test]
async fn a_write_conditioned_on_a_stale_revision_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let registry = "conditional-registry";
    create_registry(&emulator, registry).await;

    let first = r#"{"Version":"2012-10-17","Statement":[]}"#;
    let created = put_policy(&emulator, Some(registry), first, None).await;
    assert_eq!(
        created.status, 200,
        "expected 200 from the first PutResourcePolicy, got {} {:?}",
        created.status, created.body
    );
    let revision_one = revision_of(&created);

    // The current revision satisfies the precondition, and the write moves the policy
    // on to a NEW revision.
    let second = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Principal":"*","Action":"schemas:*","Resource":"*"}]}"#;
    let updated = put_policy(&emulator, Some(registry), second, Some(&revision_one)).await;
    assert_eq!(
        updated.status, 200,
        "case {:?}: a write at the current revision must succeed, got {} {:?}",
        "current revision", updated.status, updated.body
    );
    let revision_two = revision_of(&updated);
    assert_ne!(
        revision_two, revision_one,
        "case {:?}: a successful write must mint a new revision id, got {revision_two:?}",
        "current revision"
    );

    // Every revision that is not the current one — the superseded one, a well-formed
    // id this store never minted, and some values that are not ids at all.
    for (case, stale) in [
        ("the superseded revision", revision_one.clone()),
        (
            "an id shaped like ours that was never minted",
            "ffffffff-0000-4000-8000-ffffffffffff".to_string(),
        ),
        ("the empty string", String::new()),
        ("a free-text value", "not-a-revision".to_string()),
        (
            "the current revision with trailing space",
            format!("{revision_two} "),
        ),
        (
            "the current revision truncated by one character",
            revision_two[..revision_two.len() - 1].to_string(),
        ),
    ] {
        let refused = put_policy(&emulator, Some(registry), first, Some(&stale)).await;
        assert_eq!(
            refused.status, 412,
            "case {case:?}: expected 412 from a stale-revision write, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "PreconditionFailedException",
            "case {case:?}: expected PreconditionFailedException, got {:?}",
            refused.body
        );

        let got = get_policy(&emulator, Some(registry)).await;
        assert_eq!(
            policy_of(&got),
            second,
            "case {case:?}: a refused write must not change the stored policy"
        );
        assert_eq!(
            revision_of(&got),
            revision_two,
            "case {case:?}: a refused write must not move the revision on"
        );
    }

    // And the current revision still works after every refusal, so a refusal did not
    // consume it either.
    let again = put_policy(&emulator, Some(registry), first, Some(&revision_two)).await;
    assert_eq!(
        again.status, 200,
        "case {:?}: the current revision must still satisfy the precondition, got {} {:?}",
        "current revision after refusals", again.status, again.body
    );
}

/// RULE: a registry that holds no policy is at no revision, so no `RevisionId` a
/// caller can name matches it (HTTP 412), and reading its policy reports that the
/// policy does not exist (HTTP 404).
#[tokio::test]
async fn a_registry_without_a_policy_has_no_revision_to_match() {
    let emulator = Emulator::new();
    let registry = "policy-less-registry";
    create_registry(&emulator, registry).await;

    let missing = get_policy(&emulator, Some(registry)).await;
    assert_eq!(
        missing.status, 404,
        "expected 404 from GetResourcePolicy for a registry with no policy, got {} {:?}",
        missing.status, missing.body
    );
    assert_eq!(
        missing.error_code(),
        "NotFoundException",
        "expected NotFoundException, got {:?}",
        missing.body
    );

    for (case, revision) in [
        (
            "an id shaped like ours",
            "00000001-0000-4000-8000-000000000001",
        ),
        ("a free-text value", "whatever"),
    ] {
        let refused = put_policy(&emulator, Some(registry), "{}", Some(revision)).await;
        assert_eq!(
            refused.status, 412,
            "case {case:?}: a precondition on a policy that does not exist cannot hold, \
             got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "PreconditionFailedException",
            "case {case:?}: expected PreconditionFailedException, got {:?}",
            refused.body
        );
    }

    // An unconditional write still creates the policy.
    let created = put_policy(&emulator, Some(registry), "{}", None).await;
    assert_eq!(
        created.status, 200,
        "an unconditional write must create the policy, got {} {:?}",
        created.status, created.body
    );
}

/// RULE: the policy is a property of the registry the request names — each registry
/// holds its own, and deleting a registry takes its policy with it.
#[tokio::test]
async fn a_policy_belongs_to_the_registry_it_was_written_for() {
    let emulator = Emulator::new();
    let documents = [
        ("registry-a", r#"{"Sid":"a"}"#),
        ("registry-b", r#"{"Sid":"b"}"#),
        ("registry-c", r#"{"Sid":"c"}"#),
    ];
    for (registry, document) in documents {
        create_registry(&emulator, registry).await;
        let put = put_policy(&emulator, Some(registry), document, None).await;
        assert_eq!(
            put.status, 200,
            "case {registry:?}: expected 200 from PutResourcePolicy, got {} {:?}",
            put.status, put.body
        );
    }

    let mut revisions = BTreeSet::new();
    for (registry, document) in documents {
        let got = get_policy(&emulator, Some(registry)).await;
        assert_eq!(
            policy_of(&got),
            document,
            "case {registry:?}: each registry reports its own policy"
        );
        assert!(
            revisions.insert(revision_of(&got)),
            "case {registry:?}: two registries must not share a revision id, got {:?}",
            revision_of(&got)
        );
    }

    // Deleting the registry deletes the policy attached to it: the policy has no
    // existence apart from its registry.
    let deleted = emulator
        .call("DELETE", "/v1/registries/name/registry-b", None)
        .await;
    assert_eq!(
        deleted.status, 204,
        "expected 204 from DeleteRegistry, got {} {:?}",
        deleted.status, deleted.body
    );
    for (case, answer) in [
        ("read", get_policy(&emulator, Some("registry-b")).await),
        (
            "write",
            put_policy(&emulator, Some("registry-b"), "{}", None).await,
        ),
    ] {
        assert_eq!(
            answer.status, 404,
            "case {case:?}: a deleted registry holds no policy, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {:?}",
            answer.body
        );
    }

    // The registries beside it are untouched.
    for (registry, document) in [documents[0], documents[2]] {
        let got = get_policy(&emulator, Some(registry)).await;
        assert_eq!(
            policy_of(&got),
            document,
            "case {registry:?}: deleting another registry must not touch this policy"
        );
    }
}

/// RULE: a request that names a registry this store does not hold reports that the
/// registry does not exist (HTTP 404), and one that names no registry at all is
/// refused as a bad request (HTTP 400) — the policy is a property of one registry.
#[tokio::test]
async fn a_request_that_names_no_existing_registry_is_refused() {
    let emulator = Emulator::new();
    create_registry(&emulator, "real-registry").await;

    // Rows: a name nothing created, one differing only in case, and one differing
    // only by a trailing space (percent-encoded, so the URI itself stays well-formed).
    for case in ["absent-registry", "Real-Registry", "real-registry%20"] {
        for (operation, answer) in [
            (
                "GetResourcePolicy",
                get_policy(&emulator, Some(case)).await,
            ),
            (
                "PutResourcePolicy",
                put_policy(&emulator, Some(case), "{}", None).await,
            ),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {case:?} ({operation}): expected 404, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "NotFoundException",
                "case {case:?} ({operation}): expected NotFoundException, got {:?}",
                answer.body
            );
        }
    }

    for (operation, answer) in [
        ("GetResourcePolicy", get_policy(&emulator, None).await),
        (
            "PutResourcePolicy",
            put_policy(&emulator, None, "{}", None).await,
        ),
    ] {
        assert_eq!(
            answer.status, 400,
            "case {operation:?} without RegistryName: expected 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "BadRequestException",
            "case {operation:?} without RegistryName: expected BadRequestException, got {:?}",
            answer.body
        );
    }
}

/// RULE: `Policy` is modelled as a plain string, so how much of one the store will
/// retain for a registry is the handler's bound: a document up to the cap is stored,
/// one past it is refused with HTTP 400 `BadRequestException`.
#[tokio::test]
async fn a_policy_past_the_store_cap_is_refused() {
    let emulator = Emulator::new();
    let registry = "capped-registry";
    create_registry(&emulator, registry).await;

    /// A JSON object of exactly `size` bytes (`{"Sid":"…"}` is 10 bytes of frame).
    fn document_of(size: usize) -> String {
        format!(r#"{{"Sid":"{}"}}"#, "x".repeat(size - 10))
    }

    // Rows: well under the cap, both sides of the published bound, and far past it.
    // The bound is the handler's own constant, so these rows cannot drift from it.
    for (case, size, expected) in [
        ("well under the cap", 1_024, 200),
        ("one byte under the cap", MAX_POLICY_BYTES - 1, 200),
        ("exactly at the cap", MAX_POLICY_BYTES, 200),
        ("one byte past the cap", MAX_POLICY_BYTES + 1, 400),
        ("far past the cap", MAX_POLICY_BYTES * 4, 400),
    ] {
        let document = document_of(size);
        assert_eq!(
            document.len(),
            size,
            "case {case:?}: the row builds {size} bytes"
        );
        let answer = put_policy(&emulator, Some(registry), &document, None).await;
        assert_eq!(
            answer.status, expected,
            "case {case:?} ({size} bytes): expected {expected}, got {} {:?}",
            answer.status,
            answer.body.get("Message").or(answer.body.get("__type"))
        );
        if expected == 400 {
            assert_eq!(
                answer.error_code(),
                "BadRequestException",
                "case {case:?}: expected BadRequestException, got {:?}",
                answer.body.get("__type")
            );
        } else {
            assert_eq!(
                policy_of(&get_policy(&emulator, Some(registry)).await),
                document,
                "case {case:?}: an accepted document reads back verbatim"
            );
        }
    }
}

/// RULE: `Policy` is modelled `@mediaType("application/json")` and a resource-based
/// policy document is a JSON object, so a value that is not one is refused with HTTP
/// 400 `BadRequestException` and nothing is stored.
#[tokio::test]
async fn a_policy_that_is_not_a_json_object_is_refused() {
    let emulator = Emulator::new();
    let registry = "json-registry";
    create_registry(&emulator, registry).await;

    // Rows: every JSON type that is not an object, and text that is not JSON at all.
    for (case, document) in [
        ("an array", "[]"),
        ("a JSON string", "\"Allow\""),
        ("a number", "17"),
        ("a boolean", "true"),
        ("the null literal", "null"),
        ("the empty value", ""),
        ("only whitespace", "   "),
        ("a truncated object", "{\"Version\":"),
        ("an unquoted key", "{Version: 1}"),
        ("two objects in a row", "{}{}"),
        ("not JSON at all", "Allow everyone"),
    ] {
        let refused = put_policy(&emulator, Some(registry), document, None).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400 from PutResourcePolicy, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {:?}",
            refused.body
        );

        // Nothing was stored, so the registry still has no policy.
        let got = get_policy(&emulator, Some(registry)).await;
        assert_eq!(
            got.status, 404,
            "case {case:?}: a refused write must store no policy, got {} {:?}",
            got.status, got.body
        );
    }
}
