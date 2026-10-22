// SPDX-License-Identifier: CC0-1.0

//! What `GetAutoManagementConfiguration` reports, end to end over the wire.
//!
//! Every call here is a real `POST /` with `X-Amz-Target:
//! ServiceQuotasV20190624.GetAutoManagementConfiguration` dispatched through the same
//! `MultiServiceRouter` the emulator serves with, over ONE shared engine — so what is
//! exercised is the HTTP behaviour and the persisted state behind it.
//!
//! The rule asserted, over a TABLE of stored configurations rather than one case: the
//! read reports the account's STORED Automatic Management configuration — its opt-in
//! level, type and status, its notification ARN and its exclusion list, with every
//! excluded `QuotaCode` paired with the `QuotaName` the quota catalog holds for it —
//! it reports `OptInStatus: DISABLED` and no preferences while the account has no
//! configuration stored, and it writes nothing.
//!
//! The configurations are stored through the state API an opt-in writes
//! (`set_auto_management`), because `StartAutoManagement` / `UpdateAutoManagement` /
//! `StopAutoManagement` are still scaffolded at 501 under their own stub issues; the
//! READ under test is driven over the wire.
//!
//! The harness (`emulator`, `client`, `ACCOUNT`) is the one `quota_workflow.rs`
//! already uses, so every suite in this crate speaks to the emulator the same way.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};

use service_com_amazonaws_servicequotas_servicequotasv20190624::ops::types::{
    OPT_IN_LEVEL_VALUES, OPT_IN_STATUS_VALUES, OPT_IN_TYPE_VALUES,
};
use service_com_amazonaws_servicequotas_servicequotasv20190624::state::{
    self, AutoManagementConfig, OPT_IN_STATUS_DISABLED, OPT_IN_STATUS_ENABLED,
};

use crate::quota_workflow::{client, emulator, Client, ACCOUNT};

/// The only `OptInLevel` member the model declares, taken from the model's own value
/// list rather than spelled again here.
const ACCOUNT_LEVEL: &str = OPT_IN_LEVEL_VALUES[0];

/// The `OptInType` member `name`, checked against the model's own value list so a row
/// cannot name a type the model does not declare.
fn opt_in_type(name: &str) -> &'static str {
    OPT_IN_TYPE_VALUES
        .iter()
        .copied()
        .find(|member| *member == name)
        .unwrap_or_else(|| {
            panic!(
                "case {name}: not an OptInType member; the model declares \
                 {OPT_IN_TYPE_VALUES:?}"
            )
        })
}

/// The operation under test.
const GET_CONFIG: &str = "GetAutoManagementConfiguration";

/// A notification ARN shaped like a User Notifications configuration ARN of this
/// emulator's account, which satisfies the model's `AmazonResourceName` pattern.
fn notification_arn() -> String {
    format!("arn:aws:notifications::{ACCOUNT}:configuration/automatic-management")
}

/// One stored configuration, as an opt-in writes it.
fn configuration(
    opt_in_level: &str,
    opt_in_type: &str,
    opt_in_status: &str,
    notification_arn: Option<String>,
    exclusions: &[(&str, &[&str])],
) -> AutoManagementConfig {
    AutoManagementConfig {
        opt_in_level: opt_in_level.to_string(),
        opt_in_type: opt_in_type.to_string(),
        opt_in_status: opt_in_status.to_string(),
        notification_arn,
        exclusion_list: exclusions
            .iter()
            .map(|(service_code, quota_codes)| {
                (
                    (*service_code).to_string(),
                    quota_codes.iter().map(|code| (*code).to_string()).collect(),
                )
            })
            .collect(),
    }
}

/// Put a configuration in the shared store, the way an opt-in does.
fn store(engine: &Arc<engine::InMemoryEngine>, config: AutoManagementConfig) {
    state::service_state(&**engine)
        .expect("the Service Quotas state store is reachable")
        .lock()
        .set_auto_management(config);
}

/// Whether the shared store holds a configuration at all.
fn configuration_stored(engine: &Arc<engine::InMemoryEngine>) -> bool {
    state::service_state(&**engine)
        .expect("the Service Quotas state store is reachable")
        .lock()
        .auto_management()
        .is_some()
}

