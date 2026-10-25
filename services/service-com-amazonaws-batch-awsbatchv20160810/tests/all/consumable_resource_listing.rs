// SPDX-License-Identifier: CC0-1.0

//! `ListConsumableResources`, end to end over the wire.
//!
//! Every case drives a real restJson1 request through the router the emulator mounts, so
//! what is asserted is the response a `batch` client gets.
//!
//! The RULES under test, each asserted over a SET of inputs rather than one request:
//!
//! * a listing reports the consumable resources this service holds, every summary member
//!   read from the persisted record and only the members the summary models, in one stable
//!   order; an emulator holding none answers an empty list rather than a refusal;
//! * a `filters` entry narrows it: `CONSUMABLE_RESOURCE_NAME` matches the name
//!   case-insensitively, with a trailing `*` matching any name that begins with the text
//!   before it, and any of an entry's values matching; a filter name the operation does
//!   not document is the modelled `ClientException` (HTTP 400);
//! * paging with `maxResults`/`nextToken` sees every resource exactly once in the unpaged
//!   order, `maxResults` is accepted on both published bounds and refused one past either,
//!   a request that names none is still bounded at the documented default, and a
//!   `nextToken` that resumes nothing is the modelled `ClientException`;
//! * a listing is a READ: two listings at the same instant answer identically, and what it
//!   reports follows the writes — a create adds a row, an update moves that row's
//!   `totalQuantity`, a delete removes it.

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::state::{
    MAX_LIST_CONSUMABLE_RESOURCES_RESULTS, MIN_LIST_CONSUMABLE_RESOURCES_RESULTS,
};

use crate::job_lifecycle::{arn, error_code, error_message, Batch};

/// `ListConsumableResources`'s route (`smithy_query(query="operation",
/// operation="ListConsumableResources")`: POST /v1/listconsumableresources).
const LIST: &str = "/v1/listconsumableresources";

/// The `resourceType` values a consumable resource may carry. Source: the
/// `CreateConsumableResourceRequest$resourceType` doc (`smithy_query(query="doc",
/// shape="CreateConsumableResourceRequest", member="resourceType")`): "Can be one of:
/// REPLENISHABLE (default), NON_REPLENISHABLE".
const DOCUMENTED_RESOURCE_TYPES: [&str; 2] = ["REPLENISHABLE", "NON_REPLENISHABLE"];

/// `ListConsumableResources` over the wire: the status and the decoded body.
async fn list(batch: &Batch, body: Value) -> (u16, Value) {
    batch.call(LIST, body).await
}

/// `ListConsumableResources` over the wire, requiring the 200.
async fn listed(batch: &Batch, case: &str, body: Value) -> Value {
    let (status, decoded) = list(batch, body).await;
    assert_eq!(
        status, 200,
        "case {case}: expected ListConsumableResources to answer 200, got {status} {decoded}"
    );
    decoded
}

/// The `consumableResourceName`s of a listing, in the order it answered them.
fn names(body: &Value) -> Vec<String> {
    body["consumableResources"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| {
            summary["consumableResourceName"]
                .as_str()
                .unwrap_or("<no consumableResourceName>")
                .to_string()
        })
        .collect()
}

/// One consumable resource this suite lists.
struct Resource {
    name: &'static str,
    total_quantity: i64,
    resource_type: &'static str,
}

/// The table this suite lists: rows differ in the shape, the case and the length of the
/// name, in `totalQuantity` (its published lower bound of 0, a small count, and the
/// largest `Long` the member admits) and in both documented `resourceType` values.
fn resources() -> Vec<Resource> {
    vec![
        Resource {
            name: "cr-alpha",
            total_quantity: 0,
            resource_type: DOCUMENTED_RESOURCE_TYPES[0],
        },
        Resource {
            name: "CR-Alpha-2",
            total_quantity: 7,
            resource_type: DOCUMENTED_RESOURCE_TYPES[1],
        },
        Resource {
            name: "cr-beta",
            total_quantity: 1_000,
            resource_type: DOCUMENTED_RESOURCE_TYPES[0],
        },
        Resource {
            name: "z",
            total_quantity: i64::MAX,
            resource_type: DOCUMENTED_RESOURCE_TYPES[1],
        },
    ]
}

/// Create one consumable resource over the wire.
async fn create(batch: &Batch, resource: &Resource) {
    batch
        .ok(
            "/v1/createconsumableresource",
            json!({
                "consumableResourceName": resource.name,
                "totalQuantity": resource.total_quantity,
                "resourceType": resource.resource_type,
            }),
        )
        .await;
}

/// Create every row of [`resources`].
async fn fixture(batch: &Batch) {
    for resource in resources() {
        create(batch, &resource).await;
    }
}

