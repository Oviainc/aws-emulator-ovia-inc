// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's JumpStart hub registry:
//! `CreateHub` -> `DescribeHub` -> `DeleteHub`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets. The harness and the small
//! body accessors are the crate-shared ones from
//! [`crate::inference_deployment_lifecycle`], not a fourth copy.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the
//! `HubArn` form for every name shape and region, the create/describe round trip for
//! every optional member, name-or-ARN resolution on both readers, the `Creating` ->
//! `InService` transition the clock owes, and the modelled refusals (`ResourceInUse` for
//! a name in use, `ResourceNotFound` for a hub nothing created, `ResourceInUse` for a
//! hub that still holds content).

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_sagemaker_sagemaker::state::ComAmazonawsSagemakerSagemakerState;

// The hub-content seeder lives beside the `ListHubContents` suite that already needs it;
// reused here rather than copied so there is one way to put content in the store.
use crate::hub_contents_listing::{seed, DEMO_HUB_CONTENTS};
use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// Long enough to pass the span the hub's `InService` transition is owed after, derived
/// from the constant itself so retuning the span cannot make this test flaky.
fn past_in_service_due() -> Duration {
    service_com_amazonaws_sagemaker_sagemaker::state::HUB_IN_SERVICE_AFTER * 2
}

/// A `CreateHub` body carrying only the two required members.
fn minimal_hub(name: &str) -> Value {
    json!({ "HubName": name, "HubDescription": "a hub" })
}

/// RULE: `CreateHub` mints `arn:aws:sagemaker:<signed region>:<account>:hub/<HubName>`,
/// whatever the name's shape and whatever region the caller signed for (the emulator
/// default when the signed region is not a region token).
#[tokio::test]
async fn created_hub_arns_name_the_signed_region_and_the_stored_name() {
    // Rows differ along the axes the rule generalises over: the name's shape (one
    // character, digits, hyphens, and the 63-character modelled maximum) and the region.
    let longest = format!("h{}", "a".repeat(62));
    let names = ["h", "Hub1", "hub-with-hyphens", longest.as_str()];
    for region in ["us-east-1", "eu-west-2", "ap-southeast-1"] {
        // One emulator per region, so the same name can be reused across rows.
        let emulator = Emulator::new();
        for name in names {
            let case = format!("{region}/{name}");
            let (status, created) = emulator
                .call_in(region, "CreateHub", &minimal_hub(name))
                .await;
            assert_eq!(status, 200, "case {case}: CreateHub, got {status} {created}");
            let expected = format!("arn:aws:sagemaker:{region}:{ACCOUNT}:hub/{name}");
            assert_eq!(
                str_field(&created, "HubArn"),
                expected,
                "case {case}: HubArn, got {created:?}"
            );

            // And the reader replays the same ARN, so the mint is what was stored.
            let (read_status, described) = emulator
                .call_in(region, "DescribeHub", &json!({ "HubName": name }))
                .await;
            assert_eq!(
                read_status, 200,
                "case {case}: DescribeHub, got {read_status} {described}"
            );
            assert_eq!(
                str_field(&described, "HubArn"),
                expected,
                "case {case}: DescribeHub must replay the minted ARN, got {described:?}"
            );
        }
    }

    // A credential scope whose region is not a region token cannot enter an ARN (the
    // region component is positional), so the emulator default stands in.
    for malformed in ["us east 1", "US-EAST-1", "us:east/1"] {
        let emulator = Emulator::new();
        let (status, created) = emulator
            .call_in(malformed, "CreateHub", &minimal_hub("odd-region-hub"))
            .await;
        assert_eq!(
            status, 200,
            "case malformed-region={malformed}: CreateHub, got {status} {created}"
        );
        assert_eq!(
            str_field(&created, "HubArn"),
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:hub/odd-region-hub"),
            "case malformed-region={malformed}: HubArn must fall back to the default \
             region, got {created:?}"
        );
    }
}

