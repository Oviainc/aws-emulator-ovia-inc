// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for `CreateHubContentReference`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, on the
//! crate-shared harness from [`crate::inference_deployment_lifecycle`].
//!
//! The hub the references point AT is seeded straight into the shared engine's store with
//! the seeder [`crate::hub_contents_listing`] already owns, because the operation that
//! writes a public hub's model contents (`ImportHubContent`) is still a scaffold stub with
//! its own open issue. The private hub is created over the wire by `CreateHub`, the
//! reference by `CreateHubContentReference`, and every reference is read back over the
//! wire through `ListHubContents` and `ListTags`.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: which
//! stored version a reference resolves to for every shape of `MinVersion` and every
//! candidate count, the name an unnamed reference takes, the refusals for a source and a
//! hub that resolve to nothing (and their order), the collision refusal, and the tags the
//! record carries.

use serde_json::{json, Value};

use crate::hub_contents_listing::{seed, DEMO_HUB_CONTENTS};
use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// The region the harness signs for, and so the region every minted ARN names.
const REGION: &str = "us-east-1";

/// The hub whose contents stand in for the AWS-managed public catalogue. SageMaker names
/// that hub in the model itself — `CreateHubContentPresignedUrls$HubName`'s doc says "For
/// public content, use SageMakerPublicHub".
const PUBLIC_HUB: &str = "SageMakerPublicHub";

/// The private hub every case adds its references to.
const PRIVATE_HUB: &str = "my-private-hub";

/// The `HubContentType` a reference is stored as.
const REFERENCE_TYPE: &str = "ModelReference";

/// One row of the table in [`search_reports_the_stored_min_version_and_tags`]: the case
/// name, the `MinVersion` the request carried, and the tags it carried as (key, value)
/// pairs.
type MinVersionAndTagsCase<'a> = (&'a str, Option<&'a str>, &'a [(&'a str, &'a str)]);

/// A `SageMakerPublicHubContentArn` naming `model` in the public hub, in the one form the
/// modelled pattern admits (`…:sagemaker:<region>:aws:hub-content/<hub>/Model/<name>`).
fn public_arn(hub: &str, model: &str) -> String {
    format!("arn:aws:sagemaker:{REGION}:aws:hub-content/{hub}/Model/{model}")
}

/// The ARN a reference of `name` at `version` in `hub` is minted with.
fn reference_arn(hub: &str, name: &str, version: &str) -> String {
    format!(
        "arn:aws:sagemaker:{REGION}:{ACCOUNT}:hub-content/{hub}/{REFERENCE_TYPE}/{name}/{version}"
    )
}

fn hub_arn(hub: &str) -> String {
    format!("arn:aws:sagemaker:{REGION}:{ACCOUNT}:hub/{hub}")
}

/// An emulator holding the seeded public hub and a private hub created over the wire.
async fn emulator_with_hubs() -> Emulator {
    let emulator = Emulator::new();
    seed(&emulator, PUBLIC_HUB, DEMO_HUB_CONTENTS);
    emulator
        .ok(
            "CreateHub",
            &json!({ "HubName": PRIVATE_HUB, "HubDescription": "a private hub" }),
        )
        .await;
    emulator
}

/// A `CreateHubContentReference` body: the two required members, plus whatever `extra`
/// carries.
fn reference_request(hub: &str, source_arn: &str, extra: Value) -> Value {
    let mut body = json!({
        "HubName": hub,
        "SageMakerPublicHubContentArn": source_arn,
    });
    for (member, value) in extra.as_object().expect("an object of extra members") {
        body[member.clone()] = value.clone();
    }
    body
}

