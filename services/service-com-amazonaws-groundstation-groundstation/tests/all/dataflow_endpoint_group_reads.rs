// SPDX-License-Identifier: CC0-1.0

//! What `GetDataflowEndpointGroup` reports, end to end over HTTP through the same front
//! door as the rest of this target's suites (the harness, signed `restJson1` requests
//! and the response reader come from [`crate::scheduling_loop`]).
//!
//! The RULE under test: the reader answers the group PERSISTED under the id its path
//! addresses — that group's id and ARN, its endpoints in the order they were stored, its
//! tags and its pre/post-pass durations — never another group's, and a read leaves the
//! store as it found it, so a later caller sees exactly what the first one did. An id the
//! model cannot spell is refused by this service's request validator before the reader
//! runs, and a spellable id the account holds no group for is the modelled
//! `ResourceNotFoundException`.
//!
//! The sibling suite `tests/all/dataflow_endpoint_groups.rs` asserts what a CREATE
//! stores for each endpoint kind; this one asserts the READER over a store holding many
//! groups at once, so the rows differ in the dimensions the reader has to keep apart:
//! how many endpoints a group holds, which members describe them, their order, how many
//! tags it carries and which durations it was given.

use serde_json::{json, Map, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    BAD_REQUEST_ERROR_CODE, INVALID_REQUEST_BODY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::dataflow_endpoint_group_deletion::assert_group_absent;
use crate::dataflow_endpoint_group_listing::listed_ids;
use crate::dataflow_endpoint_groups::{
    create_group, details, endpoint_kinds, expected_arn, get_group, DURATION_MAX, DURATION_MIN,
};
use crate::scheduling_loop::{harness, is_uuid, Reply, ABSENT_UUID, AFTER_SETTLE};

/// One row of the table: a group as a request describes it, by the endpoint kinds it
/// holds and the name each endpoint carries.
struct Row {
    /// Names the row in every assertion.
    label: &'static str,
    /// For each endpoint of the group, its index into [`endpoint_kinds`] and the `name`
    /// the request gives it. One group may hold several endpoints, and a read must report
    /// them in the order they were stored.
    endpoints: Vec<(usize, &'static str)>,
    /// The tags the create carries, or `None` for a group created without any.
    tags: Option<Value>,
    /// `contactPrePassDurationSeconds` and `contactPostPassDurationSeconds`, each absent
    /// or a value inside the modelled range.
    durations: (Option<i64>, Option<i64>),
}

/// The table: groups that differ in endpoint count, endpoint member, endpoint order, tag
/// count and durations, all held by one account at once.
///
/// A group holds endpoints of ONE family ("You cannot mix AWS Ground Station Agent
/// endpoints with Dataflow endpoints in the same group" — the create operation's own
/// documentation), so the multi-endpoint rows stay inside one family.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "a group with no endpoint at all",
            endpoints: Vec::new(),
            tags: None,
            durations: (None, None),
        },
        Row {
            label: "one dataflow endpoint, at both published duration bounds",
            endpoints: vec![(0, "single-dataflow")],
            tags: Some(json!({"mission": "alpha"})),
            durations: (Some(DURATION_MIN), Some(DURATION_MAX)),
        },
        Row {
            label: "three dataflow endpoints and three tags",
            endpoints: vec![
                (0, "trio-first"),
                (0, "trio-second"),
                (0, "trio-third"),
            ],
            tags: Some(json!({"mission": "beta", "owner": "ops", "stage": "test"})),
            durations: (Some(120), Some(240)),
        },
        Row {
            label: "two agent endpoints of different agent members",
            endpoints: vec![(1, "agent-egress-ingress"), (2, "agent-uplink")],
            tags: None,
            durations: (Some(DURATION_MAX), Some(DURATION_MIN)),
        },
        Row {
            label: "a downlink agent endpoint with only a post-pass duration",
            endpoints: vec![(3, "agent-downlink")],
            tags: Some(json!({"mission": "delta"})),
            durations: (None, Some(300)),
        },
    ]
}

/// The modelled member names that describe an endpoint, taken from the one table of
/// endpoint kinds this target keeps (`crate::dataflow_endpoint_groups::endpoint_kinds`).
fn endpoint_members() -> Vec<&'static str> {
    endpoint_kinds().into_iter().map(|kind| kind.member).collect()
}

