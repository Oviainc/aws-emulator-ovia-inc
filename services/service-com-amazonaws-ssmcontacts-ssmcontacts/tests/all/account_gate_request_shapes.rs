// SPDX-License-Identifier: CC0-1.0

//! The account-level Incident Manager precondition is decided from the ACCOUNT alone, over
//! the request shapes the model admits — end to end over the wire.
//!
//! The rule under test:
//!
//! > A data-plane operation of this service whose subject is scoped to the account's
//! > Incident Manager replication set is refused with its OWN modelled
//! > `ValidationException` (HTTP 400), decided before the request's ARN is resolved and
//! > before anything is written, whenever the account has no ACTIVE replication set — so no
//! > request shape the model admits can change that answer into a resource-level
//! > `ResourceNotFoundException` or into a served page; only the account-wide listings that
//! > report the account's own collection (`ListContacts`, `ListEngagements`) are exempt and
//! > stay served.
//!
//! What this suite adds to its siblings is the REQUEST dimension. `tests/all/
//! cold_account_gate.rs` and `tests/all/data_plane_precondition.rs` span the gated
//! operations with one request each; here each gated operation is driven over the published
//! bounds and forms of its own input — every partition the modelled `SsmContactsArn` pattern
//! admits, the Region-less ARN that pattern also admits, a resource field of more than one
//! segment, `TagsList`/`TagKeyList` at both ends of their modelled `length` 0..50,
//! `MaxResults` on both ends of its modelled `range` 0..1024 and one value past it, and
//! `RotationName`'s `length` 1..255 on its maximum — because the gate is a fact about the
//! account, so none of them may matter.
//!
//! The listing contrast is the point the recorded corpus makes: with no replication set
//! `ListRotations` is refused where `ListContacts` is served, so an "empty but healthy"
//! `{"Rotations": []}` is a wrong answer even for an account that really does hold
//! rotations — which is why the last suite stocks the store first and then closes the gate.

use serde_json::{json, Value};

use service_com_amazonaws_ssmcontacts_ssmcontacts::ops::types::VALIDATION_EXCEPTION_REASON_VALUES;
use service_com_amazonaws_ssmcontacts_ssmcontacts::state::{
    AccountReplicationSet, ACCOUNT_NOT_FOUND_MESSAGE,
};

use crate::contact_lifecycle::{error_code, Emulator, ACCOUNT, REGION};
use crate::contact_updates::{active_replication_set, onboard, set_replication_set};

// ---------------------------------------------------------------------------
// The account states that close the gate
// ---------------------------------------------------------------------------

/// Every state of the account's replication-set record that is not ACTIVE: no record at
/// all, and a record for a set that has not finished replicating.
fn closed_gate_states() -> Vec<(&'static str, Option<AccountReplicationSet>)> {
    vec![
        ("no replication set at all", None),
        (
            "a replication set that has not finished replicating",
            Some(AccountReplicationSet {
                active: false,
                ..active_replication_set()
            }),
        ),
    ]
}

// ---------------------------------------------------------------------------
// The request catalog
// ---------------------------------------------------------------------------

/// One row: an operation, a request body, and what the SAME bytes answer once the account
/// has an active set (the complement that shows the account is what decided).
struct Row {
    case: String,
    operation: &'static str,
    request: Value,
    onboarded_status: u16,
}

/// Well-formed ARNs of resources this store never holds, spanning what the modelled
/// `SsmContactsArn` admits.
///
/// Source: `smithy_query(query="shape", shape="SsmContactsArn")` — `length` 1..2048 and
/// `pattern` `^arn:(aws|aws-cn|aws-us-gov):ssm-contacts:[-\w+=/,.@]*:[0-9]+:([\w+=/,.@:-]+)*$`,
/// whose three partition alternatives are all covered below, whose Region field admits the
/// empty string, and whose resource field admits the `/`-joined multi-segment name a contact
/// channel carries. `TagResource`/`UntagResource` take the looser `AmazonResourceName`
/// (`length` 1..1011, no pattern), which admits every one of these too.
fn absent_arns() -> Vec<(&'static str, String)> {
    vec![
        (
            "a contact ARN in the aws partition",
            format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact/never-created"),
        ),
        (
            "a contact ARN in the aws-cn partition",
            format!("arn:aws-cn:ssm-contacts:cn-north-1:{ACCOUNT}:contact/never-created"),
        ),
        (
            "a rotation ARN in the aws-us-gov partition",
            format!("arn:aws-us-gov:ssm-contacts:us-gov-west-1:{ACCOUNT}:rotation/never-created"),
        ),
        (
            "a Region-less contact ARN, which the pattern's Region field admits",
            format!("arn:aws:ssm-contacts::{ACCOUNT}:contact/never-created"),
        ),
        (
            "a contact-channel ARN, whose resource field carries two segments",
            format!(
                "arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact-channel/never-created/0123456789ab"
            ),
        ),
    ]
}

