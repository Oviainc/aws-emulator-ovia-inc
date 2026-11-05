// SPDX-License-Identifier: CC0-1.0

//! What a GuardDuty-PUBLISHED custom detection rule reports about itself, end to
//! end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rule under test: a detection rule is published BY GuardDuty, so every value
//! an entry reports is a property of the PUBLISHED rule and not of the account, the
//! Region or the process serving the read. Asserted over EVERY entry of the
//! catalogue rather than over the one entry a recording exposed:
//!
//! 1. The ARN is the AWS-managed one. `Arn` targets `DetectionRuleArn`, whose
//!    pattern is `^arn:[a-zA-Z0-9\-]+:[a-zA-Z0-9\-]+:[a-zA-Z0-9\-]*:([0-9]*|aws):.+$`
//!    (`smithy_query(shape="RuleSummary,DetectionRuleArn")`) — the Region segment may
//!    be empty and the account segment may be the literal `aws`, which is the form an
//!    AWS-owned resource carries. So the entry's ARN has an EMPTY Region segment and
//!    `aws` for its account, is the same string through every Region's endpoint, and
//!    is what the detail read reports too. An ASSOCIATION against the rule is the
//!    ACCOUNT's own resource and keeps the account-bearing form, so the two
//!    namespaces are never confused.
//! 2. `Name`, `Description`, `Tactic` and `Technique` are the published rule's own
//!    (`smithy_query(shape="RuleSummary")`), in the form GuardDuty publishes them in:
//!    the name is one unspaced capitalised word per word of the rule's id, the
//!    description the one clause saying what the rule detects, and the technique the
//!    MITRE ATT&CK technique id followed by ` - ` and the technique's name.
//! 3. `CreatedAt` and `UpdatedAt` are the rule's publication and revision instants,
//!    carried by the catalogue: they are in the past of the run, they are the same
//!    through every Region's endpoint, they do not move when the store is rebuilt (a
//!    restart), and `UpdatedAt` — optional on both `RuleSummary` and `RuleDetail` —
//!    is reported, never before `CreatedAt`, for a rule that has been revised since
//!    publication.

use std::time::{SystemTime, UNIX_EPOCH};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};

use crate::detector_lifecycle::{call_in, new_router, Router};

/// `ListCustomDetectionRules` is `POST /custom-detection-rule/rule`,
/// `GetCustomDetectionRule` is `GET /custom-detection-rule/rule/{RuleId}` and
/// `CreateCustomDetectionRuleAssociation` is
/// `POST /custom-detection-rule/association`
/// (`smithy_query(operation="ListCustomDetectionRules,GetCustomDetectionRule,CreateCustomDetectionRuleAssociation")`).
const RULES_PATH: &str = "/custom-detection-rule/rule";

/// See [`RULES_PATH`].
const ASSOCIATIONS_PATH: &str = "/custom-detection-rule/association";

/// `MaxResults` is `range 1..100`, so one page holds the whole catalogue.
const MAX_RESULTS: i64 = 100;

/// Several Regions, so "the ARN names no Region" is asserted rather than assumed,
/// and the first of them is the one the plain harness signs for.
const REGIONS: [&str; 3] = ["us-east-1", "eu-west-1", "ap-southeast-2"];

/// The entry the recorded `ListCustomDetectionRules` response disclosed, with the
/// values GuardDuty publishes it under: the one row of the table whose published
/// values are known from outside this emulator.
const RECORDED_ENTRY: PublishedEntry = PublishedEntry {
    rule_id: "admin-policy-attached-to-role",
    name: "AdminPolicyAttachedToRole",
    description: "Detects when admin policy is attached to an IAM role",
    technique: "T1098.001 - Account Manipulation: Additional Cloud Credentials",
};

/// A second entry, which the issue this suite was written for did NOT cite: its
/// published values follow the same form, so the rule is asserted on an entry no
/// recording pinned.
const UNCITED_ENTRY: PublishedEntry = PublishedEntry {
    rule_id: "cloudtrail-logging-disabled",
    name: "CloudTrailLoggingDisabled",
    description: "Detects when CloudTrail logging is disabled",
    technique: "T1562.008 - Impair Defenses: Disable or Modify Cloud Logs",
};

/// One expected catalogue entry.
struct PublishedEntry {
    rule_id: &'static str,
    name: &'static str,
    description: &'static str,
    technique: &'static str,
}

