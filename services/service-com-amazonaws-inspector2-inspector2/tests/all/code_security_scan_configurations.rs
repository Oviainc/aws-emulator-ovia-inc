// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Amazon Inspector code security scan configuration slice:
//! `CreateCodeSecurityScanConfiguration`, `GetCodeSecurityScanConfiguration` and
//! `DeleteCodeSecurityScanConfiguration`.
//!
//! The rule under test: a code security scan configuration is a stored resource — the
//! create mints its `ScanConfigurationArn` and records the name, level, scan configuration,
//! scope settings and tags it was sent; the get reports exactly that record plus the
//! instants it was created and last updated at; and the delete removes the record, so a
//! later get or delete of the same ARN is refused as a missing resource.
//!
//! Every request goes through the real `MultiServiceRouter` — routing, restJson1
//! deserialization, modelled-constraint validation, the handler, and restJson1
//! serialization — so what is asserted here is the wire behaviour a client sees.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The signed-request harness this crate's suites share: `router()` builds a router over
// its own engine, `post_in` sends one restJson1 POST signed for a region.
use crate::filters_and_enablement::{post_in, router};

/// The single account id every generated emulator presents
/// (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// A well-formed `ScanConfigurationArn` this emulator never minted — for the not-found
/// paths, which need an ARN that passes the member's modelled `@pattern` and names no
/// stored configuration.
/// Source: `smithy_query(query="shape", shape="ScanConfigurationArn")`.
const UNMINTED_ARN: &str = "arn:aws:inspector2:us-east-1:000000000000:owner/000000000000/\
                            codesecurity-configuration/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

/// The refusal text both reads answer for an ARN that names no stored configuration:
/// Amazon Inspector's own wording for a scan configuration that does not exist.
const NOT_FOUND_MESSAGE: &str = "Scan configuration not found";

/// `CreateCodeSecurityScanConfiguration` — `POST /codesecurity/scan-configuration/create`.
async fn create_in(
    router: &MultiServiceRouter,
    region: &str,
    body: &Value,
) -> (u16, Option<String>, Value) {
    post_in(router, region, "/codesecurity/scan-configuration/create", body).await
}

/// `GetCodeSecurityScanConfiguration` — `POST /codesecurity/scan-configuration/get`.
async fn get(router: &MultiServiceRouter, arn: &str) -> (u16, Option<String>, Value) {
    post_in(
        router,
        "us-east-1",
        "/codesecurity/scan-configuration/get",
        &json!({ "scanConfigurationArn": arn }),
    )
    .await
}

/// `DeleteCodeSecurityScanConfiguration` — `POST /codesecurity/scan-configuration/delete`.
async fn delete(router: &MultiServiceRouter, arn: &str) -> (u16, Option<String>, Value) {
    post_in(
        router,
        "us-east-1",
        "/codesecurity/scan-configuration/delete",
        &json!({ "scanConfigurationArn": arn }),
    )
    .await
}

/// Assert `arn` is a `ScanConfigurationArn` of `region` owned by `owner`, with the
/// hyphenated 36-character UUID id segment the real service mints.
///
/// Source: `smithy_query(query="shape", shape="ScanConfigurationArn")` — pattern
/// `^arn:(aws[a-zA-Z-]*)?:inspector2:[a-z]{2}(-gov)?-[a-z]+-\d{1}:\d{12}:owner/(\d{12}|o-[a-z0-9]{10,32})/codesecurity-configuration/[a-f0-9-]{36}$`.
fn assert_scan_configuration_arn(case: &str, arn: &str, region: &str, owner: &str) {
    let prefix =
        format!("arn:aws:inspector2:{region}:{owner}:owner/{owner}/codesecurity-configuration/");
    let id = arn.strip_prefix(&prefix).unwrap_or_else(|| {
        panic!("case {case:?}: expected an ARN starting {prefix:?}, got {arn:?}")
    });
    assert_eq!(
        id.len(),
        36,
        "case {case:?}: expected a 36-character id segment, got {id:?}"
    );
    assert!(
        id.chars()
            .all(|char| char.is_ascii_hexdigit() && !char.is_ascii_uppercase() || char == '-'),
        "case {case:?}: id segment outside [a-f0-9-], got {id:?}"
    );
    let groups: Vec<usize> = id.split('-').map(str::len).collect();
    assert_eq!(
        groups,
        vec![8, 4, 4, 4, 12],
        "case {case:?}: expected the hyphenated UUID grouping, got {id:?}"
    );
}