/// The names of [`resources`], in the order a listing must answer them.
fn stored_order() -> Vec<String> {
    let mut names: Vec<String> = resources()
        .iter()
        .map(|resource| resource.name.to_string())
        .collect();
    names.sort();
    names
}

/// RULE: a listing reports every consumable resource this service holds, in one stable
/// order, and an emulator holding none answers an empty list.
#[tokio::test]
async fn a_listing_reports_every_stored_consumable_resource() {
    let batch = Batch::new();

    // Nothing stored yet: the listing is EMPTY, never a refusal.
    let empty = listed(&batch, "an empty store", json!({})).await;
    assert_eq!(
        names(&empty),
        Vec::<String>::new(),
        "case an empty store: expected no consumable resource, got {empty}"
    );
    assert!(
        empty.get("nextToken").is_none(),
        "case an empty store: a page that ends the results carries no nextToken, got {empty}"
    );

    // Each create adds a row, and every earlier row stays listed.
    let mut created: Vec<String> = Vec::new();
    for resource in resources() {
        create(&batch, &resource).await;
        created.push(resource.name.to_string());
        created.sort();
        let case = format!("after creating {:?}", resource.name);
        let body = listed(&batch, &case, json!({})).await;
        assert_eq!(
            names(&body),
            created,
            "case {case}: expected {created:?}, got {body}"
        );
        // A read: the same request at the same instant answers identically.
        let again = listed(&batch, &case, json!({})).await;
        assert_eq!(
            again, body,
            "case {case}: listing twice must answer identically, got {again} then {body}"
        );
    }
}

/// RULE: every summary member is read from the persisted record, and the summary carries
/// only the members the model declares for it.
#[tokio::test]
async fn a_summary_reports_the_persisted_resource_and_nothing_else() {
    let batch = Batch::new();
    fixture(&batch).await;

    let body = listed(&batch, "every member", json!({})).await;
    let summaries = body["consumableResources"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        summaries.len(),
        resources().len(),
        "case every member: expected every created resource, got {body}"
    );

    for summary in &summaries {
        let case = summary["consumableResourceName"]
            .as_str()
            .unwrap_or("<no consumableResourceName>")
            .to_string();
        let row = resources()
            .into_iter()
            .find(|resource| resource.name == case)
            .unwrap_or_else(|| {
                panic!("case {case}: the listing answered a resource nothing created, got {summary}")
            });
        assert_eq!(
            summary["consumableResourceArn"].as_str(),
            Some(arn(&format!("consumable-resource/{}", row.name)).as_str()),
            "case {case}: expected the ARN the create stored, got {summary}"
        );
        assert_eq!(
            summary["totalQuantity"].as_i64(),
            Some(row.total_quantity),
            "case {case}: expected the persisted totalQuantity {}, got {summary}",
            row.total_quantity
        );
        assert_eq!(
            summary["inUseQuantity"].as_i64(),
            Some(0),
            "case {case}: nothing holds these resources, so inUseQuantity is 0, got {summary}"
        );
        assert_eq!(
            summary["resourceType"].as_str(),
            Some(row.resource_type),
            "case {case}: expected the persisted resourceType {:?}, got {summary}",
            row.resource_type
        );
        // The summary models these five members only (`smithy_query(query="shape",
        // shape="ConsumableResourceSummary")`), so nothing beyond them is invented.
        let modelled = [
            "consumableResourceArn",
            "consumableResourceName",
            "totalQuantity",
            "inUseQuantity",
            "resourceType",
        ];
        let object = summary
            .as_object()
            .unwrap_or_else(|| panic!("case {case}: expected a summary object, got {summary}"));
        for member in object.keys() {
            assert!(
                modelled.contains(&member.as_str()),
                "case {case}: {member} is not a member ConsumableResourceSummary models, \
                 got {summary}"
            );
        }
    }
}

