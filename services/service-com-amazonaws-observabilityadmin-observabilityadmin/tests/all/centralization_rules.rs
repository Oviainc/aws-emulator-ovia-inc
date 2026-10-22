// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the organization centralization-rule slice of this
//! service.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what
//! is asserted is what a signed AWS SDK request observes. The emulator, the request
//! builder and the response reader are the ones `telemetry_rules` already defines.
//!
//! The rules under test:
//!
//! 1. each operation answers from the ONE persisted centralization-rule record, so
//!    whatever a create or an update stored is what a later Get or List reports, and
//!    whatever was never stored (or has since been deleted) is a modelled 404;
//! 2. a rule name identifies a rule within the region the request was signed for, so
//!    a second create of the same name there is a 409 and the same name in another
//!    region is a different rule;
//! 3. `ListCentralizationRulesForOrganization` reports the rules created in the
//!    signing region, or — when `AllRegions` is set — those of every region, narrowed
//!    to the names `RuleNamePrefix` matches, in `(CreatedRegion, RuleName)` order and
//!    one `MaxResults`-sized page at a time;
//! 4. every member of the stored rule that targets a modelled enum carries one of
//!    that enum's own values, and every member targeting `Region` names a region of
//!    the caller's partition; anything else is a 400;
//! 5. a rule is `Provisioning` until the provisioning its create or update started
//!    comes due, and `Healthy` after it.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Map, Value};
use service_com_amazonaws_observabilityadmin_observabilityadmin::state::CENTRALIZATION_PROVISIONING_DELAY_MS;

use super::telemetry_rules::{call, emulator, error_code, Emulator, ACCOUNT};

/// The region most requests below sign for; the ARNs they mint carry exactly this one.
const HOME_REGION: &str = "us-west-2";

/// Long enough after a create or an update for the rule's provisioning to have come
/// due, whatever scheduling delay the test run adds on top.
fn past_provisioning_delay() -> Duration {
    Duration::from_millis(CENTRALIZATION_PROVISIONING_DELAY_MS as u64 + 250)
}

async fn create(emulator: &Emulator, region: &str, name: &str, rule: &Value) -> (u16, Value) {
    call(
        emulator,
        region,
        "/CreateCentralizationRuleForOrganization",
        &json!({"RuleName": name, "Rule": rule}),
    )
    .await
}

async fn create_with_tags(
    emulator: &Emulator,
    region: &str,
    name: &str,
    rule: &Value,
    tags: &Value,
) -> (u16, Value) {
    call(
        emulator,
        region,
        "/CreateCentralizationRuleForOrganization",
        &json!({"RuleName": name, "Rule": rule, "Tags": tags}),
    )
    .await
}

async fn get(emulator: &Emulator, region: &str, identifier: &str) -> (u16, Value) {
    call(
        emulator,
        region,
        "/GetCentralizationRuleForOrganization",
        &json!({"RuleIdentifier": identifier}),
    )
    .await
}

async fn update(
    emulator: &Emulator,
    region: &str,
    identifier: &str,
    rule: &Value,
) -> (u16, Value) {
    call(
        emulator,
        region,
        "/UpdateCentralizationRuleForOrganization",
        &json!({"RuleIdentifier": identifier, "Rule": rule}),
    )
    .await
}

async fn delete(emulator: &Emulator, region: &str, identifier: &str) -> (u16, Value) {
    call(
        emulator,
        region,
        "/DeleteCentralizationRuleForOrganization",
        &json!({"RuleIdentifier": identifier}),
    )
    .await
}

async fn list(emulator: &Emulator, region: &str, body: &Value) -> (u16, Value) {
    call(
        emulator,
        region,
        "/ListCentralizationRulesForOrganization",
        body,
    )
    .await
}

/// The `RuleArn` a successful create or update reported.
fn rule_arn(body: &Value) -> String {
    body.get("RuleArn")
        .and_then(Value::as_str)
        .unwrap_or("<no RuleArn>")
        .to_string()
}

/// The ARN the service must mint for `name` created in `region`.
fn expected_arn(region: &str, name: &str) -> String {
    format!("arn:aws:observabilityadmin:{region}:{ACCOUNT}:centralization-rule/{name}")
}

/// One summary member, as a string, or a marker naming what was missing.
fn field(value: &Value, member: &str) -> String {
    value
        .get(member)
        .map(|found| match found {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        })
        .unwrap_or_else(|| format!("<no {member}>"))
}

