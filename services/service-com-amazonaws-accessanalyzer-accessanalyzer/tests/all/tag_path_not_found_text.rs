// SPDX-License-Identifier: CC0-1.0

//! The MESSAGE the ARN-addressed tagging path's not-found refusal carries, over a
//! GENERATED ARN space.
//!
//! `tests/all/fixed_refusal_texts.rs` asserts rule 1 over a hand-written table of the
//! reasons an ARN resolves to nothing, and owns the expected string
//! ([`RESOURCE_WAS_NOT_FOUND`]). This suite is the generated half of the same rule: the
//! ARN is built segment by segment from the whole cross product of the six segments the
//! form `arn:partition:service:region:account:resource` has — partition, service, region,
//! account, and resource type plus name — so the inputs are not the ones anybody thought
//! to write down. For every combination except the ONE that is a stored analyzer's ARN,
//! each of the three tagging operations must answer HTTP 404, the code
//! `NotFoundException` and that same constant message; for that one combination the call
//! is served instead, which is what keeps the rule from being satisfied by refusing
//! everything.
//!
//! Every case goes through the real router (`tests/all/harness.rs`).

use serde_json::{json, Value};

// The three tagging calls, the `message` reader and the expected string all live in the
// suite that owns rule 1; this suite drives the SAME calls over generated ARNs.
use super::fixed_refusal_texts::{message, RESOURCE_WAS_NOT_FOUND, TAGGING_CALLS};
use crate::harness::{expected_arn, Emulator, REGION};

/// The error code the tagging path answers an ARN it cannot resolve with.
const NOT_FOUND_EXCEPTION: &str = "NotFoundException";

/// The analyzer the generated space contains an ARN for, so exactly one combination
/// resolves.
const LIVE_ANALYZER: &str = "live_target";

/// Every ARN the cross product of the six segments yields; the ARN is its own case name.
///
/// The segment values are the ones the form generalises over: the partitions AWS
/// publishes, this service's own `service` segment and another service's, this Region and
/// another, this account and another account and none at all, and the resource segment as
/// an analyzer (under several name forms), as a resource type this service does not tag,
/// and as a resource of another service.
fn generated_arns() -> Vec<String> {
    let account = engine::emulator_account_id();
    let long_name = "n".repeat(200);
    let partitions = ["aws", "aws-cn", "aws-us-gov"];
    let services = ["access-analyzer", "s3"];
    let regions = [REGION, "eu-west-1"];
    let accounts = [account, "111122223333", ""];
    let resources: Vec<String> = vec![
        format!("analyzer/{LIVE_ANALYZER}"),
        "analyzer/never_created".to_string(),
        format!("analyzer/{}", LIVE_ANALYZER.to_uppercase()),
        format!("analyzer/{long_name}"),
        format!("archive-rule/{LIVE_ANALYZER}"),
        format!("access-preview/{LIVE_ANALYZER}"),
        "some-bucket".to_string(),
    ];

    let mut arns = Vec::new();
    for partition in partitions {
        for service in services {
            for region in regions {
                for account_segment in accounts {
                    for resource in &resources {
                        arns.push(format!(
                            "arn:{partition}:{service}:{region}:{account_segment}:{resource}"
                        ));
                    }
                }
            }
        }
    }
    arns
}