/// The `CreateDataflowEndpointGroup` body `row` describes.
fn create_body(row: &Row) -> Value {
    let kinds = endpoint_kinds();
    let endpoints: Vec<Value> = row
        .endpoints
        .iter()
        .map(|(kind, name)| {
            let kind = &kinds[*kind];
            let mut body = kind
                .body
                .as_object()
                .expect("every endpoint kind's body is an object")
                .clone();
            body.insert("name".to_string(), json!(name));
            details(kind.member, &Value::Object(body), true)
        })
        .collect();

    let mut request = Map::new();
    request.insert("endpointDetails".to_string(), Value::Array(endpoints));
    if let Some(tags) = &row.tags {
        request.insert("tags".to_string(), tags.clone());
    }
    if let Some(pre) = row.durations.0 {
        request.insert("contactPrePassDurationSeconds".to_string(), json!(pre));
    }
    if let Some(post) = row.durations.1 {
        request.insert("contactPostPassDurationSeconds".to_string(), json!(post));
    }
    Value::Object(request)
}

/// The `name` of one reported endpoint, whichever member describes it.
fn reported_name(case: &str, endpoint: &Value) -> String {
    for member in endpoint_members() {
        if let Some(described) = endpoint.get(member) {
            return described
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case}: an endpoint with no name: {endpoint}"))
                .to_string();
        }
    }
    panic!("case {case}: a reported endpoint names no endpoint member: {endpoint}")
}

/// The names of the endpoints `reply` reports, in the order it reported them.
fn reported_names(case: &str, reply: &Reply) -> Vec<String> {
    reply
        .field("endpointsDetails")
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: endpointsDetails is not a list: {}", reply.body))
        .iter()
        .map(|endpoint| reported_name(case, endpoint))
        .collect()
}

/// Create every row of [`rows`] on `router` and answer each row's minted id, so each read
/// below runs against a store holding every other row too.
async fn create_rows(router: &MultiServiceRouter, rows: &[Row]) -> Vec<String> {
    let mut ids = Vec::new();
    for row in rows {
        let created = create_group(router, create_body(row)).await;
        assert_eq!(
            created.status, 200,
            "case {}: expected CreateDataflowEndpointGroup 200, got {} {}",
            row.label, created.status, created.body
        );
        let id = created.string("dataflowEndpointGroupId");
        assert!(
            is_uuid(&id),
            "case {}: the minted id must be a lowercase uuid, got {id:?}",
            row.label
        );
        ids.push(id);
    }
    ids
}

#[tokio::test]
async fn each_group_reads_back_its_own_persisted_state_and_nothing_of_another() {
    let router = harness();
    let rows = rows();
    let ids = create_rows(&router, &rows).await;

    for (row, id) in rows.iter().zip(&ids) {
        let case = row.label;
        let read = get_group(&router, id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected GetDataflowEndpointGroup 200, got {} {}",
            read.status, read.body
        );
        assert_eq!(
            read.string("dataflowEndpointGroupId"),
            *id,
            "case {case}: the group must read back under the id its path addressed, got {}",
            read.body
        );
        assert_eq!(
            read.string("dataflowEndpointGroupArn"),
            expected_arn(id),
            "case {case}: wrong dataflowEndpointGroupArn, got {}",
            read.body
        );
        assert_eq!(
            read.field("tags"),
            &row.tags.clone().unwrap_or_else(|| json!({})),
            "case {case}: wrong tags, got {}",
            read.body
        );
        assert_eq!(
            read.field("contactPrePassDurationSeconds"),
            &row.durations.0.map_or(Value::Null, |value| json!(value)),
            "case {case}: wrong contactPrePassDurationSeconds, got {}",
            read.body
        );
        assert_eq!(
            read.field("contactPostPassDurationSeconds"),
            &row.durations.1.map_or(Value::Null, |value| json!(value)),
            "case {case}: wrong contactPostPassDurationSeconds, got {}",
            read.body
        );

        // Every endpoint the create stored, in the order it stored them.
        let expected_names: Vec<String> = row
            .endpoints
            .iter()
            .map(|(_, name)| (*name).to_string())
            .collect();
        let actual_names = reported_names(case, &read);
        assert_eq!(
            actual_names, expected_names,
            "case {case}: the reader reports the group's endpoints in the order they were \
             stored, got {actual_names:?}"
        );

        // And the member that describes each endpoint is the member the request used, so
        // an endpoint is not reported under a neighbouring kind.
        let kinds = endpoint_kinds();
        let reported = read
            .field("endpointsDetails")
            .as_array()
            .expect("endpointsDetails is a list")
            .clone();
        for ((kind, name), endpoint) in row.endpoints.iter().zip(&reported) {
            let member = kinds[*kind].member;
            assert!(
                endpoint.get(member).is_some(),
                "case {case}: endpoint {name} must be reported under {member}, got {endpoint}"
            );
            let others: Vec<&str> = endpoint_members()
                .into_iter()
                .filter(|candidate| *candidate != member)
                .filter(|candidate| endpoint.get(candidate).is_some())
                .collect();
            assert!(
                others.is_empty(),
                "case {case}: endpoint {name} must name exactly one endpoint member, also \
                 found {others:?} in {endpoint}"
            );
        }

        // Nothing of any OTHER group leaks into this read: no other row's endpoint name
        // and no other row's id or ARN appears in it.
        let serialized = read.body.to_string();
        for (other_row, other_id) in rows.iter().zip(&ids) {
            if other_id == id {
                continue;
            }
            assert!(
                !serialized.contains(other_id.as_str()),
                "case {case}: the read must carry no other group's id, found {other_id} in \
                 {serialized}"
            );
            for (_, other_name) in &other_row.endpoints {
                assert!(
                    !serialized.contains(other_name),
                    "case {case}: the read must carry no endpoint of {}, found {other_name} \
                     in {serialized}",
                    other_row.label
                );
            }
        }
    }
}

