// SPDX-License-Identifier: CC0-1.0

//! How the three tagging operations read the `ResourceARN` they are given, end to end over
//! the wire.
//!
//! The rule under test:
//!
//! > An operation addressing its subject through a caller-supplied resource-ARN member the
//! > model constrains by LENGTH only refuses a value that does not parse as an ARN of this
//! > service with its own modelled `ValidationException` (HTTP 400), naming the value it was
//! > given, BEFORE any lookup — and answers not-found only for a value that does parse and
//! > that the store does not hold.
//!
//! `ResourceARN` is the one ARN member of this service bound to `AmazonResourceName`
//! (`length` 1..1011, no `pattern`); every other ARN member is an `SsmContactsArn` carrying
//! the `^arn:(aws|aws-cn|aws-us-gov):ssm-contacts:…` pattern, which the boundary validator
//! screens before a handler runs. So the three operations that bind it — `ListTagsForResource`,
//! `TagResource` and `UntagResource` — are where the parse has to happen, and this suite
//! asserts it over a TABLE spanning what the rule generalises over: which field of the ARN is
//! wrong (the literal `arn`, the partition, the vendor, the resource, the field count), the
//! vendor it names, the partition and Region and account it is scoped to, whether the named
//! resource exists, and which of the three operations is called.
//!
//! `tests/all/data_plane_precondition.rs` is the suite about what the tagging operations DO
//! once an ARN resolves, and about the not-found answer for well-formed ARNs; this one is
//! about telling a malformed REQUEST from a missing RESOURCE.

use serde_json::{json, Value};

use service_com_amazonaws_ssmcontacts_ssmcontacts::ops::types::VALIDATION_EXCEPTION_REASON_VALUES;

use crate::contact_lifecycle::{error_code, parse, Emulator, ACCOUNT, REGION};
use crate::contact_updates::{onboard, set_replication_set};

/// A daily recurrence, the smallest `RecurrenceSettings` that describes a schedule.
const DAILY_RECURRENCE: &str = r#"{
    "NumberOfOnCalls": 1,
    "RecurrenceMultiplier": 1,
    "DailySettings": [{"HourOfDay": 9, "MinuteOfHour": 0}]
}"#;

/// Values that are NOT an ARN of this service, each with the field of the ARN grammar it
/// breaks.
///
/// The grammar is `arn:<partition>:<vendor>:<region>:<account>:<resource>` — six
/// colon-separated fields whose first is literally `arn` and whose partition, vendor and
/// resource are non-empty — and the vendor this service's resources carry is its model's own
/// `aws.api#service$arnNamespace`, `ssm-contacts` (the same namespace the `SsmContactsArn`
/// pattern spells out). The first row is the value the issue cited; the rest are rows it did
/// not.
fn malformed_values() -> Vec<(&'static str, String)> {
    vec![
        ("a bare alias, with no ARN syntax at all", "my-contact".to_string()),
        ("a truncated ARN carrying three fields", "arn:aws:ssm-contacts".to_string()),
        (
            "an ARN carrying five fields, so it names no resource",
            format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}"),
        ),
        (
            "a six-field ARN whose resource field is empty",
            format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:"),
        ),
        (
            "an ARN whose first field is not the literal arn",
            format!("ARN:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact/never-created"),
        ),
        (
            "an ARN with an empty partition",
            format!("arn::ssm-contacts:{REGION}:{ACCOUNT}:contact/never-created"),
        ),
        (
            "an ARN with an empty vendor",
            format!("arn:aws::{REGION}:{ACCOUNT}:contact/never-created"),
        ),
        ("an ARN of another vendor", "arn:aws:s3:::some-bucket".to_string()),
        (
            "an ARN of the Incident Manager replication set this account is onboarded with",
            format!("arn:aws:ssm-incidents::{ACCOUNT}:replication-set/emulator-set"),
        ),
        (
            "an ARN whose vendor merely starts with this service's namespace",
            format!("arn:aws:ssm-contacts-beta:{REGION}:{ACCOUNT}:contact/never-created"),
        ),
        (
            "a console URL rather than an ARN",
            "https://console.aws.amazon.com/systems-manager/contacts/my-contact".to_string(),
        ),
        (
            "an ARN with leading whitespace",
            format!(" arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact/never-created"),
        ),
    ]
}