/// The whole published catalogue as `region`'s endpoint reports it.
async fn catalogue_in(router: &Router, region: &str) -> Vec<Value> {
    let (status, body) = call_in(
        router,
        region,
        "POST",
        RULES_PATH,
        Some(json!({"maxResults": MAX_RESULTS})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {region:?}: list-custom-detection-rules expected 200, got {status} {body:?}"
    );
    let rules = body["rules"]
        .as_array()
        .unwrap_or_else(|| panic!("case {region:?}: expected a rules array, got {body:?}"))
        .clone();
    assert!(
        !rules.is_empty(),
        "case {region:?}: GuardDuty publishes custom detection rules, so the catalogue must not be empty"
    );
    assert!(
        body["nextToken"].is_null(),
        "case {region:?}: the catalogue must fit one page of {MAX_RESULTS}, got {body:?}"
    );
    rules
}

/// The `RuleDetail` `GetCustomDetectionRule` reports for `rule_id` through
/// `region`'s endpoint.
async fn detail_in(router: &Router, region: &str, rule_id: &str) -> Value {
    let (status, body) =
        call_in(router, region, "GET", &format!("{RULES_PATH}/{rule_id}"), None).await;
    assert_eq!(
        status, 200,
        "case {rule_id:?} in {region}: get-custom-detection-rule expected 200, got {status} {body:?}"
    );
    body["rule"].clone()
}

/// A string member of a reported rule, failing the test when it is absent.
fn text<'a>(rule: &'a Value, member: &str, case: &str) -> &'a str {
    rule[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected the member {member:?}, got {rule:?}"))
}

/// A timestamp member, in the epoch seconds restJson1 carries one as.
fn instant(rule: &Value, member: &str, case: &str) -> f64 {
    rule[member]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected the instant {member:?}, got {rule:?}"))
}

/// The six segments of `arn`, failing the test when it has another number of them.
fn arn_segments<'a>(arn: &'a str, case: &str) -> Vec<&'a str> {
    let segments: Vec<&str> = arn.splitn(6, ':').collect();
    assert_eq!(
        segments.len(),
        6,
        "case {case:?}: an ARN has six segments, got {arn:?}"
    );
    segments
}

/// Does `arn` satisfy `DetectionRuleArn`'s own pattern —
/// `^arn:[a-zA-Z0-9\-]+:[a-zA-Z0-9\-]+:[a-zA-Z0-9\-]*:([0-9]*|aws):.+$`, `len
/// 1..1024` (`smithy_query(shape="DetectionRuleArn")`)?
fn is_detection_rule_arn(arn: &str) -> bool {
    let segments: Vec<&str> = arn.splitn(6, ':').collect();
    if segments.len() != 6 || arn.is_empty() || arn.len() > 1024 {
        return false;
    }
    let dashed = |value: &str| value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    segments[0] == "arn"
        && !segments[1].is_empty()
        && dashed(segments[1])
        && !segments[2].is_empty()
        && dashed(segments[2])
        && dashed(segments[3])
        && (segments[4] == "aws" || segments[4].chars().all(|c| c.is_ascii_digit()))
        && !segments[5].is_empty()
}