/// Every stored summary, read page by page with `max_results` per page, as
/// `(CreatedRegion, RuleName)` pairs in the order the pages delivered them. Also
/// asserts no page is larger than it may be and that paging terminates.
async fn paginate(
    emulator: &Emulator,
    region: &str,
    prefix: Option<&str>,
    all_regions: bool,
    max_results: Option<i64>,
) -> Vec<(String, String)> {
    let case = format!("region={region} prefix={prefix:?} all_regions={all_regions} max_results={max_results:?}");
    let mut collected = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..64 {
        let mut body = Map::new();
        if let Some(prefix) = prefix {
            body.insert("RuleNamePrefix".to_string(), json!(prefix));
        }
        if all_regions {
            body.insert("AllRegions".to_string(), json!(true));
        }
        if let Some(max_results) = max_results {
            body.insert("MaxResults".to_string(), json!(max_results));
        }
        if let Some(token) = &token {
            body.insert("NextToken".to_string(), json!(token));
        }
        let (status, response) = list(emulator, region, &Value::Object(body)).await;
        assert_eq!(
            status, 200,
            "case {case:?} page {page}: expected 200, got {status} {response:?}"
        );
        let summaries = response
            .get("CentralizationRuleSummaries")
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!("case {case:?} page {page}: expected CentralizationRuleSummaries, got {response:?}")
            })
            .clone();
        if let Some(max_results) = max_results {
            assert!(
                summaries.len() as i64 <= max_results,
                "case {case:?} page {page}: expected at most {max_results} summaries, got {}",
                summaries.len()
            );
        }
        for summary in &summaries {
            collected.push((field(summary, "CreatedRegion"), field(summary, "RuleName")));
        }
        token = response
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            return collected;
        }
    }
    panic!("case {case:?}: ListCentralizationRulesForOrganization never stopped handing out a NextToken");
}

/// The minimal rule body: the regions it centralizes telemetry FROM and the region it
/// centralizes INTO, the two members `CentralizationRule` makes required.
fn minimal_rule(source_regions: &[&str], destination_region: &str) -> Value {
    json!({
        "Source": {"Regions": source_regions},
        "Destination": {"Region": destination_region},
    })
}

/// A rule body whose source asks for context graph centralization, which is what makes
/// `ContextGraphStatus` apply to it.
fn context_graph_rule() -> Value {
    json!({
        "Source": {
            "Regions": [HOME_REGION],
            "SourceContextGraphConfiguration": {},
        },
        "Destination": {"Region": "us-east-1"},
    })
}

/// A rule body whose destination configures tag propagation, which is what makes
/// `TagPropagationStatus` apply to it.
fn tag_propagation_rule() -> Value {
    json!({
        "Source": {"Regions": [HOME_REGION]},
        "Destination": {
            "Region": "us-east-1",
            "DestinationLogsConfiguration": {
                "TagPropagationConfiguration": {
                    "DestinationRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/Centralization"),
                    "TagConflictResolutionStrategy": "IN_SYNC",
                },
            },
        },
    })
}

/// One rule body to store and read back. The rows differ along what the round-trip
/// rule generalises over: how many source regions the rule names, whether it carries
/// the optional scalar members at all, which of the logs / metrics / context-graph
/// configurations it nests on each side, and whether it names a destination account.
fn round_trip_rules() -> Vec<(&'static str, Value)> {
    vec![
        (
            "only-the-required-members",
            minimal_rule(&[HOME_REGION], "us-east-1"),
        ),
        (
            "many-source-regions",
            minimal_rule(&["eu-west-1", "us-east-1", "us-east-2", HOME_REGION], "eu-central-1"),
        ),
        (
            "source-logs-and-scope",
            json!({
                "Source": {
                    "Regions": [HOME_REGION, "us-east-1"],
                    "Scope": "Tags.Environment = 'production'",
                    "SourceLogsConfiguration": {
                        "LogGroupSelectionCriteria": "LogGroupName LIKE '/aws/lambda/%'",
                        "DataSourceSelectionCriteria": "DataSource IN ('lambda')",
                        "EncryptedLogGroupStrategy": "SKIP",
                    },
                },
                "Destination": {"Region": "us-east-1", "Account": ACCOUNT},
            }),
        ),
        (
            "source-metrics-only",
            json!({
                "Source": {
                    "Regions": ["us-east-1"],
                    "SourceMetricsConfiguration": {
                        "MetricsSelectionCriteria": "Namespace = 'AWS/EC2'",
                    },
                },
                "Destination": {"Region": HOME_REGION},
            }),
        ),
        ("source-context-graph", context_graph_rule()),
        ("destination-tag-propagation", tag_propagation_rule()),
        (
            "every-destination-configuration",
            json!({
                "Source": {
                    "Regions": [HOME_REGION],
                    "SourceLogsConfiguration": {
                        "LogGroupSelectionCriteria": "LogGroupName LIKE '/aws/%'",
                        "EncryptedLogGroupStrategy": "ALLOW",
                    },
                    "SourceMetricsConfiguration": {"MetricsSelectionCriteria": "Namespace LIKE 'AWS/%'"},
                    "SourceContextGraphConfiguration": {},
                },
                "Destination": {
                    "Region": "us-east-1",
                    "Account": ACCOUNT,
                    "DestinationLogsConfiguration": {
                        "LogsEncryptionConfiguration": {
                            "EncryptionStrategy": "CUSTOMER_MANAGED",
                            "KmsKeyArn": format!("arn:aws:kms:us-east-1:{ACCOUNT}:key/11111111-2222-3333-4444-555555555555"),
                            "EncryptionConflictResolutionStrategy": "ALLOW",
                            "EncryptionScope": "NEW_DESTINATION_LOG_GROUPS",
                        },
                        "BackupConfiguration": {
                            "Region": "us-east-2",
                            "KmsKeyArn": format!("arn:aws:kms:us-east-2:{ACCOUNT}:key/66666666-7777-8888-9999-000000000000"),
                        },
                        "LogGroupNameConfiguration": {
                            "LogGroupNamePattern": "centralized/${aws.accountId}",
                        },
                        "TagPropagationConfiguration": {
                            "DestinationRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/Centralization"),
                            "TagConflictResolutionStrategy": "UPDATE_SYNC",
                        },
                        "LogGroupDeletionProtection": "ENABLED",
                    },
                    "DestinationMetricsConfiguration": {
                        "BackupConfiguration": {"Region": "us-east-2"},
                    },
                },
            }),
        ),
    ]
}