/// Well-formed ARNs of this service that the store does not hold.
///
/// `region` and `account` are deliberately allowed to be EMPTY — AWS writes a
/// partition-global or account-less resource that way — and the partition spans the three
/// the `SsmContactsArn` pattern admits (`aws`, `aws-cn`, `aws-us-gov`), so none of these is a
/// malformed request however little of it this emulator's own account minted.
fn well_formed_absent_values() -> Vec<(&'static str, String)> {
    vec![
        (
            "a contact ARN nothing created",
            format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact/never-created"),
        ),
        (
            "a rotation ARN nothing created",
            format!(
                "arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:rotation/00000000-0000-0000-0000-000000000000"
            ),
        ),
        (
            "a contact-channel ARN, a kind of resource that carries no tags",
            format!(
                "arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact-channel/never-created/00000000-0000-0000-0000-000000000000"
            ),
        ),
        (
            "a well-formed ARN with an empty Region and account",
            "arn:aws:ssm-contacts:::contact/never-created".to_string(),
        ),
        (
            "a well-formed ARN in the China partition",
            format!("arn:aws-cn:ssm-contacts:cn-north-1:{ACCOUNT}:contact/never-created"),
        ),
        (
            "a well-formed ARN in the GovCloud partition",
            format!("arn:aws-us-gov:ssm-contacts:us-gov-west-1:{ACCOUNT}:contact/never-created"),
        ),
        (
            "a well-formed ARN in another account",
            format!("arn:aws:ssm-contacts:{REGION}:111122223333:contact/never-created"),
        ),
        (
            "a well-formed ARN whose resource field carries colons",
            format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact:never-created"),
        ),
    ]
}

/// The three tagging requests naming `resource_arn`, as `(operation, request)`.
fn tagging_requests(resource_arn: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "ListTagsForResource",
            json!({"ResourceARN": resource_arn}),
        ),
        (
            "TagResource",
            json!({"ResourceARN": resource_arn, "Tags": [{"Key": "team", "Value": "sre"}]}),
        ),
        (
            "UntagResource",
            json!({"ResourceARN": resource_arn, "TagKeys": ["team"]}),
        ),
    ]
}

/// A taggable resource this suite really created, and the tags it carries.
struct LiveResource<'a> {
    case: &'a str,
    arn: &'a str,
    tags: Vec<(String, String)>,
}