/// RULE: a `CONSUMABLE_RESOURCE_NAME` filter selects by name, case-insensitively, with a
/// trailing `*` matching any name that begins with the text before it and any of the
/// entry's values matching.
#[tokio::test]
async fn a_listing_honours_the_documented_name_filter() {
    let batch = Batch::new();
    fixture(&batch).await;

    // Rows differ in the case of the value, in whether it carries a trailing `*`, in how
    // many values the entry holds, and in whether anything matches at all.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "an exact match",
            json!([{"name": "CONSUMABLE_RESOURCE_NAME", "values": ["cr-beta"]}]),
            vec!["cr-beta"],
        ),
        (
            "an exact match in the other case",
            json!([{"name": "CONSUMABLE_RESOURCE_NAME", "values": ["CR-BETA"]}]),
            vec!["cr-beta"],
        ),
        (
            "a case-insensitive match of a name stored mixed-case",
            json!([{"name": "CONSUMABLE_RESOURCE_NAME", "values": ["cr-alpha-2"]}]),
            vec!["CR-Alpha-2"],
        ),
        (
            "a trailing * matches a prefix, across cases",
            json!([{"name": "CONSUMABLE_RESOURCE_NAME", "values": ["cr-alpha*"]}]),
            vec!["CR-Alpha-2", "cr-alpha"],
        ),
        (
            "a trailing * on its own matches everything",
            json!([{"name": "CONSUMABLE_RESOURCE_NAME", "values": ["*"]}]),
            vec!["CR-Alpha-2", "cr-alpha", "cr-beta", "z"],
        ),
        (
            "several values match any of them",
            json!([{"name": "CONSUMABLE_RESOURCE_NAME", "values": ["z", "cr-beta"]}]),
            vec!["cr-beta", "z"],
        ),
        (
            "a value nothing answers to selects nothing",
            json!([{"name": "CONSUMABLE_RESOURCE_NAME", "values": ["cr-absent"]}]),
            Vec::new(),
        ),
        (
            "a prefix nothing begins with selects nothing",
            json!([{"name": "CONSUMABLE_RESOURCE_NAME", "values": ["absent*"]}]),
            Vec::new(),
        ),
    ];
    for (case, filters, expected) in cases {
        let body = listed(&batch, case, json!({"filters": filters})).await;
        assert_eq!(
            names(&body),
            expected,
            "case {case}: expected {expected:?}, got {body}"
        );
    }
}

