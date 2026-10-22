// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the rule an update owes: a member an update request
//! sends REPLACES the stored one, so every later read of the resource reports what
//! the update stored, and a member the request omits leaves the stored value alone.
//!
//! The rule is asserted over a TABLE of `Description` values — both modelled bounds
//! of `__stringMin0Max256` (len 0..256), alphabets, and a repeat of the value
//! already stored — applied to BOTH resources this service lets a caller update by
//! description: a registry (`UpdateRegistry` → `DescribeRegistry`) and a schema
//! (`UpdateSchema` → `DescribeSchema`).
//!
//! Every case drives the real wire path through the harness in
//! `registry_schema_lifecycle.rs`; nothing here touches the state struct directly,
//! so a handler that answered from its input instead of from the store would still
//! have to make the following read agree.

use serde_json::{json, Value};
use service_com_amazonaws_schemas_schemas::state::AWS_MANAGED_REGISTRIES;

use crate::registry_schema_lifecycle::{
    create_registry, registry_uri, schema_uri, Answer, Emulator, ACCOUNT, REGION,
};

/// One `Description` an update sends.
#[derive(Debug)]
struct DescriptionCase {
    case: &'static str,
    description: String,
}

/// The table. Rows differ along what the rule generalises over: the value itself,
/// its length at and near each published bound of `Description`
/// (`__stringMin0Max256`, len 0..256 — source: `smithy_query(query="operation",
/// operation="UpdateRegistry")` input member `Description`), and its alphabet.
fn description_cases() -> Vec<DescriptionCase> {
    vec![
        DescriptionCase {
            case: "plain-ascii",
            description: "probe registry v2".to_string(),
        },
        DescriptionCase {
            case: "empty-at-the-modelled-lower-bound",
            description: String::new(),
        },
        DescriptionCase {
            case: "non-ascii-punctuation-and-quotes",
            description: "caf\u{e9} \u{2014} \u{30b9}\u{30ad}\u{30fc}\u{30de} \"v3\" 100%"
                .to_string(),
        },
        DescriptionCase {
            case: "one-under-the-modelled-upper-bound",
            description: "u".repeat(255),
        },
        DescriptionCase {
            case: "at-the-modelled-upper-bound",
            description: "m".repeat(256),
        },
        DescriptionCase {
            case: "single-character",
            description: "x".to_string(),
        },
    ]
}

/// Create a registry carrying a description and tags, returning the answer.
async fn create_described_registry(emulator: &Emulator, registry: &str, body: Value) -> Answer {
    let created = emulator
        .call("POST", &registry_uri(registry), Some(body))
        .await;
    assert_eq!(
        created.status, 201,
        "registry {registry:?}: expected 201 from CreateRegistry, got {} {:?}",
        created.status, created.body
    );
    created
}

/// The `Description` a read reports, or `None` when the member is absent or null.
fn description_of(answer: &Answer) -> Option<&str> {
    answer.body.get("Description").and_then(Value::as_str)
}

