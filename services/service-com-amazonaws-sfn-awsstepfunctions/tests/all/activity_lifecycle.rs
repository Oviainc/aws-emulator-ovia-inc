// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for Step Functions' ACTIVITY lifecycle (`CreateActivity`,
//! `DescribeActivity`, `DeleteActivity` and the tagging calls over an activity ARN),
//! driven over this service's own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`, so what is asserted is what a signed AWS SDK request
//! observes.
//!
//! The rules under test:
//!
//! 1. an activity is a PERSISTED resource keyed by its own ARN, which is a total
//!    function of the region and the name
//!    (`arn:aws:states:<region>:<account>:activity:<name>`), so `DescribeActivity`
//!    reports exactly the record `CreateActivity` stored — its ARN, its name, the
//!    instant it was created and its EFFECTIVE encryption settings — and reports the
//!    same thing on every later read;
//! 2. the create is idempotent on the NAME: a repeat answers the first response and
//!    updates nothing, and a repeat that would have to CHANGE the encryption settings
//!    is `ActivityAlreadyExists` instead;
//! 3. a read is judged in two steps: an `activityArn` that is not an activity ARN of
//!    this service in this endpoint's region is `InvalidArn` (400) before any lookup,
//!    and an ARN the store holds no activity under is `ActivityDoesNotExist` (400);
//! 4. an activity is one of the two TAGGABLE Step Functions resources, so the tags a
//!    create carried, and the ones the tagging calls write, are reported for its ARN.
//!
//! The harness (`Emulator`, `discriminated`, `error_code`, `string`) is the one
//! `state_machine_lifecycle` already builds; `tests/all.rs` is this binary's crate
//! root, so it is reached through `crate::state_machine_lifecycle`.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_sfn_awsstepfunctions::state::{
    ENCRYPTION_TYPE_AWS_OWNED_KEY, ENCRYPTION_TYPE_CUSTOMER_MANAGED_KMS_KEY,
    MAX_KMS_DATA_KEY_REUSE_PERIOD_SECONDS, MIN_KMS_DATA_KEY_REUSE_PERIOD_SECONDS,
};

use crate::state_machine_lifecycle::{
    create_machine, discriminated, string, Emulator, ACCOUNT, HELLO_DEFINITION, REGION,
};

/// A KMS key ARN of the form `kmsKeyId` documents ("an alias, alias ARN, key ID, or
/// key ARN"), for the customer-managed rows below.
const KMS_KEY_ARN: &str = "arn:aws:kms:us-west-2:000000000000:key/12345678-1234-1234-1234-123456789012";

/// The longest name the modelled `Name` shape admits (len 1..80).
const MAX_NAME_CHARS: usize = 80;

/// The ARN an activity of `name` carries in the region every request here signs for.
fn activity_arn(name: &str) -> String {
    format!("arn:aws:states:{REGION}:{ACCOUNT}:activity:{name}")
}

/// Create an activity that must succeed, and return the whole answer.
async fn create_activity(emulator: &Emulator, request: &Value) -> Value {
    emulator.ok("CreateActivity", request).await
}

/// `DescribeActivity` for `arn`, read TWICE and required to answer identically: the
/// report is a function of what is stored, so a read never changes what a later
/// reader sees.
async fn describe_activity(emulator: &Emulator, case: &str, arn: &str) -> Value {
    let request = json!({ "activityArn": arn });
    let first = emulator.ok("DescribeActivity", &request).await;
    let second = emulator.ok("DescribeActivity", &request).await;
    assert_eq!(
        first, second,
        "case {case:?}: two reads of {arn} must report the same activity, got {first:?} then \
         {second:?}"
    );
    first
}

