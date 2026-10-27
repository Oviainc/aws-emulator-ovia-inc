// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the PARAMETERS a `CreateWorkload` request's
//! `ClientRequestToken` identifies.
//!
//! The rule pinned here, over a TABLE of inputs rather than one recorded request: under an
//! `@idempotencyToken`, a retry is the same request only when EVERY input member it
//! carries is the same — "if you retry a request with the same client request token and the
//! same parameters after the original request has completed successfully, the result of the
//! original request is returned" (`smithy_query(query="shape",
//! shape="ClientRequestToken")`) — so a token replayed with a different value for ANY
//! member the operation takes is a different request and is refused with the modelled
//! `ConflictException` (HTTP 409), whichever member differs; the refusal writes nothing, so
//! the workload the token created keeps the parameters it was created with.
//!
//! The rows differ along which member changes, its type (enum, free text, a list, a nested
//! structure), and how it differs (one value for another, a member added, a list reordered).
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client lives in `super::workload_harness`.

use serde_json::{json, Value};

use super::workload_harness::{created_id, minimal_create, Client};

/// The `TrustedAdvisorIntegrationStatus` value set, from
/// `smithy_query(query="shape", shape="TrustedAdvisorIntegrationStatus")`.
const TRUSTED_ADVISOR_STATUS_VALUES: [&str; 2] = ["ENABLED", "DISABLED"];

/// The `DefinitionType` value set — the modes a `WorkloadResourceDefinition` entry names,
/// from `smithy_query(query="shape", shape="DefinitionType")`.
const DEFINITION_TYPE_VALUES: [&str; 2] = ["WORKLOAD_METADATA", "APP_REGISTRY"];

/// The `WorkloadIssueManagementStatus` value set, from
/// `smithy_query(query="shape", shape="WorkloadIssueManagementStatus")`.
const ISSUE_MANAGEMENT_STATUS_VALUES: [&str; 3] = ["ENABLED", "DISABLED", "INHERIT"];

/// The `IssueManagementType` value set, from
/// `smithy_query(query="shape", shape="IssueManagementType")`.
const ISSUE_MANAGEMENT_TYPE_VALUES: [&str; 2] = ["AUTO", "MANUAL"];

/// A `JiraProjectKey` the modelled `@length(1, 100)` `^[A-Z][A-Z0-9_]*$` accepts
/// (`smithy_query(query="shape", shape="JiraProjectKey")`).
const JIRA_PROJECT_KEY: &str = "WA_PROJECT";

/// One row: the member whose value differs between the two requests sent under one token,
/// and the two values. `Value::Null` means the request does not carry the member at all.
struct Case {
    case: &'static str,
    member: &'static str,
    first: Value,
    second: Value,
}

/// The request `minimal_create` builds, carrying `value` under `member` — or not carrying
/// the member at all when `value` is `Value::Null`.
fn request_with(name: &str, token: &str, member: &str, value: &Value) -> Value {
    let mut request = minimal_create(name, token);
    match value {
        Value::Null => {
            request
                .as_object_mut()
                .expect("a create request is a JSON object")
                .remove(member);
        }
        value => request[member] = value.clone(),
    }
    request
}

