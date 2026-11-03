// SPDX-License-Identifier: CC0-1.0

//! What this service REPORTS when nothing was written, and what it REFUSES to write.
//!
//! Two rules, each asserted over a table of inputs rather than one recorded call:
//!
//! 1. A response member the service always reports — a COUNT or a LIST — is reported
//!    whatever its value: `GetEnrollmentStatus$numberOfMemberAccountsOptedIn` is `0` when
//!    no member account is opted in, and `GetEffectiveRecommendationPreferences`'
//!    `utilizationPreferences` / `preferredResources` are lists carrying the service's
//!    documented default when no stored preference sets them, exactly as `lookBackPeriod`
//!    carries `DAYS_14`.
//! 2. `PutRecommendationPreferences` refuses, with `InvalidParameterValueException`
//!    (HTTP 400) and BEFORE touching the store, a preference set for a `resourceType` the
//!    service does not support that preference for — so nothing a refused request named is
//!    readable afterwards.
//!
//! Every call goes through the shared wire harness (`tests/all/harness.rs`), i.e. the same
//! `MultiServiceRouter::dispatch` the emulator's front door uses, so what is asserted here
//! is what a client sees.

use serde_json::{json, Value};

use crate::harness::{
    auto_scaling_group_arn, call, effective_for, emulator, error_code, instance_arn, opt_in,
};

/// The documented default utilization preferences, from
/// `PutRecommendationPreferences$utilizationPreferences`: "When this preference isn't
/// specified, we use the following default values. CPU utilization: `P99_5` for threshold,
/// `PERCENT_20` for headroom. Memory utilization: `PERCENT_20` for headroom" — and "the
/// threshold setting isn't available for memory utilization".
fn default_utilization_preferences() -> Value {
    json!([
        {
            "metricName": "CpuUtilization",
            "metricParameters": {"threshold": "P99_5", "headroom": "PERCENT_20"},
        },
        {"metricName": "MemoryUtilization", "metricParameters": {"headroom": "PERCENT_20"}},
    ])
}

/// The documented default preferred resources: the one name the service supports
/// ("Compute Optimizer only supports the customization of `Ec2InstanceTypes`") including
/// every candidate and excluding none, because `PreferredResource$excludeList` documents
/// that "if this isn't specified, all supported resources are included by default".
fn default_preferred_resources() -> Value {
    json!([{
        "name": "Ec2InstanceTypes",
        "includeList": ["*"],
        "effectiveIncludeList": ["*"],
        "excludeList": [],
    }])
}

/// `GetEnrollmentStatus` reports the opted-in member-account COUNT in every enrollment
/// state the account can be left in: zero member accounts opted in is the number `0`, not
/// an absent member a client cannot tell from "not reported".
#[tokio::test]
async fn enrollment_status_always_reports_the_opted_in_member_account_count() {
    let router = emulator();

    // Row 0 is the untouched account; the rest are every (status, includeMemberAccounts)
    // combination a client can send `UpdateEnrollmentStatus` (`Status` is settable only to
    // `Active` or `Inactive`), with the status that write answers for this emulator's
    // STANDALONE account: `includeMemberAccounts` enrolls "member accounts of the organization
    // if the account is the management account of an organization", which it is not, so those
    // rows are refused with 400 — and the count is reported after a refused write exactly as
    // after an accepted one.
    let rows: Vec<(&str, Option<&str>, bool, u16)> = vec![
        ("never enrolled", None, false, 0),
        ("Active with member accounts", Some("Active"), true, 400),
        ("Active without member accounts", Some("Active"), false, 200),
        ("Inactive with member accounts", Some("Inactive"), true, 400),
        (
            "Inactive without member accounts",
            Some("Inactive"),
            false,
            200,
        ),
    ];

    for (case, status, include_member_accounts, wanted) in rows {
        if let Some(status) = status {
            let (code, body) = call(
                &router,
                "UpdateEnrollmentStatus",
                json!({"status": status, "includeMemberAccounts": include_member_accounts}),
            )
            .await;
            assert_eq!(
                code, wanted,
                "case {case:?}: expected the update to answer {wanted}, got {code}: {body}"
            );
        }

        let (code, body) = call(&router, "GetEnrollmentStatus", json!({})).await;
        assert_eq!(code, 200, "case {case:?}: expected 200, got {code}: {body}");
        let count = &body["numberOfMemberAccountsOptedIn"];
        assert!(
            count.is_i64(),
            "case {case:?}: expected numberOfMemberAccountsOptedIn to be reported as an \
             integer, got {body}"
        );
        assert_eq!(
            count,
            &json!(0),
            "case {case:?}: no member account is opted in, so the count is 0, got {body}"
        );
    }
}