/// The `ModelReference` rows `ListHubContents` reports for `hub`.
async fn listed_references(emulator: &Emulator, hub: &str) -> Vec<Value> {
    let body = emulator
        .ok(
            "ListHubContents",
            &json!({ "HubName": hub, "HubContentType": REFERENCE_TYPE }),
        )
        .await;
    body.get("HubContentSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The one listed reference named `name`, or a panic naming the case.
async fn listed_reference(emulator: &Emulator, hub: &str, name: &str, case: &str) -> Value {
    let rows = listed_references(emulator, hub).await;
    rows.iter()
        .find(|row| str_field(row, "HubContentName") == name)
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: no listed reference named {name}, got {rows:?}"))
}

/// The tags `ListTags` reports for `arn`, as (key, value) pairs.
async fn listed_tags(emulator: &Emulator, arn: &str) -> Vec<(String, String)> {
    let body = emulator.ok("ListTags", &json!({ "ResourceArn": arn })).await;
    body.get("Tags")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a Tags list in {body}"))
        .iter()
        .map(|tag| (str_field(tag, "Key"), str_field(tag, "Value")))
        .collect()
}

/// RULE: a reference resolves to the GREATEST stored version of the public content its
/// `SageMakerPublicHubContentArn` names that is at or above `MinVersion`, records it in
/// the named hub as a `ModelReference` carrying that version's metadata, and answers the
/// hub's ARN beside the reference's own.
#[tokio::test]
async fn a_reference_resolves_the_latest_source_version_at_or_above_min_version() {
    // Rows differ along what the rule generalises over: how many versions of the source
    // are stored (three, two, one — see `DEMO_HUB_CONTENTS`), and where `MinVersion` cuts
    // (absent, below every version, equal to the latest, and a floor whose dotted
    // components order differently from its text: `1.9.0` is BELOW `1.10.0`).
    let cases: [(&str, Option<&str>, &str); 7] = [
        ("alpha-model", None, "1.10.0"),
        ("alpha-model", Some("1.0.0"), "1.10.0"),
        ("alpha-model", Some("1.9.0"), "1.10.0"),
        ("alpha-model", Some("1.10.0"), "1.10.0"),
        ("beta-model", None, "2.0.10"),
        ("beta-model", Some("2.0.10"), "2.0.10"),
        ("gamma-model", None, "1.0.0"),
    ];

    for (index, (model, min_version, expected_version)) in cases.iter().enumerate() {
        let emulator = emulator_with_hubs().await;
        let name = format!("ref-{index}");
        let case = format!("{model}/min={min_version:?}");
        let source = public_arn(PUBLIC_HUB, model);
        let mut extra = json!({ "HubContentName": name.clone() });
        if let Some(min) = min_version {
            extra["MinVersion"] = json!(min);
        }

        let created = emulator
            .ok(
                "CreateHubContentReference",
                &reference_request(PRIVATE_HUB, &source, extra),
            )
            .await;
        assert_eq!(
            str_field(&created, "HubArn"),
            hub_arn(PRIVATE_HUB),
            "case {case}: HubArn must name the hub the reference was added to, got {created:?}"
        );
        assert_eq!(
            str_field(&created, "HubContentArn"),
            reference_arn(PRIVATE_HUB, &name, expected_version),
            "case {case}: HubContentArn must carry the resolved version, got {created:?}"
        );

        // Read back over the wire: the record the create wrote is what the listing reports.
        let row = listed_reference(&emulator, PRIVATE_HUB, &name, &case).await;
        assert_eq!(
            str_field(&row, "HubContentVersion"),
            *expected_version,
            "case {case}: listed HubContentVersion, got {row:?}"
        );
        assert_eq!(
            str_field(&row, "HubContentArn"),
            reference_arn(PRIVATE_HUB, &name, expected_version),
            "case {case}: listed HubContentArn, got {row:?}"
        );
        assert_eq!(
            str_field(&row, "SageMakerPublicHubContentArn"),
            source,
            "case {case}: the reference must report the content it points at, got {row:?}"
        );
        assert_eq!(
            str_field(&row, "HubContentStatus"),
            "Available",
            "case {case}: a reference imports nothing, so it is available at once, \
             got {row:?}"
        );
        // The metadata is the RESOLVED content's, which the seeder derives from its name.
        assert_eq!(
            str_field(&row, "HubContentDisplayName"),
            format!("{model} display"),
            "case {case}: the display name must be the source's, got {row:?}"
        );
        assert_eq!(
            str_field(&row, "HubContentDescription"),
            format!("{model} description"),
            "case {case}: the description must be the source's, got {row:?}"
        );
    }
}

/// RULE: a reference the request does not name carries the name of the content it
/// references.
#[tokio::test]
async fn an_unnamed_reference_takes_the_name_of_the_content_it_references() {
    // Rows: every seeded public model, so the rule holds for a source with one version
    // and for sources with several.
    for model in ["alpha-model", "beta-model", "gamma-model"] {
        // One emulator per row, so the same reference name is free in every one.
        let emulator = emulator_with_hubs().await;
        let created = emulator
            .ok(
                "CreateHubContentReference",
                &reference_request(PRIVATE_HUB, &public_arn(PUBLIC_HUB, model), json!({})),
            )
            .await;

        let row = listed_reference(&emulator, PRIVATE_HUB, model, model).await;
        assert_eq!(
            str_field(&created, "HubContentArn"),
            str_field(&row, "HubContentArn"),
            "case {model}: the created ARN must be the listed one, got {created:?} {row:?}"
        );
        assert_eq!(
            str_field(&row, "HubContentName"),
            model,
            "case {model}: an unnamed reference takes the source's name, got {row:?}"
        );
    }
}

/// RULE: a request whose source resolves to no stored hub content version is refused with
/// HTTP 400 `ValidationException` and the service's fixed source-ARN sentence, before the
/// hub is resolved at all; a request whose source resolves but whose hub does not is
/// refused with the hub's own sentence. Either way nothing is recorded.
#[tokio::test]
async fn unresolvable_hub_or_source_is_refused_and_records_nothing() {
    let emulator = emulator_with_hubs().await;

    // Rows differ along WHICH identifier resolves to nothing: the hub (by name and by
    // ARN, both forms of the modelled `HubNameOrArn`), the source's hub, the source's
    // name, a source stored under another `HubContentType` (the seeded notebook is a
    // `Notebook`, and only a `Model` can be referenced), a `MinVersion` above every
    // stored version, and both at once — which is refused for the SOURCE, because that is
    // what the service resolves first.
    let source_missing = "Requested source ARN does not exist.".to_string();
    let cases: Vec<(&str, Value, String)> = vec![
        (
            "hub-never-created",
            reference_request(
                "no-such-hub",
                &public_arn(PUBLIC_HUB, "alpha-model"),
                json!({}),
            ),
            "Hub with name no-such-hub does not exist.".to_string(),
        ),
        (
            "hub-never-created-named-by-arn",
            reference_request(
                &hub_arn("no-such-hub"),
                &public_arn(PUBLIC_HUB, "alpha-model"),
                json!({}),
            ),
            "Hub with name no-such-hub does not exist.".to_string(),
        ),
        (
            "neither-hub-nor-source",
            reference_request(
                "no-such-hub",
                &public_arn(PUBLIC_HUB, "absent-model"),
                json!({}),
            ),
            source_missing.clone(),
        ),
        (
            "source-model-unknown",
            reference_request(
                PRIVATE_HUB,
                &public_arn(PUBLIC_HUB, "absent-model"),
                json!({}),
            ),
            source_missing.clone(),
        ),
        (
            "source-hub-unknown",
            reference_request(
                PRIVATE_HUB,
                &public_arn("OtherPublicHub", "alpha-model"),
                json!({}),
            ),
            source_missing.clone(),
        ),
        (
            "source-is-not-a-model",
            reference_request(
                PRIVATE_HUB,
                &public_arn(PUBLIC_HUB, "demo-notebook"),
                json!({}),
            ),
            source_missing.clone(),
        ),
        (
            "min-version-above-every-stored-version",
            reference_request(
                PRIVATE_HUB,
                &public_arn(PUBLIC_HUB, "alpha-model"),
                json!({ "MinVersion": "9.0.0" }),
            ),
            source_missing.clone(),
        ),
    ];

    for (case, body, expected) in cases {
        let (status, refused) = emulator.call("CreateHubContentReference", &body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case}: error code, got {refused:?}"
        );
        assert_eq!(
            message(&refused),
            expected,
            "case {case}: refusal message, got {refused:?}"
        );
        assert!(
            listed_references(&emulator, PRIVATE_HUB).await.is_empty(),
            "case {case}: a refused call must record nothing, got {:?}",
            listed_references(&emulator, PRIVATE_HUB).await
        );
    }
}

/// RULE: every reader of a stored reference reports what the create stored — `Search`
/// reports the reference's `ReferenceMinVersion` and `Tags` beside the members the listing
/// reports.
#[tokio::test]
async fn search_reports_the_stored_min_version_and_tags() {
    // Rows differ along which of the two optional members the request carried: both, the
    // floor alone, and neither.
    let cases: [MinVersionAndTagsCase; 3] = [
        ("min-and-tags", Some("1.0.0"), &[("team", "ml")]),
        ("min-only", Some("1.9.0"), &[]),
        ("neither", None, &[]),
    ];

    for (case, min_version, pairs) in cases {
        let emulator = emulator_with_hubs().await;
        let mut extra = json!({ "HubContentName": "searched-ref" });
        if let Some(min) = min_version {
            extra["MinVersion"] = json!(min);
        }
        if !pairs.is_empty() {
            extra["Tags"] = Value::Array(
                pairs
                    .iter()
                    .map(|(key, value)| json!({ "Key": key, "Value": value }))
                    .collect(),
            );
        }
        let created = emulator
            .ok(
                "CreateHubContentReference",
                &reference_request(PRIVATE_HUB, &public_arn(PUBLIC_HUB, "alpha-model"), extra),
            )
            .await;
        let arn = str_field(&created, "HubContentArn");

        let answered = emulator
            .ok("Search", &json!({ "Resource": "HubContent" }))
            .await;
        let record = answered
            .get("Results")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case}: expected a Results list, got {answered}"))
            .iter()
            .filter_map(|result| result.get("HubContent"))
            .find(|content| str_field(content, "HubContentArn") == arn)
            .cloned()
            .unwrap_or_else(|| panic!("case {case}: Search must report {arn}, got {answered}"));

        assert_eq!(
            record
                .get("ReferenceMinVersion")
                .and_then(Value::as_str)
                .map(str::to_string),
            min_version.map(str::to_string),
            "case {case}: ReferenceMinVersion, got {record:?}"
        );
        let reported: Vec<(String, String)> = record
            .get("Tags")
            .and_then(Value::as_array)
            .map(|tags| {
                tags.iter()
                    .map(|tag| (str_field(tag, "Key"), str_field(tag, "Value")))
                    .collect()
            })
            .unwrap_or_default();
        let expected: Vec<(String, String)> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            reported, expected,
            "case {case}: Search must report the stored tags, got {record:?}"
        );
    }
}

