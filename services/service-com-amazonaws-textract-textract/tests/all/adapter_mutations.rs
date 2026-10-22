// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the Textract adapter MUTATIONS over the wire (`awsJson1_1`,
//! `POST /` with `X-Amz-Target: Textract.<Operation>`), and of what a later READ of the
//! mutated adapter reports.
//!
//! Each test asserts a RULE over a TABLE of inputs rather than one recorded request:
//!
//! * `UpdateAdapter` writes every member the request NAMES to the stored adapter and
//!   leaves every member it omits as it was, so a later `GetAdapter` reports the written
//!   configuration — and the mutation's own response already agrees with it;
//! * the members a mutation cannot change (`FeatureTypes`, `Tags`, `CreationTime`,
//!   `AdapterId`) are the same before and after it;
//! * updates compose: a sequence of mutations leaves the adapter holding the LAST value
//!   written for each member;
//! * an update naming no updatable member, and an update of an id no create minted, are
//!   refused with the modelled errors and change nothing;
//! * `DeleteAdapter` removes the adapter, so every later `GetAdapter`, `UpdateAdapter` and
//!   `DeleteAdapter` of that `AdapterId` is refused with the modelled
//!   `ResourceNotFoundException`, while the account's OTHER adapters still read back;
//! * `ClientRequestToken` idempotency is about the CREATE request, so a replay after an
//!   update still answers with the adapter it created.
//!
//! The wire harness (`harness`, `call`, `error_code`) is the one `document_extraction`
//! exposes to this test target; only the adapter helpers live here.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};
use service_com_amazonaws_textract_textract::ops::types::AUTO_UPDATE_VALUES;
use smithy_server_runtime::MultiServiceRouter;

use crate::document_extraction::{call, error_code, harness};

fn member(body: &Value, name: &str) -> Value {
    body.get(name).cloned().unwrap_or(Value::Null)
}