/// With nothing stored, the effective-preferences read still reports every list member,
/// carrying the service's documented default — for both ARN forms the operation accepts.
#[tokio::test]
async fn effective_preferences_report_documented_defaults_when_nothing_is_stored() {
    let router = emulator();
    opt_in(&router).await;

    // `utilizationPreferences` is "only available for the Amazon EC2 instance resource
    // type", so an Auto Scaling group has no default to report and gets an empty list;
    // `preferredResources` is available for both, so both get its default.
    let rows: Vec<(&str, String, Value)> = vec![
        (
            "ec2 instance",
            instance_arn("i-0123456789abcdef0"),
            default_utilization_preferences(),
        ),
        (
            "auto scaling group",
            auto_scaling_group_arn("web-tier"),
            json!([]),
        ),
    ];

    for (case, arn, wanted_utilization) in rows {
        let effective = effective_for(&router, &arn).await;
        assert!(
            effective["utilizationPreferences"].is_array(),
            "case {case:?}: utilizationPreferences must be reported as a list, got {effective}"
        );
        assert_eq!(
            effective["utilizationPreferences"], wanted_utilization,
            "case {case:?}: expected {wanted_utilization}, got {effective}"
        );
        assert_eq!(
            effective["preferredResources"],
            default_preferred_resources(),
            "case {case:?}: expected the default preferred resources, got {effective}"
        );
        assert_eq!(
            effective["lookBackPeriod"], "DAYS_14",
            "case {case:?}: expected the default look-back period, got {effective}"
        );
    }
}

