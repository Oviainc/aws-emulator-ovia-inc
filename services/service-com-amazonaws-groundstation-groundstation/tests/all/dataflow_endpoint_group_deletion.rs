// SPDX-License-Identifier: CC0-1.0

//! `DeleteDataflowEndpointGroup`, end to end over HTTP through the same front door as the
//! rest of this target's suites (the harness, signed `restJson1` requests and the response
//! reader come from [`crate::scheduling_loop`]).
//!
//! The RULE under test, asserted over a TABLE of groups rather than one recorded call:
//! deleting a resource this service holds removes the stored resource and the ARN that
//! resolved to it, so the delete answers the id it removed and EVERY later read of that id
//! or ARN — the resource's own read, the listing, the tag read — answers exactly what it
//! answers for an id the account never held, while no other resource is touched. The
//! group's contents are not part of the rule, so the table spans what a group can hold:
//! each of the four endpoint members `EndpointDetails` declares, a group holding two
//! endpoints, and groups created with and without tags and pre/post-pass durations.
//!
//! An id outside the modelled `Uuid` never reaches the handler (this service's request
//! validator answers it), and neither a refused delete nor a delete of an absent id
//! disturbs the store.

use serde_json::{json, Map, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    service_state, BAD_REQUEST_ERROR_CODE, DATAFLOW_ENDPOINT_GROUP_NOT_FOUND,
    INVALID_REQUEST_BODY,
};

use crate::dataflow_endpoint_group_listing::listed_ids;
use crate::dataflow_endpoint_groups::{
    create_group, delete_group, details, endpoint_kinds, expected_arn, get_group, DURATION_MAX,
    DURATION_MIN,
};
use crate::resource_tagging::list_tags;
use crate::scheduling_loop::{harness_with_engine, Reply, ABSENT_UUID};

/// One group the table creates and then deletes: what it holds, and the tags and durations
/// it was created with.
struct Row {
    /// Names the row in every assertion.
    label: String,
    /// The `endpointDetails` list the create request carries.
    endpoint_details: Vec<Value>,
    /// The `tags` map the group is created with, or `None` for a group created without one.
    tags: Option<Value>,
    /// `(contactPrePassDurationSeconds, contactPostPassDurationSeconds)`, or `None` for a
    /// group created without either.
    durations: Option<(i64, i64)>,
}

/// The table: one row per endpoint kind, plus a group holding two endpoints, alternating
/// between groups created with and without tags and durations.
///
/// The endpoint kinds come from [`endpoint_kinds`] — the one table in this target that
/// lists every endpoint member of `EndpointDetails` — so a member added there is deleted
/// here too without a second catalog to update.
fn rows() -> Vec<Row> {
    let kinds = endpoint_kinds();
    let mut rows: Vec<Row> = kinds
        .iter()
        .enumerate()
        .map(|(index, kind)| Row {
            label: format!("a group holding {}", kind.label),
            endpoint_details: vec![details(kind.member, &kind.body, true)],
            // Half the rows carry tags and durations, half carry neither: a delete must
            // not depend on either being present.
            tags: (index % 2 == 0).then(|| json!({"row": kind.member})),
            durations: (index % 2 == 0).then_some((DURATION_MIN, DURATION_MAX)),
        })
        .collect();

    // A group holding MORE than one endpoint: the count of endpoints a group holds is not
    // part of the rule either.
    let dataflow = &kinds[0];
    let mut second = dataflow
        .body
        .as_object()
        .expect("an endpoint object")
        .clone();
    second.insert("name".to_string(), json!("second-endpoint"));
    second.insert(
        "address".to_string(),
        json!({"name": "172.31.0.11", "port": 55889}),
    );
    rows.push(Row {
        label: "a group holding two dataflow endpoints".to_string(),
        endpoint_details: vec![
            details(dataflow.member, &dataflow.body, true),
            details(dataflow.member, &Value::Object(second), true),
        ],
        tags: Some(json!({"row": "two-endpoints"})),
        durations: None,
    });
    rows
}