#[tokio::test]
async fn a_created_rule_is_what_every_later_read_reports() {
    let emulator = emulator();

    for (name, rule) in round_trip_rules() {
        let (status, created) = create(&emulator, HOME_REGION, name, &rule).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200 from the create, got {status} {created:?}"
        );
        assert_eq!(
            rule_arn(&created),
            expected_arn(HOME_REGION, name),
            "case {name:?}: expected the minted ARN, got {created:?}"
        );

        let (status, read) = get(&emulator, HOME_REGION, name).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200 from the read-back, got {status} {read:?}"
        );
        assert_eq!(
            read.get("CentralizationRule"),
            Some(&rule),
            "case {name:?}: expected the stored rule body, got {read:?}"
        );
        assert_eq!(
            field(&read, "RuleName"),
            name,
            "case {name:?}: expected the stored RuleName, got {read:?}"
        );
        assert_eq!(
            field(&read, "RuleArn"),
            expected_arn(HOME_REGION, name),
            "case {name:?}: expected the stored RuleArn, got {read:?}"
        );
        assert_eq!(
            field(&read, "CreatedRegion"),
            HOME_REGION,
            "case {name:?}: expected the signing region as CreatedRegion, got {read:?}"
        );
        assert_eq!(
            field(&read, "CreatorAccountId"),
            ACCOUNT,
            "case {name:?}: expected the emulator account as CreatorAccountId, got {read:?}"
        );
        let created_at = read
            .get("CreatedTimeStamp")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("case {name:?}: expected CreatedTimeStamp, got {read:?}"));
        assert!(
            created_at > 0,
            "case {name:?}: expected an epoch-millisecond CreatedTimeStamp, got {created_at}"
        );
        assert_eq!(
            read.get("LastUpdateTimeStamp").and_then(Value::as_i64),
            Some(created_at),
            "case {name:?}: expected a freshly created rule's LastUpdateTimeStamp to be its \
             CreatedTimeStamp, got {read:?}"
        );

        // The ARN is the other form `RuleIdentifier` accepts, and it has to resolve
        // the very same record.
        let (status, by_arn) = get(&emulator, HOME_REGION, &expected_arn(HOME_REGION, name)).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200 reading the rule by its ARN, got {status} {by_arn:?}"
        );
        assert_eq!(
            by_arn, read,
            "case {name:?}: expected the ARN to resolve the same record as the name, got {by_arn:?}"
        );
    }

    // Whatever each create stored is also what the list reports, as the one summary
    // per rule its own members describe.
    let (status, listed) = list(&emulator, HOME_REGION, &json!({})).await;
    assert_eq!(
        status, 200,
        "expected 200 from the list, got {status} {listed:?}"
    );
    let summaries = listed
        .get("CentralizationRuleSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected CentralizationRuleSummaries, got {listed:?}"))
        .clone();
    assert_eq!(
        summaries.len(),
        round_trip_rules().len(),
        "expected one summary per created rule, got {summaries:?}"
    );
    for (name, rule) in round_trip_rules() {
        let summary = summaries
            .iter()
            .find(|summary| field(summary, "RuleName") == name)
            .unwrap_or_else(|| panic!("case {name:?}: expected a summary for it, got {summaries:?}"));
        let (_, read) = get(&emulator, HOME_REGION, name).await;
        for member in [
            "RuleName",
            "RuleArn",
            "CreatorAccountId",
            "CreatedTimeStamp",
            "CreatedRegion",
            "LastUpdateTimeStamp",
            "RuleHealth",
            "TagPropagationStatus",
            "ContextGraphStatus",
        ] {
            assert_eq!(
                summary.get(member),
                read.get(member),
                "case {name:?}: expected the summary's {member} to be the Get's, got {summary:?}"
            );
        }
        // The two members only a summary carries name the destination the stored rule
        // body names, and the creator's own account when the body names none.
        let destination = rule.get("Destination").expect("a destination");
        assert_eq!(
            field(summary, "DestinationRegion"),
            field(destination, "Region"),
            "case {name:?}: expected the stored destination region, got {summary:?}"
        );
        let expected_account = destination
            .get("Account")
            .and_then(Value::as_str)
            .unwrap_or(ACCOUNT);
        assert_eq!(
            field(summary, "DestinationAccountId"),
            expected_account,
            "case {name:?}: expected the destination account, got {summary:?}"
        );
    }
}

