// SPDX-License-Identifier: CC0-1.0

//! The AWS WAF Classic (regional) `SizeConstraintSet` records, end to end over the
//! wire: `CreateSizeConstraintSet` and `GetSizeConstraintSet` (plus the
//! `GetChangeToken`/`GetChangeTokenStatus` protocol every mutating call follows).
//!
//! Every call goes through the real router (`POST /` with
//! `X-Amz-Target: AWSWAF_Regional_20161128.<Op>`, awsJson1_1), so what these tests
//! assert is what an SDK client sees: each response is built from the persisted
//! record, not from the request.
//!
//! The request-building helpers come from the crate's shared `wire` harness; only the
//! SizeConstraintSet-specific helpers live here.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_wafregional_awswaf_regional_20161128::ops::types::{
    FieldToMatch, SizeConstraint,
};
use service_com_amazonaws_wafregional_awswaf_regional_20161128::state::{
    service_state, MAX_SIZE_CONSTRAINT_SETS,
};

use crate::wire::{
    change_token, expect_error, is_resource_id, ok_call, router, router_on, token_status,
};

/// A router and the engine it serves, so a test can read the persisted records
/// directly — `ListSizeConstraintSets` is still a scaffold stub, so the store a
/// handler reaches is the only place a count can come from.
fn router_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    (router_on(Arc::clone(&engine)), engine)
}

/// How many `SizeConstraintSet` records the service has persisted.
fn stored_sets(engine: &Arc<engine::InMemoryEngine>) -> usize {
    service_state(&**engine)
        .expect("the service store is reachable")
        .lock()
        .size_constraint_sets
        .len()
}

/// Create a SizeConstraintSet named `name` and return its `SizeConstraintSetId`. Shared
/// with the `UpdateSizeConstraintSet` suite, which needs a stored set to update.
pub async fn create_size_constraint_set(router: &MultiServiceRouter, name: &str) -> String {
    let token = change_token(router).await;
    let body = ok_call(
        router,
        "CreateSizeConstraintSet",
        &json!({"Name": name, "ChangeToken": token}),
    )
    .await;
    body["SizeConstraintSet"]["SizeConstraintSetId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateSizeConstraintSet returned no id: {body}"))
        .to_string()
}

/// `CreateSizeConstraintSet` stores a new, EMPTY SizeConstraintSet under a minted id
/// and reports the record it stored, and `GetSizeConstraintSet` reads that same record
/// back — across a table of names differing in length, case, script and the characters
/// they use. `ResourceName` carries no uniqueness constraint, so the last two rows
/// share a name and must still be two distinct resources.
#[tokio::test]
async fn create_then_get_reads_back_the_stored_size_constraint_set() {
    let (router, engine) = router_and_engine();
    let max_length_name = "s".repeat(128);
    let cases: &[(&str, &str)] = &[
        // (label, Name) — `ResourceName` is len 1..128, pattern \S.
        ("the recording's own name", "size-constraint-set-roundtrip"),
        ("one character (the minimum length)", "q"),
        (
            "128 characters (the maximum length)",
            max_length_name.as_str(),
        ),
        ("mixed case, digits, separators", "Query_String-100"),
        ("an interior space", "long user agent header"),
        ("non-ASCII characters", "ユーザーエージェント-長さ"),
        ("a name shared with the next row", "shared-name"),
        ("the same name again", "shared-name"),
    ];

    let mut created_sets: Vec<(&str, String, Value)> = Vec::new();
    let mut seen_ids: BTreeSet<String> = BTreeSet::new();
    for (index, (label, name)) in cases.iter().enumerate() {
        let token = change_token(&router).await;
        let created = ok_call(
            &router,
            "CreateSizeConstraintSet",
            &json!({"Name": name, "ChangeToken": token}),
        )
        .await;

        let set_id = created["SizeConstraintSet"]["SizeConstraintSetId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {label}: no SizeConstraintSetId was returned: {created}")
            })
            .to_string();
        assert!(
            is_resource_id(&set_id),
            "case {label}: expected a ResourceId, got {set_id:?}"
        );
        assert!(
            seen_ids.insert(set_id.clone()),
            "case {label}: CreateSizeConstraintSet reused the id {set_id}"
        );
        assert_eq!(
            created["SizeConstraintSet"]["Name"].as_str(),
            Some(*name),
            "case {label}: CreateSizeConstraintSet must report the stored name: {created}"
        );
        assert_eq!(
            created["SizeConstraintSet"]["SizeConstraints"]
                .as_array()
                .map(Vec::len),
            Some(0),
            "case {label}: a new SizeConstraintSet holds no constraints: {created}"
        );
        assert_eq!(
            created["ChangeToken"].as_str(),
            Some(token.as_str()),
            "case {label}: the response must report the change token it consumed: {created}"
        );
        assert_eq!(
            stored_sets(&engine),
            index + 1,
            "case {label}: each accepted create must persist exactly one record, store holds {}",
            stored_sets(&engine)
        );

        let fetched = ok_call(
            &router,
            "GetSizeConstraintSet",
            &json!({"SizeConstraintSetId": set_id}),
        )
        .await;
        assert_eq!(
            fetched["SizeConstraintSet"], created["SizeConstraintSet"],
            "case {label}: GetSizeConstraintSet must read back the created record, got {fetched}"
        );
        created_sets.push((label, set_id, created["SizeConstraintSet"].clone()));
    }

    // Every record is still its own: the later creates and the reads above changed
    // none of them.
    for (label, set_id, expected) in &created_sets {
        let fetched = ok_call(
            &router,
            "GetSizeConstraintSet",
            &json!({"SizeConstraintSetId": set_id}),
        )
        .await;
        assert_eq!(
            &fetched["SizeConstraintSet"], expected,
            "case {label} re-read: the record must be unchanged, got {fetched}"
        );
    }
    assert_eq!(
        stored_sets(&engine),
        cases.len(),
        "case re-read: reads must neither create nor drop records"
    );
}

