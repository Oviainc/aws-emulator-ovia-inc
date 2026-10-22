// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `ListContactFlows`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator serves with.
//!
//! The create/describe loop a flow is stored through is covered by
//! `contact_flow_lifecycle`; every suite here asserts a RULE about the LIST: it reports
//! the instance's stored flows and nothing else, keeps exactly the types
//! `contactFlowTypes` names, walks the store in one deterministic order one
//! `MaxResults`-sized page at a time, serves the documented default page size, and
//! refuses an unknown instance, a foreign page token and a page size outside the
//! modelled range.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_connect_amazonconnectservice::state::{
    CONTACT_FLOW_STATUS_PUBLISHED, CONTACT_FLOW_STATUS_SAVED, DEFAULT_CONTACT_FLOWS,
    LIST_CONTACT_FLOWS_DEFAULT_PAGE_SIZE,
};

use super::harness::{connect_managed, ids_of, query, Connect};

/// Every member of `com.amazonaws.connect#ContactFlowType`, as
/// `smithy_query(query="shape",shape="ContactFlowType")` lists them. A summary reports
/// the flow's type verbatim, so the filter rule has to hold for each one — including the
/// members the issue never cited.
const CONTACT_FLOW_TYPES: [&str; 10] = [
    "CONTACT_FLOW",
    "CUSTOMER_QUEUE",
    "CUSTOMER_HOLD",
    "CUSTOMER_WHISPER",
    "AGENT_HOLD",
    "AGENT_WHISPER",
    "OUTBOUND_WHISPER",
    "AGENT_TRANSFER",
    "QUEUE_TRANSFER",
    "CAMPAIGN",
];

/// A flow content of the documented form: the JSON string of a flow definition.
const FLOW_CONTENT: &str = r#"{"Version":"2019-10-30","StartAction":"a1","Actions":[{"Identifier":"a1","Type":"DisconnectParticipant","Parameters":{}}]}"#;

/// The `ListContactFlows` call this suite drives, on the shared fixture.
impl Connect {
    async fn list_contact_flows(
        &self,
        instance_id: &str,
        max_results: Option<i32>,
        token: Option<&str>,
        flow_types: &[&str],
    ) -> (u16, Value) {
        let uri = format!(
            "/contact-flows-summary/{instance_id}{}",
            query(max_results, token, "contactFlowTypes", flow_types)
        );
        self.send("GET", &uri, None).await
    }

    /// `CreateContactFlow` of the given name, type and status, asserting success and
    /// handing back `(ContactFlowId, ContactFlowArn)`.
    async fn store_flow(
        &self,
        instance_id: &str,
        name: &str,
        flow_type: &str,
        status: Option<&str>,
    ) -> (String, String) {
        let mut body = json!({ "Name": name, "Type": flow_type, "Content": FLOW_CONTENT });
        if let Some(status) = status {
            body["Status"] = json!(status);
        }
        let (code, created) = self
            .send("PUT", &format!("/contact-flows/{instance_id}"), Some(body))
            .await;
        assert_eq!(code, 200, "CreateContactFlow {name}/{flow_type}: got {created}");
        (
            created["ContactFlowId"]
                .as_str()
                .expect("ContactFlowId")
                .to_string(),
            created["ContactFlowArn"]
                .as_str()
                .expect("ContactFlowArn")
                .to_string(),
        )
    }
}

/// Every flow id the instance owns before this suite stores one: the default flows
/// `CreateInstance` provisions, which a list rule has to account for because they are
/// stored flows of the instance like any other.
async fn default_flow_ids(connect: &Connect, instance_id: &str) -> BTreeSet<String> {
    let (status, listed) = connect
        .list_contact_flows(instance_id, None, None, &[])
        .await;
    assert_eq!(status, 200, "ListContactFlows of the defaults got {listed}");
    let ids: BTreeSet<String> = ids_of(&listed["ContactFlowSummaryList"], "Id")
        .into_iter()
        .collect();
    assert_eq!(
        ids.len(),
        DEFAULT_CONTACT_FLOWS.len(),
        "a new instance owns the default flows, got {listed}"
    );
    ids
}

