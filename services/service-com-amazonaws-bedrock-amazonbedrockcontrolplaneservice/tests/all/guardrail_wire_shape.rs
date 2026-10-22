// SPDX-License-Identifier: CC0-1.0

//! The wire SHAPE of a guardrail read, as three rules, each asserted over a table that
//! spans its class rather than over one recorded request.
//!
//! 1. TIMESTAMP FORM. A `Timestamp` member whose model carries
//!    `@timestampFormat("date-time")` goes onto the restJson1 wire as an RFC 3339
//!    date-time STRING, never as the epoch NUMBER the protocol default would produce.
//!    Asserted on every such member of this service's served surface
//!    (`CreateGuardrailResponse$createdAt`, `UpdateGuardrailResponse$updatedAt`,
//!    `GetGuardrailResponse$createdAt`/`$updatedAt`,
//!    `GuardrailSummary$createdAt`/`$updatedAt`), on the wire and on the member schema.
//!
//! 2. PRESENT COLLECTIONS. A modelled list member whose `@length` admits zero entries is
//!    reported as a PRESENT, possibly empty, array; a list whose `@length` has `min: 1`
//!    cannot be reported empty and so stays absent until the request configures it.
//!
//! 3. DEFAULT TIER. A policy block that models a `tier` always reports one: the tier the
//!    request named, or `CLASSIC` when it named none.
//!
//! Every row differs along what its rule generalises over — which policy block, which
//! list, which tier value, which operation reports the member — and the recorded shape of
//! any one request is at most one of them.

use serde_json::{json, Value};
use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice as bedrock;

use crate::support::{date_time_seconds, emulator, Emulator};

/// A request that configures every policy block that models a tier, with the tier value
/// each row carries (`None` configures no `tierConfig` at all).
fn with_tiers(name: &str, tier: Option<&str>) -> Value {
    let tier_config = tier.map(|tier| json!({ "tierName": tier }));
    let mut body = json!({
        "name": name,
        "blockedInputMessaging": "in",
        "blockedOutputsMessaging": "out",
        "contentPolicyConfig": {
            "filtersConfig": [ {
                "type": "HATE",
                "inputStrength": "LOW",
                "outputStrength": "NONE",
            } ],
        },
        "topicPolicyConfig": {
            "topicsConfig": [ {
                "name": "Topic",
                "definition": "a denied topic",
                "type": "DENY",
            } ],
        },
        "contextualGroundingPolicyConfig": {
            "filtersConfig": [ { "type": "GROUNDING", "threshold": 0.5 } ],
        },
    });
    if let Some(tier_config) = tier_config {
        for block in [
            "contentPolicyConfig",
            "topicPolicyConfig",
            "contextualGroundingPolicyConfig",
        ] {
            body[block]["tierConfig"] = tier_config.clone();
        }
    }
    body
}

/// Create a guardrail from `body` and read it back, failing with `case` named.
async fn create_then_get(emulator: &Emulator, case: &str, body: Value) -> crate::support::Reply {
    let created = emulator.create(body).await;
    assert_eq!(
        created.status, 202,
        "case {case}: CreateGuardrail must answer 202, got {} {:?}",
        created.status, created.body
    );
    let id = created.text("guardrailId");
    let got = emulator.get(&id, None).await;
    assert_eq!(
        got.status, 200,
        "case {case}: GetGuardrail must answer 200, got {} {:?}",
        got.status, got.body
    );
    got
}

/// RULE 1, on the wire: every `date-time` member of the guardrail surface is an RFC 3339
/// string, on the create, the update, the read and the listing alike.
#[tokio::test]
async fn every_date_time_member_is_an_rfc_3339_string() {
    let emulator = emulator();
    let created = emulator
        .create(json!({
            "name": "wire-form",
            "blockedInputMessaging": "in",
            "blockedOutputsMessaging": "out",
        }))
        .await;
    let id = created.text("guardrailId");
    let got = emulator.get(&id, None).await;
    let updated = emulator
        .update(
            &id,
            json!({
                "name": "wire-form",
                "blockedInputMessaging": "in",
                "blockedOutputsMessaging": "out",
            }),
        )
        .await;
    let listed = emulator.list("").await;
    let summary = listed
        .get("guardrails")
        .as_array()
        .and_then(|rows| rows.first().cloned())
        .unwrap_or(Value::Null);

    // (case, the JSON value the wire carried). One row per `@timestampFormat("date-time")`
    // member of the operations this service serves.
    let rows = vec![
        ("CreateGuardrailResponse$createdAt", created.get("createdAt")),
        ("UpdateGuardrailResponse$updatedAt", updated.get("updatedAt")),
        ("GetGuardrailResponse$createdAt", got.get("createdAt")),
        ("GetGuardrailResponse$updatedAt", got.get("updatedAt")),
        (
            "GuardrailSummary$createdAt",
            summary.get("createdAt").cloned().unwrap_or(Value::Null),
        ),
        (
            "GuardrailSummary$updatedAt",
            summary.get("updatedAt").cloned().unwrap_or(Value::Null),
        ),
    ];

    for (case, value) in rows {
        assert!(
            value.is_string(),
            "case {case}: a @timestampFormat(\"date-time\") member is a JSON string, \
             got {value}"
        );
        assert!(
            !value.is_number(),
            "case {case}: a @timestampFormat(\"date-time\") member is never the epoch \
             number the protocol default would write, got {value}"
        );
        let text = value.as_str().unwrap_or_default();
        let seconds = date_time_seconds(case, text);
        assert!(
            seconds > 0.0,
            "case {case}: {text:?} must name an instant after the epoch, got {seconds}"
        );
        assert!(
            text.ends_with('Z') && text.contains('T'),
            "case {case}: an RFC 3339 date-time carries the T separator and the Z offset, \
             got {text:?}"
        );
    }

    // The two instants a read reports are the ones the mutating calls minted, so the
    // string form carries the real instant and not a re-rendered now.
    assert!(
        (date_time_seconds("createdAt round trip", &got.text("createdAt"))
            - date_time_seconds("createdAt round trip", &created.text("createdAt")))
        .abs()
            < f64::EPSILON,
        "case createdAt round trip: GetGuardrail must report the instant CreateGuardrail \
         minted, got {:?} against {:?}",
        got.get("createdAt"),
        created.get("createdAt")
    );
}