#[tokio::test]
async fn a_rule_is_provisioning_until_its_provisioning_comes_due() {
    // Each row differs along which of the two independent statuses applies to it:
    // `ContextGraphStatus` only for a rule whose source asks for context graph
    // centralization, `TagPropagationStatus` only for one whose destination configures
    // tag propagation.
    let rows: Vec<(&str, Value, bool, bool)> = vec![
        (
            "plain",
            minimal_rule(&[HOME_REGION], "us-east-1"),
            false,
            false,
        ),
        ("context-graph", context_graph_rule(), true, false),
        ("tag-propagation", tag_propagation_rule(), false, true),
    ];

    let emulator = emulator();
    for (name, rule, context_graph, tag_propagation) in &rows {
        let (status, created) = create(&emulator, HOME_REGION, name, rule).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200 from the create, got {status} {created:?}"
        );

        // A caller that reads straight after the create still sees the transient
        // status: the span the provisioning takes is never zero.
        let (_, read) = get(&emulator, HOME_REGION, name).await;
        assert_eq!(
            field(&read, "RuleHealth"),
            "Provisioning",
            "case {name:?}: expected a freshly created rule to be Provisioning, got {read:?}"
        );
        assert_eq!(
            read.get("ContextGraphStatus").and_then(Value::as_str),
            context_graph.then_some("Provisioning"),
            "case {name:?}: expected ContextGraphStatus only for a context-graph rule, \
             got {read:?}"
        );
        assert_eq!(
            read.get("TagPropagationStatus").and_then(Value::as_str),
            tag_propagation.then_some("Healthy"),
            "case {name:?}: expected TagPropagationStatus only for a tag-propagating rule, \
             got {read:?}"
        );
        // Nothing failed, so neither failure reason is reported.
        assert_eq!(
            read.get("FailureReason"),
            None,
            "case {name:?}: expected no FailureReason, got {read:?}"
        );
        assert_eq!(
            read.get("TagPropagationFailureReason"),
            None,
            "case {name:?}: expected no TagPropagationFailureReason, got {read:?}"
        );
    }

    tokio::time::sleep(past_provisioning_delay()).await;

    for (name, _, context_graph, _) in &rows {
        let (_, read) = get(&emulator, HOME_REGION, name).await;
        assert_eq!(
            field(&read, "RuleHealth"),
            "Healthy",
            "case {name:?}: expected the rule to be Healthy once provisioning came due, \
             got {read:?}"
        );
        assert_eq!(
            read.get("ContextGraphStatus").and_then(Value::as_str),
            context_graph.then_some("Healthy"),
            "case {name:?}: expected the context graph to be Healthy once provisioning came \
             due, got {read:?}"
        );
        // The list reports the same health as the Get: both read the one record.
        let (_, listed) = list(&emulator, HOME_REGION, &json!({"RuleNamePrefix": name})).await;
        let summaries = listed
            .get("CentralizationRuleSummaries")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {name:?}: expected summaries, got {listed:?}"));
        assert_eq!(
            summaries.len(),
            1,
            "case {name:?}: expected exactly its own summary, got {summaries:?}"
        );
        assert_eq!(
            summaries[0].get("RuleHealth"),
            read.get("RuleHealth"),
            "case {name:?}: expected the summary's health to be the Get's, got {summaries:?}"
        );
    }
}

/// The rules every listing case below reads, as `(region, name)`, created in that
/// order. Two regions hold a rule of the same name on purpose: a name identifies a
/// rule within its region only.
fn listing_fixture() -> Vec<(&'static str, &'static str)> {
    vec![
        (HOME_REGION, "prod-logs"),
        (HOME_REGION, "prod-metrics"),
        (HOME_REGION, "dev-logs"),
        ("us-east-1", "prod-logs"),
        ("us-east-1", "east-only"),
        ("eu-west-1", "eu-logs"),
    ]
}

async fn seeded_listing_emulator() -> Emulator {
    let emulator = emulator();
    for (region, name) in listing_fixture() {
        let (status, created) = create(
            &emulator,
            region,
            name,
            &minimal_rule(&[region], "us-east-1"),
        )
        .await;
        assert_eq!(
            status, 200,
            "seeding {region}/{name}: expected 200, got {status} {created:?}"
        );
    }
    emulator
}