/// `CreateAdapter`, asserting only that it succeeded, and the `AdapterId` it minted.
async fn create_adapter(router: &MultiServiceRouter, case: &str, request: &Value) -> String {
    let (status, body) = call(router, "CreateAdapter", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateAdapter, got {status} with {body}"
    );
    body.get("AdapterId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `GetAdapter`, asserting it succeeded, and the body it reported.
async fn get_adapter(router: &MultiServiceRouter, case: &str, adapter_id: &str) -> Value {
    let (status, body) = call(router, "GetAdapter", &json!({ "AdapterId": adapter_id })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetAdapter of {adapter_id:?} must succeed, got {status} with {body}"
    );
    body
}

/// `base` with every member of `changes` applied; a `null` in `changes` DROPS the member.
fn merged(base: &Value, changes: &Value) -> Value {
    let mut out: Map<String, Value> = base.as_object().cloned().unwrap_or_default();
    for (key, value) in changes.as_object().cloned().unwrap_or_default() {
        if value.is_null() {
            out.remove(&key);
        } else {
            out.insert(key, value);
        }
    }
    Value::Object(out)
}

/// A name of `length` characters, inside `AdapterName`'s `^[a-zA-Z0-9-_]+$` pattern
/// (smithy_query member UpdateAdapterRequest$AdapterName, len 1..128).
fn name_of_length(length: usize) -> String {
    "n".repeat(length)
}

/// A description of `length` characters, inside `AdapterDescription`'s pattern
/// (smithy_query member UpdateAdapterRequest$Description, len 1..256).
fn description_of_length(length: usize) -> String {
    "d".repeat(length)
}

/// One row of the update table: the members the mutation names, and what the adapter must
/// hold afterwards.
struct UpdateCase {
    name: &'static str,
    /// The `UpdateAdapter` members, merged over `{"AdapterId": <the adapter>}`.
    update: Value,
    expect_name: String,
    expect_description: Option<String>,
    expect_auto_update: &'static str,
}

/// The configuration every row of the update table starts from.
fn initial_create() -> Value {
    json!({
        "AdapterName": "initial-adapter-name",
        "FeatureTypes": ["QUERIES"],
        "Description": "the configuration the create asked for",
        "AutoUpdate": "DISABLED",
        "Tags": { "team": "claims", "env": "test" },
    })
}

#[tokio::test]
async fn update_adapter_writes_what_a_later_read_reports() {
    let router = harness();

    // The rows differ along what the rule generalises over: WHICH subset of the three
    // updatable members the request names, and the values' length and character set.
    // `FeatureTypes` is absent from every row because, per AWS on UpdateAdapter,
    // "FeatureTypes configurations cannot be updated".
    let cases = vec![
        UpdateCase {
            name: "the description alone",
            update: json!({ "Description": "a freshly written description" }),
            expect_name: "initial-adapter-name".to_string(),
            expect_description: Some("a freshly written description".to_string()),
            expect_auto_update: "DISABLED",
        },
        UpdateCase {
            name: "AutoUpdate alone, turned on",
            update: json!({ "AutoUpdate": "ENABLED" }),
            expect_name: "initial-adapter-name".to_string(),
            expect_description: Some("the configuration the create asked for".to_string()),
            expect_auto_update: "ENABLED",
        },
        UpdateCase {
            name: "AutoUpdate alone, set to the value it already had",
            update: json!({ "AutoUpdate": "DISABLED" }),
            expect_name: "initial-adapter-name".to_string(),
            expect_description: Some("the configuration the create asked for".to_string()),
            expect_auto_update: "DISABLED",
        },
        UpdateCase {
            name: "the name alone",
            update: json!({ "AdapterName": "Renamed_Adapter-7" }),
            expect_name: "Renamed_Adapter-7".to_string(),
            expect_description: Some("the configuration the create asked for".to_string()),
            expect_auto_update: "DISABLED",
        },
        UpdateCase {
            name: "a name at the modelled minimum length",
            update: json!({ "AdapterName": name_of_length(1) }),
            expect_name: name_of_length(1),
            expect_description: Some("the configuration the create asked for".to_string()),
            expect_auto_update: "DISABLED",
        },
        UpdateCase {
            name: "a name at the modelled maximum length",
            update: json!({ "AdapterName": name_of_length(128) }),
            expect_name: name_of_length(128),
            expect_description: Some("the configuration the create asked for".to_string()),
            expect_auto_update: "DISABLED",
        },
        UpdateCase {
            name: "a description at the modelled minimum length",
            update: json!({ "Description": description_of_length(1) }),
            expect_name: "initial-adapter-name".to_string(),
            expect_description: Some(description_of_length(1)),
            expect_auto_update: "DISABLED",
        },
        UpdateCase {
            name: "a description at the modelled maximum length",
            update: json!({ "Description": description_of_length(256) }),
            expect_name: "initial-adapter-name".to_string(),
            expect_description: Some(description_of_length(256)),
            expect_auto_update: "DISABLED",
        },
        UpdateCase {
            name: "a description using the punctuation the pattern allows",
            update: json!({ "Description": "Re-trained on claim forms (2025): rev. 4! >=90%" }),
            expect_name: "initial-adapter-name".to_string(),
            expect_description: Some("Re-trained on claim forms (2025): rev. 4! >=90%".to_string()),
            expect_auto_update: "DISABLED",
        },
        UpdateCase {
            name: "the description and AutoUpdate together",
            update: json!({ "Description": "probed again", "AutoUpdate": "ENABLED" }),
            expect_name: "initial-adapter-name".to_string(),
            expect_description: Some("probed again".to_string()),
            expect_auto_update: "ENABLED",
        },
        UpdateCase {
            name: "all three updatable members at once",
            update: json!({
                "AdapterName": "fully-updated",
                "Description": "every member rewritten",
                "AutoUpdate": "ENABLED",
            }),
            expect_name: "fully-updated".to_string(),
            expect_description: Some("every member rewritten".to_string()),
            expect_auto_update: "ENABLED",
        },
    ];

    for case in &cases {
        // A fresh adapter per row, so the rows do not interfere.
        let adapter_id = create_adapter(&router, case.name, &initial_create()).await;
        let before = get_adapter(&router, case.name, &adapter_id).await;

        let request = merged(&case.update, &json!({ "AdapterId": adapter_id.clone() }));
        let (status, updated) = call(&router, "UpdateAdapter", &request).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from UpdateAdapter, got {status} with {updated}",
            case.name
        );

        // The mutation's response and the later read must agree, member by member.
        let read_back = get_adapter(&router, case.name, &adapter_id).await;
        for (reported, source) in [(&updated, "UpdateAdapter"), (&read_back, "GetAdapter")] {
            assert_eq!(
                member(reported, "AdapterId"),
                json!(adapter_id.clone()),
                "case {:?}: {source} must report the adapter's own id, got {reported}",
                case.name
            );
            assert_eq!(
                member(reported, "AdapterName"),
                json!(case.expect_name.clone()),
                "case {:?}: {source} expected AdapterName {:?}, got {reported}",
                case.name,
                case.expect_name
            );
            assert_eq!(
                member(reported, "Description"),
                case.expect_description
                    .clone()
                    .map_or(Value::Null, Value::String),
                "case {:?}: {source} expected Description {:?}, got {reported}",
                case.name,
                case.expect_description
            );
            assert_eq!(
                member(reported, "AutoUpdate"),
                json!(case.expect_auto_update),
                "case {:?}: {source} expected AutoUpdate {:?}, got {reported}",
                case.name,
                case.expect_auto_update
            );
            // What a mutation cannot change: the feature types it was trained on and the
            // instant it was created.
            assert_eq!(
                member(reported, "FeatureTypes"),
                member(&before, "FeatureTypes"),
                "case {:?}: {source} must keep the adapter's FeatureTypes {}, got {reported}",
                case.name,
                member(&before, "FeatureTypes")
            );
            assert_eq!(
                member(reported, "CreationTime"),
                member(&before, "CreationTime"),
                "case {:?}: {source} must keep CreationTime {}, got {reported}",
                case.name,
                member(&before, "CreationTime")
            );
        }
        // `Tags` is not an UpdateAdapter member and is not in its output, so only the read
        // carries it.
        assert_eq!(
            member(&read_back, "Tags"),
            member(&before, "Tags"),
            "case {:?}: an update must not change the adapter's Tags {}, got {read_back}",
            case.name,
            member(&before, "Tags")
        );
    }

    // Every AutoUpdate value the model declares was written by the table above, so a new
    // one in the model shows up here rather than going silently untested.
    let exercised: BTreeSet<&str> = cases
        .iter()
        .filter(|case| case.update.get("AutoUpdate").is_some())
        .map(|case| case.expect_auto_update)
        .collect();
    let modelled: BTreeSet<&str> = AUTO_UPDATE_VALUES.iter().copied().collect();
    assert_eq!(
        exercised, modelled,
        "the table must write every modelled AutoUpdate value, missing {:?}",
        modelled.difference(&exercised).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn updates_compose_so_the_adapter_holds_the_last_value_written() {
    let router = harness();
    let adapter_id = create_adapter(&router, "the adapter under test", &initial_create()).await;

    // A SEQUENCE of mutations on one adapter: after each one, every member holds the last
    // value any mutation wrote for it, and the members no mutation has named yet still
    // hold what the create asked for.
    let steps = [
        json!({ "Description": "revision 1" }),
        json!({ "AutoUpdate": "ENABLED" }),
        json!({ "AdapterName": "renamed-once" }),
        json!({ "Description": "revision 2", "AutoUpdate": "DISABLED" }),
        json!({ "AdapterName": "renamed-twice", "Description": "revision 3" }),
        json!({ "AutoUpdate": "ENABLED" }),
    ];

    let mut expect_name = "initial-adapter-name".to_string();
    let mut expect_description = "the configuration the create asked for".to_string();
    let mut expect_auto_update = "DISABLED".to_string();

    for (index, step) in steps.iter().enumerate() {
        let case = format!("step #{index}: {step}");
        for (key, target) in [
            ("AdapterName", &mut expect_name),
            ("Description", &mut expect_description),
            ("AutoUpdate", &mut expect_auto_update),
        ] {
            if let Some(value) = step.get(key).and_then(Value::as_str) {
                *target = value.to_string();
            }
        }

        let request = merged(step, &json!({ "AdapterId": adapter_id.clone() }));
        let (status, body) = call(&router, "UpdateAdapter", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from UpdateAdapter, got {status} with {body}"
        );

        let read_back = get_adapter(&router, &case, &adapter_id).await;
        assert_eq!(
            member(&read_back, "AdapterName"),
            json!(expect_name.clone()),
            "case {case:?}: expected AdapterName {expect_name:?}, got {read_back}"
        );
        assert_eq!(
            member(&read_back, "Description"),
            json!(expect_description.clone()),
            "case {case:?}: expected Description {expect_description:?}, got {read_back}"
        );
        assert_eq!(
            member(&read_back, "AutoUpdate"),
            json!(expect_auto_update.clone()),
            "case {case:?}: expected AutoUpdate {expect_auto_update:?}, got {read_back}"
        );
    }
}

#[tokio::test]
async fn an_update_naming_no_updatable_member_is_refused_and_changes_nothing() {
    let router = harness();
    let adapter_id = create_adapter(&router, "the adapter under test", &initial_create()).await;
    let before = get_adapter(&router, "before the refused updates", &adapter_id).await;

    // AWS, on UpdateAdapter: "At least one new parameter must be specified as an
    // argument." Rows: the id alone, and the id beside members the operation does not
    // model as a configuration change.
    let cases = [
        ("the AdapterId alone", json!({})),
        (
            "an input member the operation does not model",
            json!({ "FeatureTypes": ["QUERIES"] }),
        ),
        (
            "two members the operation does not model",
            json!({ "FeatureTypes": ["QUERIES"], "Tags": { "team": "claims" } }),
        ),
    ];

    for (name, extra) in &cases {
        let request = merged(extra, &json!({ "AdapterId": adapter_id.clone() }));
        let (status, body) = call(&router, "UpdateAdapter", &request).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterException",
            "case {name:?}: expected InvalidParameterException, got {body}"
        );
        let read_back = get_adapter(&router, name, &adapter_id).await;
        assert_eq!(
            read_back, before,
            "case {name:?}: a refused update must leave the adapter as it was, got \
             {read_back} after {before}"
        );
    }
}

#[tokio::test]
async fn an_update_with_an_auto_update_outside_the_modelled_enum_is_refused() {
    let router = harness();
    let adapter_id = create_adapter(&router, "the adapter under test", &initial_create()).await;
    let before = get_adapter(&router, "before the refused updates", &adapter_id).await;

    // `AUTO_UPDATE_VALUES` is the AutoUpdate enum from the Smithy model: a value outside
    // it is refused with HTTP 400 and writes nothing. Which 400 code answers depends on
    // whether the boundary validator or the handler refuses it, so the row asserts the
    // status and that the adapter did not move, not one of the two codes.
    for value in ["MAYBE", "enabled", "", "ENABLED "] {
        assert!(
            !AUTO_UPDATE_VALUES.contains(&value),
            "case {value:?}: the table must name values the model does not declare"
        );
        let (status, body) = call(
            &router,
            "UpdateAdapter",
            &json!({ "AdapterId": adapter_id.clone(), "AutoUpdate": value }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {value:?}: expected HTTP 400, got {status} with {body}"
        );
        assert!(
            !error_code(&body).is_empty(),
            "case {value:?}: a refusal must carry a wire error code, got {body}"
        );
        let read_back = get_adapter(&router, value, &adapter_id).await;
        assert_eq!(
            read_back, before,
            "case {value:?}: a refused update must leave the adapter as it was, got \
             {read_back} after {before}"
        );
    }
}

/// Ids that name no adapter, spanning `AdapterId`'s modelled 12..1011 length
/// (smithy_query member GetAdapterRequest$AdapterId).
fn ids_naming_no_adapter(known: &str) -> Vec<(String, String)> {
    let mut flipped = known.to_string();
    flipped.pop();
    flipped.push(if known.ends_with('0') { '1' } else { '0' });
    vec![
        (
            "an id at the modelled minimum length".to_string(),
            "a".repeat(12),
        ),
        (
            "an id at the modelled maximum length".to_string(),
            "b".repeat(1011),
        ),
        ("a plausible 32-character id".to_string(), "c".repeat(32)),
        (
            "a real id with its last character changed".to_string(),
            flipped,
        ),
    ]
}

#[tokio::test]
async fn a_mutation_of_an_id_no_create_minted_is_refused() {
    let router = harness();
    let known = create_adapter(&router, "the one real adapter", &initial_create()).await;

    // Both mutations model ResourceNotFoundException at HTTP 400, and neither may create
    // the resource it was asked to change.
    let mut messages: BTreeSet<String> = BTreeSet::new();
    for (name, adapter_id) in ids_naming_no_adapter(&known) {
        assert_ne!(
            adapter_id, known,
            "case {name:?}: the table must not name the adapter that exists"
        );
        for (operation, request) in [
            (
                "UpdateAdapter",
                json!({ "AdapterId": adapter_id.clone(), "Description": "no such adapter" }),
            ),
            (
                "DeleteAdapter",
                json!({ "AdapterId": adapter_id.clone() }),
            ),
            ("GetAdapter", json!({ "AdapterId": adapter_id.clone() })),
        ] {
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {name:?}: expected HTTP 400 from {operation}, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {name:?}: expected ResourceNotFoundException from {operation}, got {body}"
            );
            let message = body
                .get("Message")
                .or_else(|| body.get("message"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            assert!(
                !message.is_empty(),
                "case {name:?}: the refusal from {operation} must carry a message, got {body}"
            );
            assert!(
                !message.contains(&adapter_id),
                "case {name:?}: the refusal from {operation} must not echo the id back into \
                 its message, got {message:?}"
            );
            messages.insert(message);
        }
    }

    // One failure, one sentence: the message does not vary with the operation that looked
    // the adapter up or with the id it was asked for.
    assert_eq!(
        messages.len(),
        1,
        "every adapter-not-found refusal must answer the same message, got {messages:?}"
    );
}

/// One row of the deletion table: the adapter it creates, and whether it is mutated before
/// being deleted.
struct DeleteCase {
    name: &'static str,
    create: Value,
    /// The `UpdateAdapter` members applied before the delete, if any.
    update: Option<Value>,
}

#[tokio::test]
async fn a_deleted_adapter_is_gone_from_every_later_call() {
    let router = harness();

    // Rows differ along what the adapter holds when it is deleted: the create's minimum
    // members, a described and tagged adapter, one created under a ClientRequestToken, and
    // ones deleted straight after a mutation. The rule is the same for all of them.
    let cases = [
        DeleteCase {
            name: "an adapter with only the required members",
            create: json!({ "AdapterName": "minimal-doomed", "FeatureTypes": ["QUERIES"] }),
            update: None,
        },
        DeleteCase {
            name: "a described, tagged adapter",
            create: initial_create(),
            update: None,
        },
        DeleteCase {
            name: "an adapter created under a ClientRequestToken",
            create: json!({
                "AdapterName": "tokened-doomed",
                "FeatureTypes": ["QUERIES"],
                "ClientRequestToken": "doomed-token",
            }),
            update: None,
        },
        DeleteCase {
            name: "an adapter deleted right after a description change",
            create: initial_create(),
            update: Some(json!({ "Description": "about to be deleted" })),
        },
        DeleteCase {
            name: "an adapter deleted right after every member changed",
            create: initial_create(),
            update: Some(json!({
                "AdapterName": "doomed-and-renamed",
                "Description": "about to be deleted",
                "AutoUpdate": "ENABLED",
            })),
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        // A bystander adapter per row: deleting one adapter must not touch another.
        let bystander = create_adapter(
            &router,
            case.name,
            &json!({
                "AdapterName": format!("bystander-{index}"),
                "FeatureTypes": ["QUERIES"],
                "Description": format!("bystander number {index}"),
            }),
        )
        .await;
        let bystander_before = get_adapter(&router, case.name, &bystander).await;

        let adapter_id = create_adapter(&router, case.name, &case.create).await;
        if let Some(update) = &case.update {
            let request = merged(update, &json!({ "AdapterId": adapter_id.clone() }));
            let (status, body) = call(&router, "UpdateAdapter", &request).await;
            assert_eq!(
                status, 200,
                "case {:?}: the update before the delete must succeed, got {status} with {body}",
                case.name
            );
        }
        // It reads back while it exists, so the refusals below are the DELETION's doing.
        get_adapter(&router, case.name, &adapter_id).await;

        let (status, body) = call(
            &router,
            "DeleteAdapter",
            &json!({ "AdapterId": adapter_id.clone() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from DeleteAdapter, got {status} with {body}",
            case.name
        );

        // Every later call on that id is refused with the modelled not-found error — the
        // read, a second delete, and a mutation.
        for (operation, request) in [
            ("GetAdapter", json!({ "AdapterId": adapter_id.clone() })),
            ("DeleteAdapter", json!({ "AdapterId": adapter_id.clone() })),
            (
                "UpdateAdapter",
                json!({ "AdapterId": adapter_id.clone(), "AutoUpdate": "ENABLED" }),
            ),
            ("GetAdapter", json!({ "AdapterId": adapter_id.clone() })),
        ] {
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {:?}: expected HTTP 400 from {operation} after the delete, got {status} \
                 with {body}",
                case.name
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {:?}: expected ResourceNotFoundException from {operation} after the \
                 delete, got {body}",
                case.name
            );
            assert!(
                body.get("AdapterName").is_none() && body.get("AutoUpdate").is_none(),
                "case {:?}: {operation} must not report a deleted adapter's configuration, \
                 got {body}",
                case.name
            );
        }

        // The bystander is untouched.
        let bystander_after = get_adapter(&router, case.name, &bystander).await;
        assert_eq!(
            bystander_after, bystander_before,
            "case {:?}: deleting one adapter must not change another, got {bystander_after} \
             after {bystander_before}",
            case.name
        );
    }
}

#[tokio::test]
async fn a_client_request_token_still_names_its_adapter_after_a_mutation() {
    let router = harness();

    // One rule, several mutations: `ClientRequestToken` idempotency is about the CREATE
    // request, so a replay of the create answers with the adapter it created however much
    // that adapter has been updated since — and the replay is itself a read, so it must
    // not reset what the updates wrote.
    let updates = [
        json!({ "Description": "updated once" }),
        json!({ "AutoUpdate": "ENABLED" }),
        json!({ "AdapterName": "updated-name" }),
        json!({
            "AdapterName": "updated-everything",
            "Description": "updated again",
            "AutoUpdate": "ENABLED",
        }),
    ];

    for (index, update) in updates.iter().enumerate() {
        let case = format!("after the update {update}");
        let create = json!({
            "AdapterName": "token-adapter",
            "FeatureTypes": ["QUERIES"],
            "Description": "the created description",
            "AutoUpdate": "DISABLED",
            "ClientRequestToken": format!("stable-token-{index}"),
        });
        let adapter_id = create_adapter(&router, &case, &create).await;

        let request = merged(update, &json!({ "AdapterId": adapter_id.clone() }));
        let (status, body) = call(&router, "UpdateAdapter", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the update must succeed, got {status} with {body}"
        );
        let after_update = get_adapter(&router, &case, &adapter_id).await;

        let replayed = create_adapter(&router, &case, &create).await;
        assert_eq!(
            replayed, adapter_id,
            "case {case:?}: a replay of the create token must answer with {adapter_id:?}, got \
             {replayed:?}"
        );
        let after_replay = get_adapter(&router, &case, &adapter_id).await;
        assert_eq!(
            after_replay, after_update,
            "case {case:?}: replaying the create must not undo the update, got {after_replay} \
             after {after_update}"
        );
    }
}