/// The change-token protocol `CreateSizeConstraintSet` follows: the token it consumed
/// is the one it reports, that token moves `PROVISIONED` -> `PENDING` -> `INSYNC`, and
/// no second call may use it.
#[tokio::test]
async fn create_consumes_its_change_token_exactly_once() {
    let (router, engine) = router_and_engine();

    let token = change_token(&router).await;
    assert_eq!(
        token_status(&router, &token).await,
        "PROVISIONED",
        "case freshly minted: a token no mutating call has used is PROVISIONED"
    );

    let created = ok_call(
        &router,
        "CreateSizeConstraintSet",
        &json!({"Name": "token-protocol", "ChangeToken": token}),
    )
    .await;
    assert_eq!(
        created["ChangeToken"].as_str(),
        Some(token.as_str()),
        "CreateSizeConstraintSet must report the change token it consumed: {created}"
    );
    assert_eq!(
        token_status(&router, &token).await,
        "PENDING",
        "case just consumed: propagation has not finished yet, so the token is PENDING"
    );

    // "Each create, update, or delete request must use a unique change token": a
    // second use is stale data, as is a token this service never minted, and neither
    // may persist a record.
    let stored_before = stored_sets(&engine);
    for (label, request) in [
        (
            "a token this create already consumed",
            json!({"Name": "second-use", "ChangeToken": token}),
        ),
        (
            "a token this service never minted",
            json!({"Name": "never-minted", "ChangeToken": "11111111-2222-4333-8444-555555555555"}),
        ),
    ] {
        expect_error(
            &router,
            "CreateSizeConstraintSet",
            &request,
            "WAFStaleDataException",
            label,
        )
        .await;
        assert_eq!(
            stored_sets(&engine),
            stored_before,
            "case {label}: a refused create must persist nothing, store holds {}",
            stored_sets(&engine)
        );
    }

    // The PENDING -> INSYNC step is owed by the CLOCK, so it lands without any further
    // call being made. Nothing in the handler sleeps for it.
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert_eq!(
        token_status(&router, &token).await,
        "INSYNC",
        "case propagation elapsed: the token must settle to INSYNC"
    );
}