/// RULE: `DescribeHub` replays every member `CreateHub` stored — present members and
/// absent ones alike — and reports nothing the request did not carry.
#[tokio::test]
async fn describe_hub_replays_exactly_what_create_hub_stored() {
    let emulator = Emulator::new();

    // Rows differ along what the rule generalises over: which optional members the
    // request carries, the size of the description (the modelled 1023-character maximum
    // included), and the length and form of the keyword list (absent, empty, one, many).
    let longest_description = "d".repeat(1023);
    let cases: Vec<(&str, Value)> = vec![
        ("required-only", json!({ "HubDescription": "a hub" })),
        (
            "longest-description",
            json!({ "HubDescription": longest_description }),
        ),
        (
            "display-name",
            json!({ "HubDescription": "a hub", "HubDisplayName": "My Hub" }),
        ),
        (
            "keywords-empty",
            json!({ "HubDescription": "a hub", "HubSearchKeywords": [] }),
        ),
        (
            "keywords-one",
            json!({ "HubDescription": "a hub", "HubSearchKeywords": ["nlp"] }),
        ),
        (
            "keywords-many",
            json!({
                "HubDescription": "a hub",
                // `HubSearchKeyword` is `pattern ^[^A-Z]*$`, so keywords carry no
                // upper-case letters.
                "HubSearchKeywords": ["foundation-model", "nlp", "v2", "text generation"],
            }),
        ),
        (
            "s3-storage-s3-scheme",
            json!({
                "HubDescription": "a hub",
                // `S3OutputPath` is `pattern ^(https|s3)://([^/]+)/?(.*)$`: both schemes
                // are accepted, so both are exercised.
                "S3StorageConfig": { "S3OutputPath": "s3://hub-bucket/prefix" },
            }),
        ),
        (
            "s3-storage-https-scheme",
            json!({
                "HubDescription": "a hub",
                "S3StorageConfig": { "S3OutputPath": "https://hub-bucket/prefix" },
            }),
        ),
        (
            "every-member",
            json!({
                "HubDescription": "every member set",
                "HubDisplayName": "Everything",
                "HubSearchKeywords": ["everything"],
                "S3StorageConfig": { "S3OutputPath": "s3://hub-bucket/everything" },
                "Tags": [{ "Key": "team", "Value": "ml" }],
            }),
        ),
    ];

    for (index, (case, extras)) in cases.iter().enumerate() {
        let name = format!("hub{index}");
        let mut body = extras.clone();
        body["HubName"] = Value::String(name.clone());
        let created = emulator.ok("CreateHub", &body).await;

        let described = emulator
            .ok("DescribeHub", &json!({ "HubName": name }))
            .await;
        assert_eq!(
            str_field(&described, "HubName"),
            name,
            "case {case}: DescribeHub must replay the stored name, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "HubArn"),
            str_field(&created, "HubArn"),
            "case {case}: DescribeHub must replay the ARN CreateHub minted"
        );

        // Every member the request carried comes back unchanged, and a member it did not
        // carry is OMITTED rather than reported empty. `Tags` is stored but not part of
        // `DescribeHub`'s modelled output, so it is excluded from the round trip.
        for member in [
            "HubDescription",
            "HubDisplayName",
            "HubSearchKeywords",
            "S3StorageConfig",
        ] {
            match body.get(member) {
                Some(sent) => assert_eq!(
                    described.get(member),
                    Some(sent),
                    "case {case}: {member} must round-trip, got {described:?}"
                ),
                None => assert!(
                    described.get(member).is_none(),
                    "case {case}: {member} was not sent, so it must be omitted, \
                     got {described:?}"
                ),
            }
        }
        assert!(
            described.get("Tags").is_none(),
            "case {case}: Tags is not a modelled DescribeHub member, got {described:?}"
        );
        // Nothing has imported content into this hub, so there is no failure to report.
        assert!(
            described.get("FailureReason").is_none(),
            "case {case}: FailureReason must be omitted, got {described:?}"
        );
        assert!(
            described.get("CreationTime").is_some_and(Value::is_number),
            "case {case}: CreationTime must be a stored timestamp, got {described:?}"
        );

        // A read changes nothing a later reader sees: the second describe reports the
        // same hub. Only the members no clock moves are compared — `HubStatus` and
        // `LastModifiedTime` legitimately differ if the two reads straddle the instant
        // the `InService` transition falls due, which the lifecycle test covers.
        let again = emulator
            .ok("DescribeHub", &json!({ "HubName": name }))
            .await;
        for member in [
            "HubName",
            "HubArn",
            "HubDescription",
            "HubDisplayName",
            "HubSearchKeywords",
            "S3StorageConfig",
            "CreationTime",
        ] {
            assert_eq!(
                again.get(member),
                described.get(member),
                "case {case}: a second DescribeHub must report the same {member}, \
                 got {again:?}"
            );
        }
    }
}