/// RULE: `UpdateRegistry` writes the `Description` it is sent into the stored
/// registry, so the update's own answer and every later `DescribeRegistry` report
/// it — for any admitted value, and repeating an update changes nothing else.
#[tokio::test]
async fn update_registry_persists_every_description_a_later_read_reports() {
    let emulator = Emulator::new();
    let registry = "probe-registry";
    let created = create_described_registry(
        &emulator,
        registry,
        json!({"Description": "probe registry v1", "tags": {"team": "payments"}}),
    )
    .await;
    let arn = created.str_at("RegistryArn");
    assert_eq!(
        arn,
        format!("arn:aws:schemas:{REGION}:{ACCOUNT}:registry/{registry}"),
        "the create must mint the registry's ARN, got {arn:?}"
    );

    for case in description_cases() {
        // Sent TWICE: an update is not a toggle, so the same request repeated leaves
        // the same stored value.
        for attempt in 1..=2 {
            let updated = emulator
                .call(
                    "PUT",
                    &registry_uri(registry),
                    Some(json!({"Description": case.description})),
                )
                .await;
            assert_eq!(
                updated.status, 200,
                "case {:?} attempt {attempt}: expected 200 from UpdateRegistry, got {} {:?}",
                case.case, updated.status, updated.body
            );
            assert_eq!(
                description_of(&updated),
                Some(case.description.as_str()),
                "case {:?} attempt {attempt}: UpdateRegistry must answer the stored \
                 Description, got {:?}",
                case.case,
                updated.body.get("Description")
            );
            assert_eq!(
                updated.str_at("RegistryName"),
                registry,
                "case {:?} attempt {attempt}: the answer names the updated registry",
                case.case
            );
            assert_eq!(
                updated.str_at("RegistryArn"),
                arn,
                "case {:?} attempt {attempt}: an update must not re-mint the ARN",
                case.case
            );
            assert_eq!(
                updated.body["tags"],
                json!({"team": "payments"}),
                "case {:?} attempt {attempt}: an update must not touch the stored tags, \
                 got {:?}",
                case.case,
                updated.body["tags"]
            );

            // The read-after-write: this is the assertion the defect broke.
            let described = emulator.call("GET", &registry_uri(registry), None).await;
            assert_eq!(
                described.status, 200,
                "case {:?} attempt {attempt}: expected 200 from DescribeRegistry, got {} {:?}",
                case.case, described.status, described.body
            );
            assert_eq!(
                description_of(&described),
                Some(case.description.as_str()),
                "case {:?} attempt {attempt}: DescribeRegistry must report the updated \
                 Description, got {:?}",
                case.case,
                described.body.get("Description")
            );
            assert_eq!(
                described.str_at("RegistryArn"),
                arn,
                "case {:?} attempt {attempt}: the read reports the ARN create minted",
                case.case
            );
        }
    }
}