// ---------------------------------------------------------------------------
// Rule 1: the ARN of a published rule is the AWS-managed one.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_published_rule_carries_the_aws_managed_arn() {
    let router = new_router();
    let home = catalogue_in(&router, REGIONS[0]).await;

    for region in REGIONS {
        let rules = catalogue_in(&router, region).await;
        assert_eq!(
            rules.len(),
            home.len(),
            "case {region:?}: every Region publishes the same catalogue, got {rules:?}"
        );

        for (rule, expected) in rules.iter().zip(home.iter()) {
            let rule_id = text(rule, "ruleId", region).to_string();
            let case = format!("{rule_id} through {region}");
            let arn = text(rule, "arn", &case);

            // The AWS-managed form, spelled out: the resource path of a custom
            // detection rule under an empty Region and the `aws` account.
            let wanted = format!("arn:aws:guardduty::aws:detection-rule/custom/{rule_id}");
            assert_eq!(
                arn, wanted,
                "case {case:?}: expected the AWS-managed ARN {wanted:?}, got {arn:?}"
            );
            assert!(
                is_detection_rule_arn(arn),
                "case {case:?}: the ARN must satisfy DetectionRuleArn's pattern, got {arn:?}"
            );
            let segments = arn_segments(arn, &case);
            assert_eq!(
                (segments[3], segments[4]),
                ("", "aws"),
                "case {case:?}: a published rule's ARN names no Region and no account, got {arn:?}"
            );
            assert_eq!(
                arn,
                text(expected, "arn", &case),
                "case {case:?}: the ARN is the same through every Region, got {arn:?}"
            );

            // The detail read of the same rule reports the listed ARN.
            let detail = detail_in(&router, region, &rule_id).await;
            assert_eq!(
                text(&detail, "arn", &case),
                arn,
                "case {case:?}: the detail read must report the listed ARN, got {detail:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_association_against_a_published_rule_stays_account_scoped() {
    let router = new_router();
    let account = ::engine::emulator_account_id();

    // Each row: a published rule and the Region the association is made in, so the
    // account-scoped form is asserted across rules and Regions rather than once.
    let rules = catalogue_in(&router, REGIONS[0]).await;
    let rule_ids: Vec<String> = rules
        .iter()
        .take(3)
        .map(|rule| text(rule, "ruleId", "associating").to_string())
        .collect();

    for region in REGIONS {
        for rule_id in &rule_ids {
            let case = format!("an association for {rule_id} in {region}");
            let (status, body) = call_in(
                &router,
                region,
                "POST",
                ASSOCIATIONS_PATH,
                Some(json!({"ruleId": rule_id, "mode": "LIVE"})),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: create-association expected 200, got {status} {body:?}"
            );
            let association = &body["ruleAssociation"];
            let association_id = text(association, "associationId", &case);
            let wanted = format!(
                "arn:aws:guardduty:{region}:{account}:detection-rule/custom/{rule_id}/association/{association_id}"
            );
            assert_eq!(
                text(association, "arn", &case),
                wanted,
                "case {case:?}: an association is the account's own resource, so expected {wanted:?}, got {association:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the catalogue's text is the published rule's own.
// ---------------------------------------------------------------------------

/// Is `name` the published name of the rule `rule_id` — an unspaced run of ASCII
/// letters and digits opening with a capital, whose lower case is the id with its
/// hyphens taken out?
///
/// Stated again here, rather than calling the crate's own predicate, DELIBERATELY:
/// this suite judges the form of what came back over the wire, so it must not be
/// able to pass by agreeing with the production reading. The same holds for
/// [`is_mitre_technique`] below.
fn is_published_name_of(name: &str, rule_id: &str) -> bool {
    name.starts_with(|first: char| first.is_ascii_uppercase())
        && name.chars().all(|c| c.is_ascii_alphanumeric())
        && name.to_ascii_lowercase() == rule_id.replace('-', "")
}

/// Is `technique` the MITRE ATT&CK technique id, then ` - `, then the technique's
/// name?
fn is_mitre_technique(technique: &str) -> bool {
    let Some((id, name)) = technique.split_once(" - ") else {
        return false;
    };
    let Some(digits) = id.strip_prefix('T') else {
        return false;
    };
    let numbered = |part: &str| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit());
    let identified = match digits.split_once('.') {
        Some((technique_id, sub)) => numbered(technique_id) && numbered(sub),
        None => numbered(digits),
    };
    identified && !name.is_empty()
}

#[tokio::test]
async fn every_entry_carries_the_published_rules_own_metadata() {
    let router = new_router();
    let rules = catalogue_in(&router, REGIONS[0]).await;

    for rule in &rules {
        let rule_id = text(rule, "ruleId", "the catalogue").to_string();
        let case = format!("the published rule {rule_id}");

        let name = text(rule, "name", &case);
        assert!(
            is_published_name_of(name, &rule_id),
            "case {case:?}: expected the published name of {rule_id:?} (its words run together, each capitalised), got {name:?}"
        );
        let description = text(rule, "description", &case);
        assert!(
            description.starts_with("Detects ") && !description.ends_with('.'),
            "case {case:?}: expected the one clause saying what the rule detects, got {description:?}"
        );
        let technique = text(rule, "technique", &case);
        assert!(
            is_mitre_technique(technique),
            "case {case:?}: expected a MITRE technique id, ' - ' and the technique name, got {technique:?}"
        );
        let tactic = text(rule, "tactic", &case);
        assert!(
            !tactic.is_empty() && !tactic.contains(" - "),
            "case {case:?}: expected the MITRE tactic's name, got {tactic:?}"
        );

        // The detail read reports the same text as the listing.
        let detail = detail_in(&router, REGIONS[0], &rule_id).await;
        for member in ["name", "description", "tactic", "technique"] {
            assert_eq!(
                detail[member], rule[member],
                "case {case:?}: the detail read must report the listed {member}, got {:?} after {:?}",
                detail[member], rule[member]
            );
        }
    }

    // The two entries whose published values are stated outside this emulator: the
    // one a recording disclosed, and one it did not.
    for expected in [&RECORDED_ENTRY, &UNCITED_ENTRY] {
        let case = format!("the published entry {}", expected.rule_id);
        let rule = rules
            .iter()
            .find(|rule| text(rule, "ruleId", &case) == expected.rule_id)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the catalogue must publish {:?}, got {:?}",
                    expected.rule_id,
                    rules
                        .iter()
                        .map(|rule| text(rule, "ruleId", &case))
                        .collect::<Vec<_>>()
                )
            });
        for (member, wanted) in [
            ("name", expected.name),
            ("description", expected.description),
            ("technique", expected.technique),
        ] {
            assert_eq!(
                text(rule, member, &case),
                wanted,
                "case {case:?}: expected the published {member} {wanted:?}, got {rule:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the instants are the catalogue's, not the process clock's.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn publication_instants_do_not_come_from_the_process_clock() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64();

    let router = new_router();
    let rules = catalogue_in(&router, REGIONS[0]).await;

    // A second store, built after the first one has already answered reads: this is
    // what a RESTART looks like to a caller, and a published instant has to survive
    // it.
    let restarted = new_router();
    let after_restart = catalogue_in(&restarted, REGIONS[0]).await;

    // And the same catalogue through another Region's endpoint, which must report
    // the same instants: they belong to the rule, not to the endpoint.
    let elsewhere = catalogue_in(&router, REGIONS[1]).await;

    let mut revised = 0_usize;

    for ((rule, again), there) in rules.iter().zip(after_restart.iter()).zip(elsewhere.iter()) {
        let rule_id = text(rule, "ruleId", "the catalogue").to_string();
        let case = format!("the published rule {rule_id}");
        for member in ["createdAt", "updatedAt"] {
            assert_eq!(
                there[member], rule[member],
                "case {case:?}: every Region reports the same {member}, got {:?} after {:?}",
                there[member], rule[member]
            );
        }

        let created_at = instant(rule, "createdAt", &case);
        assert!(
            created_at > 0.0 && created_at < now,
            "case {case:?}: a publication instant is in the past of the run, got {created_at} against {now}"
        );
        // The instant IS the catalogue's documented publication date: midnight UTC on
        // the day the state module names, parsed here rather than recomputed.
        let published = instant_of(
            service_com_amazonaws_guardduty_guarddutyapiservice::state::CATALOGUE_PUBLISHED_ON,
        );
        assert_eq!(
            created_at, published,
            "case {case:?}: expected the catalogue's publication date {published}, got {created_at}"
        );
        assert_eq!(
            rule["createdAt"], again["createdAt"],
            "case {case:?}: createdAt is the rule's own, so a restart must report the same instant, got {:?} after {:?}",
            again["createdAt"], rule["createdAt"]
        );
        assert_eq!(
            rule["updatedAt"], again["updatedAt"],
            "case {case:?}: updatedAt is the rule's own, so a restart must report the same instant, got {:?} after {:?}",
            again["updatedAt"], rule["updatedAt"]
        );

        // `UpdatedAt` is optional: a rule GuardDuty has revised since publishing it
        // reports the revision, and one it has not reports no member at all. Either
        // way a revision never precedes the publication, and it too is a catalogue
        // date.
        if !rule["updatedAt"].is_null() {
            revised += 1;
            let updated_at = instant(rule, "updatedAt", &case);
            assert!(
                updated_at >= created_at && updated_at < now,
                "case {case:?}: a revision is after the publication and in the past of the run, got {updated_at} against {created_at} and {now}"
            );
            let revised_on = instant_of(
                service_com_amazonaws_guardduty_guarddutyapiservice::state::CATALOGUE_REVISED_ON,
            );
            assert_eq!(
                updated_at, revised_on,
                "case {case:?}: expected the catalogue's revision date {revised_on}, got {updated_at}"
            );
        }

        // The detail read reports the same instants as the listing.
        let detail = detail_in(&router, REGIONS[0], &rule_id).await;
        for member in ["createdAt", "updatedAt"] {
            assert_eq!(
                detail[member], rule[member],
                "case {case:?}: the detail read must report the listed {member}, got {:?} after {:?}",
                detail[member], rule[member]
            );
        }
    }

    assert!(
        revised > 0,
        "case \"the catalogue\": a rule revised since publication reports updatedAt, so the catalogue must carry at least one, got {rules:?}"
    );
}

/// A publication instant is the catalogue's own DATE: the instant the store carries
/// for an entry is midnight UTC on the date that entry was published, parsed here
/// from the date the catalogue documents rather than recomputed, so a stored number
/// that is not the date it claims to be fails this suite.
fn instant_of(iso: &str) -> f64 {
    DateTime::from_str(iso, Format::DateTime)
        .unwrap_or_else(|error| panic!("{iso:?} must be a timestamp: {error}"))
        .as_secs_f64()
}