/// Walk `ListContactFlows` to exhaustion, returning every id seen and the page count.
async fn walk_flows(
    connect: &Connect,
    instance_id: &str,
    page_size: Option<i32>,
) -> (Vec<String>, usize) {
    let mut seen = Vec::new();
    let mut pages = 0usize;
    let mut token: Option<String> = None;
    loop {
        let (status, listed) = connect
            .list_contact_flows(instance_id, page_size, token.as_deref(), &[])
            .await;
        assert_eq!(status, 200, "ListContactFlows got {listed}");
        pages += 1;
        let page = ids_of(&listed["ContactFlowSummaryList"], "Id");
        if let Some(size) = page_size {
            assert!(
                page.len() <= size as usize,
                "a page must hold at most maxResults items, got {page:?}"
            );
        }
        seen.extend(page);
        match listed["NextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
        assert!(pages < 64, "ListContactFlows did not terminate");
    }
    (seen, pages)
}

/// RULE: a `ContactFlowSummary` reports the identity, name, type, state and status of the
/// flow that was stored — every flow of the instance, once, and no flow of another
/// instance.
///
/// The table spans every modelled `ContactFlowType`, both `ContactFlowStatus` members
/// plus an absent `Status` (which publishes), and names differing in length.
#[tokio::test]
async fn every_stored_flow_is_summarised_once_from_its_own_instance() {
    let connect = Connect::new();
    let (instance_id, instance_arn) = connect.create_instance(connect_managed("flows")).await;
    let (other_id, _) = connect.create_instance(connect_managed("other")).await;

    // (case, name, type, the Status sent, the status the summary must report)
    let cases: Vec<(String, String, &str, Option<&str>, &str)> = CONTACT_FLOW_TYPES
        .iter()
        .enumerate()
        .map(|(index, flow_type)| {
            let (sent, expected) = match index % 3 {
                0 => (None, CONTACT_FLOW_STATUS_PUBLISHED),
                1 => (
                    Some(CONTACT_FLOW_STATUS_PUBLISHED),
                    CONTACT_FLOW_STATUS_PUBLISHED,
                ),
                _ => (Some(CONTACT_FLOW_STATUS_SAVED), CONTACT_FLOW_STATUS_SAVED),
            };
            // A one-character name (the ContactFlowName lower bound) and a long one,
            // alternating, so the summary's Name is not keyed to one length.
            let name = if index % 2 == 0 {
                format!("f{index}")
            } else {
                format!("{}-{index}", "n".repeat(120))
            };
            (
                format!("{flow_type} as {sent:?} (row {index})"),
                name,
                *flow_type,
                sent,
                expected,
            )
        })
        .collect();

    let defaults = default_flow_ids(&connect, &instance_id).await;
    let mut expected: Vec<(String, String, String, String, String)> = Vec::new();
    for (case, name, flow_type, sent, status) in &cases {
        let (id, arn) = connect
            .store_flow(&instance_id, name, flow_type, *sent)
            .await;
        assert_eq!(
            arn,
            format!("{instance_arn}/contact-flow/{id}"),
            "case {case:?}: the stored flow's ARN"
        );
        expected.push((
            id,
            arn,
            name.clone(),
            (*flow_type).to_string(),
            (*status).to_string(),
        ));
    }

    // A flow of another instance must not leak into this instance's list.
    connect
        .store_flow(&other_id, "elsewhere", "CONTACT_FLOW", None)
        .await;

    let (status, listed) = connect
        .list_contact_flows(&instance_id, None, None, &[])
        .await;
    assert_eq!(status, 200, "ListContactFlows got {listed}");
    let summaries = listed["ContactFlowSummaryList"]
        .as_array()
        .expect("ContactFlowSummaryList");
    assert_eq!(
        summaries.len(),
        expected.len() + defaults.len(),
        "every stored flow — the instance's defaults and the ones created here — is \
         summarised exactly once, got {listed}"
    );

    for (id, arn, name, flow_type, flow_status) in &expected {
        let case = format!("{name} ({flow_type})");
        let summary = summaries
            .iter()
            .find(|summary| summary["Id"].as_str() == Some(id.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: {id} is missing from {listed}"));
        assert_eq!(summary["Arn"], json!(arn), "case {case:?}: Arn");
        assert_eq!(summary["Name"], json!(name), "case {case:?}: Name");
        assert_eq!(
            summary["ContactFlowType"],
            json!(flow_type),
            "case {case:?}: ContactFlowType"
        );
        assert_eq!(
            summary["ContactFlowState"],
            json!("ACTIVE"),
            "case {case:?}: a stored flow is ACTIVE"
        );
        assert_eq!(
            summary["ContactFlowStatus"],
            json!(flow_status),
            "case {case:?}: ContactFlowStatus"
        );
    }

    // The other instance reports its own flow — and its own defaults — and nothing else.
    let (_, elsewhere) = connect.list_contact_flows(&other_id, None, None, &[]).await;
    let names: BTreeSet<String> = ids_of(&elsewhere["ContactFlowSummaryList"], "Name")
        .into_iter()
        .collect();
    let wanted: BTreeSet<String> = DEFAULT_CONTACT_FLOWS
        .iter()
        .map(|default| default.name.to_string())
        .chain(["elsewhere".to_string()])
        .collect();
    assert_eq!(
        names, wanted,
        "a list is scoped to its own instance, got {elsewhere}"
    );

    // An instance nobody has created a flow in reports exactly its default flows, not a
    // refusal: the whole default inventory is covered by `instance_default_resources`.
    let (fresh_id, _) = connect.create_instance(connect_managed("fresh")).await;
    let (status, fresh) = connect.list_contact_flows(&fresh_id, None, None, &[]).await;
    assert_eq!(status, 200, "a fresh instance lists 200, got {fresh}");
    assert_eq!(
        ids_of(&fresh["ContactFlowSummaryList"], "Id").len(),
        DEFAULT_CONTACT_FLOWS.len(),
        "a fresh instance reports its default flows, got {fresh}"
    );
    assert_eq!(
        fresh["NextToken"],
        Value::Null,
        "a single-page list carries no NextToken, got {fresh}"
    );
}

/// RULE: `ListContactFlows` keeps only the flow types `contactFlowTypes` names, and
/// reports every stored flow when the parameter is absent.
#[tokio::test]
async fn list_filters_on_the_requested_contact_flow_types() {
    let connect = Connect::new();
    let (instance_id, _) = connect.create_instance(connect_managed("flows")).await;

    // One flow of every modelled type, so a filter's result is exactly the types it
    // names.
    for flow_type in CONTACT_FLOW_TYPES {
        connect
            .store_flow(&instance_id, &flow_type.to_lowercase(), flow_type, None)
            .await;
    }

    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("no filter", vec![]),
        ("one type", vec!["CONTACT_FLOW"]),
        ("another single type", vec!["CAMPAIGN"]),
        ("two types", vec!["AGENT_HOLD", "QUEUE_TRANSFER"]),
        (
            "three types",
            vec!["CUSTOMER_QUEUE", "CUSTOMER_HOLD", "OUTBOUND_WHISPER"],
        ),
        ("every type", CONTACT_FLOW_TYPES.to_vec()),
    ];

    for (case, filter) in cases {
        let (status, listed) = connect
            .list_contact_flows(&instance_id, None, None, &filter)
            .await;
        assert_eq!(status, 200, "case {case:?}: got {listed}");
        let types: BTreeSet<String> = ids_of(&listed["ContactFlowSummaryList"], "ContactFlowType")
            .into_iter()
            .collect();
        let wanted: BTreeSet<String> = if filter.is_empty() {
            CONTACT_FLOW_TYPES
                .iter()
                .map(|flow_type| (*flow_type).to_string())
                .collect()
        } else {
            filter
                .iter()
                .map(|flow_type| (*flow_type).to_string())
                .collect()
        };
        assert_eq!(
            types, wanted,
            "case {case:?}: expected exactly the requested types, got {listed}"
        );
    }
}

/// RULE: `ListContactFlows` walks the store in one deterministic order, returns at most
/// `MaxResults` items per page with a `NextToken` only while flows remain, and resuming
/// from that token yields every remaining flow exactly once.
#[tokio::test]
async fn list_pages_cover_every_flow_exactly_once() {
    // (how many flows exist, the MaxResults asked for) — page sizes below, equal to and
    // above the population, plus the unpaginated call.
    let cases: Vec<(usize, Option<i32>)> = vec![
        (1, Some(1)),
        (3, Some(1)),
        (5, Some(2)),
        (4, Some(4)),
        (4, Some(10)),
        (7, None),
    ];

    for (population, page_size) in cases {
        let case = format!("{population} flows, maxResults={page_size:?}");
        let connect = Connect::new();
        let (instance_id, _) = connect.create_instance(connect_managed("flows")).await;

        // The instance's default flows are stored flows too, so the pages must cover
        // them beside the ones this row creates.
        let mut created: BTreeSet<String> = default_flow_ids(&connect, &instance_id).await;
        for index in 0..population {
            let (id, _) = connect
                .store_flow(
                    &instance_id,
                    &format!("flow{index}"),
                    CONTACT_FLOW_TYPES[index % CONTACT_FLOW_TYPES.len()],
                    None,
                )
                .await;
            created.insert(id);
        }

        let (seen, pages) = walk_flows(&connect, &instance_id, page_size).await;
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<_>>(),
            created,
            "case {case:?}: the pages must cover exactly the created flows, got {seen:?}"
        );
        assert_eq!(
            seen.len(),
            created.len(),
            "case {case:?}: no flow may appear on two pages, got {seen:?}"
        );
        let mut sorted = seen.clone();
        sorted.sort();
        assert_eq!(
            seen, sorted,
            "case {case:?}: the pages must walk the store in id order, got {seen:?}"
        );
        let expected_pages =
            page_size.map_or(1, |size| created.len().div_ceil(size as usize).max(1));
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages, got {pages}"
        );
    }
}

