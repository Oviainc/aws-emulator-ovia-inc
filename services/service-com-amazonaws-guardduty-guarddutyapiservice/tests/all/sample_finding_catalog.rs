// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the GuardDuty sample-finding generator: which finding types
//! it accepts, and what a generated finding reports.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, reusing the
//! harness in [`super::detector_lifecycle`], so routing, restJson1 (de)serialization and
//! the handlers are exercised together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. A `findingTypes` entry that is not a GuardDuty finding type is refused with the
//!    modelled `BadRequestException` envelope — HTTP 400,
//!    `x-amzn-errortype: BadRequestException` — carrying the error CLASS
//!    `InvalidInputException` in the body's `__type` discriminator and in its `type`
//!    member — and the refused request writes nothing.
//! 2. A generated sample finding's `severity` is the value GuardDuty publishes for its
//!    finding TYPE, which spans the four documented severity levels.
//! 3. A generated sample finding reports `service.additionalInfo` as a JSON DOCUMENT
//!    string carrying the sample marker, with `type` describing the document.

use serde_json::{json, Value};

use super::detector_lifecycle::{
    all_finding_ids, call, enabled_detector, error_code, error_shape, new_router,
};

/// A well-formed `DetectorId` (32 lowercase hex characters) that no create minted.
const ABSENT_DETECTOR_ID: &str = "0123456789abcdef0123456789abcdef";

/// The numeric `Finding.severity` of each documented severity LEVEL: Low 1.0–3.9,
/// Medium 4.0–6.9, High 7.0–8.9, Critical 9.0–10.0 (GuardDuty "Severity levels for
/// GuardDuty findings"), each reported as the representative value of its band.
const LOW: f64 = 2.0;
const MEDIUM: f64 = 5.0;
const HIGH: f64 = 8.0;
const CRITICAL: f64 = 9.0;

/// Every severity a generated finding may carry.
const PUBLISHED_SEVERITIES: [f64; 4] = [LOW, MEDIUM, HIGH, CRITICAL];

/// Ask for sample findings of `finding_types` (`None` for a request that names none).
async fn create_sample_findings(
    router: &super::detector_lifecycle::Router,
    detector_id: &str,
    finding_types: Option<Value>,
) -> (u16, Value) {
    let body = match finding_types {
        Some(types) => json!({"findingTypes": types}),
        None => json!({}),
    };
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/findings/create"),
        Some(body),
    )
    .await
}

/// Read findings back by id, in chunks of 50 — `GetFindings.findingIds` is
/// `len 0..50` (`smithy_query(operation="GetFindings")`).
pub(crate) async fn get_findings(
    router: &super::detector_lifecycle::Router,
    detector_id: &str,
    ids: &[String],
) -> Vec<Value> {
    let mut findings = Vec::new();
    for chunk in ids.chunks(50) {
        let (status, got) = call(
            router,
            "POST",
            &format!("/detector/{detector_id}/findings/get"),
            Some(json!({"findingIds": chunk})),
        )
        .await;
        assert_eq!(status, 200, "get-findings: expected 200, got {status} {got:?}");
        findings.extend(got["findings"].as_array().cloned().unwrap_or_default());
    }
    findings
}

/// The finding of `finding_type` among `findings`.
fn finding_of<'a>(findings: &'a [Value], finding_type: &str) -> &'a Value {
    findings
        .iter()
        .find(|f| f["type"] == json!(finding_type))
        .unwrap_or_else(|| panic!("no finding of type {finding_type:?} in {findings:?}"))
}

// ---------------------------------------------------------------------------
// Rule 1: a finding type outside the catalogue is refused, and nothing is written.
// ---------------------------------------------------------------------------

