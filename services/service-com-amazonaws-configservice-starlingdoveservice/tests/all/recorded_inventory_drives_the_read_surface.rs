// SPDX-License-Identifier: CC0-1.0

//! What the recorded inventory DRIVES: the reads and the actions downstream of a running
//! configuration recorder, for resources Config observed in their own service.
//!
//! The rule pinned here: while a configuration recorder is recording, the in-scope
//! resources the account holds in the other services are admitted to Config's inventory,
//! and every surface that projects that inventory reports them from it — the aggregator's
//! discovered-resource view reports them under the source account and Region they were
//! recorded in, narrowed only by the aggregator's scope and the request's filters; every
//! `ACTIVE` rule whose scope covers such a resource reaches a TERMINAL verdict on it,
//! whether the rule was written straight through the rule API or deployed by a conformance
//! pack; and a rule whose remediation configuration remediates automatically acts on a
//! resource it finds non-compliant with no call from the caller, unless a remediation
//! exception exempts that resource.
//!
//! No row here writes a configuration item through Config's own API: every identifier,
//! verdict and remediation below exists because a recorder was recording while the
//! resource existed in its own service.
//!
//! Config and S3 are mounted on ONE engine, reusing the harness
//! `config_recorded_resource_compliance` builds: the recorder reads the resources it
//! records from S3's own API, and a remediation acts on them through the same API.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state::conformance_pack_rule_name;

use crate::config_recorded_resource_compliance::{Emulator, ACCOUNT_ID, REGION};

/// The resource type Config records an Amazon S3 general purpose bucket under — an entry
/// of `com.amazonaws.configservice#ResourceType`, from
/// `smithy_query(query="shape", shape="ResourceType")`.
const BUCKET_RESOURCE_TYPE: &str = "AWS::S3::Bucket";

/// Another entry of that same enum, for the rows that ask about a type the account holds
/// nothing of.
const EC2_VOLUME_RESOURCE_TYPE: &str = "AWS::EC2::Volume";

/// The aggregator every aggregate read below is made against.
const AGGREGATOR: &str = "the-aggregator";

/// An account id that is not this store's, for the rows that narrow to another source.
/// Twelve digits, as `AccountId`'s own pattern requires.
const ANOTHER_ACCOUNT: &str = "210987654321";

/// A Region that is not the one the harness signs for, for the same reason.
const ANOTHER_REGION: &str = "eu-west-1";

/// The buckets the aggregate-read table is read over, named so their resource ids sort in
/// the order a listing reports them in.
const ALPHA: &str = "aggregated-alpha";
/// See [`ALPHA`].
const BETA: &str = "aggregated-beta";
/// A bucket name nothing creates, for the rows that narrow to a resource the account does
/// not hold.
const ABSENT: &str = "aggregated-absent";

/// `ComplianceType` values a managed rule places a recorded resource at
/// (`com.amazonaws.configservice#ComplianceType`).
const COMPLIANT: &str = "COMPLIANT";
/// See [`COMPLIANT`].
const NON_COMPLIANT: &str = "NON_COMPLIANT";

/// The managed rules this suite scopes to a recorded bucket, by the `SourceIdentifier`
/// Config knows each by (`Source$SourceIdentifier`).
const VERSIONING_RULE: &str = "S3_BUCKET_VERSIONING_ENABLED";
/// See [`VERSIONING_RULE`]. The entry this issue did not cite.
const ENCRYPTION_RULE: &str = "S3_BUCKET_SERVER_SIDE_ENCRYPTION_ENABLED";

/// The bucket versioning status that satisfies [`VERSIONING_RULE`]
/// (`com.amazonaws.s3#BucketVersioningStatus`).
const ENABLED: &str = "Enabled";

/// The only published `RemediationTargetType`: an SSM automation document.
const TARGET_TYPE: &str = "SSM_DOCUMENT";