/// The `(Key, Value)` pairs `ListTagsForResource` reports for `arn`.
async fn reported_tags(emulator: &Emulator, arn: &str, case: &str) -> Vec<(String, String)> {
    let (status, body) = emulator
        .call("ListTagsForResource", json!({"ResourceARN": arn}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListTagsForResource to answer 200, got {status} {body:?}"
    );
    body["Tags"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or_default().to_string(),
                tag["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// Every value that is not an ARN of this service is refused on the INPUT, on all three
/// tagging operations — with the modelled `ValidationException` naming the value, never with
/// a not-found answer about a resource.
#[tokio::test]
async fn a_resource_arn_that_is_not_an_arn_of_this_service_is_refused_as_a_bad_request() {
    let emulator = Emulator::new();

    for (case, value) in malformed_values() {
        for (operation, request) in tagging_requests(&value) {
            let case = format!("{operation} on {case}");
            let (status, body) = emulator.call(operation, request).await;

            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for a value that names no resource \
                 identity, got {status} {body:?}"
            );
            assert!(
                error_code(&body).contains("ValidationException"),
                "case {case:?}: expected the error code ValidationException — NOT a \
                 ResourceNotFoundException, which is reserved for a well-formed ARN that \
                 names nothing — got {:?} in {body:?}",
                error_code(&body)
            );

            // The refusal is about the member the caller supplied, so it names that value
            // and reports a field-level fault.
            let message = body["Message"].as_str().unwrap_or_default();
            assert!(
                message.contains(value.trim()),
                "case {case:?}: expected a Message naming the supplied value {value:?}, \
                 got {message:?} in {body:?}"
            );
            assert!(
                !message.contains("Account"),
                "case {case:?}: an onboarded account is not at fault here, got {message:?} \
                 in {body:?}"
            );

            let reason = body["Reason"].as_str().unwrap_or_default();
            assert!(
                VALIDATION_EXCEPTION_REASON_VALUES.contains(&reason),
                "case {case:?}: expected a Reason inside the modelled \
                 ValidationExceptionReason set {VALIDATION_EXCEPTION_REASON_VALUES:?}, got \
                 {reason:?} in {body:?}"
            );
            // And it must be the FIELD-level reason of that set, not the account-level
            // `OTHER` the replication-set gate answers with: one member of the request is
            // what is at fault here, which is a different remedy for the caller.
            assert_eq!(
                reason, "FIELD_VALIDATION_FAILED",
                "case {case:?}: expected the member-level reason, got {reason:?} in {body:?}"
            );

            let fields = body["Fields"].as_array().cloned().unwrap_or_default();
            assert_eq!(
                fields.len(),
                1,
                "case {case:?}: one member is at fault, so Fields must name exactly it, \
                 got {body:?}"
            );
            let named = fields[0]["Name"].as_str().unwrap_or_default();
            // Neither the model nor the service's documentation fixes a spelling for a
            // `ValidationExceptionField` name, so what is asserted is that the field
            // identifies the resource-ARN member rather than any particular casing of it.
            assert!(
                named.to_ascii_lowercase().contains("resourcearn"),
                "case {case:?}: expected Fields to blame the resource-ARN member, got \
                 {named:?} in {body:?}"
            );
        }
    }
}

/// A well-formed ARN of this service that the store does not hold keeps answering the
/// modelled `ResourceNotFoundException` (HTTP 404), whatever partition, Region or account it
/// is scoped to and whatever kind of resource it names.
#[tokio::test]
async fn a_well_formed_arn_naming_nothing_still_answers_not_found() {
    let emulator = Emulator::new();

    for (case, value) in well_formed_absent_values() {
        for (operation, request) in tagging_requests(&value) {
            let case = format!("{operation} on {case}");
            let (status, body) = emulator.call(operation, request).await;

            assert_eq!(
                status, 404,
                "case {case:?}: a well-formed ARN that names nothing is a missing RESOURCE, \
                 not a bad request, got {status} {body:?}"
            );
            assert!(
                error_code(&body).contains("ResourceNotFoundException"),
                "case {case:?}: expected a ResourceNotFoundException, got {:?} in {body:?}",
                error_code(&body)
            );
            assert_eq!(
                body["ResourceId"].as_str(),
                Some(value.as_str()),
                "case {case:?}: the not-found answer reports the ARN that was looked up, \
                 got {body:?}"
            );
        }
    }
}

/// The ARN the store DOES hold is still served, and a refused call changes nothing a later
/// caller sees: the input screen neither swallows a real resource nor writes through a
/// refusal.
#[tokio::test]
async fn the_screen_leaves_a_stored_resource_served_and_unchanged() {
    let emulator = Emulator::new();

    let contact = emulator
        .create_contact(
            json!({
                "Alias": "oncall-primary",
                "Type": "PERSONAL",
                "Plan": {"Stages": [{"DurationInMinutes": 5, "Targets": []}]},
                "Tags": [{"Key": "team", "Value": "sre"}],
            }),
            "the suite's live contact",
        )
        .await;

    let (status, body) = emulator
        .call(
            "CreateRotation",
            json!({
                "Name": "live-rotation",
                "ContactIds": [contact],
                "TimeZoneId": "UTC",
                "Recurrence": parse(DAILY_RECURRENCE),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "the suite's live rotation must be created, got {status} {body:?}"
    );
    let rotation = body["RotationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a RotationArn, got {body:?}"))
        .to_string();

    let live = [
        LiveResource {
            case: "the stored contact",
            arn: contact.as_str(),
            tags: vec![("team".to_string(), "sre".to_string())],
        },
        LiveResource {
            case: "the stored rotation",
            arn: rotation.as_str(),
            tags: Vec::new(),
        },
    ];

    for LiveResource {
        case,
        arn,
        tags: want,
    } in &live
    {
        assert_eq!(
            &reported_tags(&emulator, arn, case).await,
            want,
            "case {case:?}: expected the stored tags to be reported"
        );

        // Every malformed value is refused, and none of them touches this resource.
        for (malformed_case, value) in malformed_values() {
            for (operation, request) in tagging_requests(&value) {
                let (status, body) = emulator.call(operation, request).await;
                assert_eq!(
                    status, 400,
                    "case {case:?} / {malformed_case:?}: expected {operation} to refuse the \
                     malformed value, got {status} {body:?}"
                );
            }
        }

        assert_eq!(
            &reported_tags(&emulator, arn, case).await,
            want,
            "case {case:?}: a refused call must leave the resource's tags exactly as they were"
        );

        // And the writes still reach the resource through its own ARN.
        let (status, body) = emulator
            .call(
                "TagResource",
                json!({"ResourceARN": arn, "Tags": [{"Key": "owner", "Value": "sre"}]}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected TagResource to answer 200, got {status} {body:?}"
        );
        assert!(
            reported_tags(&emulator, arn, case)
                .await
                .contains(&("owner".to_string(), "sre".to_string())),
            "case {case:?}: expected the written tag to be reported back"
        );

        let (status, body) = emulator
            .call(
                "UntagResource",
                json!({"ResourceARN": arn, "TagKeys": ["owner"]}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UntagResource to answer 200, got {status} {body:?}"
        );
        assert_eq!(
            &reported_tags(&emulator, arn, case).await,
            want,
            "case {case:?}: expected the removal to leave the tags it started with"
        );
    }
}

/// The ACCOUNT-level precondition still comes first: on an account with no active
/// replication set even a malformed `ResourceARN` is answered about the account, because the
/// gate is evaluated before the member is read.
#[tokio::test]
async fn the_account_gate_precedes_the_resource_arn_screen() {
    let emulator = Emulator::new();
    set_replication_set(&emulator, None);

    for (case, value) in malformed_values() {
        for (operation, request) in tagging_requests(&value) {
            let case = format!("{operation} on {case}, account not onboarded");
            let (status, body) = emulator.call(operation, request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {body:?}"
            );
            let message = body["Message"].as_str().unwrap_or_default();
            assert!(
                message.contains("Account"),
                "case {case:?}: expected the account-level refusal to be reported first, \
                 got {message:?} in {body:?}"
            );
        }
    }

    // Onboarded again, the very same requests are refused on the member instead.
    onboard(&emulator);
    let (case, value) = malformed_values()
        .into_iter()
        .next()
        .expect("the malformed table is not empty");
    for (operation, request) in tagging_requests(&value) {
        let case = format!("{operation} on {case}, account onboarded");
        let (status, body) = emulator.call(operation, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            body["Fields"].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: once the account is onboarded the member is what is at fault, \
             got {body:?}"
        );
    }
}