/// The `CreateDataflowEndpointGroup` body `row` describes.
fn create_body(row: &Row) -> Value {
    let mut body = Map::new();
    body.insert(
        "endpointDetails".to_string(),
        Value::Array(row.endpoint_details.clone()),
    );
    if let Some(tags) = &row.tags {
        body.insert("tags".to_string(), tags.clone());
    }
    if let Some((pre, post)) = row.durations {
        body.insert("contactPrePassDurationSeconds".to_string(), json!(pre));
        body.insert("contactPostPassDurationSeconds".to_string(), json!(post));
    }
    Value::Object(body)
}

/// Assert that `reply` is the refusal this service answers for a dataflow endpoint group
/// the account does not hold: HTTP 434 `ResourceNotFoundException`
/// (`smithy_query(query="operation", operation="DeleteDataflowEndpointGroup")` lists it
/// with that status).
///
/// `pub(crate)` so this target has ONE owner of the not-found shape: the sibling group
/// suite asserts it through this helper too.
pub(crate) fn assert_group_absent(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 434,
        "case {case}: expected 434, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        "ResourceNotFoundException",
        "case {case}: wrong error shape, got {}",
        reply.body
    );
    assert_eq!(
        reply.string("message"),
        DATAFLOW_ENDPOINT_GROUP_NOT_FOUND,
        "case {case}: wrong refusal text, got {}",
        reply.body
    );
}

#[tokio::test]
async fn a_deleted_group_is_gone_from_every_read_and_the_others_are_untouched() {
    let (router, engine) = harness_with_engine();
    let rows = rows();

    // Every row's group, created first so each delete below runs against a store holding
    // the groups it must leave alone.
    let mut live: Vec<(String, String, Option<Value>)> = Vec::new();
    for row in &rows {
        let created = create_group(&router, create_body(row)).await;
        assert_eq!(
            created.status, 200,
            "case {}: expected CreateDataflowEndpointGroup 200, got {} {}",
            row.label, created.status, created.body
        );
        live.push((
            row.label.clone(),
            created.string("dataflowEndpointGroupId"),
            row.tags.clone(),
        ));
    }

    while !live.is_empty() {
        // Deleted in creation order, so each row runs against a store that still holds the
        // groups the delete must leave alone.
        let (label, id, _) = live.remove(0);
        let arn = expected_arn(&id);

        let deleted = delete_group(&router, &id).await;
        assert_eq!(
            deleted.status, 200,
            "case {label}: expected DeleteDataflowEndpointGroup 200, got {} {}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.string("dataflowEndpointGroupId"),
            id,
            "case {label}: the delete must answer the id it removed, got {}",
            deleted.body
        );

        // The group is gone from its own read and from a repeat of the delete, which is
        // answered the way a delete of an id the account never held is.
        assert_group_absent(
            &format!("{label}: read after the delete"),
            &get_group(&router, &id).await,
        );
        assert_group_absent(
            &format!("{label}: the same delete repeated"),
            &delete_group(&router, &id).await,
        );
        assert_group_absent(
            &format!("{label}: read of an id never created"),
            &get_group(&router, ABSENT_UUID).await,
        );

        // The ARN that resolved to it resolves to nothing: the tag read of a deleted
        // resource is the tag read of one that never existed.
        let tags_read = list_tags(&router, &arn).await;
        assert_eq!(
            tags_read.status, 434,
            "case {label}: expected ListTagsForResource 434 for the deleted ARN, got {} {}",
            tags_read.status, tags_read.body
        );
        assert_eq!(
            tags_read.string("message"),
            "Resource was not found",
            "case {label}: wrong refusal text for the deleted ARN, got {}",
            tags_read.body
        );
        {
            let state = service_state(&*engine).expect("this service's state store");
            let guard = state.lock();
            assert!(
                !guard.dataflow_endpoint_groups.contains_key(&id),
                "case {label}: the deleted group must be gone from state, {} remain",
                guard.dataflow_endpoint_groups.len()
            );
            assert!(
                !guard.arn_index.contains_key(&arn),
                "case {label}: the deleted group's ARN must resolve to nothing"
            );
        }

        // Nothing else moved: the listing reports exactly the groups still held, and each
        // of them still reads back with the tags it was created with.
        let mut expected: Vec<String> = live.iter().map(|(_, id, _)| id.clone()).collect();
        expected.sort();
        let listed = listed_ids(&router, Some(2), expected.len()).await;
        assert_eq!(
            listed, expected,
            "case {label}: after the delete the listing must report the remaining groups: \
             expected {expected:?}, got {listed:?}"
        );
        for (other_label, other_id, other_tags) in &live {
            let read = get_group(&router, other_id).await;
            assert_eq!(
                read.status, 200,
                "case {label}: {other_label} must still read back, got {} {}",
                read.status, read.body
            );
            assert_eq!(
                read.field("tags"),
                &other_tags.clone().unwrap_or_else(|| json!({})),
                "case {label}: {other_label} must keep its own tags, got {}",
                read.body
            );
        }
    }

    // Every row deleted: the store holds no group and no group ARN at all.
    let state = service_state(&*engine).expect("this service's state store");
    let guard = state.lock();
    assert!(
        guard.dataflow_endpoint_groups.is_empty(),
        "case every row deleted: expected an empty store, {} groups remain",
        guard.dataflow_endpoint_groups.len()
    );
    assert!(
        guard.arn_index.is_empty(),
        "case every row deleted: expected no ARN to resolve, {} remain",
        guard.arn_index.len()
    );
}