/// The `QuotaName` the quota catalog reports for a pair, read back over the wire
/// through the operation that serves the catalog, so this test carries no second copy
/// of those names. `None` for a `QuotaCode` the catalog does not know — and ONLY for
/// that: any other answer fails here rather than degrading into an absent name.
async fn catalog_quota_name(
    client: &Client,
    service_code: &str,
    quota_code: &str,
) -> Option<String> {
    let request = json!({ "ServiceCode": service_code, "QuotaCode": quota_code });
    let (status, body) = client.call("GetAWSDefaultServiceQuota", &request).await;
    match status {
        200 => {
            let name = body
                .get("Quota")
                .and_then(|quota| quota.get("QuotaName"))
                .and_then(Value::as_str);
            assert!(
                name.is_some(),
                "case catalog {service_code}/{quota_code}: expected a QuotaName, got {body}"
            );
            name.map(str::to_string)
        }
        // This awsJson front end answers every modelled client fault on 400, with the
        // shape in `__type`: a pair the catalog does not hold is
        // `NoSuchResourceException`, and nothing else is accepted as "unknown".
        400 => {
            let code = body
                .get("__type")
                .and_then(Value::as_str)
                .unwrap_or("<no __type>");
            assert!(
                code.contains("NoSuchResourceException"),
                "case catalog {service_code}/{quota_code}: expected NoSuchResourceException \
                 for a code outside the catalog, got {code} in {body}"
            );
            None
        }
        other => panic!(
            "case catalog {service_code}/{quota_code}: expected HTTP 200 or 400, got {other} \
             with {body}"
        ),
    }
}

#[tokio::test]
async fn an_account_that_never_opted_in_reads_as_disabled_and_the_read_stores_nothing() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");

    let first = client.ok(GET_CONFIG, json!({})).await;
    assert_eq!(
        first.get("OptInStatus").and_then(Value::as_str),
        Some(OPT_IN_STATUS_DISABLED),
        "case never-opted-in: expected OptInStatus DISABLED, got {first:?}"
    );
    // Nothing was ever chosen, so there is no preference to report.
    for member in ["OptInLevel", "OptInType", "NotificationArn", "ExclusionList"] {
        assert!(
            first.get(member).is_none(),
            "case never-opted-in: expected no {member}, got {:?}",
            first.get(member)
        );
    }

    // A reporting operation must not create what it reports.
    assert!(
        !configuration_stored(&engine),
        "case never-opted-in: the read stored a configuration; reading must not write"
    );
    let second = client.ok(GET_CONFIG, json!({})).await;
    assert_eq!(
        second, first,
        "case never-opted-in: expected a second read to report {first:?}, got {second:?}"
    );
}

#[tokio::test]
async fn every_modelled_opt_in_combination_reads_back_as_stored() {
    // The members come from the model's own enum value lists
    // (`ops::types::OPT_IN_LEVEL_VALUES` / `OPT_IN_TYPE_VALUES` /
    // `OPT_IN_STATUS_VALUES`, generated from `OptInLevel`, `OptInType` and
    // `OptInStatus`), so a member the model gains is covered without editing this
    // table.
    for opt_in_level in OPT_IN_LEVEL_VALUES {
        for opt_in_type in OPT_IN_TYPE_VALUES {
            for opt_in_status in OPT_IN_STATUS_VALUES {
                let case = format!("{opt_in_level}/{opt_in_type}/{opt_in_status}");
                let engine = emulator();
                let client = client(&engine, "eu-west-1");
                let arn = notification_arn();
                store(
                    &engine,
                    configuration(
                        opt_in_level,
                        opt_in_type,
                        opt_in_status,
                        Some(arn.clone()),
                        &[],
                    ),
                );

                let body = client.ok(GET_CONFIG, json!({})).await;
                assert_eq!(
                    body.get("OptInLevel").and_then(Value::as_str),
                    Some(*opt_in_level),
                    "case {case}: expected OptInLevel {opt_in_level}, got {body:?}"
                );
                assert_eq!(
                    body.get("OptInType").and_then(Value::as_str),
                    Some(*opt_in_type),
                    "case {case}: expected OptInType {opt_in_type}, got {body:?}"
                );
                assert_eq!(
                    body.get("OptInStatus").and_then(Value::as_str),
                    Some(*opt_in_status),
                    "case {case}: expected OptInStatus {opt_in_status}, got {body:?}"
                );
                assert_eq!(
                    body.get("NotificationArn").and_then(Value::as_str),
                    Some(arn.as_str()),
                    "case {case}: expected NotificationArn {arn}, got {body:?}"
                );
                // Nothing is excluded, so there is no exclusion list on the wire.
                assert!(
                    body.get("ExclusionList").is_none(),
                    "case {case}: expected no ExclusionList for an empty exclusion list, got {:?}",
                    body.get("ExclusionList")
                );
            }
        }
    }
}