#[tokio::test]
async fn reading_a_group_changes_nothing_a_later_caller_sees() {
    let router = harness();
    let rows = rows();
    let ids = create_rows(&router, &rows).await;

    // Past every settle span this service chooses, so the `creating` -> `created` move it
    // owed a dataflow endpoint is already due and the reads below differ in nothing but
    // the reader's own effects.
    std::thread::sleep(AFTER_SETTLE);

    let listed_before = listed_ids(&router, Some(2), ids.len()).await;

    let mut first: Vec<Value> = Vec::new();
    for (row, id) in rows.iter().zip(&ids) {
        let read = get_group(&router, id).await;
        assert_eq!(
            read.status, 200,
            "case {}: expected GetDataflowEndpointGroup 200, got {} {}",
            row.label, read.status, read.body
        );
        first.push(read.body);
    }

    // Read again in the opposite order, so a read of one group cannot be what makes the
    // next read of another group agree.
    for index in (0..ids.len()).rev() {
        let again = get_group(&router, &ids[index]).await;
        assert_eq!(
            again.body, first[index],
            "case {}: a read must report what the previous read did, got {} after {}",
            rows[index].label, again.body, first[index]
        );
    }

    let listed_after = listed_ids(&router, Some(2), ids.len()).await;
    assert_eq!(
        listed_after, listed_before,
        "the reads must leave the account's groups as they were: expected \
         {listed_before:?}, got {listed_after:?}"
    );
}

#[tokio::test]
async fn an_unspellable_id_is_refused_and_a_spellable_one_naming_nothing_is_not_found() {
    let router = harness();
    let rows = rows();
    let ids = create_rows(&router, &rows).await;
    let present = ids.first().expect("the table creates a group").clone();

    // An id outside the modelled `Uuid` (len 36..36, lower-case hex in five groups) never
    // reaches the reader: this service's request validator answers it, and that is a bad
    // request rather than a missing resource. Rows span each bound of that length and one
    // value past each, plus spellings the pattern excludes.
    let unspellable: Vec<(&str, String)> = vec![
        ("not hexadecimal at all", "nope".to_string()),
        ("one character short", present[..present.len() - 1].to_string()),
        ("one character long", format!("{present}0")),
        ("upper-case hex", present.to_uppercase()),
        ("the groups joined without separators", present.replace('-', "")),
    ];
    for (case, id) in &unspellable {
        let reply = get_group(&router, id).await;
        assert_eq!(
            reply.status, 400,
            "case {case} ({id}): expected 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            BAD_REQUEST_ERROR_CODE,
            "case {case} ({id}): expected {BAD_REQUEST_ERROR_CODE}, got {}",
            reply.body
        );
        assert_eq!(
            reply.string("message"),
            INVALID_REQUEST_BODY,
            "case {case} ({id}): wrong refusal text, got {}",
            reply.body
        );
    }

    // Spellable ids that name no group of this account: a run of zeros, a run of `f`s and
    // a mixed one, so the refusal cannot be passing on one id's shape.
    for id in [
        ABSENT_UUID,
        "ffffffff-ffff-4fff-bfff-ffffffffffff",
        "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d",
    ] {
        let reply = get_group(&router, id).await;
        assert_group_absent(&format!("a group never created ({id})"), &reply);
    }

    // The refusals disturbed nothing: every created group still reads back under its own
    // id, and the account still holds exactly the groups it was given.
    for (row, id) in rows.iter().zip(&ids) {
        let read = get_group(&router, id).await;
        assert_eq!(
            read.status, 200,
            "case {} after the refusals: expected 200, got {} {}",
            row.label, read.status, read.body
        );
        assert_eq!(
            read.string("dataflowEndpointGroupId"),
            *id,
            "case {} after the refusals: wrong group, got {}",
            row.label,
            read.body
        );
    }
    let mut listed = listed_ids(&router, None, ids.len()).await;
    let mut created = ids.clone();
    listed.sort();
    created.sort();
    assert_eq!(
        listed, created,
        "after the refusals the account holds exactly the groups it was given: expected \
         {created:?}, got {listed:?}"
    );
}