#[tokio::test]
async fn a_refused_delete_removes_nothing() {
    let (router, engine) = harness_with_engine();
    let kinds = endpoint_kinds();
    let kind = &kinds[0];

    let created = create_group(
        &router,
        json!({"endpointDetails": [details(kind.member, &kind.body, true)]}),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case the surviving group: expected 200, got {} {}",
        created.status, created.body
    );
    let id = created.string("dataflowEndpointGroupId");

    // A well-formed id no group holds is a missing resource.
    assert_group_absent(
        "an id the account never held",
        &delete_group(&router, ABSENT_UUID).await,
    );

    // An id outside the modelled `Uuid` (len 36..36, lower-case hex in five groups —
    // `smithy_query(query="shape", shape="Uuid")`) never reaches the handler: this
    // service's own request validator (`src/service.rs`) refuses a value failing the
    // model's rules as HTTP 400 `BadRequestException` before the handler runs, which is
    // what the whole target asserts for a boundary refusal; the modelled
    // `InvalidParameterException` (431) is what the HANDLER raises for a parameter it
    // reads itself. Each row is one way to leave the pattern, and none of them is the
    // cited one: the four-group row is exactly 36 characters, so it fails the grouping
    // rule and not the length one.
    let malformed: &[(&str, &str)] = &[
        ("not hexadecimal at all", "nope"),
        ("one character short", "0000000-0000-0000-0000-000000000000"),
        ("one character long", "000000000-0000-0000-0000-000000000000"),
        ("upper-case hex", "0A1B2C3D-4E5F-4A6B-8C7D-9E0F1A2B3C4D"),
        ("four groups instead of five", "00000000-0000-0000-00000000000000000"),
        ("a non-hex digit in the last group", "00000000-0000-0000-0000-00000000000g"),
    ];
    for (case, bad_id) in malformed {
        let reply = delete_group(&router, bad_id).await;
        assert_eq!(
            reply.status, 400,
            "case {case}: expected 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            BAD_REQUEST_ERROR_CODE,
            "case {case}: expected {BAD_REQUEST_ERROR_CODE}, got {}",
            reply.body
        );
        assert_eq!(
            reply.string("message"),
            INVALID_REQUEST_BODY,
            "case {case}: wrong refusal text, got {}",
            reply.body
        );
    }

    // None of those refusals touched the store: the one group created still reads back and
    // is still the only one this account holds.
    let read = get_group(&router, &id).await;
    assert_eq!(
        read.status, 200,
        "case after the refusals: the created group must still read back, got {} {}",
        read.status, read.body
    );
    assert_eq!(
        read.string("dataflowEndpointGroupId"),
        id,
        "case after the refusals: wrong group read back, got {}",
        read.body
    );
    let state = service_state(&*engine).expect("this service's state store");
    let held = state.lock().dataflow_endpoint_groups.len();
    assert_eq!(
        held, 1,
        "case after the refusals: the store must still hold exactly the one group, got {held}"
    );
}