/// RULE: paging with `maxResults`/`nextToken` sees every resource exactly once in the
/// unpaged order, and `maxResults` is accepted on both published bounds.
#[tokio::test]
async fn paging_sees_every_consumable_resource_exactly_once() {
    let batch = Batch::new();
    fixture(&batch).await;

    let expected = stored_order();
    let all = listed(&batch, "unpaged", json!({})).await;
    assert_eq!(
        names(&all),
        expected,
        "case unpaged: expected every created resource, got {all}"
    );

    // Page sizes differ: one at a time, a size that does not divide the total, the exact
    // total, and both published bounds of `maxResults` (1 and 100).
    for page_size in [
        MIN_LIST_CONSUMABLE_RESOURCES_RESULTS,
        3,
        i32::try_from(expected.len()).expect("the fixture fits in an i32"),
        MAX_LIST_CONSUMABLE_RESOURCES_RESULTS,
    ] {
        let case = format!("maxResults {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"maxResults": page_size});
            if let Some(resume) = token.as_deref() {
                request["nextToken"] = json!(resume);
            }
            let body = listed(&batch, &case, request).await;
            let page = names(&body);
            assert!(
                page.len() <= usize::try_from(page_size).expect("a positive page size"),
                "case {case}: a page holds at most maxResults resources, got {body}"
            );
            seen.extend(page);
            token = body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {case}: paging did not terminate after {pages} pages, last token {token:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, expected,
            "case {case}: paging must see every resource exactly once in the unpaged order, \
             got {seen:?}"
        );
    }
}

/// RULE: a request that names no `maxResults` is still bounded at the documented default
/// ("returns up to 100 results and a nextToken value if applicable"), and paging from that
/// first page sees the rest.
#[tokio::test]
async fn a_listing_that_names_no_page_size_is_bounded_at_the_documented_default() {
    let batch = Batch::new();
    let default = usize::try_from(MAX_LIST_CONSUMABLE_RESOURCES_RESULTS)
        .expect("the documented default is positive");
    // One past the default, so the first page cannot hold them all.
    let total = default + 1;
    for index in 0..total {
        // Zero-padded, so the stored order is the order these names were created in.
        batch
            .ok(
                "/v1/createconsumableresource",
                json!({"consumableResourceName": format!("paged-{index:04}"), "totalQuantity": 1}),
            )
            .await;
    }

    let first = listed(&batch, "no maxResults", json!({})).await;
    let page = names(&first);
    assert_eq!(
        page.len(),
        default,
        "case no maxResults: expected the documented default page size {default}, got {}",
        page.len()
    );
    let token = first
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    assert!(
        token.is_some(),
        "case no maxResults: a page that leaves results carries a nextToken, got {first}"
    );

    let rest = listed(
        &batch,
        "resuming the default page",
        json!({"nextToken": token.expect("the first page carries a token")}),
    )
    .await;
    let mut seen = page;
    seen.extend(names(&rest));
    assert_eq!(
        seen.len(),
        total,
        "case resuming the default page: expected every resource exactly once, got {} of \
         {total}",
        seen.len()
    );
    assert!(
        rest.get("nextToken").is_none(),
        "case resuming the default page: the last page carries no nextToken, got {rest}"
    );
}

/// RULE: a request this operation cannot answer is the modelled `ClientException` (HTTP
/// 400): a filter name it does not document, a `maxResults` outside the published 1..100,
/// and a `nextToken` that resumes nothing.
#[tokio::test]
async fn a_listing_refuses_what_it_cannot_answer() {
    let batch = Batch::new();
    fixture(&batch).await;

    // Rows differ in WHICH part of the request is unusable; each must be the modelled
    // 400, never a 5xx or a 501.
    let cases: Vec<(&str, Value)> = vec![
        (
            "a filter name the operation does not document",
            json!({"filters": [{"name": "JOB_NAME", "values": ["cr-beta"]}]}),
        ),
        (
            "a filter entry carrying no name",
            json!({"filters": [{"values": ["cr-beta"]}]}),
        ),
        (
            "the documented filter name in the wrong case",
            json!({"filters": [{"name": "consumable_resource_name", "values": ["cr-beta"]}]}),
        ),
        (
            "maxResults one below the published minimum",
            json!({"maxResults": MIN_LIST_CONSUMABLE_RESOURCES_RESULTS - 1}),
        ),
        (
            "maxResults one past the published maximum",
            json!({"maxResults": MAX_LIST_CONSUMABLE_RESOURCES_RESULTS + 1}),
        ),
        ("a negative maxResults", json!({"maxResults": -1})),
        (
            "a nextToken that resumes nothing",
            json!({"maxResults": 1, "nextToken": "not-a-token"}),
        ),
        (
            "a nextToken past the end of the results",
            json!({"nextToken": "9999"}),
        ),
    ];
    for (case, request) in cases {
        let (status, body) = list(&batch, request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected the modelled ClientException with HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ClientException",
            "case {case}: expected ClientException, got {body} ({})",
            error_message(&body)
        );
    }

    // Both published bounds of `maxResults` are ACCEPTED, so the refusals above are the
    // bound and not the member.
    for accepted in [
        MIN_LIST_CONSUMABLE_RESOURCES_RESULTS,
        MAX_LIST_CONSUMABLE_RESOURCES_RESULTS,
    ] {
        let case = format!("maxResults {accepted}");
        listed(&batch, &case, json!({"maxResults": accepted})).await;
    }
}

/// RULE: a listing follows the writes — an update moves the row's `totalQuantity` and a
/// delete removes the row — and a listing itself changes nothing a later one sees.
#[tokio::test]
async fn a_listing_follows_the_writes_and_mutates_nothing() {
    let batch = Batch::new();
    fixture(&batch).await;

    /// The `totalQuantity` a listing reports for `name`.
    fn quantity_of(body: &Value, name: &str) -> Option<i64> {
        body["consumableResources"]
            .as_array()?
            .iter()
            .find(|summary| summary["consumableResourceName"].as_str() == Some(name))?
            .get("totalQuantity")?
            .as_i64()
    }

    // Rows differ in the documented `operation` and in the quantity it moves the total by
    // (`smithy_query(query="doc", shape="UpdateConsumableResourceRequest",
    // member="operation")`: SET, ADD, REMOVE).
    let cases: [(&str, i64, i64); 3] = [("SET", 42, 42), ("ADD", 8, 50), ("REMOVE", 50, 0)];
    for (operation, quantity, expected) in cases {
        let case = format!("{operation} {quantity}");
        batch
            .ok(
                "/v1/updateconsumableresource",
                json!({
                    "consumableResource": "cr-beta",
                    "operation": operation,
                    "quantity": quantity,
                }),
            )
            .await;
        let body = listed(&batch, &case, json!({})).await;
        assert_eq!(
            quantity_of(&body, "cr-beta"),
            Some(expected),
            "case {case}: expected the updated totalQuantity {expected}, got {body}"
        );
        // Every other row is untouched by that update.
        assert_eq!(
            names(&body),
            stored_order(),
            "case {case}: an update changes one row only, got {body}"
        );
    }

    // A delete removes the row, and a repeated listing still answers identically.
    batch
        .ok(
            "/v1/deleteconsumableresource",
            json!({"consumableResource": "cr-beta"}),
        )
        .await;
    let after = listed(&batch, "after the delete", json!({})).await;
    let expected: Vec<String> = stored_order()
        .into_iter()
        .filter(|name| name != "cr-beta")
        .collect();
    assert_eq!(
        names(&after),
        expected,
        "case after the delete: expected {expected:?}, got {after}"
    );
    let again = listed(&batch, "after the delete, again", json!({})).await;
    assert_eq!(
        again, after,
        "case after the delete, again: a listing must not change what a later listing \
         sees, got {again} then {after}"
    );
}
