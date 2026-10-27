// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how EVERY paginated read of this service answers a `NextToken`
//! that names no page of what it was replayed against.
//!
//! RULE pinned here: a cursor is bound to the selection it was issued for, so a stale or
//! foreign token is refused with HTTP 400 `ValidationException` whose `Message` is
//! "NextToken does not name a page of <the collection the read reports>", whose `Reason`
//! is `FIELD_VALIDATION_FAILED` and whose single `Fields` entry is named `NextToken` and
//! says "must be a token returned by an earlier <Operation>", followed by
//! " for the same <the request members the cursor is scoped to>" when the read binds any.
//! One wording, one shape, every read.
//!
//! The rows span the paginated reads of this service and differ along how the cursor
//! travels (query string vs. body), whether the read binds a selecting member, and which
//! collection the token is replayed against.

use serde_json::{json, Value};

use super::configuration_lifecycle::{cloud_formation, Emulator, Reply};

/// A well-formed `UuidNextToken` (len 36, `^[a-f0-9]{8}-...$`, from
/// `smithy_query(query="shape",shape="UuidNextToken")`) that no read of this service
/// ever issued, so it names a page of nothing while still passing input validation.
const FOREIGN_TOKEN: &str = "0a1b2c3d-0000-0000-0000-000000000000";

/// One paginated read, with the cursor refusal it must answer.
struct PaginatedRead {
    /// The operation the refusal's field message names.
    operation: &'static str,
    /// The collection the refusal's message names.
    collection: &'static str,
    /// The request members the cursor is scoped to, or `None` for a read that binds no
    /// selecting member and whose cursor is therefore bound to the operation alone.
    scoped_by: Option<&'static str>,
    method: &'static str,
    path: String,
    body: Option<Value>,
}

impl PaginatedRead {
    /// The `Message` the refusal must carry.
    fn message(&self) -> String {
        format!("NextToken does not name a page of {}", self.collection)
    }

    /// The `Fields[0].Message` the refusal must carry.
    fn field_message(&self) -> String {
        match self.scoped_by {
            Some(members) => format!(
                "must be a token returned by an earlier {} for the same {members}",
                self.operation
            ),
            None => format!("must be a token returned by an earlier {}", self.operation),
        }
    }
}

/// Every paginated read, each replayed with `token`. The HTTP bindings are each
/// operation's own, from
/// `smithy_query(query="digest",model_path="services/service-com-amazonaws-devopsguru-capstonecontrolplaneservice/smithy-model/com-amazonaws-devopsguru-capstonecontrolplaneservice.json")`.
fn paginated_reads(token: &str) -> Vec<PaginatedRead> {
    vec![
        PaginatedRead {
            operation: "GetResourceCollection",
            collection: "this resource collection",
            scoped_by: Some("ResourceCollectionType"),
            method: "GET",
            path: format!("/resource-collections/AWS_CLOUD_FORMATION?NextToken={token}"),
            body: None,
        },
        PaginatedRead {
            operation: "DescribeResourceCollectionHealth",
            collection: "this resource collection's health",
            scoped_by: Some("ResourceCollectionType"),
            method: "GET",
            path: format!("/accounts/health/resource-collection/AWS_TAGS?NextToken={token}"),
            body: None,
        },
        PaginatedRead {
            operation: "DescribeOrganizationResourceCollectionHealth",
            collection: "this organization resource collection's health",
            scoped_by: Some(
                "OrganizationResourceCollectionType, AccountIds and OrganizationalUnitIds",
            ),
            method: "POST",
            path: "/organization/health/resource-collection".to_string(),
            body: Some(json!({
                "OrganizationResourceCollectionType": "AWS_ACCOUNT",
                "NextToken": token,
            })),
        },
        PaginatedRead {
            operation: "ListMonitoredResources",
            collection: "the monitored resources these filters select",
            scoped_by: Some("Filters"),
            method: "POST",
            path: "/monitoredResources".to_string(),
            body: Some(json!({ "NextToken": token })),
        },
        PaginatedRead {
            operation: "ListNotificationChannels",
            collection: "this account's notification channels",
            scoped_by: None,
            method: "POST",
            path: "/channels".to_string(),
            body: Some(json!({ "NextToken": token })),
        },
        PaginatedRead {
            operation: "GetCostEstimation",
            collection: "this cost estimation",
            scoped_by: None,
            method: "GET",
            path: format!("/cost-estimation?NextToken={token}"),
            body: None,
        },
        // The reads that already answered through the shared refusal, so the table
        // covers both sides of the one rule.
        PaginatedRead {
            operation: "ListInsights",
            collection: "the insights this status filter selects",
            scoped_by: Some("StatusFilter"),
            method: "POST",
            path: "/insights".to_string(),
            body: Some(json!({
                "StatusFilter": { "Ongoing": { "Type": "PROACTIVE" } },
                "NextToken": token,
            })),
        },
        PaginatedRead {
            operation: "ListEvents",
            collection: "the events these filters select",
            scoped_by: Some("Filters"),
            method: "POST",
            path: "/events".to_string(),
            body: Some(json!({ "Filters": {}, "NextToken": token })),
        },
    ]
}

