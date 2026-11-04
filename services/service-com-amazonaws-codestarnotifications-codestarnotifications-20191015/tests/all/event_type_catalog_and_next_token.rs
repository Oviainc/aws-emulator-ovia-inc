// SPDX-License-Identifier: CC0-1.0

//! Two published facts of this service's paginated listings, each asserted as a RULE over
//! a TABLE of inputs rather than as one recorded request:
//!
//! * every catalogued event type is published with its own name AND at its own position
//!   in the catalog — the position that decides which page it lands on — and the same
//!   published values are what `DescribeNotificationRule` resolves a stored id through;
//! * a `NextToken` a listing cannot decode is refused by naming the rejected input member
//!   and echoing the token the request submitted, identically on every paginated
//!   operation, while a token a page really issued keeps paging.
//!
//! The harness (`Emulator`, `error_code`, `message`, …) is the one
//! `notification_rule_lifecycle` builds: one shared engine behind the same
//! `MultiServiceRouter` dispatch the emulator serves production requests with.

use serde_json::{json, Value};

use crate::notification_rule_lifecycle::{error_code, message, string, Emulator};

/// One row of the published-catalog table: an event type's id, the position the catalog
/// publishes it at, and the three values every surface reports for it.
struct CatalogRow {
    position: usize,
    event_type_id: &'static str,
    service_name: &'static str,
    resource_type: &'static str,
    event_type_name: &'static str,
}

/// The rows this suite pins, written by hand from the catalog's published source: the
/// event-type list under "Notification concepts" in the Developer Tools Console User
/// Guide, the page `EventTypeSummary$EventTypeId`'s own documentation links to
/// (<https://docs.aws.amazon.com/codestar-notifications/latest/userguide/concepts.html#concepts-api>),
/// whose "Event type" column is the `EventTypeName` the API reports.
///
/// The rows differ along what the rule generalises over: the owning service, the resource
/// type, how the name's state word is spelled (one word, hyphenated, several words), and
/// the position in the published order — the first entry, entries in the middle, an entry
/// that does not sit beside the other entries of its own service, and the last entry.
const CATALOG_ROWS: &[CatalogRow] = &[
    CatalogRow {
        position: 0,
        event_type_id: "codecommit-repository-comments-on-commits",
        service_name: "CodeCommit",
        resource_type: "Repository",
        event_type_name: "Comments: On commits",
    },
    CatalogRow {
        position: 3,
        event_type_id: "codecommit-repository-approvals-rule-override",
        service_name: "CodeCommit",
        resource_type: "Repository",
        event_type_name: "Approvals: Rule override",
    },
    CatalogRow {
        position: 7,
        event_type_id: "codecommit-repository-pull-request-source-updated",
        service_name: "CodeCommit",
        resource_type: "Repository",
        event_type_name: "Pull request: Source updated",
    },
    CatalogRow {
        position: 11,
        event_type_id: "codebuild-project-build-state-failed",
        service_name: "CodeBuild",
        resource_type: "Project",
        event_type_name: "Build state: Failed",
    },
    CatalogRow {
        position: 18,
        event_type_id: "codedeploy-application-deployment-started",
        service_name: "CodeDeploy",
        resource_type: "Application",
        event_type_name: "Deployment: Started",
    },
    CatalogRow {
        position: 27,
        event_type_id: "codepipeline-pipeline-manual-approval-succeeded",
        service_name: "CodePipeline",
        resource_type: "Pipeline",
        event_type_name: "Manual approval: Succeeded",
    },
    CatalogRow {
        position: 28,
        event_type_id: "codebuild-project-build-state-in-progress",
        service_name: "CodeBuild",
        resource_type: "Project",
        event_type_name: "Build state: In-progress",
    },
    CatalogRow {
        position: 37,
        event_type_id: "codepipeline-pipeline-stage-execution-failed",
        service_name: "CodePipeline",
        resource_type: "Pipeline",
        event_type_name: "Stage execution: Failed",
    },
];

/// The four members of one `EventTypeSummary`, as a row to compare against the table.
fn summary_tuple(summary: &Value) -> (String, String, String, String) {
    (
        string(summary, "EventTypeId"),
        string(summary, "ServiceName"),
        string(summary, "ResourceType"),
        string(summary, "EventTypeName"),
    )
}

fn expected_tuple(row: &CatalogRow) -> (String, String, String, String) {
    (
        row.event_type_id.to_string(),
        row.service_name.to_string(),
        row.resource_type.to_string(),
        row.event_type_name.to_string(),
    )
}