#[tokio::test]
async fn a_list_spans_every_region_only_when_all_regions_asks_it_to() {
    let emulator = seeded_listing_emulator().await;

    // Rows differ along the signing region and along the `AllRegions` flag's three
    // forms: absent, false and true.
    for region in ["us-west-2", "us-east-1", "eu-west-1", "ap-south-1"] {
        let own: Vec<(String, String)> = listing_fixture()
            .into_iter()
            .filter(|(rule_region, _)| *rule_region == region)
            .map(|(rule_region, name)| (rule_region.to_string(), name.to_string()))
            .collect();
        let mut expected_own = own.clone();
        expected_own.sort();

        for flag in [None, Some(false)] {
            let mut body = Map::new();
            if let Some(flag) = flag {
                body.insert("AllRegions".to_string(), json!(flag));
            }
            let (status, listed) = list(&emulator, region, &Value::Object(body)).await;
            assert_eq!(
                status, 200,
                "case region={region} AllRegions={flag:?}: expected 200, got {status} {listed:?}"
            );
            let reported: Vec<(String, String)> = listed
                .get("CentralizationRuleSummaries")
                .and_then(Value::as_array)
                .unwrap_or_else(|| {
                    panic!("case region={region} AllRegions={flag:?}: expected summaries, got {listed:?}")
                })
                .iter()
                .map(|summary| (field(summary, "CreatedRegion"), field(summary, "RuleName")))
                .collect();
            assert_eq!(
                reported, expected_own,
                "case region={region} AllRegions={flag:?}: expected only this region's rules, \
                 name-ordered"
            );
        }

        let mut expected_all: Vec<(String, String)> = listing_fixture()
            .into_iter()
            .map(|(rule_region, name)| (rule_region.to_string(), name.to_string()))
            .collect();
        expected_all.sort();
        let reported = paginate(&emulator, region, None, true, None).await;
        assert_eq!(
            reported, expected_all,
            "case region={region} AllRegions=true: expected every region's rules in \
             (CreatedRegion, RuleName) order"
        );
    }
}

#[tokio::test]
async fn a_name_prefix_narrows_a_list_to_the_names_it_matches() {
    let emulator = seeded_listing_emulator().await;

    // Rows differ along the prefix's length and selectivity: a whole name, a shared
    // stem, a one-character stem, and one no name starts with.
    for prefix in ["prod-logs", "prod-", "p", "d", "e", "nothing-starts-with-this"] {
        for all_regions in [false, true] {
            let mut expected: Vec<(String, String)> = listing_fixture()
                .into_iter()
                .filter(|(region, name)| {
                    name.starts_with(prefix) && (all_regions || *region == HOME_REGION)
                })
                .map(|(region, name)| (region.to_string(), name.to_string()))
                .collect();
            expected.sort();
            let reported =
                paginate(&emulator, HOME_REGION, Some(prefix), all_regions, None).await;
            assert_eq!(
                reported, expected,
                "case prefix={prefix:?} all_regions={all_regions}: expected exactly the \
                 matching rules"
            );
        }
    }
}