/// RULE: an operation whose `HubName` is modelled `HubNameOrArn` accepts the hub's name
/// or its ARN, and both forms reach the same record.
#[tokio::test]
async fn a_hub_is_reachable_by_name_or_by_its_arn() {
    // Rows: each operation that takes `HubNameOrArn`, each reached by both forms.
    for (case, reader) in [("describe", "DescribeHub"), ("delete", "DeleteHub")] {
        for by_arn in [false, true] {
            let emulator = Emulator::new();
            let created = emulator.ok("CreateHub", &minimal_hub("ref-hub")).await;
            let arn = str_field(&created, "HubArn");
            let reference = if by_arn {
                arn.clone()
            } else {
                "ref-hub".to_string()
            };

            let (status, answered) = emulator.call(reader, &json!({ "HubName": reference })).await;
            assert_eq!(
                status, 200,
                "case {case}/by-arn={by_arn}: {reader} must accept {reference}, \
                 got {status} {answered}"
            );
            if reader == "DescribeHub" {
                assert_eq!(
                    str_field(&answered, "HubArn"),
                    arn,
                    "case {case}/by-arn={by_arn}: both forms must reach the same hub, \
                     got {answered:?}"
                );
            } else {
                // The delete really released the hub the reference named.
                let (gone, body) = emulator
                    .call("DescribeHub", &json!({ "HubName": "ref-hub" }))
                    .await;
                assert_eq!(
                    gone, 400,
                    "case {case}/by-arn={by_arn}: the hub must be gone, got {gone} {body}"
                );
            }
        }
    }
}

/// RULE: a hub is created in the non-terminal `Creating` status and the `InService`
/// transition the clock owes is materialised once it is due, with `LastModifiedTime`
/// moving to that instant and `CreationTime` unchanged.
#[tokio::test]
async fn a_new_hub_is_creating_until_the_transition_is_due() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateHub", &minimal_hub("lifecycle-hub"))
        .await;

    let fresh = emulator
        .ok("DescribeHub", &json!({ "HubName": "lifecycle-hub" }))
        .await;
    assert_eq!(
        str_field(&fresh, "HubStatus"),
        "Creating",
        "case straight-after-create: a caller must still observe the transient status, \
         got {fresh:?}"
    );

    tokio::time::sleep(past_in_service_due()).await;
    let settled = emulator
        .ok("DescribeHub", &json!({ "HubName": "lifecycle-hub" }))
        .await;
    assert_eq!(
        str_field(&settled, "HubStatus"),
        "InService",
        "case after-the-due-span: the transition must be materialised, got {settled:?}"
    );
    assert_eq!(
        settled.get("CreationTime"),
        fresh.get("CreationTime"),
        "case after-the-due-span: CreationTime must not move, got {settled:?}"
    );
    let created_at = fresh
        .get("CreationTime")
        .and_then(Value::as_f64)
        .expect("CreationTime");
    let modified_at = settled
        .get("LastModifiedTime")
        .and_then(Value::as_f64)
        .expect("LastModifiedTime");
    assert!(
        modified_at > created_at,
        "case after-the-due-span: LastModifiedTime {modified_at} must move past \
         CreationTime {created_at} when the status changes"
    );

    // The settled status is stable: a later read reports the same instant, not "now".
    let later = emulator
        .ok("DescribeHub", &json!({ "HubName": "lifecycle-hub" }))
        .await;
    assert_eq!(
        later, settled,
        "case stable-after-settling: the hub must not keep changing on reads"
    );
}

/// RULE: `CreateHub` refuses a `HubName` already in use with its modelled `ResourceInUse`
/// (HTTP 400) and leaves the stored hub untouched.
#[tokio::test]
async fn a_hub_name_already_in_use_is_refused_with_resource_in_use() {
    let emulator = Emulator::new();

    // Rows: the second create differs from the first in every member it may differ in,
    // so the refusal follows from the name alone.
    let cases: Vec<(&str, Value)> = vec![
        ("identical", minimal_hub("taken")),
        (
            "different-description",
            json!({ "HubName": "taken", "HubDescription": "another description" }),
        ),
        (
            "more-members",
            json!({
                "HubName": "taken",
                "HubDescription": "a hub",
                "HubDisplayName": "Taken",
                "HubSearchKeywords": ["dup"],
            }),
        ),
    ];

    let created = emulator.ok("CreateHub", &minimal_hub("taken")).await;
    let arn = str_field(&created, "HubArn");
    let before = emulator
        .ok("DescribeHub", &json!({ "HubName": "taken" }))
        .await;

    for (case, body) in &cases {
        let (status, parsed) = emulator.call("CreateHub", body).await;
        assert_eq!(
            status, 400,
            "case {case}: a name already in use must be refused, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceInUse",
            "case {case}: the refusal must use the operation's modelled error, \
             got {parsed:?}"
        );
        assert!(
            message(&parsed).contains(&arn),
            "case {case}: the refusal must name the existing hub, got {parsed:?}"
        );

        let after = emulator
            .ok("DescribeHub", &json!({ "HubName": "taken" }))
            .await;
        assert_eq!(
            after, before,
            "case {case}: the refused create must not have overwritten the stored hub"
        );
    }
}