/// The runbook the automatic remediation under test names, and the `Parameters` key
/// carrying the role it runs as.
const VERSIONING_RUNBOOK: &str = "AWS-ConfigureS3BucketVersioning";
/// See [`VERSIONING_RUNBOOK`].
const AUTOMATION_ASSUME_ROLE: &str = "AutomationAssumeRole";

/// The resource type a conformance pack template declares a Config rule as, from
/// `PutConformancePack`'s own model doc.
const CONFIG_RULE_TYPE: &str = "AWS::Config::ConfigRule";

/// How long a poll waits for a span the clock owes — a recording pass, an evaluation, a
/// remediation run. Each falls due a fraction of a second after the call that makes it
/// due, so this is many passes' worth of slack and not a span any assertion depends on.
const CONVERGENCE_BUDGET: Duration = Duration::from_secs(15);

/// How long a row waits while asserting that NOTHING happens — several passes' worth of
/// time, so an action that was going to be taken would have been taken.
const QUIET_BUDGET: Duration = Duration::from_millis(2_500);

/// How long a poll waits between two reads.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Create the aggregator [`AGGREGATOR`] from the aggregation source `source`, with
/// `filters` as its `AggregatorFilters` when it carries any.
async fn put_aggregator(emulator: &Emulator, source: Value, filters: Option<Value>) {
    let mut body = json!({
        "ConfigurationAggregatorName": AGGREGATOR,
        "AccountAggregationSources": [source],
    });
    if let Some(filters) = filters {
        body["AggregatorFilters"] = filters;
    }
    emulator.config("PutConfigurationAggregator", &body).await;
}

/// An account aggregation source over `account_id` in `regions`.
fn account_source(account_id: &str, regions: &[&str]) -> Value {
    json!({ "AccountIds": [account_id], "AwsRegions": regions })
}

/// The resource ids `ListDiscoveredResources` reports for `resource_type`.
async fn discovered(emulator: &Emulator, resource_type: &str) -> Vec<String> {
    let body = emulator
        .config(
            "ListDiscoveredResources",
            &json!({ "resourceType": resource_type }),
        )
        .await;
    body["resourceIdentifiers"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|identifier| {
            identifier["resourceId"]
                .as_str()
                .map(ToString::to_string)
        })
        .collect()
}