/// The `Fields` entries of a refusal, as `(Name, Message)` pairs in the order reported.
fn fields(reply: &Reply) -> Vec<(String, String)> {
    reply
        .at("/Fields")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    let member = |name: &str| {
                        item.get(name)
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    };
                    (member("Name"), member("Message"))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// An emulator in the state every row of the table needs: the organization administered
/// by the account it serves (without which the organization-wide read is refused before
/// it ever resolves a cursor) and some coverage to page over.
async fn emulator() -> Emulator {
    let emulator = Emulator::new();
    emulator.delegate_organization_administration(engine::emulator_account_id());
    let reply = emulator
        .update_resource_collection("ADD", cloud_formation(&["cursor-stack"]))
        .await;
    assert_eq!(
        reply.status, 200,
        "the coverage setup step must succeed, got {}",
        reply.raw
    );
    emulator
}

/// RULE: every paginated read answers a stale or foreign cursor the same way — HTTP 400
/// `ValidationException`, `Reason` `FIELD_VALIDATION_FAILED`, and exactly one
/// `ValidationExceptionField` named `NextToken` whose message names the issuing
/// operation and the members the cursor is scoped to.
#[tokio::test]
async fn every_paginated_read_refuses_a_foreign_cursor_with_the_same_wording() {
    let emulator = emulator().await;

    for read in paginated_reads(FOREIGN_TOKEN) {
        let case = read.operation;
        let refused = emulator
            .call(read.method, &read.path, read.body.as_ref())
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} {}",
            refused.status, refused.raw
        );
        assert!(
            refused.names_error("ValidationException"),
            "case {case:?}: expected ValidationException, got {}",
            refused.raw
        );
        assert_eq!(
            refused.at("/Message").and_then(Value::as_str),
            Some(read.message().as_str()),
            "case {case:?}: expected the shared cursor refusal message, got {}",
            refused.raw
        );
        assert_eq!(
            refused.at("/Reason").and_then(Value::as_str),
            Some("FIELD_VALIDATION_FAILED"),
            "case {case:?}: expected Reason FIELD_VALIDATION_FAILED, got {}",
            refused.raw
        );
        assert_eq!(
            fields(&refused),
            vec![("NextToken".to_string(), read.field_message())],
            "case {case:?}: expected one NextToken field carrying the shared wording, got {}",
            refused.raw
        );
    }
}

/// RULE: the refusal is about the cursor alone, so it changes nothing a later caller
/// sees — a read that follows a refused one answers its first page exactly as it would
/// have before.
#[tokio::test]
async fn a_refused_cursor_leaves_the_store_as_it_was() {
    let emulator = emulator().await;

    let before = emulator
        .get_resource_collection("AWS_CLOUD_FORMATION", None)
        .await;
    assert_eq!(
        before.status, 200,
        "case {:?}: the first read must succeed, got {}",
        "before the refusals", before.raw
    );

    for read in paginated_reads(FOREIGN_TOKEN) {
        let refused = emulator
            .call(read.method, &read.path, read.body.as_ref())
            .await;
        assert_eq!(
            refused.status, 400,
            "case {:?}: expected HTTP 400, got {} {}",
            read.operation, refused.status, refused.raw
        );
    }

    let after = emulator
        .get_resource_collection("AWS_CLOUD_FORMATION", None)
        .await;
    assert_eq!(
        after.body, before.body,
        "case {:?}: expected the refusals to leave the reported collection unchanged, got {}",
        "after the refusals", after.raw
    );
}