#[tokio::test]
async fn pagination_walks_every_rule_once_and_terminates() {
    let emulator = seeded_listing_emulator().await;

    // Rows differ along the page size: every size from one rule per page up to the
    // modelled maximum, plus the request that names none at all.
    for max_results in [None, Some(1), Some(2), Some(3), Some(5), Some(100)] {
        for all_regions in [false, true] {
            let mut expected: Vec<(String, String)> = listing_fixture()
                .into_iter()
                .filter(|(region, _)| all_regions || *region == HOME_REGION)
                .map(|(region, name)| (region.to_string(), name.to_string()))
                .collect();
            expected.sort();
            let reported =
                paginate(&emulator, HOME_REGION, None, all_regions, max_results).await;
            assert_eq!(
                reported, expected,
                "case max_results={max_results:?} all_regions={all_regions}: expected every \
                 rule exactly once, in key order"
            );
            let unique: BTreeSet<(String, String)> = reported.iter().cloned().collect();
            assert_eq!(
                unique.len(),
                reported.len(),
                "case max_results={max_results:?} all_regions={all_regions}: expected no rule \
                 on two pages, got {reported:?}"
            );
        }
    }

    // A token this operation never issued is a modelled 400, not a page.
    for token in [
        "not-hex".to_string(),
        "00".to_string(),
        "7c".to_string(),
        hex::encode("us-west-2|"),
        hex::encode("|prod-logs"),
        hex::encode("us-west-2/prod-logs"),
    ] {
        let (status, response) = list(
            &emulator,
            HOME_REGION,
            &json!({"NextToken": token.to_string()}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case token={token:?}: expected 400, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case token={token:?}: expected a ValidationException, got {response:?}"
        );
    }
}

#[tokio::test]
async fn a_second_create_of_one_name_in_one_region_conflicts() {
    let emulator = emulator();
    let name = "shared-name";
    let first = minimal_rule(&[HOME_REGION], "us-east-1");
    let (status, created) = create(&emulator, HOME_REGION, name, &first).await;
    assert_eq!(
        status, 200,
        "expected 200 from the first create, got {status} {created:?}"
    );

    let second = minimal_rule(&["eu-west-1"], "eu-central-1");
    let (status, conflict) = create(&emulator, HOME_REGION, name, &second).await;
    assert_eq!(
        status, 409,
        "case same-region: expected 409, got {status} {conflict:?}"
    );
    assert_eq!(
        error_code(&conflict),
        "ConflictException",
        "case same-region: expected a ConflictException, got {conflict:?}"
    );

    // The refused create changed nothing.
    let (_, read) = get(&emulator, HOME_REGION, name).await;
    assert_eq!(
        read.get("CentralizationRule"),
        Some(&first),
        "case same-region: expected the first rule body to survive the refusal, got {read:?}"
    );

    // The same name in another region is another rule, with its own ARN and body.
    let (status, elsewhere) = create(&emulator, "us-east-1", name, &second).await;
    assert_eq!(
        status, 200,
        "case other-region: expected 200, got {status} {elsewhere:?}"
    );
    assert_eq!(
        rule_arn(&elsewhere),
        expected_arn("us-east-1", name),
        "case other-region: expected an ARN in that region, got {elsewhere:?}"
    );
    let (_, read_elsewhere) = get(&emulator, "us-east-1", name).await;
    assert_eq!(
        read_elsewhere.get("CentralizationRule"),
        Some(&second),
        "case other-region: expected its own rule body, got {read_elsewhere:?}"
    );
}

#[tokio::test]
async fn an_update_replaces_the_body_and_keeps_the_rule_s_identity() {
    let emulator = emulator();
    let name = "updated-rule";
    let original = minimal_rule(&[HOME_REGION], "us-east-1");
    let (status, created) = create_with_tags(
        &emulator,
        HOME_REGION,
        name,
        &original,
        &json!({"Team": "observability"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected 200 from the create, got {status} {created:?}"
    );
    let (_, before) = get(&emulator, HOME_REGION, name).await;
    let created_at = before
        .get("CreatedTimeStamp")
        .and_then(Value::as_i64)
        .expect("a CreatedTimeStamp");

    // Rows differ along what the replacement changes: the source regions, the nested
    // configurations, and the destination it centralizes into.
    let replacements: Vec<(&str, Value)> = vec![
        (
            "more-source-regions",
            minimal_rule(&[HOME_REGION, "us-east-1", "us-east-2"], "us-east-1"),
        ),
        ("adds-context-graph", context_graph_rule()),
        ("adds-tag-propagation", tag_propagation_rule()),
        (
            "another-destination",
            minimal_rule(&["eu-west-1"], "eu-central-1"),
        ),
    ];

    for (case, replacement) in replacements {
        tokio::time::sleep(past_provisioning_delay()).await;
        let (status, updated) = update(&emulator, HOME_REGION, name, &replacement).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the update, got {status} {updated:?}"
        );
        assert_eq!(
            rule_arn(&updated),
            expected_arn(HOME_REGION, name),
            "case {case:?}: expected the update to keep the rule's ARN, got {updated:?}"
        );

        let (_, read) = get(&emulator, HOME_REGION, name).await;
        assert_eq!(
            read.get("CentralizationRule"),
            Some(&replacement),
            "case {case:?}: expected the replacement body, got {read:?}"
        );
        assert_eq!(
            read.get("CreatedTimeStamp").and_then(Value::as_i64),
            Some(created_at),
            "case {case:?}: expected CreatedTimeStamp to survive the update, got {read:?}"
        );
        let last_update = read
            .get("LastUpdateTimeStamp")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("case {case:?}: expected LastUpdateTimeStamp, got {read:?}"));
        assert!(
            last_update > created_at,
            "case {case:?}: expected the update to bump LastUpdateTimeStamp past \
             {created_at}, got {last_update}"
        );
        // The replacement has its own provisioning to do, so the rule is transiently
        // Provisioning again and comes due Healthy.
        assert_eq!(
            field(&read, "RuleHealth"),
            "Provisioning",
            "case {case:?}: expected the updated rule to be Provisioning, got {read:?}"
        );
        tokio::time::sleep(past_provisioning_delay()).await;
        let (_, settled) = get(&emulator, HOME_REGION, name).await;
        assert_eq!(
            field(&settled, "RuleHealth"),
            "Healthy",
            "case {case:?}: expected the updated rule to settle Healthy, got {settled:?}"
        );
    }
}

#[tokio::test]
async fn a_deleted_rule_is_gone_for_every_later_reader() {
    let emulator = seeded_listing_emulator().await;

    let (status, deleted) = delete(&emulator, HOME_REGION, "prod-logs").await;
    assert_eq!(
        status, 200,
        "expected 200 from the delete, got {status} {deleted:?}"
    );

    let (status, read) = get(&emulator, HOME_REGION, "prod-logs").await;
    assert_eq!(
        status, 404,
        "case get-after-delete: expected 404, got {status} {read:?}"
    );
    assert_eq!(
        error_code(&read),
        "ResourceNotFoundException",
        "case get-after-delete: expected a ResourceNotFoundException, got {read:?}"
    );

    let (status, repeat) = delete(&emulator, HOME_REGION, "prod-logs").await;
    assert_eq!(
        status, 404,
        "case delete-twice: expected 404, got {status} {repeat:?}"
    );

    let mut expected: Vec<(String, String)> = listing_fixture()
        .into_iter()
        .filter(|(region, name)| !(*region == HOME_REGION && *name == "prod-logs"))
        .map(|(region, name)| (region.to_string(), name.to_string()))
        .collect();
    expected.sort();
    let reported = paginate(&emulator, HOME_REGION, None, true, Some(2)).await;
    assert_eq!(
        reported, expected,
        "case list-after-delete: expected every surviving rule and nothing else"
    );

    // The same name in another region is another rule, which this delete left alone.
    let (status, survivor) = get(&emulator, "us-east-1", "prod-logs").await;
    assert_eq!(
        status, 200,
        "case other-region-survives: expected 200, got {status} {survivor:?}"
    );
}

#[tokio::test]
async fn an_unknown_identifier_is_a_modelled_404() {
    let emulator = emulator();
    let (status, created) = create(
        &emulator,
        HOME_REGION,
        "known",
        &minimal_rule(&[HOME_REGION], "us-east-1"),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected 200 from the create, got {status} {created:?}"
    );

    // Rows differ along HOW the identifier misses: a name nothing was stored under, a
    // name stored in another region, an ARN of this rule in another region, an ARN in
    // another account, and an ARN of another service's resource.
    let cases = [
        ("unknown-name", "no-such-rule".to_string()),
        ("other-region-name", "east-only".to_string()),
        ("other-region-arn", expected_arn("us-east-1", "known")),
        (
            "other-account-arn",
            format!("arn:aws:observabilityadmin:{HOME_REGION}:999999999999:centralization-rule/known"),
        ),
        (
            "telemetry-rule-arn",
            format!("arn:aws:observabilityadmin:{HOME_REGION}:{ACCOUNT}:telemetry-rule/known"),
        ),
    ];

    for (case, identifier) in cases {
        for (operation, answer) in [
            ("Get", get(&emulator, HOME_REGION, &identifier).await),
            (
                "Update",
                update(
                    &emulator,
                    HOME_REGION,
                    &identifier,
                    &minimal_rule(&[HOME_REGION], "us-east-1"),
                )
                .await,
            ),
            ("Delete", delete(&emulator, HOME_REGION, &identifier).await),
        ] {
            let (status, body) = answer;
            assert_eq!(
                status, 404,
                "case {case:?} on {operation}: expected 404, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?} on {operation}: expected a ResourceNotFoundException, got {body:?}"
            );
        }
    }

    // The rule that IS there was untouched by any of those refusals.
    let (status, read) = get(&emulator, HOME_REGION, "known").await;
    assert_eq!(
        status, 200,
        "expected the stored rule to survive, got {status} {read:?}"
    );
}

/// Every modelled enum a `CentralizationRule` carries, as
/// `(case, the JSON path to it, that enum's own values)`.
///
/// The value catalogs are exactly what
/// `smithy_query(query="shape",shape="<Enum>")` reports for
/// `EncryptedLogGroupStrategy`, `EncryptionStrategy`,
/// `EncryptionConflictResolutionStrategy`, `EncryptionScope`, `DeletionProtection` and
/// `TagConflictResolutionStrategy`.
fn enum_members() -> Vec<(&'static str, Vec<&'static str>, &'static [&'static str])> {
    vec![
        (
            "EncryptedLogGroupStrategy",
            vec![
                "Source",
                "SourceLogsConfiguration",
                "EncryptedLogGroupStrategy",
            ],
            &["ALLOW", "SKIP"],
        ),
        (
            "EncryptionStrategy",
            vec![
                "Destination",
                "DestinationLogsConfiguration",
                "LogsEncryptionConfiguration",
                "EncryptionStrategy",
            ],
            &["CUSTOMER_MANAGED", "AWS_OWNED"],
        ),
        (
            "EncryptionConflictResolutionStrategy",
            vec![
                "Destination",
                "DestinationLogsConfiguration",
                "LogsEncryptionConfiguration",
                "EncryptionConflictResolutionStrategy",
            ],
            &["ALLOW", "SKIP"],
        ),
        (
            "EncryptionScope",
            vec![
                "Destination",
                "DestinationLogsConfiguration",
                "LogsEncryptionConfiguration",
                "EncryptionScope",
            ],
            &["ENCRYPTED_SOURCE_ONLY", "NEW_DESTINATION_LOG_GROUPS"],
        ),
        (
            "LogGroupDeletionProtection",
            vec![
                "Destination",
                "DestinationLogsConfiguration",
                "LogGroupDeletionProtection",
            ],
            &["ENABLED"],
        ),
        (
            "TagConflictResolutionStrategy",
            vec![
                "Destination",
                "DestinationLogsConfiguration",
                "TagPropagationConfiguration",
                "TagConflictResolutionStrategy",
            ],
            &["IN_SYNC", "ADD_ONLY", "UPDATE_SYNC"],
        ),
    ]
}