/// RULE 1, on the schema: every member the model marks `@timestampFormat("date-time")`
/// reports that format, and a member the model leaves without one reports none — which is
/// what makes the codec write a string for the first and a number for the second.
///
/// The two lists are the model's: `smithy_query(query="operation", operation="GetGuardrail,
/// CreateGuardrail,UpdateGuardrail")` and `smithy_query(query="shape",
/// shape="GuardrailSummary")` report `"timestampFormat": "date-time"` on exactly the six
/// timestamp members this service's guardrail responses carry.
#[test]
fn every_date_time_member_schema_carries_the_format() {
    use ::aws_smithy_schema::traits::TimestampFormat;
    use bedrock::ops::types::{
        CreateGuardrailOutput, GetGuardrailOutput, GuardrailSummary, UpdateGuardrailOutput,
    };

    let date_time = vec![
        ("CreateGuardrailResponse$createdAt", &CreateGuardrailOutput::M_CREATED_AT),
        ("UpdateGuardrailResponse$updatedAt", &UpdateGuardrailOutput::M_UPDATED_AT),
        ("GetGuardrailResponse$createdAt", &GetGuardrailOutput::M_CREATED_AT),
        ("GetGuardrailResponse$updatedAt", &GetGuardrailOutput::M_UPDATED_AT),
        ("GuardrailSummary$createdAt", &GuardrailSummary::M_CREATED_AT),
        ("GuardrailSummary$updatedAt", &GuardrailSummary::M_UPDATED_AT),
    ];
    for (case, schema) in date_time {
        let format = schema.timestamp_format().map(|trait_| trait_.format());
        assert!(
            matches!(format, Some(TimestampFormat::DateTime)),
            "case {case}: the member schema must carry @timestampFormat(date-time), \
             got {format:?}"
        );
    }

    // The control rows: a member the model gives no `@timestampFormat` reports none, so
    // the assertion above is about the trait and not about every member schema.
    let without = vec![
        ("GetGuardrailResponse$status", &GetGuardrailOutput::M_STATUS),
        ("GuardrailSummary$version", &GuardrailSummary::M_VERSION),
    ];
    for (case, schema) in without {
        let format = schema.timestamp_format().map(|trait_| trait_.format());
        assert!(
            format.is_none(),
            "case {case}: the model gives this member no @timestampFormat, got {format:?}"
        );
    }
}