/// RULE: the same rule on the other resource an update's `Description` reaches —
/// `UpdateSchema` stores it and `DescribeSchema` reports it, whatever the value and
/// whichever version the read selects: `Description` is the SCHEMA's metadata, not a
/// property of one definition.
#[tokio::test]
async fn update_schema_persists_every_description_a_later_read_reports() {
    let emulator = Emulator::new();
    let registry = "probe-registry";
    let schema = "probe-schema";
    create_registry(&emulator, registry).await;
    // Two stored versions, each with its own content: the rule must hold for a read
    // pinned to EITHER, not only for the latest.
    let contents = [r#"{"openapi":"3.0.0"}"#, r#"{"openapi":"3.0.1"}"#];
    let created = emulator
        .call(
            "POST",
            &schema_uri(registry, schema),
            Some(json!({"Content": contents[0], "Type": "OpenApi3", "Description": "v1"})),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "expected 201 from CreateSchema, got {} {:?}",
        created.status, created.body
    );
    let second = emulator
        .call(
            "PUT",
            &schema_uri(registry, schema),
            Some(json!({"Content": contents[1]})),
        )
        .await;
    assert_eq!(
        second.status, 200,
        "expected 200 from the UpdateSchema that mints version 2, got {} {:?}",
        second.status, second.body
    );
    assert_eq!(
        second.str_at("SchemaVersion"),
        "2",
        "an update carrying Content mints the next version, got {:?}",
        second.body["SchemaVersion"]
    );
    assert_eq!(
        description_of(&second),
        Some("v1"),
        "an update that sends no Description keeps the stored one, got {:?}",
        second.body.get("Description")
    );

    for case in description_cases() {
        let updated = emulator
            .call(
                "PUT",
                &schema_uri(registry, schema),
                Some(json!({"Description": case.description})),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: expected 200 from UpdateSchema, got {} {:?}",
            case.case, updated.status, updated.body
        );
        assert_eq!(
            description_of(&updated),
            Some(case.description.as_str()),
            "case {:?}: UpdateSchema must answer the stored Description, got {:?}",
            case.case,
            updated.body.get("Description")
        );

        // Every read of the schema reports the new description: the unpinned read and
        // each version pinned by number — while each version still answers the
        // content it was minted with.
        for (index, content) in contents.iter().enumerate() {
            let version = index + 1;
            let described = emulator
                .call(
                    "GET",
                    &format!("{}?schemaVersion={version}", schema_uri(registry, schema)),
                    None,
                )
                .await;
            assert_eq!(
                described.status, 200,
                "case {:?} version {version}: expected 200 from DescribeSchema, got {} {:?}",
                case.case, described.status, described.body
            );
            assert_eq!(
                description_of(&described),
                Some(case.description.as_str()),
                "case {:?} version {version}: DescribeSchema must report the schema's \
                 updated Description, got {:?}",
                case.case,
                described.body.get("Description")
            );
            assert_eq!(
                described.str_at("Content"),
                *content,
                "case {:?} version {version}: changing the description must not touch a \
                 stored definition",
                case.case
            );
        }

        let described = emulator
            .call("GET", &schema_uri(registry, schema), None)
            .await;
        assert_eq!(
            description_of(&described),
            Some(case.description.as_str()),
            "case {:?}: an unpinned DescribeSchema must report the updated Description, \
             got {:?}",
            case.case,
            described.body.get("Description")
        );
        assert_eq!(
            described.str_at("SchemaVersion"),
            contents.len().to_string(),
            "case {:?}: a description change mints no version",
            case.case
        );
    }
}

/// RULE: a member the request does not send asks for NO change, so an update that
/// carries no `Description` leaves the stored one exactly as the last write left it.
#[tokio::test]
async fn an_update_that_sends_no_description_leaves_the_stored_one() {
    let emulator = Emulator::new();
    let registry = "unchanged";
    let schema = "unchanged-schema";
    create_described_registry(&emulator, registry, json!({"Description": "as created"})).await;
    let created = emulator
        .call(
            "POST",
            &schema_uri(registry, schema),
            Some(json!({"Content": r#"{"openapi":"3.0.0"}"#, "Type": "OpenApi3",
                        "Description": "schema as created"})),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "expected 201 from CreateSchema, got {} {:?}",
        created.status, created.body
    );

    // Rows: the stored value the omitting update has to leave alone — the one the
    // create wrote, then each value a preceding update wrote over it.
    for (case, stored) in [
        ("as-created", None),
        ("after-one-update", Some("rewritten")),
        ("after-an-update-to-the-empty-string", Some("")),
    ] {
        if let Some(stored) = stored {
            let rewritten = emulator
                .call(
                    "PUT",
                    &registry_uri(registry),
                    Some(json!({"Description": stored})),
                )
                .await;
            assert_eq!(
                rewritten.status, 200,
                "case {case:?}: expected 200 from the preceding UpdateRegistry, got {} {:?}",
                rewritten.status, rewritten.body
            );
            let rewritten = emulator
                .call(
                    "PUT",
                    &schema_uri(registry, schema),
                    Some(json!({"Description": stored})),
                )
                .await;
            assert_eq!(
                rewritten.status, 200,
                "case {case:?}: expected 200 from the preceding UpdateSchema, got {} {:?}",
                rewritten.status, rewritten.body
            );
        }
        let expected_registry_description = stored.unwrap_or("as created");
        let expected_schema_description = stored.unwrap_or("schema as created");

        // The update under test: a body naming no `Description` at all.
        let updated = emulator
            .call("PUT", &registry_uri(registry), Some(json!({})))
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected 200 from UpdateRegistry, got {} {:?}",
            updated.status, updated.body
        );
        assert_eq!(
            description_of(&updated),
            Some(expected_registry_description),
            "case {case:?}: an update naming no Description keeps the stored one, got {:?}",
            updated.body.get("Description")
        );
        let described = emulator.call("GET", &registry_uri(registry), None).await;
        assert_eq!(
            description_of(&described),
            Some(expected_registry_description),
            "case {case:?}: DescribeRegistry still reports the stored Description, got {:?}",
            described.body.get("Description")
        );

        let schema_updated = emulator
            .call("PUT", &schema_uri(registry, schema), Some(json!({})))
            .await;
        assert_eq!(
            schema_updated.status, 200,
            "case {case:?}: expected 200 from UpdateSchema, got {} {:?}",
            schema_updated.status, schema_updated.body
        );
        let schema_described = emulator
            .call("GET", &schema_uri(registry, schema), None)
            .await;
        assert_eq!(
            description_of(&schema_described),
            Some(expected_schema_description),
            "case {case:?}: DescribeSchema still reports the stored Description, got {:?}",
            schema_described.body.get("Description")
        );
    }
}