/// Wait until the account's own discovered-resource read reports exactly `wanted`, which
/// is the inventory every surface below projects.
async fn await_inventory(case: &str, emulator: &Emulator, wanted: &[&str]) {
    let deadline = Instant::now() + CONVERGENCE_BUDGET;
    loop {
        let reported = discovered(emulator, BUCKET_RESOURCE_TYPE).await;
        if reported.iter().map(String::as_str).collect::<Vec<&str>>() == wanted {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the recorder must admit {wanted:?} to the inventory, got {reported:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One `ListAggregateDiscoveredResources` read, with `extras` merged into it.
async fn aggregate_listing(emulator: &Emulator, resource_type: &str, extras: Value) -> Value {
    let mut body = json!({
        "ConfigurationAggregatorName": AGGREGATOR,
        "ResourceType": resource_type,
    });
    if let Some(members) = extras.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    emulator
        .config("ListAggregateDiscoveredResources", &body)
        .await
}

/// The `ResourceId` of every aggregate identifier a read reported, in the order reported.
fn aggregate_ids(case: &str, body: &Value) -> Vec<String> {
    body["ResourceIdentifiers"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected ResourceIdentifiers, got {body}"))
        .iter()
        .map(|identifier| {
            identifier["ResourceId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every identifier reports a ResourceId, got {body}")
                })
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn the_aggregator_reports_the_recorded_inventory_however_the_request_narrows_it() {
    let emulator = Emulator::new();
    emulator.recording().await;
    // Both buckets are created through S3's own API. Config is told nothing about either.
    emulator.create_bucket(ALPHA, &[]).await;
    emulator.create_bucket(BETA, &[]).await;
    await_inventory("the account's buckets", &emulator, &[ALPHA, BETA]).await;

    put_aggregator(
        &emulator,
        account_source(ACCOUNT_ID, &[REGION]),
        None,
    )
    .await;

    // The rows differ along what the rule generalises over: which member of `Filters`
    // narrows the read, whether the value it names is this store's source or another's,
    // and whether it names a resource the account holds.
    let cases: [(&str, Value, &[&str]); 11] = [
        ("no filters at all", json!({}), &[ALPHA, BETA]),
        (
            "the resource id of one of them",
            json!({ "Filters": { "ResourceId": ALPHA } }),
            &[ALPHA],
        ),
        (
            "a resource id the account never held",
            json!({ "Filters": { "ResourceId": ABSENT } }),
            &[],
        ),
        (
            "the resource name of one of them",
            json!({ "Filters": { "ResourceName": BETA } }),
            &[BETA],
        ),
        (
            "a resource name none of them carries",
            json!({ "Filters": { "ResourceName": ABSENT } }),
            &[],
        ),
        (
            "this store's own source account",
            json!({ "Filters": { "AccountId": ACCOUNT_ID } }),
            &[ALPHA, BETA],
        ),
        (
            "another source account",
            json!({ "Filters": { "AccountId": ANOTHER_ACCOUNT } }),
            &[],
        ),
        (
            "this store's own source Region",
            json!({ "Filters": { "Region": REGION } }),
            &[ALPHA, BETA],
        ),
        (
            "another source Region",
            json!({ "Filters": { "Region": ANOTHER_REGION } }),
            &[],
        ),
        (
            "the source and one resource id together",
            json!({
                "Filters": {
                    "AccountId": ACCOUNT_ID,
                    "Region": REGION,
                    "ResourceId": BETA,
                },
            }),
            &[BETA],
        ),
        (
            "a page of one, which reports the first resource only",
            json!({ "Limit": 1 }),
            &[ALPHA],
        ),
    ];

    for (case, extras, expected) in cases {
        let body = aggregate_listing(&emulator, BUCKET_RESOURCE_TYPE, extras).await;
        let reported = aggregate_ids(case, &body);
        assert_eq!(
            reported.iter().map(String::as_str).collect::<Vec<&str>>(),
            expected,
            "case {case:?}: expected the aggregated resource ids {expected:?}, got {reported:?}"
        );

        // Each identifier names the resource as the source it was recorded in: this
        // account, in the Region the request is signed for, under the type it is recorded
        // as, with the bucket's name as both its id and its custom name.
        for identifier in body["ResourceIdentifiers"].as_array().unwrap_or(&vec![]) {
            let resource_id = identifier["ResourceId"].as_str().unwrap_or_default();
            assert_eq!(
                (
                    identifier["SourceAccountId"].as_str(),
                    identifier["SourceRegion"].as_str(),
                    identifier["ResourceType"].as_str(),
                    identifier["ResourceName"].as_str(),
                ),
                (
                    Some(ACCOUNT_ID),
                    Some(REGION),
                    Some(BUCKET_RESOURCE_TYPE),
                    Some(resource_id),
                ),
                "case {case:?}: expected the source and identity the resource was recorded \
                 with, got {identifier}"
            );
        }
    }

    // A resource type the account holds nothing of is reported as nothing, not refused.
    let body = aggregate_listing(&emulator, EC2_VOLUME_RESOURCE_TYPE, json!({})).await;
    assert_eq!(
        aggregate_ids("a resource type the account holds nothing of", &body),
        Vec::<String>::new(),
        "a type the account holds nothing of must report no identifier, got {body}"
    );

    // The page token the first page issued resumes at the resource the page stopped
    // before, and the last page issues none.
    let first = aggregate_listing(&emulator, BUCKET_RESOURCE_TYPE, json!({ "Limit": 1 })).await;
    let token = first["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a page that stopped early must issue a token, got {first}"))
        .to_string();
    let second = aggregate_listing(
        &emulator,
        BUCKET_RESOURCE_TYPE,
        json!({ "Limit": 1, "NextToken": token }),
    )
    .await;
    assert_eq!(
        (
            aggregate_ids("the second page", &second),
            second["NextToken"].as_str(),
        ),
        (vec![BETA.to_string()], None),
        "the second page must report the resource the first stopped before, got {second}"
    );

    // A resource that goes away in its own service is no longer one the aggregator holds
    // data for, while the account's own inventory keeps it as deleted.
    emulator
        .s3()
        .delete_bucket()
        .bucket(ALPHA)
        .send()
        .await
        .unwrap_or_else(|error| panic!("deleting {ALPHA} must succeed, got {error:?}"));
    await_inventory("a bucket deleted in its own service", &emulator, &[BETA]).await;
    let after = aggregate_listing(&emulator, BUCKET_RESOURCE_TYPE, json!({})).await;
    assert_eq!(
        aggregate_ids("a bucket deleted in its own service", &after),
        vec![BETA.to_string()],
        "a deleted resource must not be reported by the aggregate read, got {after}"
    );
}

#[tokio::test]
async fn an_aggregator_reports_nothing_of_a_source_or_a_type_outside_its_scope() {
    // The rows differ along what puts this store outside the aggregator's scope: the
    // Regions its source takes in, the account its source names, and the resource types
    // its filters take in.
    let cases: [(&str, Value, Option<Value>, bool); 5] = [
        (
            "this account in this Region, and no type filter",
            account_source(ACCOUNT_ID, &[REGION]),
            None,
            true,
        ),
        (
            "this account in another Region only",
            account_source(ACCOUNT_ID, &[ANOTHER_REGION]),
            None,
            false,
        ),
        (
            "another account in this Region",
            account_source(ANOTHER_ACCOUNT, &[REGION]),
            None,
            false,
        ),
        (
            "a type filter naming the bucket type",
            account_source(ACCOUNT_ID, &[REGION]),
            Some(json!({
                "ResourceType": { "Type": "INCLUDE", "Value": [BUCKET_RESOURCE_TYPE] },
            })),
            true,
        ),
        (
            "a type filter naming another type only",
            account_source(ACCOUNT_ID, &[REGION]),
            Some(json!({
                "ResourceType": { "Type": "INCLUDE", "Value": [EC2_VOLUME_RESOURCE_TYPE] },
            })),
            false,
        ),
    ];

    for (case, source, filters, aggregated) in cases {
        let emulator = Emulator::new();
        emulator.recording().await;
        emulator.create_bucket(ALPHA, &[]).await;
        await_inventory(case, &emulator, &[ALPHA]).await;
        put_aggregator(&emulator, source, filters).await;

        let body = aggregate_listing(&emulator, BUCKET_RESOURCE_TYPE, json!({})).await;
        let reported = aggregate_ids(case, &body);
        let expected: Vec<&str> = if aggregated { vec![ALPHA] } else { vec![] };
        assert_eq!(
            reported.iter().map(String::as_str).collect::<Vec<&str>>(),
            expected,
            "case {case:?}: expected the aggregated resource ids {expected:?}, got {reported:?}"
        );
    }
}

/// A YAML conformance pack template declaring one Config rule per `(logical name,
/// managed rule identifier)` pair, which is what a pack deploys its rules from.
fn pack_template(declared: &[(&str, &str)]) -> String {
    let mut body = String::from("Resources:\n");
    for (index, (name, identifier)) in declared.iter().enumerate() {
        body.push_str(&format!(
            "  Rule{index}:\n    Type: {CONFIG_RULE_TYPE}\n    Properties:\n      \
             ConfigRuleName: {name}\n      Source:\n        Owner: AWS\n        \
             SourceIdentifier: {identifier}\n"
        ));
    }
    body
}

/// Deploy the pack `pack` from `declared`, and answer the names Config gave the rules it
/// deployed — in the same order as `declared`.
async fn deploy_pack(emulator: &Emulator, pack: &str, declared: &[(&str, &str)]) -> Vec<String> {
    emulator
        .config(
            "PutConformancePack",
            &json!({
                "ConformancePackName": pack,
                "TemplateBody": pack_template(declared),
            }),
        )
        .await;
    let status = emulator
        .config(
            "DescribeConformancePackStatus",
            &json!({ "ConformancePackNames": [pack] }),
        )
        .await;
    let id = status["ConformancePackStatusDetails"][0]["ConformancePackId"]
        .as_str()
        .unwrap_or_else(|| panic!("expected the deployed pack's id, got {status}"))
        .to_string();
    declared
        .iter()
        .map(|(name, _)| conformance_pack_rule_name(name, &id))
        .collect()
}

/// The verdict the rule `rule` holds for the resource `resource_id` right now, as
/// `GetComplianceDetailsByConfigRule` reports it.
async fn verdict_for(emulator: &Emulator, rule: &str, resource_id: &str) -> Option<String> {
    let body = emulator
        .config(
            "GetComplianceDetailsByConfigRule",
            &json!({ "ConfigRuleName": rule }),
        )
        .await;
    body["EvaluationResults"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .find(|result| {
            result["EvaluationResultIdentifier"]["EvaluationResultQualifier"]["ResourceId"]
                .as_str()
                == Some(resource_id)
        })
        .and_then(|result| result["ComplianceType"].as_str().map(ToString::to_string))
}

/// Wait until the rule `rule` places `resource_id` at `wanted`.
async fn await_verdict(case: &str, emulator: &Emulator, rule: &str, resource_id: &str, wanted: &str) {
    let deadline = Instant::now() + CONVERGENCE_BUDGET;
    loop {
        let placed = verdict_for(emulator, rule, resource_id).await;
        if placed.as_deref() == Some(wanted) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: rule {rule} must place {resource_id} at {wanted}, got {placed:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The `ComplianceType` the pack `pack` reports for each of its rules.
async fn pack_compliance(emulator: &Emulator, pack: &str) -> Vec<(String, String)> {
    let body = emulator
        .config(
            "DescribeConformancePackCompliance",
            &json!({ "ConformancePackName": pack }),
        )
        .await;
    body["ConformancePackRuleComplianceList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            let text = |value: &Value| value.as_str().unwrap_or_default().to_string();
            (text(&entry["ConfigRuleName"]), text(&entry["ComplianceType"]))
        })
        .collect()
}

/// Wait until the pack `pack` reports `rule` at `wanted`.
async fn await_pack_compliance(
    case: &str,
    emulator: &Emulator,
    pack: &str,
    rule: &str,
    wanted: &str,
) {
    let deadline = Instant::now() + CONVERGENCE_BUDGET;
    loop {
        let reported = pack_compliance(emulator, pack).await;
        if reported
            .iter()
            .any(|(name, compliance)| name == rule && compliance == wanted)
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the pack must report {rule} at {wanted}, got {reported:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The `(pack name, status)` pairs `GetConformancePackComplianceSummary` reports for
/// `packs`.
async fn pack_summary(emulator: &Emulator, packs: &[&str]) -> Vec<(String, String)> {
    let body = emulator
        .config(
            "GetConformancePackComplianceSummary",
            &json!({ "ConformancePackNames": packs }),
        )
        .await;
    body["ConformancePackComplianceSummaryList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            let text = |value: &Value| value.as_str().unwrap_or_default().to_string();
            (
                text(&entry["ConformancePackName"]),
                text(&entry["ConformancePackComplianceStatus"]),
            )
        })
        .collect()
}

#[tokio::test]
async fn every_compliance_read_reports_a_terminal_verdict_for_a_recorded_resource() {
    let emulator = Emulator::new();
    emulator.recording().await;

    // The buckets differ along the recorded configuration each managed rule's check
    // reads: one whose versioning has never been set, and one with versioning enabled.
    let bare = "verdict-bare";
    let versioned = "verdict-versioned";
    emulator.create_bucket(bare, &[]).await;
    emulator.create_bucket(versioned, &[]).await;
    emulator
        .s3()
        .put_bucket_versioning()
        .bucket(versioned)
        .versioning_configuration(
            aws_sdk_s3::types::VersioningConfiguration::builder()
                .status(aws_sdk_s3::types::BucketVersioningStatus::from(ENABLED))
                .build(),
        )
        .send()
        .await
        .unwrap_or_else(|error| panic!("enabling versioning must succeed, got {error:?}"));
    await_inventory("the account's buckets", &emulator, &[bare, versioned]).await;

    // One rule written straight through the rule API, and two deployed by a conformance
    // pack — so the rows below cover both ways a rule comes to cover a recorded resource.
    let own_rule = "written-versioning-rule";
    emulator
        .config(
            "PutConfigRule",
            &json!({
                "ConfigRule": {
                    "ConfigRuleName": own_rule,
                    "Source": { "Owner": "AWS", "SourceIdentifier": VERSIONING_RULE },
                    "Scope": { "ComplianceResourceTypes": [BUCKET_RESOURCE_TYPE] },
                }
            }),
        )
        .await;
    let pack = "the-pack";
    let deployed = deploy_pack(
        &emulator,
        pack,
        &[
            ("packed-versioning", VERSIONING_RULE),
            ("packed-encryption", ENCRYPTION_RULE),
        ],
    )
    .await;
    // A second pack, declaring only the rule every bucket satisfies, so the cumulative
    // summary below has one pack of each status to report.
    let compliant_pack = "the-compliant-pack";
    deploy_pack(
        &emulator,
        compliant_pack,
        &[("packed-encryption-only", ENCRYPTION_RULE)],
    )
    .await;

    // The rows differ along which rule is asked, which bucket is asked about and what the
    // recorded configuration item of that bucket makes the rule's check answer. A bucket
    // with no encryption configuration of its own still carries the base level of default
    // encryption Amazon S3 applies to every bucket, so the encryption rule places both.
    let cases: [(&str, &str, &str, &str); 6] = [
        (
            "the written rule, a bucket whose versioning was never set",
            own_rule,
            bare,
            NON_COMPLIANT,
        ),
        (
            "the written rule, a bucket with versioning enabled",
            own_rule,
            versioned,
            COMPLIANT,
        ),
        (
            "the pack's versioning rule, a bucket whose versioning was never set",
            &deployed[0],
            bare,
            NON_COMPLIANT,
        ),
        (
            "the pack's versioning rule, a bucket with versioning enabled",
            &deployed[0],
            versioned,
            COMPLIANT,
        ),
        (
            "the pack's encryption rule, a bucket whose versioning was never set",
            &deployed[1],
            bare,
            COMPLIANT,
        ),
        (
            "the pack's encryption rule, a bucket with versioning enabled",
            &deployed[1],
            versioned,
            COMPLIANT,
        ),
    ];

    for (case, rule, bucket, expected) in cases {
        await_verdict(case, &emulator, rule, bucket, expected).await;
    }

    // The same verdicts, rolled up: a rule with a non-compliant resource rolls up to
    // NON_COMPLIANT and one with none rolls up to COMPLIANT, through the rule read and
    // through the pack's own compliance read.
    let rollups: [(&str, &str, &str); 3] = [
        ("the written rule", own_rule, NON_COMPLIANT),
        ("the pack's versioning rule", &deployed[0], NON_COMPLIANT),
        ("the pack's encryption rule", &deployed[1], COMPLIANT),
    ];
    for (case, rule, expected) in rollups {
        let body = emulator
            .config(
                "DescribeComplianceByConfigRule",
                &json!({ "ConfigRuleNames": [rule] }),
            )
            .await;
        let rolled = body["ComplianceByConfigRules"][0]["Compliance"]["ComplianceType"].as_str();
        assert_eq!(
            rolled,
            Some(expected),
            "case {case:?}: expected the rule to roll up to {expected}, got {body}"
        );
    }
    for (case, rule, expected) in [
        ("the pack's versioning rule", &deployed[0], NON_COMPLIANT),
        ("the pack's encryption rule", &deployed[1], COMPLIANT),
    ] {
        await_pack_compliance(case, &emulator, pack, rule, expected).await;
    }

    // And the CUMULATIVE compliance of each pack: a pack with a non-compliant rule rolls
    // up to NON_COMPLIANT, one with none rolls up to COMPLIANT. The rows are the two
    // packs, in the order the summary reports them (its own name ordering).
    let deadline = Instant::now() + CONVERGENCE_BUDGET;
    let wanted = vec![
        (compliant_pack.to_string(), COMPLIANT.to_string()),
        (pack.to_string(), NON_COMPLIANT.to_string()),
    ];
    loop {
        let reported = pack_summary(&emulator, &[pack, compliant_pack]).await;
        if reported == wanted {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the cumulative summary must report {wanted:?}, got {reported:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    // A page of one reports the first pack and a token for the rest, and a name this
    // account holds no pack of is refused rather than skipped.
    let paged = emulator
        .config(
            "GetConformancePackComplianceSummary",
            &json!({ "ConformancePackNames": [pack, compliant_pack], "Limit": 1 }),
        )
        .await;
    assert_eq!(
        (
            paged["ConformancePackComplianceSummaryList"]
                .as_array()
                .map(Vec::len),
            paged["ConformancePackComplianceSummaryList"][0]["ConformancePackName"].as_str(),
            paged["NextToken"].is_string(),
        ),
        (Some(1), Some(compliant_pack), true),
        "a page of one must report the first pack and a token, got {paged}"
    );
    let (status, code) = emulator
        .refused(
            "GetConformancePackComplianceSummary",
            &json!({ "ConformancePackNames": ["no-such-pack"] }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "NoSuchConformancePackException"),
        "a pack this account does not hold must be refused"
    );
}

/// The `PutRemediationConfigurations` body for `rule`: the versioning runbook, remediating
/// automatically or only on request.
fn remediation(rule: &str, automatic: bool) -> Value {
    json!({
        "RemediationConfigurations": [{
            "ConfigRuleName": rule,
            "TargetType": TARGET_TYPE,
            "TargetId": VERSIONING_RUNBOOK,
            "ResourceType": BUCKET_RESOURCE_TYPE,
            "Automatic": automatic,
            "Parameters": {
                AUTOMATION_ASSUME_ROLE: {
                    "StaticValue": {
                        "Values": [format!("arn:aws:iam::{ACCOUNT_ID}:role/config-remediation")],
                    },
                },
            },
        }],
    })
}

/// The bucket's versioning status, absent while its versioning has never been set.
async fn versioning_of(emulator: &Emulator, bucket: &str) -> Option<String> {
    emulator
        .s3()
        .get_bucket_versioning()
        .bucket(bucket)
        .send()
        .await
        .ok()
        .and_then(|answer| answer.status().map(|status| status.as_str().to_string()))
}

/// The resource ids the rule `rule` reports a remediation execution for.
async fn executed_for(emulator: &Emulator, rule: &str) -> Vec<String> {
    let body = emulator
        .config(
            "DescribeRemediationExecutionStatus",
            &json!({ "ConfigRuleName": rule }),
        )
        .await;
    body["RemediationExecutionStatuses"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|status| {
            status["ResourceKey"]["resourceId"]
                .as_str()
                .map(ToString::to_string)
        })
        .collect()
}

#[tokio::test]
async fn an_automatic_remediation_acts_on_the_resource_its_rule_finds_non_compliant() {
    // The rows differ along what decides whether Config acts without being asked: whether
    // the rule's remediation configuration remediates automatically, whether an
    // unexpired remediation exception exempts the resource, and whether the exception has
    // already expired.
    let cases: [(&str, bool, Option<i64>, bool); 4] = [
        ("an automatic remediation", true, None, true),
        (
            "a remediation that runs only on request",
            false,
            None,
            false,
        ),
        (
            "an automatic remediation, with the resource excepted",
            true,
            Some(600),
            false,
        ),
        (
            "an automatic remediation, with an expired exception",
            true,
            Some(-600),
            true,
        ),
    ];

    for (case, automatic, exception_in, remediated) in cases {
        let emulator = Emulator::new();
        let rule = "auto-remediated-rule";
        let bucket = "auto-remediated-bucket";
        emulator.recording().await;
        emulator.create_bucket(bucket, &[]).await;
        await_inventory(case, &emulator, &[bucket]).await;
        emulator
            .config(
                "PutConfigRule",
                &json!({
                    "ConfigRule": {
                        "ConfigRuleName": rule,
                        "Source": { "Owner": "AWS", "SourceIdentifier": VERSIONING_RULE },
                        "Scope": { "ComplianceResourceTypes": [BUCKET_RESOURCE_TYPE] },
                    }
                }),
            )
            .await;
        // The bucket is found non-compliant first: what an automatic remediation acts on
        // is that finding.
        await_verdict(case, &emulator, rule, bucket, NON_COMPLIANT).await;

        if let Some(seconds) = exception_in {
            // An exception names the rule's remediation, so the configuration has to be
            // in place before one can be written for it; it is put as a remediation that
            // runs only on request, and switched to what the row asks for below, so the
            // exception is in place before anything could act on the finding.
            emulator
                .config("PutRemediationConfigurations", &remediation(rule, false))
                .await;
            // `ExpirationTime` is an instant, so an exception that has already expired is
            // one whose instant is in the past.
            let expires_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("the test clock is past the epoch")
                .as_secs() as i64
                + seconds;
            emulator
                .config(
                    "PutRemediationExceptions",
                    &json!({
                        "ConfigRuleName": rule,
                        "ResourceKeys": [{
                            "ResourceType": BUCKET_RESOURCE_TYPE,
                            "ResourceId": bucket,
                        }],
                        "ExpirationTime": expires_at,
                    }),
                )
                .await;
        }
        emulator
            .config("PutRemediationConfigurations", &remediation(rule, automatic))
            .await;

        if remediated {
            // Nobody calls StartRemediationExecution: the finding is the trigger, so the
            // execution appears and the bucket ends up as the runbook specifies — which
            // the recorder then records, flipping the rule's verdict.
            let deadline = Instant::now() + CONVERGENCE_BUDGET;
            loop {
                let executed = executed_for(&emulator, rule).await;
                let versioning = versioning_of(&emulator, bucket).await;
                if executed.iter().any(|resource| resource == bucket)
                    && versioning.as_deref() == Some(ENABLED)
                {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "case {case:?}: the automatic remediation must run for {bucket}; \
                     executions {executed:?}, versioning {versioning:?}"
                );
                tokio::time::sleep(POLL_INTERVAL).await;
            }
            await_verdict(case, &emulator, rule, bucket, COMPLIANT).await;
        } else {
            // Nothing is remediated, for as long as several passes: no execution is
            // recorded, the bucket is untouched and the rule keeps finding it
            // non-compliant.
            let deadline = Instant::now() + QUIET_BUDGET;
            loop {
                let executed = executed_for(&emulator, rule).await;
                let versioning = versioning_of(&emulator, bucket).await;
                assert_eq!(
                    (executed.as_slice(), versioning.as_deref()),
                    (&[][..], None),
                    "case {case:?}: nothing must be remediated for {bucket}"
                );
                if Instant::now() >= deadline {
                    break;
                }
                tokio::time::sleep(POLL_INTERVAL).await;
            }
            assert_eq!(
                verdict_for(&emulator, rule, bucket).await.as_deref(),
                Some(NON_COMPLIANT),
                "case {case:?}: the rule must keep finding {bucket} non-compliant"
            );
        }
    }
}