/// One create request: a row of the table below.
struct Row {
    case: &'static str,
    region: &'static str,
    name: String,
    /// A `ConfigurationLevel` token.
    level: &'static str,
    /// The `CodeSecurityScanConfiguration` as sent.
    configuration: Value,
    /// The `ScopeSettings` as sent, absent for a general scan configuration.
    scope_settings: Option<Value>,
    tags: Option<Value>,
}

/// The create requests the round-trip test drives, spanning what the rule generalises
/// over: both `ConfigurationLevel` tokens, every `RuleSetCategory` token and the modelled
/// `ruleSetCategories` length bounds (1 and 3), the periodic settings absent / frequency
/// only / frequency with a cron expression and every `PeriodicScanFrequency` token, the
/// continuous-integration settings absent / one event / both events (the modelled maximum),
/// scope settings absent (a general configuration) and present (a default one), tags
/// absent / empty / present, the `ScanConfigurationName` length bounds (1 and 60), and
/// four regions.
///
/// Sources: `smithy_query(query="shape", shape="ConfigurationLevel,RuleSetCategory,RuleSetCategories,PeriodicScanFrequency,ContinuousIntegrationScanEvent,ContinuousIntegrationScanSupportedEvents,ProjectSelectionScope,ScanConfigurationName")`.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "account level, one rule set category, no periodic or CI settings",
            region: "us-east-1",
            // The shortest name the member's modelled length admits.
            name: "a".to_string(),
            level: "ACCOUNT",
            configuration: json!({ "ruleSetCategories": ["SAST"] }),
            scope_settings: None,
            tags: None,
        },
        Row {
            case: "organization level, every rule set category, weekly periodic scan, default scope",
            region: "eu-west-2",
            name: "org-wide.scan_config:v1$".to_string(),
            level: "ORGANIZATION",
            configuration: json!({
                "ruleSetCategories": ["SAST", "IAC", "SCA"],
                "periodicScanConfiguration": { "frequency": "WEEKLY" },
            }),
            scope_settings: Some(json!({ "projectSelectionScope": "ALL" })),
            tags: Some(json!({ "owner": "security-team", "costCenter": "4711" })),
        },
        Row {
            case: "monthly periodic scan with a cron expression, one CI event, empty tags",
            region: "ap-southeast-1",
            name: "monthly-with-expression".to_string(),
            level: "ACCOUNT",
            configuration: json!({
                "ruleSetCategories": ["IAC", "SCA"],
                "periodicScanConfiguration": {
                    "frequency": "MONTHLY",
                    "frequencyExpression": "cron(0 0 1 * ? *)",
                },
                "continuousIntegrationScanConfiguration": {
                    "supportedEvents": ["PULL_REQUEST"],
                },
            }),
            scope_settings: None,
            tags: Some(json!({})),
        },
        Row {
            case: "periodic scanning off, both CI events, longest name",
            region: "us-west-2",
            // The longest name the member's modelled length admits (60 characters).
            name: "n".repeat(60),
            level: "ACCOUNT",
            configuration: json!({
                "ruleSetCategories": ["SCA"],
                "periodicScanConfiguration": { "frequency": "NEVER" },
                "continuousIntegrationScanConfiguration": {
                    "supportedEvents": ["PULL_REQUEST", "PUSH"],
                },
            }),
            scope_settings: Some(json!({ "projectSelectionScope": "ALL" })),
            tags: Some(json!({ "purpose": "code-security" })),
        },
        Row {
            case: "organization level, CI scans only, no scope settings",
            region: "eu-west-2",
            name: "ci-only".to_string(),
            level: "ORGANIZATION",
            configuration: json!({
                "ruleSetCategories": ["SAST", "SCA"],
                "continuousIntegrationScanConfiguration": { "supportedEvents": ["PUSH"] },
            }),
            scope_settings: None,
            tags: None,
        },
    ]
}