/// THE RULE: every not-found refusal of the tagging path carries the service's constant
/// message, over the generated ARN space — and the one generated ARN that resolves is
/// served rather than refused.
#[tokio::test]
async fn every_generated_unresolvable_arn_is_refused_with_the_fixed_text() {
    let emulator = Emulator::new();
    let created = emulator
        .create_analyzer(json!({"analyzerName": LIVE_ANALYZER, "type": "ACCOUNT"}))
        .await;
    assert_eq!(
        created.status, 200,
        "the fixture create of {LIVE_ANALYZER:?} must succeed, got {created:?}"
    );
    let live = expected_arn(LIVE_ANALYZER);

    let account = engine::emulator_account_id();
    let mut served = 0usize;
    let mut refused = 0usize;
    for resource_arn in generated_arns() {
        for call in TAGGING_CALLS {
            let case = format!("{call:?} on {resource_arn}");
            let reply = call.send(&emulator, &resource_arn).await;
            if resource_arn == live {
                // The boundary built from the same generator: this ARN resolves, so the
                // rule must not refuse it.
                assert_eq!(
                    reply.status,
                    call.served_status(),
                    "case {case:?}: the stored analyzer's ARN must be served, got {reply:?}"
                );
                served += 1;
                continue;
            }
            assert_eq!(
                reply.status, 404,
                "case {case:?}: an unresolvable ARN is HTTP 404, got {reply:?}"
            );
            assert_eq!(
                reply.code(),
                NOT_FOUND_EXCEPTION,
                "case {case:?}: expected the code {NOT_FOUND_EXCEPTION:?}, got {:?}",
                reply.code()
            );
            assert_eq!(
                message(&reply),
                RESOURCE_WAS_NOT_FOUND,
                "case {case:?}: expected the fixed text {RESOURCE_WAS_NOT_FOUND:?}, got {:?}",
                message(&reply)
            );
            // The text is a constant and the shape carries no structured identifier, so
            // nothing of the request or of the environment may appear in it.
            for leaked in [
                resource_arn.as_str(),
                LIVE_ANALYZER,
                account,
                REGION,
                "arn:",
                "analyzer",
                "probe",
            ] {
                assert!(
                    !message(&reply).contains(leaked),
                    "case {case:?}: the text must carry no request or environment value, \
                     but {:?} contains {leaked:?}",
                    message(&reply)
                );
            }
            refused += 1;
        }
    }

    // The generator really did produce both sides of the rule.
    assert_eq!(
        served,
        TAGGING_CALLS.len(),
        "exactly one generated ARN resolves, so each tagging call must be served once, \
         got {served} served and {refused} refused"
    );
    assert!(
        refused > 100,
        "the generated space must cover far more than the written-down cases, got {refused}"
    );

    // Every refusal wrote nothing: the only tag the live analyzer holds is the one the
    // served call left, and the analyzer is still there.
    let listed = emulator.list_tags_for_resource(&live).await;
    assert_eq!(
        listed.status, 200,
        "the stored analyzer's ARN must still resolve, got {listed:?}"
    );
    assert_eq!(
        listed.member("tags"),
        Some(&json!({})),
        "the served tag was removed again by the served removal, and a refused call must \
         write nothing, got {listed:?}"
    );
}

/// The OTHER boundary of the rule: an ARN the tagging surface refuses as INVALID input is
/// a 400 and must not carry the not-found text, whichever way it is malformed.
#[tokio::test]
async fn a_malformed_resource_arn_does_not_carry_the_not_found_text() {
    let emulator = Emulator::new();
    let account = engine::emulator_account_id();

    // The rows differ along HOW the value misses the ARN form: not an ARN at all, too few
    // segments, an empty resource segment, and an ARN-looking value whose first segment is
    // not `arn`.
    let malformed = [
        ("a bare analyzer name", LIVE_ANALYZER.to_string()),
        (
            "too few segments",
            format!("arn:aws:access-analyzer:{REGION}:{account}"),
        ),
        (
            "an empty resource segment",
            format!("arn:aws:access-analyzer:{REGION}:{account}:"),
        ),
        (
            "a first segment that is not arn",
            format!("urn:aws:access-analyzer:{REGION}:{account}:analyzer/{LIVE_ANALYZER}"),
        ),
    ];

    for (row, resource_arn) in malformed {
        for call in TAGGING_CALLS {
            let case = format!("{call:?} on {row}");
            let reply = call.send(&emulator, &resource_arn).await;
            assert_eq!(
                (reply.status, reply.code()),
                (400, "ValidationException"),
                "case {case:?}: a value outside the ARN form is a 400 ValidationException, \
                 got {reply:?}"
            );
            assert_ne!(
                message(&reply),
                RESOURCE_WAS_NOT_FOUND,
                "case {case:?}: the not-found text must not answer a validation refusal, \
                 got {:?}",
                message(&reply)
            );
            // The validation refusal is about a member of the request, so it DOES name it.
            assert_eq!(
                reply
                    .member("fieldList")
                    .and_then(Value::as_array)
                    .map(|fields| fields
                        .iter()
                        .filter_map(|field| field.get("name").and_then(Value::as_str))
                        .collect::<Vec<_>>()),
                Some(vec!["resourceArn"]),
                "case {case:?}: expected fieldList to name resourceArn, got {reply:?}"
            );
        }
    }
}
