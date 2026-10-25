// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the account's global settings: `GetGlobalSettings`
//! (`GET /global-settings`, a Unit input) and the `UpdateGlobalSettings` write it reads
//! back (`PATCH /global-settings`) —
//! `smithy_query(query="operation", operation="GetGlobalSettings")` and
//! `smithy_query(query="operation", operation="UpdateGlobalSettings")`.
//!
//! The rules under test:
//!
//! 1. IMPLEMENTED AND ROUTED. `GET /global-settings` is answered 200 with a
//!    `GetGlobalSettingsOutput` — never the framework's unroutable-operation answer and
//!    never the scaffold channel's 501.
//! 2. REPORTED FROM WHAT IS RECORDED. Every status the account has recorded is reported
//!    as recorded; a status no write has set is reported as the value an account that has
//!    turned the feature off is running under (`DISABLED`, and `NOT_CONFIGURED` for the
//!    Jira connection), and a Jira member that is not a status is reported only once a
//!    write recorded it.
//! 3. PARTIAL WRITE. The write is bound to `PATCH`, so a member a request names replaces
//!    what was recorded for it and a member it omits leaves that setting alone — nested
//!    Jira members included.
//! 4. PURE READ. "Global settings for all workloads" reports; it does not revise. Any
//!    number of reads leave the recorded settings and every later answer unchanged.
//! 5. REFUSED VALUES RECORD NOTHING. A value outside a published value set or outside a
//!    published `@length` bound is refused `400 ValidationException` and leaves the
//!    account's settings as they were.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client lives in `super::workload_harness`.

use serde_json::{json, Value};

use super::workload_harness::{Answer, Client};

/// The route both operations are bound to.
const SETTINGS_URI: &str = "/global-settings";

/// Every value `OrganizationSharingStatus`, `DiscoveryIntegrationStatus` and
/// `AccountJiraIssueManagementStatus` publish — each publishes exactly these two
/// (`smithy_query(query="shape", shape="OrganizationSharingStatus")`).
const OPT_IN_STATUSES: [&str; 2] = ["ENABLED", "DISABLED"];

/// Every value `IssueManagementType` publishes
/// (`smithy_query(query="shape", shape="IssueManagementType")`).
const ISSUE_MANAGEMENT_TYPES: [&str; 2] = ["AUTO", "MANUAL"];

/// The `@length` ceiling of `JiraProjectKey`, whose pattern is `^[A-Z][A-Z0-9_]*$`
/// (`smithy_query(query="shape", shape="JiraProjectKey")`).
const MAX_JIRA_PROJECT_KEY: usize = 100;

/// What `GetGlobalSettings` reports for an account that has recorded nothing: every
/// opt-in status off, the Jira connection not configured, and no member the Jira
/// connection itself would carry.
fn unconfigured() -> Value {
    json!({
        "OrganizationSharingStatus": "DISABLED",
        "DiscoveryIntegrationStatus": "DISABLED",
        "JiraConfiguration": {
            "IntegrationStatus": "NOT_CONFIGURED",
            "IssueManagementStatus": "DISABLED",
        },
    })
}

/// `GET /global-settings`, failing the test unless the call answered 200 — so no row
/// below can read its assertions off a 501 or a routing error.
async fn read(client: &Client, case: &str) -> Value {
    let answer = client.send("GET", SETTINGS_URI, None).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: GetGlobalSettings must answer 200, got {answer:?}"
    );
    answer.body
}

/// `PATCH /global-settings` with the request body as sent.
async fn write(client: &Client, body: Value) -> Answer {
    client.send("PATCH", SETTINGS_URI, Some(body)).await
}

/// A `JiraProjectKey` of `length` characters that satisfies the published pattern, so a
/// row about a bound is about the bound and not about the value's syntax.
fn project_key(length: usize) -> String {
    format!("W{}", "A".repeat(length.saturating_sub(1)))
}

/// Rule 1 and the un-recorded half of rule 2: the read is served, and an account that has
/// written nothing is reported as running with every feature off.
#[tokio::test]
async fn an_account_that_recorded_nothing_reports_every_feature_off() {
    let client = Client::new();
    let body = read(&client, "fresh-store").await;
    assert_eq!(
        body,
        unconfigured(),
        "case {:?}: an account that recorded nothing must report every feature off, got \
         {body:?}",
        "fresh-store"
    );
}