#[tokio::test]
async fn a_configuration_with_no_notification_arn_reports_none() {
    let engine = emulator();
    let client = client(&engine, "sa-east-1");
    store(
        &engine,
        configuration(
            ACCOUNT_LEVEL,
            opt_in_type("NotifyOnly"),
            OPT_IN_STATUS_ENABLED,
            None,
            &[],
        ),
    );

    let body = client.ok(GET_CONFIG, json!({})).await;
    assert!(
        body.get("NotificationArn").is_none(),
        "case no-notification-arn: expected no NotificationArn, got {body:?}"
    );
    assert_eq!(
        body.get("OptInStatus").and_then(Value::as_str),
        Some(OPT_IN_STATUS_ENABLED),
        "case no-notification-arn: expected OptInStatus ENABLED, got {body:?}"
    );
}

/// One row of the exclusion-list table: the stored exclusions, keyed by service code.
struct ExclusionCase {
    case: &'static str,
    exclusions: &'static [(&'static str, &'static [&'static str])],
}

#[tokio::test]
async fn the_exclusion_list_is_reported_with_the_catalogs_quota_names() {
    // The rows differ along the number of excluded services, the number of quotas per
    // service, and whether the catalog knows the excluded code.
    let cases = [
        ExclusionCase {
            case: "one quota of one service",
            exclusions: &[("dynamodb", &["L-F98FE922"])],
        },
        ExclusionCase {
            case: "two quotas of one service",
            exclusions: &[("ec2", &["L-0263D0A3", "L-1216C47A"])],
        },
        ExclusionCase {
            case: "one quota of each of three services",
            exclusions: &[
                ("ec2", &["L-1216C47A"]),
                ("lambda", &["L-2ACBD22F"]),
                ("s3", &["L-DC2B2D3D"]),
            ],
        },
        ExclusionCase {
            case: "a code the catalog does not know, beside one it does",
            exclusions: &[("vpc", &["L-F678F1CE", "L-00000000"])],
        },
    ];

    for ExclusionCase { case, exclusions } in cases {
        let engine = emulator();
        let client = client(&engine, "us-west-2");
        store(
            &engine,
            configuration(
                ACCOUNT_LEVEL,
                opt_in_type("NotifyOnly"),
                OPT_IN_STATUS_ENABLED,
                None,
                exclusions,
            ),
        );

        let body = client.ok(GET_CONFIG, json!({})).await;
        let Some(reported) = body.get("ExclusionList").and_then(Value::as_object) else {
            panic!("case {case}: expected an ExclusionList, got {body:?}");
        };
        assert_eq!(
            reported.len(),
            exclusions.len(),
            "case {case}: expected {} excluded service(s), got {reported:?}",
            exclusions.len()
        );

        for (service_code, quota_codes) in exclusions {
            let Some(entries) = reported.get(*service_code).and_then(Value::as_array) else {
                panic!("case {case}: expected an entry for {service_code}, got {reported:?}");
            };
            assert_eq!(
                entries.len(),
                quota_codes.len(),
                "case {case}: expected {} excluded quota(s) for {service_code}, got {entries:?}",
                quota_codes.len()
            );

            for (position, quota_code) in quota_codes.iter().enumerate() {
                let entry = &entries[position];
                assert_eq!(
                    entry.get("QuotaCode").and_then(Value::as_str),
                    Some(*quota_code),
                    "case {case}: expected QuotaCode {quota_code} at position {position}, \
                     got {entry:?}"
                );
                let expected = catalog_quota_name(&client, service_code, quota_code).await;
                assert_eq!(
                    entry
                        .get("QuotaName")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    expected,
                    "case {case}: expected QuotaName {expected:?} for {quota_code}, got {entry:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_configuration_is_account_scoped_and_a_read_never_changes_it() {
    let engine = emulator();
    store(
        &engine,
        configuration(
            ACCOUNT_LEVEL,
            opt_in_type("NotifyAndAdjust"),
            OPT_IN_STATUS_ENABLED,
            Some(notification_arn()),
            &[("lambda", &["L-B99A9384"])],
        ),
    );

    // One stored configuration, read at several regional endpoints: the only
    // `OptInLevel` the model declares is ACCOUNT, so the configuration belongs to the
    // account and reads the same everywhere.
    let mut answers: BTreeMap<&str, Value> = BTreeMap::new();
    for region in ["us-east-1", "eu-central-1", "ap-southeast-2"] {
        let client = client(&engine, region);
        let first = client.ok(GET_CONFIG, json!({})).await;
        let second = client.ok(GET_CONFIG, json!({})).await;
        assert_eq!(
            second, first,
            "case {region}: expected a second read to report {first:?}, got {second:?}"
        );
        answers.insert(region, first);
    }

    let reference = answers
        .values()
        .next()
        .expect("at least one region was read")
        .clone();
    for (region, answer) in &answers {
        assert_eq!(
            answer, &reference,
            "case {region}: expected the account-scoped configuration {reference:?}, \
             got {answer:?}"
        );
    }
}