/// The create body of one row.
fn create_body(row: &Row) -> Value {
    let mut body = json!({
        "name": row.name,
        "level": row.level,
        "configuration": row.configuration,
    });
    if let Some(scope_settings) = &row.scope_settings {
        body["scopeSettings"] = scope_settings.clone();
    }
    if let Some(tags) = &row.tags {
        body["tags"] = tags.clone();
    }
    body
}

/// RULE: a created code security scan configuration is a stored record — the get reports
/// exactly the name, level, scan configuration, scope settings and tags the create was
/// sent, plus the instants it was created and last updated at; and the delete removes it,
/// so a later get or delete of its ARN is a 404.
#[tokio::test]
async fn a_created_configuration_is_read_back_as_stored_and_gone_after_delete() {
    for row in rows() {
        let case = row.case;
        let router = router();

        let (status, error_type, created) =
            create_in(&router, row.region, &create_body(&row)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateCodeSecurityScanConfiguration: expected 200, got {status} / \
             {created} ({error_type:?})"
        );
        let arn = created["scanConfigurationArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no scanConfigurationArn in {created}"))
            .to_string();
        assert_scan_configuration_arn(case, &arn, row.region, ACCOUNT);

        // The get reports the stored record, member by member.
        let (status, error_type, read) = get(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetCodeSecurityScanConfiguration: expected 200, got {status} / {read} \
             ({error_type:?})"
        );
        assert_eq!(
            read["scanConfigurationArn"],
            json!(arn),
            "case {case:?}: expected the created ARN, got {read}"
        );
        assert_eq!(
            read["name"],
            json!(row.name),
            "case {case:?}: expected the name as sent, got {read}"
        );
        assert_eq!(
            read["level"],
            json!(row.level),
            "case {case:?}: expected the level as sent, got {read}"
        );
        assert_eq!(
            read["configuration"], row.configuration,
            "case {case:?}: expected the configuration as sent, got {read}"
        );
        match &row.scope_settings {
            // Absent for a general scan configuration, and as sent for a default one.
            None => assert_eq!(
                read["scopeSettings"],
                Value::Null,
                "case {case:?}: expected no scopeSettings member, got {read}"
            ),
            Some(scope_settings) => assert_eq!(
                read["scopeSettings"], *scope_settings,
                "case {case:?}: expected the scope settings as sent, got {read}"
            ),
        }
        match &row.tags {
            // Absent when the create carried no `tags`, and the map it carried otherwise —
            // an empty one included.
            None => assert_eq!(
                read["tags"],
                Value::Null,
                "case {case:?}: expected no tags member, got {read}"
            ),
            Some(tags) => assert_eq!(
                read["tags"], *tags,
                "case {case:?}: expected the tags as sent, got {read}"
            ),
        }
        let created_at = read["createdAt"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: no createdAt timestamp in {read}"));
        let last_updated_at = read["lastUpdatedAt"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: no lastUpdatedAt timestamp in {read}"));
        assert_eq!(
            created_at, last_updated_at,
            "case {case:?}: a configuration that was never updated reports its creation \
             instant as its last update, got {read}"
        );
        assert!(
            created_at > 0.0,
            "case {case:?}: expected an epoch timestamp, got {created_at}"
        );

        // A get is a pure read: a second one answers exactly the same thing.
        let (status, _, again) = get(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a repeated get: expected 200, got {status} / {again}"
        );
        assert_eq!(
            again, read,
            "case {case:?}: a get must not change what a later caller sees, got {again}"
        );

        // The delete reports the ARN it removed.
        let (status, error_type, deleted) = delete(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteCodeSecurityScanConfiguration: expected 200, got {status} / \
             {deleted} ({error_type:?})"
        );
        assert_eq!(
            deleted["scanConfigurationArn"],
            json!(arn),
            "case {case:?}: expected the deleted ARN, got {deleted}"
        );

        // And the record is gone: both reads of it are a missing resource, whether the ARN
        // was deleted or never minted at all.
        for (attempt, candidate) in [("already deleted", arn.as_str()), ("never minted", UNMINTED_ARN)]
        {
            for (operation, answered) in [
                ("get", get(&router, candidate).await),
                ("delete", delete(&router, candidate).await),
            ] {
                let (status, error_type, body) = answered;
                assert_eq!(
                    status, 404,
                    "case {case:?}/{attempt}/{operation}: expected 404, got {status} / {body}"
                );
                assert_eq!(
                    error_type.as_deref(),
                    Some("ResourceNotFoundException"),
                    "case {case:?}/{attempt}/{operation}: expected ResourceNotFoundException, got \
                     {error_type:?} / {body}"
                );
                assert_eq!(
                    body["message"],
                    json!(NOT_FOUND_MESSAGE),
                    "case {case:?}/{attempt}/{operation}: unexpected refusal message, got {body}"
                );
            }
        }
    }
}

/// RULE: every create mints a NEW configuration with an ARN of its own — the ARN
/// identifies the record, so two configurations never share one, not even under the same
/// name, and deleting one leaves the others readable.
#[tokio::test]
async fn every_create_mints_a_distinct_configuration() {
    let router = router();
    // Rows differ along how many creates carry the SAME name: a repeated name is a second
    // configuration, not a second write to the first.
    let names = ["same", "same", "other", "same"];
    let mut created: Vec<(usize, String)> = Vec::new();
    for (index, name) in names.iter().enumerate() {
        let body = json!({
            "name": name,
            "level": "ACCOUNT",
            "configuration": { "ruleSetCategories": ["SAST"] },
            // One tag per create, so each record carries something only it carries.
            "tags": { "index": index.to_string() },
        });
        let (status, error_type, response) = create_in(&router, "us-east-1", &body).await;
        assert_eq!(
            status, 200,
            "case {name:?}/{index}: CreateCodeSecurityScanConfiguration: expected 200, got \
             {status} / {response} ({error_type:?})"
        );
        let arn = response["scanConfigurationArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}/{index}: no scanConfigurationArn in {response}"))
            .to_string();
        created.push((index, arn));
    }

    let arns: BTreeSet<&str> = created.iter().map(|(_, arn)| arn.as_str()).collect();
    assert_eq!(
        arns.len(),
        created.len(),
        "expected one ARN per create, got {created:?}"
    );

    // Each ARN reads back its OWN record.
    for (index, arn) in &created {
        let (status, _, read) = get(&router, arn).await;
        assert_eq!(
            status, 200,
            "case {index}: expected 200 for {arn:?}, got {status} / {read}"
        );
        assert_eq!(
            read["name"],
            json!(names[*index]),
            "case {index}: expected the name this ARN was created with, got {read}"
        );
        assert_eq!(
            read["tags"],
            json!({ "index": index.to_string() }),
            "case {index}: expected the tags this ARN was created with, got {read}"
        );
    }

    // Deleting one record leaves every other one readable.
    let (deleted_index, deleted_arn) = created.remove(1);
    let (status, _, deleted) = delete(&router, &deleted_arn).await;
    assert_eq!(
        status, 200,
        "case {deleted_index}: expected 200 deleting {deleted_arn:?}, got {status} / {deleted}"
    );
    for (index, arn) in &created {
        let (status, _, read) = get(&router, arn).await;
        assert_eq!(
            status, 200,
            "case {index}: expected 200 after deleting another configuration, got {status} / \
             {read}"
        );
    }
}