fn summaries(body: &Value, member: &str) -> Vec<Value> {
    body.get(member)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// RULE: the catalog's published values are reference data reported verbatim — each entry
/// carries the name the service publishes for it, at the position the published catalog
/// gives it, which is the stable total order the listing walks.
#[tokio::test]
async fn each_event_type_is_published_with_its_own_name_at_its_own_position() {
    let emulator = Emulator::new();
    let body = emulator
        .ok("whole catalog", "/listEventTypes", &json!({ "MaxResults": 100 }))
        .await;
    let listed = summaries(&body, "EventTypes");

    for row in CATALOG_ROWS {
        let case = row.event_type_id;
        let Some(summary) = listed.get(row.position) else {
            panic!(
                "case {case:?}: expected an entry at published position {}, got {} entries",
                row.position,
                listed.len()
            );
        };
        assert_eq!(
            summary_tuple(summary),
            expected_tuple(row),
            "case {case:?}: expected the published entry at position {}, got {summary}",
            row.position
        );
    }
}

/// RULE: because the published position is the order pages walk, an entry lands on the
/// page its position implies, at the offset inside that page its position implies — for
/// ANY page size.
///
/// The page sizes divide the catalog differently (and include a size that puts the rows
/// on several different pages), so a row's page number is a consequence of its position
/// rather than of one chosen size.
#[tokio::test]
async fn an_entry_lands_on_the_page_its_published_position_implies() {
    let emulator = Emulator::new();

    for page_size in [5_usize, 10, 20] {
        for row in CATALOG_ROWS {
            let case = format!("{} at page size {page_size}", row.event_type_id);
            let wanted_page = row.position / page_size;
            let offset_in_page = row.position % page_size;

            // Walk the pages from the first, following only the tokens the service issues,
            // and check the entry is on NO page before the one its position implies.
            let mut token: Option<String> = None;
            let mut page_body = Value::Null;
            for page in 0..=wanted_page {
                let mut request = json!({ "MaxResults": page_size });
                if let Some(next) = token.clone() {
                    request["NextToken"] = Value::String(next);
                }
                page_body = emulator.ok(&case, "/listEventTypes", &request).await;
                token = page_body
                    .get("NextToken")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                assert!(
                    page == wanted_page || token.is_some(),
                    "case {case:?}: expected a token after page {page}, got {page_body}"
                );
                if page < wanted_page {
                    let ids: Vec<String> = summaries(&page_body, "EventTypes")
                        .iter()
                        .map(|summary| string(summary, "EventTypeId"))
                        .collect();
                    assert!(
                        !ids.iter().any(|id| id == row.event_type_id),
                        "case {case:?}: expected it on page {wanted_page}, found it on page \
                         {page}: {ids:?}"
                    );
                }
            }

            let page = summaries(&page_body, "EventTypes");
            let Some(summary) = page.get(offset_in_page) else {
                panic!(
                    "case {case:?}: expected the entry at offset {offset_in_page} of page \
                     {wanted_page}, got {} entries",
                    page.len()
                );
            };
            assert_eq!(
                summary_tuple(summary),
                expected_tuple(row),
                "case {case:?}: expected it at offset {offset_in_page} of page \
                 {wanted_page}, got {summary}"
            );
        }
    }
}

/// RULE: the catalog is the ONE place an event type's published values come from, so
/// `DescribeNotificationRule` resolves a rule's stored ids through the same table and
/// reports the same name, service and resource type the listing publishes.
#[tokio::test]
async fn describe_resolves_a_stored_id_to_the_same_published_values() {
    let emulator = Emulator::new();
    let ids: Vec<&str> = CATALOG_ROWS.iter().map(|row| row.event_type_id).collect();
    let created = emulator
        .ok(
            "catalog round trip",
            "/createNotificationRule",
            &json!({
                "Name": "catalog-position-rule",
                "Resource": "arn:aws:codecommit:us-east-1:123456789012:MyRepo",
                "DetailType": "FULL",
                "EventTypeIds": ids,
                "Targets": [],
            }),
        )
        .await;
    let described = emulator
        .ok(
            "catalog round trip",
            "/describeNotificationRule",
            &json!({ "Arn": string(&created, "Arn") }),
        )
        .await;
    let resolved = summaries(&described, "EventTypes");

    for (index, row) in CATALOG_ROWS.iter().enumerate() {
        let case = row.event_type_id;
        let Some(summary) = resolved.get(index) else {
            panic!(
                "case {case:?}: expected a resolved summary at {index}, got {} entries",
                resolved.len()
            );
        };
        assert_eq!(
            summary_tuple(summary),
            expected_tuple(row),
            "case {case:?}: expected the catalog's published values, got {summary}"
        );
    }
}

/// Every paginated operation of this service, by the path a request reaches it at.
const PAGINATED_PATHS: &[&str] = &["/listEventTypes", "/listNotificationRules", "/listTargets"];

/// RULE: a `NextToken` a listing cannot decode is refused with HTTP 400
/// `InvalidNextTokenException` whose message names the rejected input member and echoes
/// the token the request submitted — the same text on every paginated operation.
///
/// The tokens all satisfy `NextToken`'s `^[\w/+=]+$` pattern, so each reaches the handler,
/// and differ along what the rule generalises over: length (including a single
/// character), whether the value is hexadecimal at all, whether it decodes to text this
/// service could have minted, and whether it carries the `/+=` characters the pattern
/// admits.
#[tokio::test]
async fn an_undecodable_token_names_the_parameter_and_echoes_the_token() {
    let emulator = Emulator::new();
    // Give the rule and target listings something to page over, so the refusal is the
    // token's and not an empty store's.
    for index in 0..2 {
        emulator
            .ok(
                "seed",
                "/createNotificationRule",
                &json!({
                    "Name": format!("token-rule-{index}"),
                    "Resource": "arn:aws:codecommit:us-east-1:123456789012:MyRepo",
                    "DetailType": "BASIC",
                    "EventTypeIds": ["codecommit-repository-comments-on-commits"],
                    "Targets": [{
                        "TargetType": "SNS",
                        "TargetAddress": format!("arn:aws:sns:us-east-1:123456789012:topic-{index}"),
                    }],
                }),
            )
            .await;
    }

    let tokens = [
        // Not hexadecimal at all.
        "notarealtoken",
        // One character: too short to be a hex pair.
        "7",
        // The padding and slash characters the pattern admits.
        "AAAA==",
        "a/b+c=",
        // Hex that decodes to text, but not to a position this service minted.
        "76393a33",
        "7631",
        // Hex whose bytes are not text at all.
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    ];

    for path in PAGINATED_PATHS {
        for token in tokens {
            let case = format!("{path} with {token:?}");
            let (status, body) = emulator.post(path, &json!({ "NextToken": token })).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidNextTokenException",
                "case {case:?}: expected InvalidNextTokenException, got {body}"
            );
            let expected = format!("Invalid nextToken parameter {token}.");
            assert_eq!(
                message(&body),
                expected,
                "case {case:?}: expected {expected:?}, got {body}"
            );
        }
    }
}