/// RULE: an operation naming a hub that is not in state is refused with the modelled
/// `ResourceNotFound` (HTTP 400) — never a 200 with empty members, and never a 501.
#[tokio::test]
async fn an_absent_hub_is_refused_with_resource_not_found() {
    let emulator = Emulator::new();
    // A hub that does exist, so the refusals below are about the name, not an empty store.
    emulator.ok("CreateHub", &minimal_hub("present")).await;

    // Rows: every operation that resolves a hub name, each with a name nothing created,
    // in both accepted reference forms.
    for target in ["DescribeHub", "DeleteHub"] {
        for reference in [
            "absent".to_string(),
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:hub/absent"),
        ] {
            let case = format!("{target}/{reference}");
            let (status, parsed) = emulator.call(target, &json!({ "HubName": reference })).await;
            assert_eq!(
                status, 400,
                "case {case}: expected HTTP 400 for an absent hub, got {status} {parsed}"
            );
            assert_eq!(
                error_code(&parsed),
                "ResourceNotFound",
                "case {case}: error code, got {parsed:?}"
            );
            assert!(
                message(&parsed).contains("absent"),
                "case {case}: the refusal must name the hub it could not find, \
                 got {parsed:?}"
            );
        }
    }

    // And the hub that does exist is still readable: nothing above touched the store.
    let present = emulator
        .ok("DescribeHub", &json!({ "HubName": "present" }))
        .await;
    assert_eq!(
        str_field(&present, "HubName"),
        "present",
        "case store-intact: the existing hub must survive the refusals, got {present:?}"
    );
}

/// RULE: the hub store holds at most its quota of records; the create past it is refused
/// with the modelled `ResourceLimitExceeded`, stores nothing, and a freed slot makes the
/// same create succeed — while a name already in use is refused as `ResourceInUse` even
/// at the limit, because a call that adds no record cannot be over it.
#[tokio::test]
async fn a_create_past_the_hub_quota_is_refused_with_resource_limit_exceeded() {
    const QUOTA: usize = service_com_amazonaws_sagemaker_sagemaker::state::HUB_QUOTA;

    let emulator = Emulator::new();
    // The records that merely FILL the store below the boundary are seeded straight into
    // the shared store the handler reads (the same seeder the `ListHubContents` suite
    // uses), so only the calls the rule is about go over the wire: a thousand wire
    // creates would say nothing more and would dominate the suite's runtime.
    for index in 0..QUOTA - 1 {
        seed(&emulator, &format!("q{index:04}"), &[]);
    }
    // The slot that reaches the bound is taken by a real create, so the boundary itself
    // is exercised end to end rather than seeded.
    let last = format!("q{:04}", QUOTA - 1);
    emulator.ok("CreateHub", &minimal_hub(&last)).await;

    let overflow = format!("q{QUOTA:04}");
    let (status, parsed) = emulator.call("CreateHub", &minimal_hub(&overflow)).await;
    assert_eq!(
        status, 400,
        "case overflow: a create past the quota must be refused, got {status} {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ResourceLimitExceeded",
        "case overflow: the refusal must use the operation's modelled limit error, \
         got {parsed:?}"
    );

    // Nothing was stored for the refused request.
    let (read_status, read_body) = emulator
        .call("DescribeHub", &json!({ "HubName": overflow }))
        .await;
    assert_eq!(
        read_status, 400,
        "case overflow: the refused hub must not be stored, got {read_status} {read_body}"
    );

    // A name already in use is still the collision refusal, not the limit one: the store
    // would gain no record.
    let (taken_status, taken) = emulator.call("CreateHub", &minimal_hub("q0001")).await;
    assert_eq!(
        taken_status, 400,
        "case taken-at-the-limit: expected HTTP 400, got {taken_status} {taken}"
    );
    assert_eq!(
        error_code(&taken),
        "ResourceInUse",
        "case taken-at-the-limit: a name in use must be refused as in use, got {taken:?}"
    );

    // And a delete frees a slot, so the same create then succeeds: the bound is a live
    // count, not a one-way counter.
    emulator
        .ok("DeleteHub", &json!({ "HubName": last }))
        .await;
    let created = emulator.ok("CreateHub", &minimal_hub(&overflow)).await;
    assert_eq!(
        str_field(&created, "HubArn"),
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:hub/{overflow}"),
        "case freed-slot: the create must succeed once a slot is free, got {created:?}"
    );
}