/// Rule: every member the request carries is part of the parameters its token identifies.
#[tokio::test]
async fn a_token_replayed_with_any_different_member_is_a_conflict() {
    let cases: Vec<Case> = vec![
        // The two members the fingerprint used to omit, differing in each of their own
        // members and in presence.
        Case {
            case: "discovery-trusted-advisor-status",
            member: "DiscoveryConfig",
            first: json!({ "TrustedAdvisorIntegrationStatus": TRUSTED_ADVISOR_STATUS_VALUES[1] }),
            second: json!({ "TrustedAdvisorIntegrationStatus": TRUSTED_ADVISOR_STATUS_VALUES[0] }),
        },
        Case {
            case: "discovery-resource-definition-value",
            member: "DiscoveryConfig",
            first: json!({ "WorkloadResourceDefinition": [DEFINITION_TYPE_VALUES[0]] }),
            second: json!({ "WorkloadResourceDefinition": [DEFINITION_TYPE_VALUES[1]] }),
        },
        Case {
            case: "discovery-resource-definition-count",
            member: "DiscoveryConfig",
            first: json!({ "WorkloadResourceDefinition": [DEFINITION_TYPE_VALUES[0]] }),
            second: json!({ "WorkloadResourceDefinition": DEFINITION_TYPE_VALUES }),
        },
        Case {
            case: "discovery-resource-definition-order",
            member: "DiscoveryConfig",
            first: json!({
                "WorkloadResourceDefinition": [DEFINITION_TYPE_VALUES[0], DEFINITION_TYPE_VALUES[1]]
            }),
            second: json!({
                "WorkloadResourceDefinition": [DEFINITION_TYPE_VALUES[1], DEFINITION_TYPE_VALUES[0]]
            }),
        },
        Case {
            case: "discovery-config-added",
            member: "DiscoveryConfig",
            first: Value::Null,
            second: json!({ "TrustedAdvisorIntegrationStatus": TRUSTED_ADVISOR_STATUS_VALUES[0] }),
        },
        Case {
            case: "jira-issue-management-status",
            member: "JiraConfiguration",
            first: json!({ "IssueManagementStatus": ISSUE_MANAGEMENT_STATUS_VALUES[0] }),
            second: json!({ "IssueManagementStatus": ISSUE_MANAGEMENT_STATUS_VALUES[1] }),
        },
        Case {
            case: "jira-issue-management-type",
            member: "JiraConfiguration",
            first: json!({
                "IssueManagementStatus": ISSUE_MANAGEMENT_STATUS_VALUES[0],
                "IssueManagementType": ISSUE_MANAGEMENT_TYPE_VALUES[0],
            }),
            second: json!({
                "IssueManagementStatus": ISSUE_MANAGEMENT_STATUS_VALUES[0],
                "IssueManagementType": ISSUE_MANAGEMENT_TYPE_VALUES[1],
            }),
        },
        Case {
            case: "jira-project-key",
            member: "JiraConfiguration",
            first: json!({
                "IssueManagementStatus": ISSUE_MANAGEMENT_STATUS_VALUES[0],
                "IssueManagementType": ISSUE_MANAGEMENT_TYPE_VALUES[1],
                "JiraProjectKey": JIRA_PROJECT_KEY,
            }),
            second: json!({
                "IssueManagementStatus": ISSUE_MANAGEMENT_STATUS_VALUES[0],
                "IssueManagementType": ISSUE_MANAGEMENT_TYPE_VALUES[1],
                "JiraProjectKey": "OTHER",
            }),
        },
        Case {
            case: "jira-configuration-added",
            member: "JiraConfiguration",
            first: Value::Null,
            second: json!({ "IssueManagementStatus": ISSUE_MANAGEMENT_STATUS_VALUES[2] }),
        },
        // Members of other kinds, so the rule is pinned over the whole input rather than
        // over the two members this suite was written for.
        Case {
            case: "architectural-design",
            member: "ArchitecturalDesign",
            first: json!("https://example.com/design-a"),
            second: json!("https://example.com/design-b"),
        },
        Case {
            case: "industry",
            member: "Industry",
            first: json!("Retail & Wholesale"),
            second: json!("Healthcare"),
        },
        Case {
            case: "notes-added",
            member: "Notes",
            first: Value::Null,
            second: json!("notes the retry brought"),
        },
        Case {
            case: "account-ids",
            member: "AccountIds",
            first: json!(["123456789012"]),
            second: json!(["210987654321"]),
        },
        Case {
            case: "pillar-priorities-order",
            member: "PillarPriorities",
            first: json!(["security", "reliability"]),
            second: json!(["reliability", "security"]),
        },
    ];

    for row in cases {
        let Case {
            case,
            member,
            first,
            second,
        } = row;
        assert_ne!(
            first, second,
            "case {case:?}: the row's two values must differ, got {first:?}"
        );

        let client = Client::new();
        let name = format!("wl-{case}");
        let token = format!("tok-{case}");
        let original_request = request_with(&name, &token, member, &first);
        let original = created_id(&client.create(original_request.clone()).await, case);

        // The same token with the same parameters is the same request.
        let replay = client.create(original_request.clone()).await;
        assert_eq!(
            replay.body.get("WorkloadId").and_then(Value::as_str),
            Some(original.as_str()),
            "case {case:?}: an identical retry must return the original WorkloadId, got {replay:?}"
        );

        // The same token with a different `member` is a DIFFERENT request.
        let changed = client
            .create(request_with(&name, &token, member, &second))
            .await;
        assert_eq!(
            changed.status, 409,
            "case {case:?}: the token was replayed with a different {member}, expected HTTP 409, \
             got {changed:?}"
        );
        assert_eq!(
            changed.error_code, "ConflictException",
            "case {case:?}: expected ConflictException, got {changed:?}"
        );
        assert_eq!(
            changed.body.get("ResourceId").and_then(Value::as_str),
            Some(original.as_str()),
            "case {case:?}: the conflict must name the workload the token created, got {changed:?}"
        );
        assert_eq!(
            changed.body.get("ResourceType").and_then(Value::as_str),
            Some("WORKLOAD"),
            "case {case:?}: the conflict must name the resource type, got {changed:?}"
        );
        assert_eq!(
            changed.body.get("Message").and_then(Value::as_str),
            Some(
                format!(
                    "[Conflict] A request with ClientRequestToken {token} was already submitted \
                     with different parameters."
                )
                .as_str()
            ),
            "case {case:?}: expected the replayed-token conflict message, got {changed:?}"
        );

        // The refusal wrote nothing: the original request still replays to the workload it
        // created, and no second workload exists.
        let after = client.create(original_request).await;
        assert_eq!(
            after.body.get("WorkloadId").and_then(Value::as_str),
            Some(original.as_str()),
            "case {case:?}: the refused retry must not have changed the stored parameters, got \
             {after:?}"
        );
        let listed = client.list(json!({})).await;
        let summaries = listed
            .body
            .get("WorkloadSummaries")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            summaries.len(),
            1,
            "case {case:?}: one token must have created exactly one workload, got {summaries:?}"
        );
    }
}