/// RULE: a `ListContactFlows` that sends no `MaxResults` serves the page size its
/// `MaxResults` member documents ("The default MaxResult size is 100.") and reports a
/// `NextToken` only while flows remain beyond that page.
#[tokio::test]
async fn an_unpaginated_list_serves_the_documented_default_page_size() {
    let connect = Connect::new();
    let (instance_id, _) = connect.create_instance(connect_managed("flows")).await;

    // One flow past the default page size in total, the instance's own default flows
    // included: the rule is about the page size, so what matters is the population.
    let population = LIST_CONTACT_FLOWS_DEFAULT_PAGE_SIZE + 1;
    for index in 0..(population - DEFAULT_CONTACT_FLOWS.len()) {
        connect
            .store_flow(&instance_id, &format!("flow{index:04}"), "CONTACT_FLOW", None)
            .await;
    }

    let (status, listed) = connect
        .list_contact_flows(&instance_id, None, None, &[])
        .await;
    assert_eq!(status, 200, "ListContactFlows got {listed}");
    let first_page = ids_of(&listed["ContactFlowSummaryList"], "Id");
    assert_eq!(
        first_page.len(),
        LIST_CONTACT_FLOWS_DEFAULT_PAGE_SIZE,
        "case {:?}: expected {LIST_CONTACT_FLOWS_DEFAULT_PAGE_SIZE} flows, got {}",
        "the default page",
        first_page.len()
    );
    let token = listed["NextToken"]
        .as_str()
        .expect("a NextToken while flows remain")
        .to_string();

    let (status, rest) = connect
        .list_contact_flows(&instance_id, None, Some(&token), &[])
        .await;
    assert_eq!(status, 200, "resuming ListContactFlows got {rest}");
    let last_page = ids_of(&rest["ContactFlowSummaryList"], "Id");
    assert_eq!(
        last_page.len(),
        population - LIST_CONTACT_FLOWS_DEFAULT_PAGE_SIZE,
        "case {:?}: expected the remaining flows, got {last_page:?}",
        "the last page"
    );
    assert_eq!(
        rest["NextToken"],
        Value::Null,
        "case {:?}: the last page carries no NextToken, got {rest}",
        "the last page"
    );
    assert_eq!(
        first_page
            .iter()
            .chain(last_page.iter())
            .collect::<BTreeSet<_>>()
            .len(),
        population,
        "case {:?}: the two pages cover every flow exactly once",
        "both pages"
    );
}