/// RULE: a `Description` outside the modelled length range is refused, and a refused
/// update stores nothing — the resource still reports the value it held.
#[tokio::test]
async fn a_description_past_the_modelled_bound_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    let registry = "bounded";
    create_described_registry(&emulator, registry, json!({"Description": "within bounds"})).await;

    // Rows: each published bound of `Description` (`__stringMin0Max256`, len 0..256)
    // and lengths past the top. A length the model admits is stored; one past it is
    // refused and the value already stored survives.
    for (case, length, expected_status) in [
        ("at-the-modelled-lower-bound", 0usize, 200u16),
        ("at-the-modelled-upper-bound", 256, 200),
        ("one-past-the-upper-bound", 257, 400),
        ("far-past-the-upper-bound", 4_096, 400),
    ] {
        let before = emulator.call("GET", &registry_uri(registry), None).await;
        let stored_before = description_of(&before).map(str::to_string);
        let sent = "p".repeat(length);
        let answer = emulator
            .call(
                "PUT",
                &registry_uri(registry),
                Some(json!({"Description": sent})),
            )
            .await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: a Description of {length} characters must answer \
             {expected_status}, got {} {:?}",
            answer.status, answer.body
        );

        let expected_after = if expected_status == 200 {
            Some(sent)
        } else {
            stored_before
        };
        let described = emulator.call("GET", &registry_uri(registry), None).await;
        assert_eq!(
            description_of(&described).map(str::to_string),
            expected_after,
            "case {case:?}: the stored Description after the call is wrong, got {:?}",
            described.body.get("Description")
        );
    }
}

/// RULE: an update names the resource it writes, so an update of a registry that does
/// not exist is a 404 `NotFoundException` and creates nothing.
#[tokio::test]
async fn updating_a_registry_that_does_not_exist_is_refused() {
    let emulator = Emulator::new();

    // Rows: names no create ever stored — a plain one, one shaped like a deleted
    // registry, and one that differs from an existing registry only in case.
    create_described_registry(&emulator, "exists", json!({"Description": "here"})).await;
    for case in ["absent-registry", "exists-not", "EXISTS"] {
        let refused = emulator
            .call(
                "PUT",
                &registry_uri(case),
                Some(json!({"Description": "never stored"})),
            )
            .await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected 404 from UpdateRegistry, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {:?}",
            refused.body
        );
        let described = emulator.call("GET", &registry_uri(case), None).await;
        assert_eq!(
            described.status, 404,
            "case {case:?}: a refused update must not create the registry, got {} {:?}",
            described.status, described.body
        );
    }

    // The registry that DOES exist was not touched by any of the refusals above.
    let described = emulator.call("GET", &registry_uri("exists"), None).await;
    assert_eq!(
        description_of(&described),
        Some("here"),
        "the existing registry keeps its stored Description, got {:?}",
        described.body.get("Description")
    );
}

/// RULE: a service-owned registry's contents are AWS's, so a caller's update of one
/// is refused 403 and leaves it as the service seeded it.
#[tokio::test]
async fn updating_a_service_owned_registry_is_refused() {
    let emulator = Emulator::new();
    // Source: `crate::state::AWS_MANAGED_REGISTRIES` — the service-owned registries
    // every account sees.
    for registry in AWS_MANAGED_REGISTRIES {
        let refused = emulator
            .call(
                "PUT",
                &registry_uri(registry),
                Some(json!({"Description": "a caller's description"})),
            )
            .await;
        assert_eq!(
            refused.status, 403,
            "case {registry:?}: expected 403 from UpdateRegistry, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ForbiddenException",
            "case {registry:?}: expected ForbiddenException, got {:?}",
            refused.body
        );
        let described = emulator.call("GET", &registry_uri(registry), None).await;
        assert_eq!(
            described.status, 200,
            "case {registry:?}: the service-owned registry is still there, got {} {:?}",
            described.status, described.body
        );
        assert_eq!(
            description_of(&described),
            None,
            "case {registry:?}: a refused update must leave the seeded registry's \
             Description alone, got {:?}",
            described.body.get("Description")
        );
    }
}