/// RULE: the reference's tags are the request's `Tags`, reachable through `ListTags` over
/// the ARN the create answered; and a reference the hub already holds at that version is
/// refused with `ResourceInUse` rather than overwritten.
#[tokio::test]
async fn tags_are_stored_and_a_second_identical_call_is_refused() {
    // Rows differ along the size and shape of the tag list: absent, one tag, several.
    let cases: [(&str, &[(&str, &str)]); 3] = [
        ("no-tags", &[]),
        ("one-tag", &[("team", "ml")]),
        ("many-tags", &[("team", "ml"), ("env", "test"), ("owner", "a")]),
    ];

    for (case, pairs) in cases {
        let emulator = emulator_with_hubs().await;
        let mut extra = json!({ "HubContentName": "tagged-ref" });
        if !pairs.is_empty() {
            extra["Tags"] = Value::Array(
                pairs
                    .iter()
                    .map(|(key, value)| json!({ "Key": key, "Value": value }))
                    .collect(),
            );
        }
        let body = reference_request(
            PRIVATE_HUB,
            &public_arn(PUBLIC_HUB, "alpha-model"),
            extra,
        );

        let created = emulator.ok("CreateHubContentReference", &body).await;
        let arn = str_field(&created, "HubContentArn");
        let mut reported = listed_tags(&emulator, &arn).await;
        reported.sort();
        let mut expected: Vec<(String, String)> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {case}: ListTags must report the tags the reference was created with"
        );

        // The same call again collides with the record it wrote.
        let (status, refused) = emulator.call("CreateHubContentReference", &body).await;
        assert_eq!(
            status, 400,
            "case {case}: a second identical call must be refused, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceInUse",
            "case {case}: error code, got {refused:?}"
        );
        assert_eq!(
            message(&refused),
            format!(
                "A hub content with name tagged-ref and version 1.10.0 already exists in hub \
                 {PRIVATE_HUB}. Please use a different name and try again."
            ),
            "case {case}: refusal message, got {refused:?}"
        );
        // And the hub still holds exactly the one reference.
        let rows = listed_references(&emulator, PRIVATE_HUB).await;
        assert_eq!(
            rows.len(),
            1,
            "case {case}: the refused call must add no record, got {rows:?}"
        );
    }
}