/// `GetSizeConstraintSet` refuses an id the service holds no record for with its
/// modelled `WAFNonexistentItemException`, whatever shape that id has — including an
/// id minted for a resource of another kind.
#[tokio::test]
async fn get_refuses_an_id_no_record_exists_for() {
    let router = router();
    // One real record, so the refusals below are about the id and not about an empty
    // store.
    let existing = create_size_constraint_set(&router, "the-one-that-exists").await;

    let token = change_token(&router).await;
    let ip_set = ok_call(
        &router,
        "CreateIPSet",
        &json!({"Name": "a-different-resource-kind", "ChangeToken": token}),
    )
    .await;
    let ip_set_id = ip_set["IPSet"]["IPSetId"]
        .as_str()
        .expect("CreateIPSet returned an id")
        .to_string();

    let cases: &[(&str, &str)] = &[
        (
            "a UUID-shaped id that was never minted",
            "00000000-0000-0000-0000-000000000000",
        ),
        (
            "another well-formed id this service never minted",
            "11111111-2222-4333-8444-555555555555",
        ),
        ("an id that is not UUID-shaped", "not-an-id"),
        ("a one-character id (the model's minimum)", "x"),
        // An id that names a record of ANOTHER kind is not a SizeConstraintSet either:
        // the two stores are separate.
        ("an IPSet's id", ip_set_id.as_str()),
    ];
    for (label, set_id) in cases {
        expect_error(
            &router,
            "GetSizeConstraintSet",
            &json!({"SizeConstraintSetId": set_id}),
            "WAFNonexistentItemException",
            label,
        )
        .await;
    }

    // The refusals left the real record alone.
    let fetched = ok_call(
        &router,
        "GetSizeConstraintSet",
        &json!({"SizeConstraintSetId": existing}),
    )
    .await;
    assert_eq!(
        fetched["SizeConstraintSet"]["Name"].as_str(),
        Some("the-one-that-exists"),
        "case after the refusals: the stored record must still read back: {fetched}"
    );
}

/// The store a caller can grow is bounded: at the account's SizeConstraintSet quota
/// `CreateSizeConstraintSet` answers its modelled `WAFLimitsExceededException`, and
/// the refused call consumes neither the change token nor a record slot.
#[tokio::test]
async fn create_refuses_past_the_size_constraint_set_quota() {
    let (router, engine) = router_and_engine();
    for n in 0..MAX_SIZE_CONSTRAINT_SETS {
        create_size_constraint_set(&router, &format!("quota-probe-{n}")).await;
    }
    assert_eq!(
        stored_sets(&engine),
        MAX_SIZE_CONSTRAINT_SETS,
        "case at the quota: every accepted create must have persisted a record"
    );

    let token = change_token(&router).await;
    expect_error(
        &router,
        "CreateSizeConstraintSet",
        &json!({"Name": "one-too-many", "ChangeToken": token}),
        "WAFLimitsExceededException",
        "at the quota",
    )
    .await;
    assert_eq!(
        token_status(&router, &token).await,
        "PROVISIONED",
        "case at the quota: a refused create must not consume the change token"
    );
    assert_eq!(
        stored_sets(&engine),
        MAX_SIZE_CONSTRAINT_SETS,
        "case at the quota: a refused create must not persist a record"
    );
}

/// One row of the constraint table below: the `SizeConstraint` a record holds, as the
/// wire spells it.
struct ConstraintRow {
    /// What this row varies, named in every assertion failure.
    label: &'static str,
    /// `FieldToMatch.Type`, a `MatchFieldType` value.
    field: &'static str,
    /// `FieldToMatch.Data`, which only HEADER and SINGLE_QUERY_ARG carry.
    data: Option<&'static str>,
    /// A `TextTransformation` value.
    transformation: &'static str,
    /// A `ComparisonOperator` value.
    operator: &'static str,
    /// `Size`, within the modelled `@range` of 0..21474836480.
    size: i64,
}