/// A finding type is an IDENTIFIER published by GuardDuty, so a string that is not one
/// of them is not a finding type however plausible it looks. The rows differ along
/// the ways a string can fail to be one: no segments at all, an invented threat
/// purpose, an invented affected-resource type, an invented threat family under a real
/// purpose and resource, the wrong case, stray whitespace around a real type, a real
/// type with an artifact it does not have, a truncated type, and the longest string
/// the model allows (`FindingType` is `len 1..100`,
/// `smithy_query(shape="FindingType")`).
#[tokio::test]
async fn a_finding_type_outside_the_catalogue_is_refused() {
    let longest = "X".repeat(100);
    let cases: [(&str, &str); 9] = [
        ("no segments", "NotAFindingType"),
        ("invented threat purpose", "Bogus:EC2/PortProbeUnprotectedPort"),
        ("invented resource type", "Recon:NotAResource/PortProbeUnprotectedPort"),
        ("invented threat family", "Recon:EC2/NoSuchThreatFamily"),
        ("wrong case", "recon:ec2/portprobeunprotectedport"),
        ("trailing whitespace", "Recon:EC2/PortProbeUnprotectedPort "),
        ("invented artifact", "Recon:EC2/PortProbeUnprotectedPort!PROC"),
        ("truncated type", "Recon:EC2"),
        ("longest the model allows", longest.as_str()),
    ];

    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    for case in cases {
        let (label, finding_type) = case;
        let (status, body) =
            create_sample_findings(&router, &detector_id, Some(json!([finding_type]))).await;
        assert_eq!(
            status, 400,
            "case {label:?} ({finding_type:?}): expected 400, got {status} {body:?}"
        );
        assert_eq!(
            error_shape(&body),
            "BadRequestException",
            "case {label:?} ({finding_type:?}): expected the modelled error shape, got {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {label:?} ({finding_type:?}): expected the invalid-input error class in __type, got {body:?}"
        );
        assert_eq!(
            body["type"],
            json!("InvalidInputException"),
            "case {label:?} ({finding_type:?}): expected the invalid-input error class, got {body:?}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("finding type"),
            "case {label:?} ({finding_type:?}): expected a message about the finding type, got {body:?}"
        );
        assert!(
            !message.contains(finding_type),
            "case {label:?}: the refusal must not echo the request's value, got {body:?}"
        );

        // A refused request writes nothing: the detector still holds no finding.
        let ids = all_finding_ids(&router, &detector_id, 50).await;
        assert!(
            ids.is_empty(),
            "case {label:?} ({finding_type:?}): a refused request left {} finding(s) behind",
            ids.len()
        );
    }

    // A list the service CAN generate every entry of is accepted — the refusal above
    // is about the entries, not about validation refusing everything.
    let (status, body) = create_sample_findings(
        &router,
        &detector_id,
        Some(json!(["Recon:EC2/PortProbeUnprotectedPort", "Impact:S3/TorIPCaller"])),
    )
    .await;
    assert_eq!(status, 200, "two catalogued types: expected 200, got {status} {body:?}");
    assert_eq!(
        all_finding_ids(&router, &detector_id, 50).await.len(),
        2,
        "two catalogued types: expected one finding per type"
    );
}

/// One unacceptable entry refuses the WHOLE request: the findings of the entries
/// beside it are not generated either. The rows put the bad entry at each position of
/// the list.
#[tokio::test]
async fn one_entry_the_service_cannot_generate_refuses_the_whole_list() {
    let cases: [(&str, Value); 3] = [
        ("first", json!(["Fake:EC2/NotAType", "Recon:EC2/Portscan", "Impact:EC2/PortSweep"])),
        ("middle", json!(["Recon:EC2/Portscan", "Fake:EC2/NotAType", "Impact:EC2/PortSweep"])),
        ("last", json!(["Recon:EC2/Portscan", "Impact:EC2/PortSweep", "Fake:EC2/NotAType"])),
    ];

    for case in cases {
        let (label, types) = case;
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        let (status, body) = create_sample_findings(&router, &detector_id, Some(types.clone())).await;
        assert_eq!(
            status, 400,
            "case {label:?} ({types:?}): expected 400, got {status} {body:?}"
        );
        let ids = all_finding_ids(&router, &detector_id, 50).await;
        assert!(
            ids.is_empty(),
            "case {label:?} ({types:?}): a refused request left {} finding(s) behind",
            ids.len()
        );
    }
}