/// RULE: only a token the service cannot decode is refused — a `NextToken` a page really
/// issued still pages, on every paginated operation.
#[tokio::test]
async fn a_token_a_page_issued_still_pages() {
    let emulator = Emulator::new();
    for index in 0..3 {
        emulator
            .ok(
                "seed",
                "/createNotificationRule",
                &json!({
                    "Name": format!("paging-rule-{index}"),
                    "Resource": "arn:aws:codecommit:us-east-1:123456789012:MyRepo",
                    "DetailType": "BASIC",
                    "EventTypeIds": ["codecommit-repository-comments-on-commits"],
                    "Targets": [{
                        "TargetType": "SNS",
                        "TargetAddress": format!("arn:aws:sns:us-east-1:123456789012:paging-{index}"),
                    }],
                }),
            )
            .await;
    }

    // (path, the output list member each listing reports)
    let listings = [
        ("/listEventTypes", "EventTypes"),
        ("/listNotificationRules", "NotificationRules"),
        ("/listTargets", "Targets"),
    ];
    for (path, member) in listings {
        let first = emulator
            .ok(path, path, &json!({ "MaxResults": 1 }))
            .await;
        let token = first
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        let Some(token) = token else {
            panic!("case {path:?}: expected a NextToken after a one-entry page, got {first}");
        };
        let (status, second) = emulator
            .post(path, &json!({ "MaxResults": 1, "NextToken": token }))
            .await;
        assert_eq!(
            status, 200,
            "case {path:?}: expected the issued token to page, got {status} with {second}"
        );
        assert_eq!(
            summaries(&second, member).len(),
            1,
            "case {path:?}: expected the next one-entry page, got {second}"
        );
        assert_ne!(
            summaries(&second, member).first(),
            summaries(&first, member).first(),
            "case {path:?}: the second page must resume past the first, got {second}"
        );
    }
}