/// A rule body carrying `value` at `path`, on top of the members each enclosing
/// structure makes required.
fn rule_with(path: &[&str], value: &str) -> Value {
    let mut rule = json!({
        "Source": {
            "Regions": [HOME_REGION],
            "SourceLogsConfiguration": {
                "LogGroupSelectionCriteria": "LogGroupName LIKE '/aws/%'",
                "EncryptedLogGroupStrategy": "ALLOW",
            },
        },
        "Destination": {
            "Region": "us-east-1",
            "DestinationLogsConfiguration": {
                "LogsEncryptionConfiguration": {"EncryptionStrategy": "AWS_OWNED"},
                "TagPropagationConfiguration": {
                    "DestinationRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/Centralization"),
                },
            },
        },
    });
    let mut cursor = &mut rule;
    for segment in &path[..path.len() - 1] {
        cursor = cursor
            .get_mut(*segment)
            .unwrap_or_else(|| panic!("the fixture carries {segment}"));
    }
    cursor[path[path.len() - 1]] = json!(value);
    rule
}

#[tokio::test]
async fn every_enum_member_accepts_its_own_values_and_refuses_anything_else() {
    let emulator = emulator();

    for (member, path, catalog) in enum_members() {
        // Every value the enum publishes is stored and read back verbatim — including
        // the ones no recorded request carries.
        for value in catalog {
            let name = format!("{member}-{value}");
            let rule = rule_with(&path, value);
            let (status, created) = create(&emulator, HOME_REGION, &name, &rule).await;
            assert_eq!(
                status, 200,
                "case {member}={value}: expected 200, got {status} {created:?}"
            );
            let (_, read) = get(&emulator, HOME_REGION, &name).await;
            assert_eq!(
                read.get("CentralizationRule"),
                Some(&rule),
                "case {member}={value}: expected the stored body to carry it, got {read:?}"
            );
        }

        // Anything the enum does not publish is a 400, on the create and on the update
        // alike, and leaves the store as it was.
        for value in ["", "healthy", "ALLOW_ALL", "allow"] {
            if catalog.contains(&value) {
                continue;
            }
            let name = format!("{member}-rejected");
            let rule = rule_with(&path, value);
            let (status, refused) = create(&emulator, HOME_REGION, &name, &rule).await;
            assert_eq!(
                status, 400,
                "case {member}={value:?}: expected 400 from the create, got {status} {refused:?}"
            );
            assert_eq!(
                error_code(&refused),
                "ValidationException",
                "case {member}={value:?}: expected a ValidationException, got {refused:?}"
            );
            let (status, read) = get(&emulator, HOME_REGION, &name).await;
            assert_eq!(
                status, 404,
                "case {member}={value:?}: expected the refused create to store nothing, \
                 got {status} {read:?}"
            );

            let good = format!("{member}-{}", catalog[0]);
            let (status, refused) = update(&emulator, HOME_REGION, &good, &rule).await;
            assert_eq!(
                status, 400,
                "case {member}={value:?}: expected 400 from the update, got {status} {refused:?}"
            );
            let (_, survivor) = get(&emulator, HOME_REGION, &good).await;
            assert_eq!(
                survivor.get("CentralizationRule"),
                Some(&rule_with(&path, catalog[0])),
                "case {member}={value:?}: expected the refused update to change nothing, \
                 got {survivor:?}"
            );
        }
    }
}