/// `Tags` at both ends of `TagsList`' modelled `length` 0..50, plus one list in between.
fn tag_lists() -> Vec<(&'static str, Value)> {
    vec![
        ("no tags at all, the list's modelled minimum length", json!([])),
        (
            "one tag",
            json!([{"Key": "team", "Value": "sre"}]),
        ),
        (
            "fifty tags, the list's modelled maximum length",
            Value::Array(
                (0..50)
                    .map(|index| json!({"Key": format!("k{index}"), "Value": "v"}))
                    .collect(),
            ),
        ),
    ]
}

/// `TagKeys` at both ends of `TagKeyList`' modelled `length` 0..50, plus one list in
/// between.
fn tag_key_lists() -> Vec<(&'static str, Value)> {
    vec![
        ("no keys at all, the list's modelled minimum length", json!([])),
        ("one key", json!(["team"])),
        (
            "fifty keys, the list's modelled maximum length",
            Value::Array((0..50).map(|index| json!(format!("k{index}"))).collect()),
        ),
    ]
}

/// The member combinations `UpdateContact` admits: its two optional writable members, each
/// alone and both together, with `DisplayName` once on the empty string its modelled
/// `length` 0..255 admits and `Plan` in both shapes the model gives it.
fn update_bodies() -> Vec<(&'static str, Value)> {
    vec![
        ("a DisplayName only", json!({"DisplayName": "renamed"})),
        (
            "a Plan carrying the Stages shape only",
            json!({"Plan": {"Stages": [{"DurationInMinutes": 10, "Targets": []}]}}),
        ),
        (
            "an empty DisplayName and a Plan carrying the RotationIds shape",
            json!({"DisplayName": "", "Plan": {"RotationIds": []}}),
        ),
    ]
}

/// Every gated request this suite sends against an ARN the store does not hold: the three
/// ARN-addressed operations of the issue over the ARN catalog and their own input bounds,
/// plus the account-wide `ListRotations` over the bounds of its paging and filter members.
///
/// `onboarded_status` is what the same bytes answer once the account has an active set: the
/// ARN-addressed rows then answer about their absent resource (404), and the listing is
/// served (200).
fn gated_rows() -> Vec<Row> {
    let mut rows = Vec::new();

    for (arn_case, arn) in absent_arns() {
        for (tags_case, tags) in tag_lists() {
            rows.push(Row {
                case: format!("TagResource with {tags_case} on {arn_case}"),
                operation: "TagResource",
                request: json!({"ResourceARN": arn, "Tags": tags}),
                onboarded_status: 404,
            });
        }
        for (keys_case, keys) in tag_key_lists() {
            rows.push(Row {
                case: format!("UntagResource with {keys_case} on {arn_case}"),
                operation: "UntagResource",
                request: json!({"ResourceARN": arn, "TagKeys": keys}),
                onboarded_status: 404,
            });
        }
        for (update_case, body) in update_bodies() {
            let mut request = json!({"ContactId": arn});
            for (member, value) in body.as_object().expect("an update body is an object") {
                request[member] = value.clone();
            }
            rows.push(Row {
                case: format!("UpdateContact with {update_case} on {arn_case}"),
                operation: "UpdateContact",
                request,
                onboarded_status: 404,
            });
        }
    }

    // `ListRotations`' own members, on their published bounds: `MaxResults` has modelled
    // `range` 0..1024, `NextToken` modelled `length` 0..2048 and `RotationNamePrefix`
    // modelled `length` 1..255 with pattern `^[a-zA-Z0-9_\-\s\.]*$`.
    let listings: Vec<(String, Value)> = vec![
        ("no member at all".to_string(), json!({})),
        (
            "a one-character RotationNamePrefix, the member's modelled minimum length".to_string(),
            json!({"RotationNamePrefix": "r"}),
        ),
        (
            "a 255-character RotationNamePrefix, the member's modelled maximum length".to_string(),
            json!({"RotationNamePrefix": "r".repeat(255)}),
        ),
        (
            "MaxResults 0, the member's modelled minimum".to_string(),
            json!({"MaxResults": 0}),
        ),
        (
            "MaxResults 1024, the member's modelled maximum".to_string(),
            json!({"MaxResults": 1024}),
        ),
        (
            "an empty NextToken, the member's modelled minimum length".to_string(),
            json!({"NextToken": ""}),
        ),
    ];
    for (case, request) in listings {
        rows.push(Row {
            case: format!("ListRotations with {case}"),
            operation: "ListRotations",
            request,
            onboarded_status: 200,
        });
    }

    rows
}