/// RULE: `ListContactFlows` refuses what it cannot serve — an instance the store does
/// not hold is `ResourceNotFoundException` (404), a `NextToken` this service did not
/// issue is a 400 rather than a silent restart of the scan, and a `MaxResults` outside
/// the modelled range is a 400.
#[tokio::test]
async fn unknown_instances_bad_tokens_and_page_sizes_are_refused() {
    let connect = Connect::new();
    let (instance_id, _) = connect.create_instance(connect_managed("flows")).await;
    connect
        .store_flow(&instance_id, "support", "CONTACT_FLOW", None)
        .await;

    // Instance identifiers of the right modelled form that name nothing.
    for absent in [
        "11111111-2222-3333-4444-555555555555",
        "00000000-0000-4000-8000-000000000000",
    ] {
        let (status, body) = connect.list_contact_flows(absent, None, None, &[]).await;
        assert_eq!(
            status, 404,
            "case {absent:?}: listing an absent instance is a 404, got {status} {body}"
        );
        assert!(
            body["message"].is_string() || body["Message"].is_string(),
            "case {absent:?}: the error must carry a message, got {body}"
        );
    }

    // Tokens no page cursor this service issues could have produced: non-hex text, an
    // odd hex length, and hex that decodes to bytes which are not UTF-8.
    for token in ["not-a-token", "zz", "0", "ff"] {
        let (status, body) = connect
            .list_contact_flows(&instance_id, Some(10), Some(token), &[])
            .await;
        assert_eq!(
            status, 400,
            "case {token:?}: a foreign NextToken is refused, got {status} {body}"
        );
    }

    // MaxResult1000 range 1..1000: both published bounds, then one past each.
    for (case, max_results, expected) in [
        ("below the lower bound", 0, 400),
        ("the lower bound", 1, 200),
        ("the upper bound", 1000, 200),
        ("past the upper bound", 1001, 400),
    ] {
        let (status, body) = connect
            .list_contact_flows(&instance_id, Some(max_results), None, &[])
            .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} {body}"
        );
    }

    // A deleted instance takes its flows with it, so the list loses the instance too.
    assert_eq!(connect.delete_instance(&instance_id).await, 200, "DeleteInstance");
    assert_eq!(
        connect
            .list_contact_flows(&instance_id, None, None, &[])
            .await
            .0,
        404,
        "a deleted instance's flows are gone with it"
    );
}