#[tokio::test]
async fn every_region_member_must_name_a_region_of_the_caller_s_partition() {
    let emulator = emulator();

    // Rows differ along WHICH region member carries the unusable value: the source
    // regions the rule centralizes from, the destination it centralizes into, and each
    // backup region on the destination side.
    let refused: Vec<(&str, Value)> = vec![
        (
            "source-region",
            minimal_rule(&["not-a-region"], "us-east-1"),
        ),
        (
            "source-region-other-partition",
            minimal_rule(&["cn-north-1"], "us-east-1"),
        ),
        (
            "destination-region",
            minimal_rule(&[HOME_REGION], "us-east-99"),
        ),
        (
            "destination-region-other-partition",
            minimal_rule(&[HOME_REGION], "us-gov-west-1"),
        ),
        (
            "logs-backup-region",
            json!({
                "Source": {"Regions": [HOME_REGION]},
                "Destination": {
                    "Region": "us-east-1",
                    "DestinationLogsConfiguration": {
                        "BackupConfiguration": {"Region": "elsewhere"},
                    },
                },
            }),
        ),
        (
            "metrics-backup-region",
            json!({
                "Source": {"Regions": [HOME_REGION]},
                "Destination": {
                    "Region": "us-east-1",
                    "DestinationMetricsConfiguration": {
                        "BackupConfiguration": {"Region": "elsewhere"},
                    },
                },
            }),
        ),
    ];

    for (case, rule) in refused {
        let (status, response) = create(&emulator, HOME_REGION, case, &rule).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {response:?}"
        );
        let (status, read) = get(&emulator, HOME_REGION, case).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the refusal to store nothing, got {status} {read:?}"
        );
    }

    // A region of the caller's own partition is accepted wherever one is accepted —
    // including regions no recorded request names.
    let accepted: Vec<(&str, Value)> = vec![
        (
            "ap-region",
            minimal_rule(&["ap-southeast-2"], "ap-northeast-1"),
        ),
        (
            "sa-region",
            minimal_rule(&["sa-east-1", "ca-central-1"], "eu-north-1"),
        ),
    ];
    for (case, rule) in accepted {
        let (status, response) = create(&emulator, HOME_REGION, case, &rule).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {response:?}"
        );
        let (_, read) = get(&emulator, HOME_REGION, case).await;
        assert_eq!(
            read.get("CentralizationRule"),
            Some(&rule),
            "case {case:?}: expected the stored body, got {read:?}"
        );
    }
}