/// The account-level refusal, asserted as a class rather than as a transcript: HTTP 400,
/// the error code `ValidationException`, a `Message` that reports the ACCOUNT (read from the
/// crate's own constant, not copied), a `Reason` inside the modelled value set and a present
/// but empty `Fields`, because no member of the request is at fault.
fn assert_account_refusal(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected the account precondition to answer HTTP 400, got {status} \
         with {body:?}"
    );
    assert!(
        error_code(body).contains("ValidationException"),
        "case {case:?}: expected a ValidationException — not a ResourceNotFoundException and \
         not a NotImplemented — got {:?} in {body:?}",
        error_code(body)
    );
    let message = body["Message"].as_str().unwrap_or_default();
    assert!(
        message.contains(ACCOUNT_NOT_FOUND_MESSAGE),
        "case {case:?}: expected the account-level refusal {ACCOUNT_NOT_FOUND_MESSAGE:?}, got \
         {message:?} in {body:?}"
    );
    let reason = body["Reason"].as_str().unwrap_or_default();
    assert!(
        VALIDATION_EXCEPTION_REASON_VALUES.contains(&reason),
        "case {case:?}: expected a Reason inside the modelled ValidationExceptionReason set \
         {VALIDATION_EXCEPTION_REASON_VALUES:?}, got {reason:?} in {body:?}"
    );
    assert_eq!(
        body["Fields"].as_array().map(Vec::len),
        Some(0),
        "case {case:?}: an account-level refusal blames no member, so Fields must be present \
         and empty, got {body:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule: no request shape changes the account's answer
// ---------------------------------------------------------------------------

#[tokio::test]
async fn no_admitted_request_shape_escapes_the_closed_account_gate() {
    for (account_case, replication_set) in closed_gate_states() {
        let emulator = Emulator::new();
        set_replication_set(&emulator, replication_set.clone());

        for row in gated_rows() {
            let case = format!("{}, {account_case}", row.case);
            let (status, body) = emulator.call(row.operation, row.request.clone()).await;
            assert_account_refusal(&case, status, &body);
        }
    }
}

/// The complement: the SAME bytes, on an account whose only difference is an ACTIVE
/// replication set, answer about their own subject — 404 for an ARN the store does not hold,
/// a served page for the listing. So the account's record is what decided above, not any
/// member of the request.
#[tokio::test]
async fn the_same_requests_answer_about_their_subject_once_the_account_is_onboarded() {
    let emulator = Emulator::new();
    onboard(&emulator);

    for row in gated_rows() {
        let case = &row.case;
        let (status, body) = emulator.call(row.operation, row.request.clone()).await;
        assert_eq!(
            status, row.onboarded_status,
            "case {case:?}: expected HTTP {} on an onboarded account, got {status} with \
             {body:?}",
            row.onboarded_status
        );
        let message = body["Message"].as_str().unwrap_or_default();
        assert!(
            !message.contains(ACCOUNT_NOT_FOUND_MESSAGE),
            "case {case:?}: an onboarded account may not be refused by the account gate, got \
             {message:?} in {body:?}"
        );
        if row.onboarded_status == 404 {
            assert!(
                error_code(&body).contains("ResourceNotFoundException"),
                "case {case:?}: expected the answer to be about the absent resource, got {:?} \
                 in {body:?}",
                error_code(&body)
            );
        } else {
            assert!(
                body["Rotations"].is_array(),
                "case {case:?}: expected a served page carrying the required Rotations list, \
                 got {body:?}"
            );
        }
    }
}

/// A value PAST a published bound is refused on the member, whatever the account's state:
/// the account gate is not what screens a request the model does not admit, so the two
/// refusals stay distinguishable — the account's names the account, the member's names the
/// member.
#[tokio::test]
async fn a_value_past_a_published_bound_is_refused_on_the_member_not_on_the_account() {
    // `MaxResults`' modelled range is 0..1024 and `RotationNamePrefix`' modelled length is
    // 1..255, so each of these is the first value outside one of them.
    let past_the_bound = [
        ("MaxResults one past its modelled maximum", json!({"MaxResults": 1025})),
        (
            "a RotationNamePrefix one character past its modelled maximum length",
            json!({"RotationNamePrefix": "r".repeat(256)}),
        ),
    ];

    for (case, request) in past_the_bound {
        for onboarded in [false, true] {
            let emulator = Emulator::new();
            if onboarded {
                onboard(&emulator);
            } else {
                set_replication_set(&emulator, None);
            }
            let case = format!("ListRotations with {case}, onboarded={onboarded}");

            let (status, body) = emulator.call("ListRotations", request.clone()).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected a value outside the published bound to be refused \
                 with HTTP 400, got {status} with {body:?}"
            );
            assert!(
                error_code(&body).contains("ValidationException"),
                "case {case:?}: expected a ValidationException, got {:?} in {body:?}",
                error_code(&body)
            );
            if onboarded {
                let message = body["Message"].as_str().unwrap_or_default();
                assert!(
                    !message.contains(ACCOUNT_NOT_FOUND_MESSAGE),
                    "case {case:?}: an onboarded account's refusal must be about the member, \
                     got {message:?} in {body:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rule: the gate decides before the resource is touched, and writes nothing
// ---------------------------------------------------------------------------

/// What a reader reports about the resources this suite stocked the store with, read over
/// the wire so the assertion is about what a later caller SEES.
#[derive(Debug, PartialEq)]
struct Reported {
    contact_display_name: Option<String>,
    contact_tags: Vec<(String, String)>,
    rotation_names: Vec<String>,
}

async fn reported(emulator: &Emulator, contact: &str) -> Reported {
    let (status, contact_body) = emulator.call("GetContact", json!({"ContactId": contact})).await;
    assert_eq!(
        status, 200,
        "GetContact must answer 200 on the onboarded account, got {status} {contact_body:?}"
    );
    let (status, tag_body) = emulator
        .call("ListTagsForResource", json!({"ResourceARN": contact}))
        .await;
    assert_eq!(
        status, 200,
        "ListTagsForResource must answer 200 on the onboarded account, got {status} {tag_body:?}"
    );
    let (status, rotation_body) = emulator.call("ListRotations", json!({})).await;
    assert_eq!(
        status, 200,
        "ListRotations must answer 200 on the onboarded account, got {status} {rotation_body:?}"
    );
    Reported {
        contact_display_name: contact_body["DisplayName"].as_str().map(str::to_string),
        contact_tags: tag_body["Tags"]
            .as_array()
            .map(|tags| {
                tags.iter()
                    .map(|tag| {
                        (
                            tag["Key"].as_str().unwrap_or_default().to_string(),
                            tag["Value"].as_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default(),
        rotation_names: rotation_body["Rotations"]
            .as_array()
            .map(|rotations| {
                rotations
                    .iter()
                    .filter_map(|rotation| rotation["Name"].as_str())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// The same refusal for a resource the store DOES hold, and nothing of it changed: the gate
/// is decided before the ARN is resolved, so a stored target is refused identically to an
/// absent one and no tag, name or plan the refused request carried is written.
#[tokio::test]
async fn a_closed_gate_refuses_a_stored_resource_identically_and_writes_nothing() {
    for (account_case, replication_set) in closed_gate_states() {
        let emulator = Emulator::new();
        onboard(&emulator);
        let contact = emulator
            .create_contact(
                json!({
                    "Alias": "oncall-primary",
                    "DisplayName": "On-call primary",
                    "Type": "PERSONAL",
                    "Plan": {"Stages": [{"DurationInMinutes": 5, "Targets": []}]},
                    "Tags": [{"Key": "team", "Value": "sre"}],
                }),
                "the stored contact this suite refuses calls against",
            )
            .await;
        let rotation = emulator
            .create_rotation(&contact, "stored-rotation", "the stored rotation")
            .await;
        let before = reported(&emulator, &contact).await;

        set_replication_set(&emulator, replication_set.clone());
        for (target_case, target) in [("the stored contact", &contact), ("the stored rotation", &rotation)]
        {
            let requests: Vec<(String, &'static str, Value)> = vec![
                (
                    "TagResource".to_string(),
                    "TagResource",
                    json!({"ResourceARN": target, "Tags": [{"Key": "gate", "Value": "closed"}]}),
                ),
                (
                    "UntagResource".to_string(),
                    "UntagResource",
                    json!({"ResourceARN": target, "TagKeys": ["team"]}),
                ),
                (
                    "UpdateContact".to_string(),
                    "UpdateContact",
                    json!({"ContactId": target, "DisplayName": "renamed-by-a-refused-call"}),
                ),
            ];
            for (label, operation, request) in requests {
                let case = format!("{label} on {target_case}, {account_case}");
                let (status, body) = emulator.call(operation, request).await;
                assert_account_refusal(&case, status, &body);
            }
        }

        // Onboarded again, every reader reports exactly what it reported before the refusals.
        onboard(&emulator);
        let after = reported(&emulator, &contact).await;
        assert_eq!(
            after, before,
            "case {account_case:?}: a refused call must change nothing a later caller sees, \
             got {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule: which listings the precondition exempts
// ---------------------------------------------------------------------------

/// With the gate closed the exempt account-wide listings keep REPORTING the account's own
/// resources, while `ListRotations` is refused — even though the account really does hold a
/// rotation, so a served `{"Rotations": []}` would be an "empty but healthy" answer about an
/// account AWS says is not onboarded at all.
#[tokio::test]
async fn the_exempt_listings_report_their_collections_while_the_rotation_listing_is_refused() {
    for (account_case, replication_set) in closed_gate_states() {
        let emulator = Emulator::new();
        onboard(&emulator);
        let contact = emulator
            .create_contact(
                json!({
                    "Alias": "oncall-primary",
                    "Type": "PERSONAL",
                    "Plan": {"Stages": [{"DurationInMinutes": 5, "Targets": []}]},
                }),
                "the contact the exempt listings report",
            )
            .await;
        emulator
            .create_rotation(&contact, "stocked-rotation", "the rotation the gated listing hides")
            .await;
        let (status, body) = emulator
            .call(
                "StartEngagement",
                json!({
                    "ContactId": contact,
                    "Sender": "cli",
                    "Subject": "disk pressure",
                    "Content": "node-7 is at 95%",
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "StartEngagement must answer 200 on the onboarded account, got {status} {body:?}"
        );

        set_replication_set(&emulator, replication_set.clone());

        // Exempt: the listing answers about the account's own collection, and reports what
        // the store holds rather than an empty page.
        for (operation, request, collection) in [
            ("ListContacts", json!({}), "Contacts"),
            (
                "ListContacts",
                json!({"AliasPrefix": "oncall"}),
                "Contacts",
            ),
            ("ListEngagements", json!({}), "Engagements"),
        ] {
            let case = format!("{operation} with {request}, {account_case}");
            let (status, body) = emulator.call(operation, request.clone()).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a precondition-exempt listing stays served, got {status} with \
                 {body:?}"
            );
            assert_eq!(
                body[collection].as_array().map(Vec::len),
                Some(1),
                "case {case:?}: expected the stored {collection} to be reported, got {body:?}"
            );
        }

        // Gated: the rotation listing is refused, and its body carries no page at all.
        let case = format!("ListRotations, {account_case}");
        let (status, body) = emulator
            .call("ListRotations", json!({"RotationNamePrefix": "stocked"}))
            .await;
        assert_account_refusal(&case, status, &body);
        assert!(
            body.get("Rotations").is_none(),
            "case {case:?}: a refusal may not also carry a page, got {body:?}"
        );
    }
}