/// RULE 2: a list member whose `@length` admits zero entries is present as `[]` when
/// nothing is configured; a list whose `@length` has `min: 1` stays absent instead.
///
/// The rows differ along which block is configured and which of its lists the request
/// fills, and the bounds come from the model (`smithy_query(query="shape", …)`):
/// `GuardrailWordPolicy$managedWordLists` and `GuardrailSensitiveInformationPolicy$regexes`
/// carry no `min`, `GuardrailTopicExamples` is `@length(min: 0, max: 5)`, while
/// `GuardrailWords` and `GuardrailPiiEntities` are `@length(min: 1)`.
#[tokio::test]
async fn a_list_that_admits_zero_entries_is_reported_empty() {
    let emulator = emulator();

    // (case, request additions, the members GetGuardrail must then report)
    let cases = vec![
        (
            "word policy with words only",
            json!({ "wordPolicyConfig": { "wordsConfig": [ { "text": "nope" } ] } }),
            json!({
                "wordPolicy": {
                    "words": [ { "text": "nope" } ],
                    "managedWordLists": [],
                },
            }),
        ),
        (
            "word policy with managed lists only",
            json!({
                "wordPolicyConfig": {
                    "managedWordListsConfig": [ { "type": "PROFANITY" } ],
                },
            }),
            json!({
                // `GuardrailWords` is @length(min: 1), so an unconfigured `words` is
                // absent rather than empty.
                "wordPolicy": {
                    "managedWordLists": [ { "type": "PROFANITY" } ],
                },
            }),
        ),
        (
            "word policy with neither list",
            json!({ "wordPolicyConfig": {} }),
            json!({ "wordPolicy": { "managedWordLists": [] } }),
        ),
        (
            "no word policy at all",
            json!({}),
            // The BLOCK itself is absent when the request configured none; only its
            // list members are emitted empty.
            json!({ "wordPolicy": null }),
        ),
        (
            "topic with no examples",
            json!({
                "topicPolicyConfig": {
                    "topicsConfig": [ {
                        "name": "Topic",
                        "definition": "a denied topic",
                        "type": "DENY",
                    } ],
                },
            }),
            json!({
                "topicPolicy": {
                    "topics": [ {
                        "name": "Topic",
                        "definition": "a denied topic",
                        "type": "DENY",
                        "examples": [],
                    } ],
                    "tier": { "tierName": "CLASSIC" },
                },
            }),
        ),
        (
            "topic with the most examples the model admits",
            json!({
                "topicPolicyConfig": {
                    "topicsConfig": [ {
                        "name": "Topic",
                        "definition": "a denied topic",
                        "type": "DENY",
                        "examples": [ "a", "b", "c", "d", "e" ],
                    } ],
                },
            }),
            json!({
                "topicPolicy": {
                    "topics": [ {
                        "name": "Topic",
                        "definition": "a denied topic",
                        "type": "DENY",
                        "examples": [ "a", "b", "c", "d", "e" ],
                    } ],
                    "tier": { "tierName": "CLASSIC" },
                },
            }),
        ),
        (
            "sensitive information policy with PII entities only",
            json!({
                "sensitiveInformationPolicyConfig": {
                    "piiEntitiesConfig": [ { "type": "EMAIL", "action": "ANONYMIZE" } ],
                },
            }),
            json!({
                "sensitiveInformationPolicy": {
                    "piiEntities": [ { "type": "EMAIL", "action": "ANONYMIZE" } ],
                    "regexes": [],
                },
            }),
        ),
        (
            "sensitive information policy with regexes only",
            json!({
                "sensitiveInformationPolicyConfig": {
                    "regexesConfig": [ {
                        "name": "nine-digits",
                        "pattern": "[0-9]{9}",
                        "action": "BLOCK",
                    } ],
                },
            }),
            json!({
                // `GuardrailPiiEntities` is @length(min: 1), so it stays absent.
                "sensitiveInformationPolicy": {
                    "regexes": [ {
                        "name": "nine-digits",
                        "pattern": "[0-9]{9}",
                        "action": "BLOCK",
                    } ],
                },
            }),
        ),
    ];

    for (index, (case, extra, expected)) in cases.into_iter().enumerate() {
        let mut body = json!({
            "name": format!("lists-{index}"),
            "blockedInputMessaging": "in",
            "blockedOutputsMessaging": "out",
        });
        for (key, value) in extra.as_object().expect("an object") {
            body[key] = value.clone();
        }
        let got = create_then_get(&emulator, case, body).await;

        for (member, want) in expected.as_object().expect("an object") {
            let actual = got.get(member);
            assert_eq!(
                &actual, want,
                "case {case}: member {member}: expected {want}, got {actual}"
            );
        }

        // The two top-level lists of the read are the same rule: both admit zero
        // entries, and a guardrail that is not FAILED has nothing to report in either,
        // so both are present and empty on EVERY row.
        for member in ["statusReasons", "failureRecommendations"] {
            assert_eq!(
                got.get(member),
                json!([]),
                "case {case}: {member} admits zero entries, so it is reported as the \
                 empty list, got {:?}",
                got.get(member)
            );
        }
    }
}

/// RULE 3: a policy block that models a `tier` always reports one — the configured tier,
/// or `CLASSIC` when the request configured none.
///
/// The rows cover every value the three tier enums publish
/// (`GuardrailContentFiltersTierName`, `GuardrailTopicsTierName` and
/// `GuardrailContextualGroundingTierName` are each `[CLASSIC, STANDARD]`), plus the
/// unconfigured row the default is about.
#[tokio::test]
async fn a_policy_block_always_reports_a_tier() {
    let emulator = emulator();

    // (case, the tier the request configures, the tier the read must report)
    let cases = [
        ("no tierConfig", None, "CLASSIC"),
        ("tierConfig CLASSIC", Some("CLASSIC"), "CLASSIC"),
        ("tierConfig STANDARD", Some("STANDARD"), "STANDARD"),
    ];

    for (index, (case, configured, expected)) in cases.into_iter().enumerate() {
        let got = create_then_get(
            &emulator,
            case,
            with_tiers(&format!("tiers-{index}"), configured),
        )
        .await;
        for block in [
            "contentPolicy",
            "topicPolicy",
            "contextualGroundingPolicy",
        ] {
            let tier = got
                .get(block)
                .get("tier")
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                tier,
                json!({ "tierName": expected }),
                "case {case}: {block}.tier: expected {expected}, got {tier}"
            );
        }
    }
}