/// A stored preference replaces the default it covers, and `effectiveIncludeList` is the
/// expansion of the include list: the identity for a list of concrete values or for the
/// catch-all wildcard, and omitted rather than invented for a narrower wildcard this
/// emulator cannot resolve without the EC2 instance-type catalog.
#[tokio::test]
async fn a_stored_preference_replaces_the_default_the_effective_read_reports() {
    let instance = instance_arn("i-0abcdef1234567890");

    // (case, the preference written at the resource scope, the expected effective
    // projection of the member it sets)
    let rows: Vec<(&str, Value, &str, Value)> = vec![
        (
            "one utilization preference",
            json!({"utilizationPreferences": [{
                "metricName": "CpuUtilization",
                "metricParameters": {"threshold": "P90", "headroom": "PERCENT_30"},
            }]}),
            "utilizationPreferences",
            json!([{
                "metricName": "CpuUtilization",
                "metricParameters": {"threshold": "P90", "headroom": "PERCENT_30"},
            }]),
        ),
        (
            "two utilization preferences",
            json!({"utilizationPreferences": [
                {"metricName": "CpuUtilization", "metricParameters": {"threshold": "P95"}},
                {"metricName": "MemoryUtilization", "metricParameters": {"headroom": "PERCENT_10"}},
            ]}),
            "utilizationPreferences",
            json!([
                {"metricName": "CpuUtilization", "metricParameters": {"threshold": "P95"}},
                {"metricName": "MemoryUtilization", "metricParameters": {"headroom": "PERCENT_10"}},
            ]),
        ),
        (
            "concrete include list",
            json!({"preferredResources": [{
                "name": "Ec2InstanceTypes",
                "includeList": ["m5.large", "m5.xlarge"],
            }]}),
            "preferredResources",
            json!([{
                "name": "Ec2InstanceTypes",
                "includeList": ["m5.large", "m5.xlarge"],
                "effectiveIncludeList": ["m5.large", "m5.xlarge"],
            }]),
        ),
        (
            "catch-all wildcard include list",
            json!({"preferredResources": [{
                "name": "Ec2InstanceTypes",
                "includeList": ["*"],
            }]}),
            "preferredResources",
            json!([{
                "name": "Ec2InstanceTypes",
                "includeList": ["*"],
                "effectiveIncludeList": ["*"],
            }]),
        ),
        (
            "narrower wildcard include list",
            json!({"preferredResources": [{
                "name": "Ec2InstanceTypes",
                "includeList": ["m5*"],
            }]}),
            "preferredResources",
            // No `effectiveIncludeList`: expanding `m5*` needs the instance-type catalog,
            // which this emulator does not hold, so it is omitted rather than invented.
            json!([{"name": "Ec2InstanceTypes", "includeList": ["m5*"]}]),
        ),
        (
            "exclude list only",
            json!({"preferredResources": [{
                "name": "Ec2InstanceTypes",
                "excludeList": ["t2.micro"],
            }]}),
            "preferredResources",
            json!([{"name": "Ec2InstanceTypes", "excludeList": ["t2.micro"]}]),
        ),
    ];

    for (case, preference, member, wanted) in rows {
        // A fresh emulator per row: what is asserted is this row's write, not the last.
        let router = emulator();
        opt_in(&router).await;

        let mut request = json!({
            "resourceType": "Ec2Instance",
            "scope": {"name": "ResourceArn", "value": instance},
        });
        for (field, value) in preference.as_object().expect("an object of preferences") {
            request[field] = value.clone();
        }
        let (status, body) = call(&router, "PutRecommendationPreferences", request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {request} to be accepted, got {status}: {body}"
        );

        let effective = effective_for(&router, &instance).await;
        assert_eq!(
            effective[member], wanted,
            "case {case:?}: expected {member} {wanted}, got {effective}"
        );
        // The member this row did not write still reports its default, so one stored
        // preference does not blank the others.
        let untouched = if member == "preferredResources" {
            ("utilizationPreferences", default_utilization_preferences())
        } else {
            ("preferredResources", default_preferred_resources())
        };
        assert_eq!(
            effective[untouched.0], untouched.1,
            "case {case:?}: expected {} to keep its default {}, got {effective}",
            untouched.0, untouched.1
        );
    }
}