/// Rule 2: every recorded setting is reported back, and only the members a write recorded
/// move off the un-recorded value.
///
/// The rows differ along WHAT a request names — nothing at all, one top-level status,
/// both, one nested Jira member, several, every member at once — and along the VALUE each
/// published value set allows, with the project key at its published `@length` ceiling.
/// Each row runs against a FRESH store, so a row's expectation is about that row's write
/// alone.
#[tokio::test]
async fn every_recorded_setting_is_reported_back() {
    let mut rows: Vec<(String, Value, Value)> = vec![
        ("names-nothing".to_string(), json!({}), unconfigured()),
        (
            "both-top-level-statuses".to_string(),
            json!({
                "OrganizationSharingStatus": "ENABLED",
                "DiscoveryIntegrationStatus": "ENABLED",
            }),
            json!({
                "OrganizationSharingStatus": "ENABLED",
                "DiscoveryIntegrationStatus": "ENABLED",
                "JiraConfiguration": {
                    "IntegrationStatus": "NOT_CONFIGURED",
                    "IssueManagementStatus": "DISABLED",
                },
            }),
        ),
        (
            "jira-issue-management-alone".to_string(),
            json!({ "JiraConfiguration": { "IssueManagementStatus": "ENABLED" } }),
            json!({
                "OrganizationSharingStatus": "DISABLED",
                "DiscoveryIntegrationStatus": "DISABLED",
                "JiraConfiguration": {
                    "IntegrationStatus": "NOT_CONFIGURED",
                    "IssueManagementStatus": "ENABLED",
                },
            }),
        ),
        (
            "jira-connection-not-configured".to_string(),
            json!({ "JiraConfiguration": { "IntegrationStatus": "NOT_CONFIGURED" } }),
            unconfigured(),
        ),
        (
            "every-member-with-the-longest-project-key".to_string(),
            json!({
                "OrganizationSharingStatus": "ENABLED",
                "DiscoveryIntegrationStatus": "DISABLED",
                "JiraConfiguration": {
                    "IntegrationStatus": "NOT_CONFIGURED",
                    "IssueManagementStatus": "ENABLED",
                    "IssueManagementType": "MANUAL",
                    "JiraProjectKey": project_key(MAX_JIRA_PROJECT_KEY),
                },
            }),
            json!({
                "OrganizationSharingStatus": "ENABLED",
                "DiscoveryIntegrationStatus": "DISABLED",
                "JiraConfiguration": {
                    "IntegrationStatus": "NOT_CONFIGURED",
                    "IssueManagementStatus": "ENABLED",
                    "IssueManagementType": "MANUAL",
                    "JiraProjectKey": project_key(MAX_JIRA_PROJECT_KEY),
                },
            }),
        ),
    ];

    // One row per value of each published set, so no row's answer can come from a value
    // the handler happens to special-case.
    for status in OPT_IN_STATUSES {
        rows.push((
            format!("organization-sharing-{status}"),
            json!({ "OrganizationSharingStatus": status }),
            json!({
                "OrganizationSharingStatus": status,
                "DiscoveryIntegrationStatus": "DISABLED",
                "JiraConfiguration": {
                    "IntegrationStatus": "NOT_CONFIGURED",
                    "IssueManagementStatus": "DISABLED",
                },
            }),
        ));
        rows.push((
            format!("discovery-integration-{status}"),
            json!({ "DiscoveryIntegrationStatus": status }),
            json!({
                "OrganizationSharingStatus": "DISABLED",
                "DiscoveryIntegrationStatus": status,
                "JiraConfiguration": {
                    "IntegrationStatus": "NOT_CONFIGURED",
                    "IssueManagementStatus": "DISABLED",
                },
            }),
        ));
    }
    for issue_management_type in ISSUE_MANAGEMENT_TYPES {
        rows.push((
            format!("jira-issue-management-type-{issue_management_type}"),
            json!({
                "JiraConfiguration": {
                    "IssueManagementStatus": "ENABLED",
                    "IssueManagementType": issue_management_type,
                    "JiraProjectKey": "WA",
                },
            }),
            json!({
                "OrganizationSharingStatus": "DISABLED",
                "DiscoveryIntegrationStatus": "DISABLED",
                "JiraConfiguration": {
                    "IntegrationStatus": "NOT_CONFIGURED",
                    "IssueManagementStatus": "ENABLED",
                    "IssueManagementType": issue_management_type,
                    "JiraProjectKey": "WA",
                },
            }),
        ));
    }

    for (case, request, expected) in rows {
        let client = Client::new();
        let written = write(&client, request.clone()).await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: UpdateGlobalSettings must accept {request:?}, got {written:?}"
        );
        let reported = read(&client, &case).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// Rule 3: a write names some members and leaves the rest alone, so the settings the read
/// reports are everything recorded so far — not only the last request.
///
/// One store, one cumulative sequence: each step names a DIFFERENT subset (a top-level
/// status, a nested Jira member, a member already recorded, nothing at all) and the
/// expectation after it carries every earlier step's value too.
#[tokio::test]
async fn a_partial_write_leaves_every_other_setting_as_it_was() {
    let steps: Vec<(&str, Value, Value)> = vec![
        (
            "enable-organization-sharing",
            json!({ "OrganizationSharingStatus": "ENABLED" }),
            json!({
                "OrganizationSharingStatus": "ENABLED",
                "DiscoveryIntegrationStatus": "DISABLED",
                "JiraConfiguration": {
                    "IntegrationStatus": "NOT_CONFIGURED",
                    "IssueManagementStatus": "DISABLED",
                },
            }),
        ),
        (
            "enable-discovery-integration",
            json!({ "DiscoveryIntegrationStatus": "ENABLED" }),
            json!({
                "OrganizationSharingStatus": "ENABLED",
                "DiscoveryIntegrationStatus": "ENABLED",
                "JiraConfiguration": {
                    "IntegrationStatus": "NOT_CONFIGURED",
                    "IssueManagementStatus": "DISABLED",
                },
            }),
        ),
        (
            "record-jira-issue-management",
            json!({
                "JiraConfiguration": {
                    "IssueManagementStatus": "ENABLED",
                    "IssueManagementType": "AUTO",
                    "JiraProjectKey": "WA",
                },
            }),
            json!({
                "OrganizationSharingStatus": "ENABLED",
                "DiscoveryIntegrationStatus": "ENABLED",
                "JiraConfiguration": {
                    "IntegrationStatus": "NOT_CONFIGURED",
                    "IssueManagementStatus": "ENABLED",
                    "IssueManagementType": "AUTO",
                    "JiraProjectKey": "WA",
                },
            }),
        ),
        (
            "switch-the-issue-management-type-alone",
            json!({ "JiraConfiguration": { "IssueManagementType": "MANUAL" } }),
            json!({
                "OrganizationSharingStatus": "ENABLED",
                "DiscoveryIntegrationStatus": "ENABLED",
                "JiraConfiguration": {
                    "IntegrationStatus": "NOT_CONFIGURED",
                    "IssueManagementStatus": "ENABLED",
                    "IssueManagementType": "MANUAL",
                    "JiraProjectKey": "WA",
                },
            }),
        ),
        (
            "turn-organization-sharing-back-off",
            json!({ "OrganizationSharingStatus": "DISABLED" }),
            json!({
                "OrganizationSharingStatus": "DISABLED",
                "DiscoveryIntegrationStatus": "ENABLED",
                "JiraConfiguration": {
                    "IntegrationStatus": "NOT_CONFIGURED",
                    "IssueManagementStatus": "ENABLED",
                    "IssueManagementType": "MANUAL",
                    "JiraProjectKey": "WA",
                },
            }),
        ),
        (
            "name-nothing-at-all",
            json!({}),
            json!({
                "OrganizationSharingStatus": "DISABLED",
                "DiscoveryIntegrationStatus": "ENABLED",
                "JiraConfiguration": {
                    "IntegrationStatus": "NOT_CONFIGURED",
                    "IssueManagementStatus": "ENABLED",
                    "IssueManagementType": "MANUAL",
                    "JiraProjectKey": "WA",
                },
            }),
        ),
    ];

    let client = Client::new();
    for (case, request, expected) in steps {
        let written = write(&client, request.clone()).await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: UpdateGlobalSettings must accept {request:?}, got {written:?}"
        );
        let reported = read(&client, case).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// Rule 4: the read reports and does not revise, so repeated reads answer identically and
/// leave the stored settings byte-identical — before any write and after one.
#[tokio::test]
async fn reads_leave_the_settings_and_every_later_answer_unchanged() {
    let client = Client::new();

    for case in ["before-any-write", "after-a-write"] {
        if case == "after-a-write" {
            let written = write(
                &client,
                json!({
                    "DiscoveryIntegrationStatus": "ENABLED",
                    "JiraConfiguration": { "IssueManagementStatus": "ENABLED" },
                }),
            )
            .await;
            assert_eq!(
                written.status, 200,
                "case {case:?}: UpdateGlobalSettings must answer 200, got {written:?}"
            );
        }

        let first = read(&client, case).await;
        let stored = stored_settings(&client);
        for attempt in 1..=4 {
            let again = read(&client, case).await;
            assert_eq!(
                again, first,
                "case {case:?} read {attempt}: expected {first:?}, got {again:?}"
            );
            let after = stored_settings(&client);
            assert_eq!(
                after, stored,
                "case {case:?} read {attempt}: the stored settings must not move, expected \
                 {stored:?}, got {after:?}"
            );
        }
    }
}

/// The recorded settings as the store holds them, rendered so two snapshots can be
/// compared. The HTTP boundary is what every assertion about behaviour goes through; this
/// is only how a test says "the store did not move".
fn stored_settings(client: &Client) -> String {
    let state = client.store();
    let store = state.lock();
    format!("{:?}", store.global_settings)
}

/// Rule 5: a value outside a published value set or `@length` bound is refused
/// `400 ValidationException`, and the account is left holding what it held.
///
/// The rows span the constraint classes a setting can break — a value set (including the
/// `CONFIGURED` the REPORTED status publishes but no request may ask for, since
/// `IntegrationStatusInput` publishes `NOT_CONFIGURED` alone), a pattern, and the project
/// key's `@length` on its published bound and one past it.
#[tokio::test]
async fn a_value_outside_a_published_set_or_bound_records_nothing() {
    let refused: Vec<(String, Value)> = vec![
        (
            "organization-sharing-lowercase".to_string(),
            json!({ "OrganizationSharingStatus": "enabled" }),
        ),
        (
            "discovery-integration-outside-the-set".to_string(),
            json!({ "DiscoveryIntegrationStatus": "ON" }),
        ),
        (
            "issue-management-status-outside-the-set".to_string(),
            json!({ "JiraConfiguration": { "IssueManagementStatus": "PAUSED" } }),
        ),
        (
            "issue-management-type-outside-the-set".to_string(),
            json!({ "JiraConfiguration": { "IssueManagementType": "SEMI_AUTO" } }),
        ),
        (
            "integration-status-a-request-may-not-ask-for".to_string(),
            json!({ "JiraConfiguration": { "IntegrationStatus": "CONFIGURED" } }),
        ),
        (
            "project-key-outside-the-pattern".to_string(),
            json!({ "JiraConfiguration": { "JiraProjectKey": "wa-1" } }),
        ),
        (
            "project-key-below-the-length-floor".to_string(),
            json!({ "JiraConfiguration": { "JiraProjectKey": "" } }),
        ),
        (
            "project-key-one-past-the-length-ceiling".to_string(),
            json!({
                "JiraConfiguration": {
                    "JiraProjectKey": project_key(MAX_JIRA_PROJECT_KEY + 1),
                },
            }),
        ),
    ];

    for (case, request) in refused {
        let client = Client::new();
        let answer = write(&client, request.clone()).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: {request:?} must be refused 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: a refused value is a ValidationException, got {answer:?}"
        );
        let reported = read(&client, &case).await;
        assert_eq!(
            reported,
            unconfigured(),
            "case {case:?}: a refused write records nothing, expected {:?}, got {reported:?}",
            unconfigured()
        );
    }

    // ON the published ceiling the same member is accepted, so the refusals above are
    // about the bound and not about the member.
    let client = Client::new();
    let accepted = write(
        &client,
        json!({
            "JiraConfiguration": { "JiraProjectKey": project_key(MAX_JIRA_PROJECT_KEY) },
        }),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case {:?}: a project key ON the published ceiling must be accepted, got {accepted:?}",
        "project-key-on-the-length-ceiling"
    );
    let reported = read(&client, "project-key-on-the-length-ceiling").await;
    assert_eq!(
        reported["JiraConfiguration"]["JiraProjectKey"],
        json!(project_key(MAX_JIRA_PROJECT_KEY)),
        "case {:?}: expected the key as sent, got {reported:?}",
        "project-key-on-the-length-ceiling"
    );
}