/// What the wire carries is the PROJECTION of the stored record, so a record holding
/// constraints reads them back, in order — across a table spanning the operators,
/// transformations and `Size` bounds the model enumerates.
///
/// The constraints are written straight into the state entry both handlers project —
/// the field `UpdateSizeConstraintSet` fills over the wire — which is what pins
/// `GetSizeConstraintSet` to the record rather than to a constant empty list,
/// independently of the update path (which `size_constraint_set_updates` covers end to
/// end).
#[tokio::test]
async fn get_projects_the_constraints_the_stored_record_holds() {
    let (router, engine) = router_and_engine();
    let set_id = create_size_constraint_set(&router, "projection").await;

    // Rows spanning the constraint shape. Sources: `ComparisonOperator`'s modelled
    // values (EQ, NE, LE, LT, GE, GT), `TextTransformation`'s (NONE,
    // COMPRESS_WHITE_SPACE, HTML_ENTITY_DECODE, LOWERCASE, CMD_LINE, URL_DECODE),
    // `MatchFieldType`'s (HEADER, METHOD, QUERY_STRING, URI, BODY, SINGLE_QUERY_ARG,
    // ALL_QUERY_ARGS) and `Size`'s `@range` of 0..21474836480.
    let cases: &[ConstraintRow] = &[
        ConstraintRow {
            label: "query string, strictly greater, 100 bytes",
            field: "QUERY_STRING",
            data: None,
            transformation: "NONE",
            operator: "GT",
            size: 100,
        },
        ConstraintRow {
            label: "header carrying Data, equal, the minimum Size",
            field: "HEADER",
            data: Some("User-Agent"),
            transformation: "LOWERCASE",
            operator: "EQ",
            size: 0,
        },
        ConstraintRow {
            label: "URI, not equal, mid-range",
            field: "URI",
            data: None,
            transformation: "URL_DECODE",
            operator: "NE",
            size: 8_192,
        },
        ConstraintRow {
            label: "body, at most, the maximum Size",
            field: "BODY",
            data: None,
            transformation: "NONE",
            operator: "LE",
            size: 21_474_836_480,
        },
        ConstraintRow {
            label: "method, strictly less",
            field: "METHOD",
            data: None,
            transformation: "CMD_LINE",
            operator: "LT",
            size: 7,
        },
        ConstraintRow {
            label: "single query arg, at least",
            field: "SINGLE_QUERY_ARG",
            data: Some("UserName"),
            transformation: "COMPRESS_WHITE_SPACE",
            operator: "GE",
            size: 42,
        },
        ConstraintRow {
            label: "all query args, html entity decoded",
            field: "ALL_QUERY_ARGS",
            data: None,
            transformation: "HTML_ENTITY_DECODE",
            operator: "GT",
            size: 1,
        },
    ];

    let mut expected: Vec<Value> = Vec::new();
    for row in cases {
        let label = row.label;
        let mut field_to_match = json!({"Type": row.field});
        if let Some(data) = row.data {
            field_to_match["Data"] = json!(data);
        }
        expected.push(json!({
            "FieldToMatch": field_to_match,
            "TextTransformation": row.transformation,
            "ComparisonOperator": row.operator,
            "Size": row.size,
        }));

        // The record is the single copy both handlers project; writing it here keeps
        // this assertion about the projection alone.
        {
            let state = service_state(&*engine).expect("the service store is reachable");
            let mut state = state.lock();
            let entry = state
                .size_constraint_sets
                .get_mut(&set_id)
                .unwrap_or_else(|| panic!("case {label}: the created record is stored"));
            entry.size_constraints.push(SizeConstraint {
                field_to_match: FieldToMatch {
                    r#type: row.field.to_string(),
                    data: row.data.map(str::to_string),
                },
                text_transformation: row.transformation.to_string(),
                comparison_operator: row.operator.to_string(),
                size: row.size,
            });
        }

        let fetched = ok_call(
            &router,
            "GetSizeConstraintSet",
            &json!({"SizeConstraintSetId": set_id}),
        )
        .await;
        assert_eq!(
            fetched["SizeConstraintSet"]["SizeConstraints"],
            Value::Array(expected.clone()),
            "case {label}: the stored constraints must be projected in order, got {fetched}"
        );
    }

    // A record created alongside it is still empty: the constraints belong to one
    // record, not to the service.
    let other = create_size_constraint_set(&router, "still-empty").await;
    let fetched = ok_call(
        &router,
        "GetSizeConstraintSet",
        &json!({"SizeConstraintSetId": other}),
    )
    .await;
    assert_eq!(
        fetched["SizeConstraintSet"]["SizeConstraints"]
            .as_array()
            .map(Vec::len),
        Some(0),
        "case a second record: it must still hold no constraints: {fetched}"
    );
}