/// `PutRecommendationPreferences` refuses a preference the requested `resourceType` does
/// not support — with HTTP 400 and `InvalidParameterValueException` — and stores nothing;
/// the same preference for a resource type that supports it is accepted and readable back.
///
/// The rows below span every preference member whose documentation restricts the resource
/// types it may be set for, each on a type that supports it and on types that do not. The
/// `ResourceType` values are the model's own enum: AuroraDBClusterStorage |
/// AutoScalingGroup | EbsVolume | Ec2Instance | EcsService | Idle | LambdaFunction |
/// License | NotApplicable | RdsDBInstance.
#[tokio::test]
async fn put_refuses_a_preference_the_resource_type_does_not_support_and_stores_nothing() {
    let group = auto_scaling_group_arn("batch-tier");
    let group_scope = json!({"name": "ResourceArn", "value": group});

    // (case, accepted, the member the row sets, the request)
    let rows: Vec<(&str, bool, &str, Value)> = vec![
        // `inferredWorkloadTypes` is a status switch whose documentation states no
        // resource-type restriction and whose refusal names none, so the service accepts
        // it for every `ResourceType` — see
        // `a_status_switch_preference_is_settable_for_every_resource_type` in
        // `tests/all/unrestricted_preference_resource_types.rs` for the whole enum.
        // `enhancedInfrastructureMetrics` is restricted to the resource types whose
        // effective-preferences shape in the model carries it: the instance / Auto Scaling
        // group one and the Amazon Aurora and RDS database one.
        (
            "enhanced metrics on Ec2Instance",
            true,
            "enhancedInfrastructureMetrics",
            json!({"resourceType": "Ec2Instance", "enhancedInfrastructureMetrics": "Active"}),
        ),
        (
            "enhanced metrics on AutoScalingGroup",
            true,
            "enhancedInfrastructureMetrics",
            json!({
                "resourceType": "AutoScalingGroup",
                "scope": group_scope,
                "enhancedInfrastructureMetrics": "Active",
            }),
        ),
        (
            "enhanced metrics on RdsDBInstance",
            true,
            "enhancedInfrastructureMetrics",
            json!({"resourceType": "RdsDBInstance", "enhancedInfrastructureMetrics": "Inactive"}),
        ),
        (
            "enhanced metrics on AuroraDBClusterStorage",
            true,
            "enhancedInfrastructureMetrics",
            json!({
                "resourceType": "AuroraDBClusterStorage",
                "enhancedInfrastructureMetrics": "Active",
            }),
        ),
        (
            "enhanced metrics on EbsVolume",
            false,
            "enhancedInfrastructureMetrics",
            json!({"resourceType": "EbsVolume", "enhancedInfrastructureMetrics": "Inactive"}),
        ),
        (
            "enhanced metrics on Idle",
            false,
            "enhancedInfrastructureMetrics",
            json!({"resourceType": "Idle", "enhancedInfrastructureMetrics": "Active"}),
        ),
        (
            "inferred workload types on Ec2Instance",
            true,
            "inferredWorkloadTypes",
            json!({"resourceType": "Ec2Instance", "inferredWorkloadTypes": "Inactive"}),
        ),
        (
            "inferred workload types on NotApplicable",
            true,
            "inferredWorkloadTypes",
            json!({"resourceType": "NotApplicable", "inferredWorkloadTypes": "Active"}),
        ),
        // `savingsEstimationMode` is restricted to no resource type at all.
        (
            "savings estimation mode on License",
            true,
            "savingsEstimationMode",
            json!({"resourceType": "License", "savingsEstimationMode": "AfterDiscounts"}),
        ),
        // `externalMetricsPreference` "can only be set for the Ec2Instance resource type".
        (
            "external metrics on Ec2Instance",
            true,
            "externalMetricsPreference",
            json!({
                "resourceType": "Ec2Instance",
                "externalMetricsPreference": {"source": "Datadog"},
            }),
        ),
        (
            "external metrics on RdsDBInstance",
            false,
            "externalMetricsPreference",
            json!({
                "resourceType": "RdsDBInstance",
                "externalMetricsPreference": {"source": "Datadog"},
            }),
        ),
        // `utilizationPreferences`: "You can only set CPU and memory utilization
        // preferences for the Amazon EC2 instance resource type."
        (
            "utilization preferences on Ec2Instance",
            true,
            "utilizationPreferences",
            json!({
                "resourceType": "Ec2Instance",
                "utilizationPreferences": [{
                    "metricName": "CpuUtilization",
                    "metricParameters": {"headroom": "PERCENT_20"},
                }],
            }),
        ),
        (
            "utilization preferences on EcsService",
            false,
            "utilizationPreferences",
            json!({
                "resourceType": "EcsService",
                "utilizationPreferences": [{
                    "metricName": "CpuUtilization",
                    "metricParameters": {"headroom": "PERCENT_20"},
                }],
            }),
        ),
        // `preferredResources`: settable for the EC2 instance, Auto Scaling group, EBS
        // volume, ECS service, RDS DB instance and Aurora DB cluster storage types.
        (
            "preferred resources on EbsVolume",
            true,
            "preferredResources",
            json!({
                "resourceType": "EbsVolume",
                "preferredResources": [{"name": "Ec2InstanceTypes", "includeList": ["m5.large"]}],
            }),
        ),
        (
            "preferred resources on AuroraDBClusterStorage",
            true,
            "preferredResources",
            json!({
                "resourceType": "AuroraDBClusterStorage",
                "preferredResources": [{"name": "Ec2InstanceTypes", "excludeList": ["t2.micro"]}],
            }),
        ),
        (
            "preferred resources on LambdaFunction",
            false,
            "preferredResources",
            json!({
                "resourceType": "LambdaFunction",
                "preferredResources": [{"name": "Ec2InstanceTypes", "includeList": ["m5.large"]}],
            }),
        ),
        (
            "preferred resources on License",
            false,
            "preferredResources",
            json!({
                "resourceType": "License",
                "preferredResources": [{"name": "Ec2InstanceTypes", "includeList": ["m5.large"]}],
            }),
        ),
        // `lookBackPeriod`: the same six types, and nothing else.
        (
            "look-back period on RdsDBInstance",
            true,
            "lookBackPeriod",
            json!({"resourceType": "RdsDBInstance", "lookBackPeriod": "DAYS_32"}),
        ),
        (
            "look-back period on NotApplicable",
            false,
            "lookBackPeriod",
            json!({"resourceType": "NotApplicable", "lookBackPeriod": "DAYS_32"}),
        ),
        (
            "look-back period on License",
            false,
            "lookBackPeriod",
            json!({"resourceType": "License", "lookBackPeriod": "DAYS_93"}),
        ),
    ];

    for (case, accepted, member, request) in rows {
        // A fresh emulator per row, so what a later read reports is this row's request
        // alone — which is what proves a refused request wrote nothing.
        let router = emulator();
        opt_in(&router).await;

        let (status, body) = call(&router, "PutRecommendationPreferences", request.clone()).await;
        let resource_type = request["resourceType"].as_str().expect("a resourceType");
        let mut read_request = json!({"resourceType": resource_type});
        if let Some(scope) = request.get("scope") {
            read_request["scope"] = scope.clone();
        }
        let (read_status, read) =
            call(&router, "GetRecommendationPreferences", read_request.clone()).await;
        assert_eq!(
            read_status, 200,
            "case {case:?}: expected 200 from {read_request}, got {read_status}: {read}"
        );
        let details = read["recommendationPreferencesDetails"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a list of details, got {read}"));

        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected {request} to be accepted, got {status}: {body}"
            );
            assert_eq!(
                details.len(),
                1,
                "case {case:?}: expected the accepted preference to be stored, got {read}"
            );
            if member == "preferredResources" {
                // Reported as `EffectivePreferredResource`: the stored lists plus the
                // `effectiveIncludeList` expansion, so the stored lists are what a
                // read-back must carry.
                for (index, written) in request[member]
                    .as_array()
                    .expect("a list of preferred resources")
                    .iter()
                    .enumerate()
                {
                    let got = &details[0][member][index];
                    for field in ["name", "includeList", "excludeList"] {
                        assert_eq!(
                            got[field], written[field],
                            "case {case:?}: expected the stored {field} {written}, got {got}"
                        );
                    }
                }
            } else {
                assert_eq!(
                    details[0][member], request[member],
                    "case {case:?}: expected the stored {member} to be read back, got {read}"
                );
            }
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected {request} to be refused, got {status}: {body}"
            );
            assert!(
                error_code(&body).contains("InvalidParameterValueException"),
                "case {case:?}: expected InvalidParameterValueException, got {body}"
            );
            let message = body["message"].as_str().unwrap_or_default();
            assert!(
                message.starts_with("The specified resource type is not valid for "),
                "case {case:?}: the refusal must be the service's sentence for a preference \
                 the resource type does not support, got {body}"
            );
            assert!(
                !message.contains(resource_type) && !message.contains(member),
                "case {case:?}: that sentence names neither the rejected resource type nor \
                 the member the preference arrived on, got {body}"
            );
            assert!(
                details.is_empty(),
                "case {case:?}: a refused request must store nothing, got {read}"
            );
        }
    }
}