/// What the request CARRIES is validated before the detector it names is looked up, so
/// an unacceptable finding type is reported as such whether or not the detector
/// exists.
#[tokio::test]
async fn an_unacceptable_finding_type_is_reported_before_the_detector_is_looked_up() {
    let router = new_router();
    let (status, body) =
        create_sample_findings(&router, ABSENT_DETECTOR_ID, Some(json!(["Fake:EC2/NotAType"]))).await;
    assert_eq!(status, 400, "expected 400, got {status} {body:?}");
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("finding type"),
        "expected the finding-type rejection, got {body:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 2: severity is the one published for the finding TYPE.
// ---------------------------------------------------------------------------

/// GuardDuty publishes one severity LEVEL per finding type, so two findings sharing a
/// threat purpose can sit in different bands and two sharing a band can have different
/// purposes. The rows span the four levels, the affected-resource families and the
/// threat purposes, and none of them is a type the issue cited.
#[tokio::test]
async fn a_sample_finding_carries_the_severity_published_for_its_finding_type() {
    // (finding type, the severity GuardDuty publishes for it) — source: the
    // "Severity" column of the GuardDuty finding-types reference tables.
    let cases: [(&str, f64); 12] = [
        ("AttackSequence:S3/CompromisedData", CRITICAL),
        ("Backdoor:EC2/C&CActivity.B!DNS", HIGH),
        ("Policy:S3/BucketPublicAccessGranted", HIGH),
        ("CredentialAccess:RDS/TorIPCaller.SuccessfulLogin", HIGH),
        ("UnauthorizedAccess:Lambda/TorRelay", HIGH),
        ("Behavior:EC2/TrafficVolumeUnusual", MEDIUM),
        ("PenTest:S3/ParrotLinux", MEDIUM),
        ("Execution:Runtime/NewBinaryExecuted", MEDIUM),
        ("Persistence:Kubernetes/ContainerWithSensitiveMount", MEDIUM),
        ("Impact:EC2/SuspiciousDomainRequest.Reputation", LOW),
        ("Stealth:S3/ServerAccessLoggingDisabled", LOW),
        ("Discovery:IAMUser/AnomalousBehavior", LOW),
    ];

    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let requested: Vec<&str> = cases.iter().map(|(ty, _)| *ty).collect();
    let (status, body) = create_sample_findings(&router, &detector_id, Some(json!(requested))).await;
    assert_eq!(status, 200, "create-sample-findings: got {status} {body:?}");

    let ids = all_finding_ids(&router, &detector_id, 50).await;
    assert_eq!(
        ids.len(),
        cases.len(),
        "expected one finding per requested type, got {ids:?}"
    );
    let findings = get_findings(&router, &detector_id, &ids).await;

    for case in cases {
        let (finding_type, expected) = case;
        let finding = finding_of(&findings, finding_type);
        let severity = finding["severity"].as_f64().unwrap_or_default();
        assert!(
            (severity - expected).abs() < f64::EPSILON,
            "case {case:?}: expected severity {expected}, got {severity} in {finding:?}"
        );
    }
}

/// With no `findingTypes` the service "generates sample findings of all supported
/// finding types" (`smithy_query(query="doc",shape="CreateSampleFindings")`). Over that
/// whole set the invariants hold: one finding per type, every severity one of the four
/// published values, and all four levels represented — so a client filtering by a
/// severity threshold sees findings on both sides of it.
#[tokio::test]
async fn the_default_sample_set_covers_the_published_severity_levels() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let (status, body) = create_sample_findings(&router, &detector_id, None).await;
    assert_eq!(status, 200, "create-sample-findings with no types: got {status} {body:?}");

    let ids = all_finding_ids(&router, &detector_id, 50).await;
    assert!(
        ids.len() > PUBLISHED_SEVERITIES.len(),
        "the supported set must span the taxonomy, got {} finding(s)",
        ids.len()
    );
    let findings = get_findings(&router, &detector_id, &ids).await;
    assert_eq!(
        findings.len(),
        ids.len(),
        "expected a finding for every listed id, got {} of {}",
        findings.len(),
        ids.len()
    );

    let mut types = std::collections::BTreeSet::new();
    let mut seen_severities = std::collections::BTreeSet::new();
    for finding in &findings {
        let finding_type = finding["type"].as_str().unwrap_or_default().to_string();
        let severity = finding["severity"].as_f64().unwrap_or_default();
        assert!(
            PUBLISHED_SEVERITIES
                .iter()
                .any(|published| (severity - published).abs() < f64::EPSILON),
            "case {finding_type:?}: expected one of the published severities {PUBLISHED_SEVERITIES:?}, got {severity}"
        );
        assert!(
            types.insert(finding_type.clone()),
            "case {finding_type:?}: the supported set generated the same type twice"
        );
        seen_severities.insert(format!("{severity}"));
    }
    for published in PUBLISHED_SEVERITIES {
        assert!(
            seen_severities.contains(&format!("{published}")),
            "the supported set must cover severity {published}, got {seen_severities:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the sample marker is a JSON document in additionalInfo.value.
// ---------------------------------------------------------------------------

/// `Service.AdditionalInfo.Value` is "the value of the additional information"
/// (`smithy_query(shape="ServiceAdditionalInfo")`) — a document string a client
/// PARSES — and `Type` describes the kind of document it is. The rows span finding
/// types from different resource families: the marker is about the finding being a
/// sample, so it does not vary with the type.
#[tokio::test]
async fn a_sample_finding_reports_the_sample_marker_as_a_json_document() {
    let cases: [&str; 4] = [
        "Trojan:EC2/DropPoint!DNS",
        "Policy:Kubernetes/AnonymousAccessGranted",
        "Discovery:S3/TorIPCaller",
        "AttackSequence:IAM/CompromisedCredentials",
    ];

    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let (status, body) = create_sample_findings(&router, &detector_id, Some(json!(cases))).await;
    assert_eq!(status, 200, "create-sample-findings: got {status} {body:?}");

    let ids = all_finding_ids(&router, &detector_id, 50).await;
    let findings = get_findings(&router, &detector_id, &ids).await;

    for case in cases {
        let finding = finding_of(&findings, case);
        let additional_info = &finding["service"]["additionalInfo"];
        assert_eq!(
            additional_info["type"],
            json!("default"),
            "case {case:?}: expected additionalInfo.type default, got {additional_info:?}"
        );
        let value = additional_info["value"].as_str().unwrap_or_else(|| {
            panic!("case {case:?}: expected a document string in additionalInfo.value, got {additional_info:?}")
        });
        let parsed: Value = serde_json::from_str(value).unwrap_or_else(|error| {
            panic!("case {case:?}: additionalInfo.value {value:?} is not a JSON document: {error}")
        });
        assert_eq!(
            parsed,
            json!({"sample": true}),
            "case {case:?}: expected the sample marker document, got {value:?}"
        );
    }
}