/// The tag set `ListTagsForResource` reports for `arn`, as a key -> value map.
async fn tags_of(emulator: &Emulator, arn: &str) -> BTreeMap<String, String> {
    let listed = emulator
        .ok("ListTagsForResource", &json!({ "resourceArn": arn }))
        .await;
    listed
        .get("tags")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a 'tags' array, got {listed:?}"))
        .iter()
        .map(|entry| {
            (
                string(entry, "key"),
                entry
                    .get("value")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Create -> Describe: the record, not the request
// ---------------------------------------------------------------------------

/// A create stores the activity and a later describe reports exactly what was
/// stored, over a table whose rows differ along the NAME's form and the encryption
/// settings the create asked for.
#[tokio::test]
async fn describe_activity_reports_the_created_record() {
    let emulator = Emulator::new();

    let longest = "a".repeat(MAX_NAME_CHARS);
    let aws_owned = discriminated(
        "EncryptionConfiguration",
        json!({ "type": ENCRYPTION_TYPE_AWS_OWNED_KEY }),
    );
    // Each row: the create request, the name the record must carry, and the
    // encryption configuration the read must report — the EFFECTIVE one, which for a
    // create that configured none is the Amazon-Web-Services-owned key.
    let cases: Vec<(&str, Value, String, Value)> = vec![
        (
            "a bare name",
            json!({ "name": "worker" }),
            "worker".to_string(),
            aws_owned.clone(),
        ),
        (
            "the shortest name the model admits",
            json!({ "name": "a" }),
            "a".to_string(),
            aws_owned.clone(),
        ),
        (
            "the longest name the model admits",
            json!({ "name": longest.clone() }),
            longest.clone(),
            aws_owned.clone(),
        ),
        (
            "a name of the characters CloudWatch logging allows",
            json!({ "name": "order-worker_v2" }),
            "order-worker_v2".to_string(),
            aws_owned.clone(),
        ),
        (
            "a name of characters outside ASCII, which the rule does not forbid",
            json!({ "name": "wörker-日本" }),
            "wörker-日本".to_string(),
            aws_owned.clone(),
        ),
        (
            "the owned-key type spelled out",
            json!({
                "name": "spelled-owned",
                "encryptionConfiguration": { "type": ENCRYPTION_TYPE_AWS_OWNED_KEY },
            }),
            "spelled-owned".to_string(),
            aws_owned.clone(),
        ),
        (
            "a customer-managed key at the shortest reuse period",
            json!({
                "name": "cmk-low",
                "encryptionConfiguration": {
                    "type": ENCRYPTION_TYPE_CUSTOMER_MANAGED_KMS_KEY,
                    "kmsKeyId": KMS_KEY_ARN,
                    "kmsDataKeyReusePeriodSeconds": MIN_KMS_DATA_KEY_REUSE_PERIOD_SECONDS,
                },
            }),
            "cmk-low".to_string(),
            discriminated(
                "EncryptionConfiguration",
                json!({
                    "type": ENCRYPTION_TYPE_CUSTOMER_MANAGED_KMS_KEY,
                    "kmsKeyId": KMS_KEY_ARN,
                    "kmsDataKeyReusePeriodSeconds": MIN_KMS_DATA_KEY_REUSE_PERIOD_SECONDS,
                }),
            ),
        ),
        (
            "a customer-managed key at the longest reuse period",
            json!({
                "name": "cmk-high",
                "encryptionConfiguration": {
                    "type": ENCRYPTION_TYPE_CUSTOMER_MANAGED_KMS_KEY,
                    "kmsKeyId": "alias/activities",
                    "kmsDataKeyReusePeriodSeconds": MAX_KMS_DATA_KEY_REUSE_PERIOD_SECONDS,
                },
            }),
            "cmk-high".to_string(),
            discriminated(
                "EncryptionConfiguration",
                json!({
                    "type": ENCRYPTION_TYPE_CUSTOMER_MANAGED_KMS_KEY,
                    "kmsKeyId": "alias/activities",
                    "kmsDataKeyReusePeriodSeconds": MAX_KMS_DATA_KEY_REUSE_PERIOD_SECONDS,
                }),
            ),
        ),
        (
            "a create carrying tags, which the describe does not report",
            json!({
                "name": "tagged",
                "tags": [{ "key": "team", "value": "orders" }],
            }),
            "tagged".to_string(),
            aws_owned.clone(),
        ),
    ];

    for (case, request, name, encryption) in cases {
        let created = create_activity(&emulator, &request).await;
        let arn = string(&created, "activityArn");
        let expected_arn = activity_arn(&name);
        assert_eq!(
            arn, expected_arn,
            "case {case:?}: expected the ARN {expected_arn}, got {arn:?}"
        );
        let created_date = created
            .get("creationDate")
            .unwrap_or_else(|| panic!("case {case:?}: expected a creationDate, got {created:?}"))
            .clone();

        let reported = describe_activity(&emulator, case, &arn).await;
        assert_eq!(
            string(&reported, "activityArn"),
            expected_arn,
            "case {case:?}: expected the describe to report {expected_arn}, got {reported:?}"
        );
        assert_eq!(
            string(&reported, "name"),
            name,
            "case {case:?}: expected the name {name:?}, got {reported:?}"
        );
        assert_eq!(
            reported.get("creationDate"),
            Some(&created_date),
            "case {case:?}: expected the creationDate the create answered ({created_date:?}), got \
             {reported:?}"
        );
        assert_eq!(
            reported.get("encryptionConfiguration"),
            Some(&encryption),
            "case {case:?}: expected the effective encryption configuration {encryption:?}, got \
             {reported:?}"
        );
    }
}

/// The create is idempotent on the NAME: a repeat answers the FIRST response and
/// changes nothing, whatever else the repeat carries — and a repeat that would have
/// to change the encryption settings is refused instead.
#[tokio::test]
async fn creating_the_same_activity_again_answers_the_first_response() {
    let emulator = Emulator::new();

    let first = create_activity(
        &emulator,
        &json!({
            "name": "poller",
            "tags": [{ "key": "team", "value": "orders" }],
        }),
    )
    .await;
    let arn = string(&first, "activityArn");

    // Each row is a repeat that is the SAME request as far as the idempotency check
    // is concerned: the differences are in members the check ignores.
    let repeats: Vec<(&str, Value)> = vec![
        ("the identical request", json!({ "name": "poller" })),
        (
            "different tag values, which are ignored",
            json!({
                "name": "poller",
                "tags": [{ "key": "team", "value": "payments" }],
            }),
        ),
        (
            "the default encryption type spelled out",
            json!({
                "name": "poller",
                "encryptionConfiguration": { "type": ENCRYPTION_TYPE_AWS_OWNED_KEY },
            }),
        ),
    ];
    for (case, repeat) in repeats {
        let again = create_activity(&emulator, &repeat).await;
        assert_eq!(
            again, first,
            "case {case:?}: expected the first response {first:?}, got {again:?}"
        );
        // "tags will not be updated, even if they are different."
        let tags = tags_of(&emulator, &arn).await;
        assert_eq!(
            tags,
            BTreeMap::from([("team".to_string(), "orders".to_string())]),
            "case {case:?}: expected the tags the first create stored, got {tags:?}"
        );
    }

    // "Activity already exists. EncryptionConfiguration may not be updated."
    let (status, code) = emulator
        .err(
            "CreateActivity",
            &json!({
                "name": "poller",
                "encryptionConfiguration": {
                    "type": ENCRYPTION_TYPE_CUSTOMER_MANAGED_KMS_KEY,
                    "kmsKeyId": KMS_KEY_ARN,
                },
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ActivityAlreadyExists"),
        "expected a repeat changing the encryption settings to be refused, got {status} {code}"
    );
    // Nothing was written: the record still reports the owned key.
    let reported = describe_activity(&emulator, "after the refused repeat", &arn).await;
    assert_eq!(
        reported.get("encryptionConfiguration"),
        Some(&discriminated(
            "EncryptionConfiguration",
            json!({ "type": ENCRYPTION_TYPE_AWS_OWNED_KEY }),
        )),
        "expected the refused repeat to have changed nothing, got {reported:?}"
    );
}

// ---------------------------------------------------------------------------
// The two refusals: the ARN, then the resource
// ---------------------------------------------------------------------------

/// An `activityArn` that is not an activity ARN of this service in this endpoint's
/// region is `InvalidArn` (400) — BEFORE any lookup, so it is never reported as a
/// resource that is absent.
#[tokio::test]
async fn describe_activity_refuses_an_arn_it_cannot_read() {
    let emulator = Emulator::new();
    // A state machine that really exists, so the state-machine row below is refused
    // for its ARN's resource type and not for an absent resource.
    let machine_arn = create_machine(&emulator, "machine", HELLO_DEFINITION).await;
    create_activity(&emulator, &json!({ "name": "present" })).await;

    // The rows differ along WHICH component of the ARN is wrong.
    let cases: Vec<(&str, String)> = vec![
        ("text that is not an ARN at all", "present".to_string()),
        (
            "an ARN of another service",
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:activity:present"),
        ),
        (
            "an ARN of another region",
            format!("arn:aws:states:eu-west-1:{ACCOUNT}:activity:present"),
        ),
        (
            "an ARN of another Step Functions resource type",
            machine_arn.clone(),
        ),
        (
            "an execution ARN",
            format!("arn:aws:states:{REGION}:{ACCOUNT}:execution:machine:run"),
        ),
        (
            "an activity ARN with no name",
            format!("arn:aws:states:{REGION}:{ACCOUNT}:activity:"),
        ),
        (
            "an activity ARN carrying a qualifier",
            format!("arn:aws:states:{REGION}:{ACCOUNT}:activity:present:1"),
        ),
        (
            "an ARN with no partition",
            format!("arn::states:{REGION}:{ACCOUNT}:activity:present"),
        ),
    ];

    for (case, arn) in cases {
        for operation in ["DescribeActivity", "DeleteActivity"] {
            let (status, code) = emulator
                .err(operation, &json!({ "activityArn": arn }))
                .await;
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidArn"),
                "case {case:?} on {operation}: expected a 400 InvalidArn for {arn}, got {status} \
                 {code}"
            );
        }
    }

    // The activity the rows above named by a defective ARN is still there.
    let reported = describe_activity(&emulator, "after every refusal", &activity_arn("present")).await;
    assert_eq!(
        string(&reported, "name"),
        "present",
        "expected the stored activity to be untouched, got {reported:?}"
    );
}

/// A well-formed activity ARN the store holds no activity under is
/// `ActivityDoesNotExist` (400): never created, deleted since, or created against
/// another region's endpoint.
#[tokio::test]
async fn describe_activity_refuses_an_activity_that_is_not_stored() {
    let emulator = Emulator::new();
    create_activity(&emulator, &json!({ "name": "live" })).await;
    create_activity(&emulator, &json!({ "name": "doomed" })).await;

    // `DeleteActivity` models no does-not-exist error, so it answers the same empty
    // 200 for a stored activity and for one that was never there.
    for (case, name) in [("a stored activity", "doomed"), ("one never stored", "ghost")] {
        let deleted = emulator
            .ok("DeleteActivity", &json!({ "activityArn": activity_arn(name) }))
            .await;
        let empty = matches!(&deleted, Value::Null)
            || matches!(&deleted, Value::Object(members) if members.is_empty());
        assert!(
            empty,
            "case {case:?}: expected an empty DeleteActivity body, got {deleted:?}"
        );
    }

    // An activity created against ANOTHER region's endpoint is a different resource:
    // its own ARN is the one that region serves, and this endpoint reports nothing
    // under the name that region's record carries.
    let elsewhere = Emulator::in_region_on(&emulator, "eu-west-1");
    create_activity(&elsewhere, &json!({ "name": "abroad" })).await;
    // A STATE MACHINE of the same name is a different resource too: the resource type
    // is a field of the ARN.
    create_machine(&emulator, "shared-name", HELLO_DEFINITION).await;

    let cases: Vec<(&str, String)> = vec![
        ("a name never created", activity_arn("ghost")),
        ("an activity that was deleted", activity_arn("doomed")),
        (
            "a name created only against another region's endpoint",
            activity_arn("abroad"),
        ),
        (
            "a name that only a state machine carries",
            activity_arn("shared-name"),
        ),
    ];
    for (case, arn) in cases {
        let (status, code) = emulator
            .err("DescribeActivity", &json!({ "activityArn": arn }))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ActivityDoesNotExist"),
            "case {case:?}: expected a 400 ActivityDoesNotExist for {arn}, got {status} {code}"
        );
    }

    // The one that was not deleted is still reported, from either endpoint's own ARN.
    let reported = describe_activity(&emulator, "the surviving activity", &activity_arn("live")).await;
    assert_eq!(
        string(&reported, "name"),
        "live",
        "expected the surviving activity to be reported, got {reported:?}"
    );
    let abroad = describe_activity(
        &elsewhere,
        "the other region's activity",
        &format!("arn:aws:states:eu-west-1:{ACCOUNT}:activity:abroad"),
    )
    .await;
    assert_eq!(
        string(&abroad, "activityArn"),
        format!("arn:aws:states:eu-west-1:{ACCOUNT}:activity:abroad"),
        "expected the other region's own record, got {abroad:?}"
    );
}

/// A name outside the character rule `name` documents is `InvalidName` (400), and
/// nothing is stored for it.
#[tokio::test]
async fn create_activity_refuses_a_name_outside_the_character_rule() {
    let emulator = Emulator::new();

    // One row per class of character the member's documentation forbids.
    let cases = [
        ("white space", "two words"),
        ("a bracket", "worker<1>"),
        ("a wildcard", "worker*"),
        ("a special character", "worker|pipe"),
        ("a path separator", "team/worker"),
        ("a control character", "worker\u{1}"),
    ];
    for (case, name) in cases {
        let (status, code) = emulator
            .err("CreateActivity", &json!({ "name": name }))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidName"),
            "case {case:?}: expected a 400 InvalidName for {name:?}, got {status} {code}"
        );
        let (status, code) = emulator
            .err(
                "DescribeActivity",
                &json!({ "activityArn": activity_arn(name) }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected nothing stored for {name:?}, got {status} {code}"
        );
    }
}

// ---------------------------------------------------------------------------
// An activity is a taggable resource
// ---------------------------------------------------------------------------

/// The tags a create carried are reported for the activity's ARN, and the tagging
/// calls write to the activity exactly as they do to a state machine.
#[tokio::test]
async fn an_activity_carries_the_tags_written_against_its_arn() {
    let emulator = Emulator::new();
    create_activity(
        &emulator,
        &json!({
            "name": "tagged",
            "tags": [
                { "key": "team", "value": "orders" },
                { "key": "tier", "value": "gold" },
            ],
        }),
    )
    .await;
    let arn = activity_arn("tagged");

    let created = tags_of(&emulator, &arn).await;
    assert_eq!(
        created,
        BTreeMap::from([
            ("team".to_string(), "orders".to_string()),
            ("tier".to_string(), "gold".to_string()),
        ]),
        "expected the tags the create carried, got {created:?}"
    );

    emulator
        .ok(
            "TagResource",
            &json!({
                "resourceArn": arn,
                "tags": [
                    { "key": "tier", "value": "silver" },
                    { "key": "owner", "value": "platform" },
                ],
            }),
        )
        .await;
    let merged = tags_of(&emulator, &arn).await;
    assert_eq!(
        merged,
        BTreeMap::from([
            ("team".to_string(), "orders".to_string()),
            ("tier".to_string(), "silver".to_string()),
            ("owner".to_string(), "platform".to_string()),
        ]),
        "expected the merged tag set, got {merged:?}"
    );

    emulator
        .ok(
            "UntagResource",
            &json!({ "resourceArn": arn, "tagKeys": ["tier", "absent"] }),
        )
        .await;
    let remaining = tags_of(&emulator, &arn).await;
    assert_eq!(
        remaining,
        BTreeMap::from([
            ("team".to_string(), "orders".to_string()),
            ("owner".to_string(), "platform".to_string()),
        ]),
        "expected only the named key removed, got {remaining:?}"
    );

    // A write against an activity ARN the store holds nothing under is the
    // modelled `ResourceNotFound`, as it is for a state machine.
    let absent = activity_arn("ghost");
    for (operation, request) in [
        (
            "TagResource",
            json!({
                "resourceArn": absent,
                "tags": [{ "key": "team", "value": "orders" }],
            }),
        ),
        (
            "UntagResource",
            json!({ "resourceArn": absent, "tagKeys": ["team"] }),
        ),
    ] {
        let (status, code) = emulator.err(operation, &request).await;
        assert_eq!(
            (status, code.as_str()),
            (404, "ResourceNotFound"),
            "case {operation}: expected a 404 ResourceNotFound for an absent activity, got \
             {status} {code}"
        );
    }

    // Tagging reads nothing into existence: the ARN above still has no activity.
    let (status, code) = emulator
        .err("DescribeActivity", &json!({ "activityArn": absent }))
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ActivityDoesNotExist"),
        "expected the tagging refusals to have created nothing, got {status} {code}"
    );
}