/// The other half of the rule: the two members the token now covers are STORED and reported
/// back, so what the conflict above protects is a real difference in the created workload.
/// The rows span both published value sets and every `WorkloadResourceDefinition` subset.
#[tokio::test]
async fn a_created_workload_reports_the_discovery_configuration_it_was_created_with() {
    let resource_definitions: Vec<Option<Vec<&str>>> = vec![
        None,
        Some(vec![DEFINITION_TYPE_VALUES[0]]),
        Some(vec![DEFINITION_TYPE_VALUES[1]]),
        // "You can specify WORKLOAD_METADATA, APP_REGISTRY, or both"
        // (`smithy_query(query="shape", shape="WorkloadDiscoveryConfig")`).
        Some(vec![DEFINITION_TYPE_VALUES[0], DEFINITION_TYPE_VALUES[1]]),
    ];

    for status in TRUSTED_ADVISOR_STATUS_VALUES {
        for definition in &resource_definitions {
            let case = format!("{status}-{definition:?}");
            let client = Client::new();
            let mut config = json!({ "TrustedAdvisorIntegrationStatus": status });
            if let Some(definition) = definition {
                config["WorkloadResourceDefinition"] = json!(definition);
            }
            let mut request = minimal_create("discovery-workload", "tok-discovery");
            request["DiscoveryConfig"] = config.clone();
            let workload_id = created_id(&client.create(request).await, &case);

            let fetched = client.get(&workload_id).await;
            let reported = fetched
                .body
                .get("Workload")
                .and_then(|workload| workload.get("DiscoveryConfig"))
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                reported, config,
                "case {case:?}: expected the workload to report the discovery configuration it \
                 was created with ({config:?}), got {fetched:?}"
            );
        }
    }
}

/// The same for `JiraConfiguration`, over every published status and type.
#[tokio::test]
async fn a_created_workload_reports_the_jira_configuration_it_was_created_with() {
    for status in ISSUE_MANAGEMENT_STATUS_VALUES {
        for issue_type in ISSUE_MANAGEMENT_TYPE_VALUES {
            let case = format!("{status}-{issue_type}");
            let client = Client::new();
            let configuration = json!({
                "IssueManagementStatus": status,
                "IssueManagementType": issue_type,
                "JiraProjectKey": JIRA_PROJECT_KEY,
            });
            let mut request = minimal_create("jira-workload", "tok-jira");
            request["JiraConfiguration"] = configuration.clone();
            let workload_id = created_id(&client.create(request).await, &case);

            let fetched = client.get(&workload_id).await;
            let reported = fetched
                .body
                .get("Workload")
                .and_then(|workload| workload.get("JiraConfiguration"))
                .cloned()
                .unwrap_or(Value::Null);
            for member in ["IssueManagementStatus", "IssueManagementType", "JiraProjectKey"] {
                assert_eq!(
                    reported.get(member),
                    configuration.get(member),
                    "case {case:?}: expected the workload to report the {member} it was created \
                     with, got {fetched:?}"
                );
            }
        }
    }
}