/// RULE: `DeleteHub` releases exactly the hub it names, leaves every sibling in place,
/// and a second delete of the same hub is refused.
#[tokio::test]
async fn delete_hub_releases_only_the_hub_it_names() {
    let emulator = Emulator::new();

    // Rows: a store holding several hubs, each deleted in turn, so the rule is asserted
    // over every entry rather than one.
    let names = ["alpha", "beta", "gamma-1"];
    for name in names {
        emulator.ok("CreateHub", &minimal_hub(name)).await;
    }

    let mut remaining: Vec<&str> = names.to_vec();
    for name in names {
        let deleted = emulator.ok("DeleteHub", &json!({ "HubName": name })).await;
        assert!(
            deleted.is_null() || deleted.as_object().is_some_and(|body| body.is_empty()),
            "case {name}: DeleteHub answers an empty body, got {deleted:?}"
        );
        remaining.retain(|entry| *entry != name);

        let (status, parsed) = emulator
            .call("DescribeHub", &json!({ "HubName": name }))
            .await;
        assert_eq!(
            status, 400,
            "case {name}: the deleted hub must be gone, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceNotFound",
            "case {name}: error code after delete, got {parsed:?}"
        );

        // A second delete of the same name is refused: the entry is really gone.
        let (repeat_status, repeat) = emulator.call("DeleteHub", &json!({ "HubName": name })).await;
        assert_eq!(
            repeat_status, 400,
            "case {name}: a second DeleteHub must be refused, got {repeat_status} {repeat}"
        );
        assert_eq!(
            error_code(&repeat),
            "ResourceNotFound",
            "case {name}: error code on the second delete, got {repeat:?}"
        );

        for survivor in &remaining {
            let described = emulator
                .ok("DescribeHub", &json!({ "HubName": survivor }))
                .await;
            assert_eq!(
                str_field(&described, "HubName"),
                *survivor,
                "case {name}: the sibling hub {survivor} must survive, got {described:?}"
            );
        }
    }

    // The released name is free again, and the new hub is a new record.
    let recreated = emulator.ok("CreateHub", &minimal_hub("alpha")).await;
    assert_eq!(
        str_field(&recreated, "HubArn"),
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:hub/alpha"),
        "case recreate-after-delete: HubArn, got {recreated:?}"
    );
}

/// RULE: a hub that still holds hub content is in use, so `DeleteHub` refuses it with the
/// modelled `ResourceInUse` (HTTP 400) and leaves the hub readable; once the content is
/// released the same delete succeeds. The refusal follows from content being PRESENT, not
/// from how much of it there is or of which type.
#[tokio::test]
async fn a_hub_that_still_holds_content_is_refused_with_resource_in_use() {
    // Rows differ along what the rule generalises over: how many content versions the hub
    // holds and of which `HubContentType`s — one version, two versions of the same
    // content, and the full seeded set (one content of every modelled type).
    let cases: [(&str, &[_]); 3] = [
        ("one-version", &DEMO_HUB_CONTENTS[..1]),
        ("two-versions-of-one-content", &DEMO_HUB_CONTENTS[..2]),
        ("every-content-type", DEMO_HUB_CONTENTS),
    ];

    for (case, contents) in cases {
        let emulator = Emulator::new();
        seed(&emulator, "contentful", contents);

        let (status, parsed) = emulator
            .call("DeleteHub", &json!({ "HubName": "contentful" }))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: a hub still holding content must be refused, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceInUse",
            "case {case}: the refusal must use the operation's modelled in-use error, \
             got {parsed:?}"
        );

        // The refused delete released nothing: the hub is still readable.
        let described = emulator
            .ok("DescribeHub", &json!({ "HubName": "contentful" }))
            .await;
        assert_eq!(
            str_field(&described, "HubName"),
            "contentful",
            "case {case}: the refused delete must leave the hub in place, got {described:?}"
        );

        // Releasing the content releases the hub: the same delete then succeeds, so the
        // refusal is the live content count rather than a latch on the record.
        let store = emulator
            .engine
            .state::<ComAmazonawsSagemakerSagemakerState>();
        store
            .lock()
            .hub_contents
            .retain(|key, _| key.hub_name != "contentful");
        let (freed, body) = emulator
            .call("DeleteHub", &json!({ "HubName": "contentful" }))
            .await;
        assert_eq!(
            freed, 200,
            "case {case}: the delete must succeed once the content is released, \
             got {freed} {body}"
        );
    }
}
