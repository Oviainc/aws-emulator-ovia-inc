// SPDX-License-Identifier: CC0-1.0

//! `SendContactMethodVerification` — sending a fresh verification request to a
//! contact method — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The `Authorization` header carries only a credential SCOPE, which is
//! what names the Region a request is made in.
//!
//! The rule under test: a verification request is sent to the ONE contact method
//! the requested protocol addresses in the Region the request was made in — the
//! request names no Region and no resource, so that pair is the whole address. It
//! leaves that contact method awaiting verification without re-creating it,
//! answers exactly one terminal operation-log entry naming it, leaves every other
//! Region's and every other protocol's contact method alone, and is refused with
//! the service's `DoesNotExist` envelope when that pair addresses nothing — there
//! is no destination to send a verification to.
//!
//! What a contact method's creation and its reads do with the record is the
//! sibling `contact_method_notifications` suite's subject; this one asserts only
//! what sending a verification changes.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_lightsail_lightsail_20161128::state as lightsail_state;

// The harness, the wire error code and the minted-id check this test target
// already exposes: a sibling suite drives the service through the same front
// door rather than standing up a second one.
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// `ContactMethodStatus` values, from the modelled enum: a contact method whose
/// verification has not been completed is `PendingVerification`, and one
/// notifications can be sent through is `Valid`.
const PENDING_VERIFICATION: &str = "PendingVerification";
/// See [`PENDING_VERIFICATION`].
const VALID: &str = "Valid";

/// The Region a request that carries no credential scope is served in.
const DEFAULT_REGION: &str = "us-east-1";

/// Every contact method `GetContactMethods` reports in `region`, unfiltered.
async fn contact_methods(emulator: &Emulator, region: Option<&str>) -> Vec<Value> {
    let (status, body) = emulator
        .call_in(region, "GetContactMethods", json!({}))
        .await;
    assert_eq!(
        status, 200,
        "case reading contact methods in {region:?}: GetContactMethods must succeed, got {body:?}"
    );
    body["contactMethods"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case reading contact methods in {region:?}: expected a contactMethods array, \
                 got {body:?}"
            )
        })
        .clone()
}

/// The one contact method of `protocol` in `region`, as its own read reports it.
async fn contact_method(
    emulator: &Emulator,
    region: Option<&str>,
    protocol: &str,
    case: &str,
) -> Value {
    let reported: Vec<Value> = contact_methods(emulator, region)
        .await
        .into_iter()
        .filter(|method| method["protocol"].as_str() == Some(protocol))
        .collect();
    assert_eq!(
        reported.len(),
        1,
        "case {case}: expected exactly one {protocol} contact method in {region:?}, \
         got {reported:?}"
    );
    reported[0].clone()
}

/// Register one contact method of `protocol` in `region`.
async fn create(emulator: &Emulator, region: Option<&str>, protocol: &str, endpoint: &str) {
    emulator
        .ok_at(
            None,
            region,
            "CreateContactMethod",
            json!({"protocol": protocol, "contactEndpoint": endpoint}),
        )
        .await;
}

// ---------------------------------------------------------------------------
// Sending a verification request
// ---------------------------------------------------------------------------

/// One send case: the Region the call is made in, the email address that
/// Region's contact method was registered with, and whether the Region also
/// holds an SMS contact method the email verification must not touch.
///
/// The rows differ along everything the rule generalises over: the Region the
/// call is made in — including a request that carries no credential scope at all
/// — the destination the addressed contact method carries, and whether the same
/// Region holds a contact method of another protocol.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    /// The credential scope's Region, or `None` for a request that carries no
    /// scope at all — served in [`DEFAULT_REGION`].
    region: Option<&'static str>,
    /// The Region the addressed contact method, and the operation the send
    /// answers, must report.
    reported_region: &'static str,
    email: &'static str,
    /// An SMS contact method registered in the same Region first, as the phone
    /// number it carries. `None` leaves the Region holding the email contact
    /// method alone.
    sms: Option<&'static str>,
}

/// The table. `us-west-2` is one of the Regions the modelled
/// `CreateContactMethodRequest$protocol` documentation lists as serving the SMS
/// protocol, so it is the row that can hold a second contact method.
const ROWS: &[Row] = &[
    Row {
        case: "a request with no credential scope",
        region: None,
        reported_region: DEFAULT_REGION,
        email: "ops@example.com",
        sms: None,
    },
    Row {
        case: "a region that also holds an SMS contact method",
        region: Some("us-west-2"),
        reported_region: "us-west-2",
        email: "first.last+lightsail@mail.eu.example.co.uk",
        sms: Some("+12065550100"),
    },
    Row {
        case: "a region on another continent",
        region: Some("ap-south-1"),
        reported_region: "ap-south-1",
        email: "alerts@example.org",
        sms: None,
    },
];

/// RULE: `SendContactMethodVerification` sends a fresh verification request to
/// the contact method the requested protocol addresses in the request's own
/// Region — answering one terminal operation-log entry that names that contact
/// method, leaving it awaiting verification without re-creating it, and leaving
/// every other Region's and every other protocol's contact method alone.
#[tokio::test]
async fn sending_a_verification_request_answers_the_regions_own_email_contact_method() {
    let emulator = Emulator::new();
    for row in ROWS {
        create(&emulator, row.region, "Email", row.email).await;
        if let Some(number) = row.sms {
            create(&emulator, row.region, "SMS", number).await;
        }
    }

    // Every operation this suite is handed, so no two calls answer one entry.
    let mut operation_ids = BTreeSet::new();
    for row in ROWS {
        let case = row.case;
        let before = contact_method(&emulator, row.region, "Email", case).await;
        let name = before["name"].as_str().unwrap_or_default().to_string();

        // "Use this action to send another verification request if a previous
        // verification request was deleted, or has expired": a second send is
        // the same call again, and answers its own operation-log entry.
        for attempt in 1..=2 {
            let case = format!("{case}, attempt {attempt}");
            let sent = emulator
                .ok_at(
                    None,
                    row.region,
                    "SendContactMethodVerification",
                    json!({"protocol": "Email"}),
                )
                .await;
            let operations = sent["operations"].as_array().unwrap_or_else(|| {
                panic!("case {case}: expected an operations array, got {sent:?}")
            });
            assert_eq!(
                operations.len(),
                1,
                "case {case}: expected exactly one operation, got {sent:?}"
            );
            let operation = &operations[0];
            for (member, want) in [
                ("operationType", "SendContactMethodVerification"),
                ("resourceType", "ContactMethod"),
                ("resourceName", &name[..]),
                ("status", "Succeeded"),
            ] {
                assert_eq!(
                    operation[member].as_str(),
                    Some(want),
                    "case {case}: expected {member} {want}, got {operation:?}"
                );
            }
            assert_eq!(
                operation["isTerminal"].as_bool(),
                Some(true),
                "case {case}: the verification request has gone out when the call answers, \
                 got {operation:?}"
            );
            assert_eq!(
                operation["location"]["regionName"].as_str(),
                Some(row.reported_region),
                "case {case}: expected the operation in the contact method's own region, \
                 got {operation:?}"
            );
            let id = operation["id"].as_str().unwrap_or_default();
            assert!(
                is_uuid_shaped(id),
                "case {case}: expected a uuid-shaped operation id, got {operation:?}"
            );
            assert!(
                operation_ids.insert(id.to_string()),
                "case {case}: operation id {id} was already handed to another call"
            );
        }

        // The send does not re-create the contact method: its identity and its
        // destination survive, and a verification request is outstanding.
        let after = contact_method(&emulator, row.region, "Email", case).await;
        for member in ["arn", "supportCode", "createdAt", "contactEndpoint", "name"] {
            assert_eq!(
                before[member], after[member],
                "case {case}: expected {member} to survive the send, got {:?} then {:?}",
                before[member], after[member]
            );
        }
        assert_eq!(
            after["status"].as_str(),
            Some(PENDING_VERIFICATION),
            "case {case}: expected the contact method to await verification, got {after:?}"
        );

        // A contact method of another protocol is a different record, and an
        // email verification is not sent to it: an SMS one needs no verifying
        // and stays as it was created.
        if row.sms.is_some() {
            let sms = contact_method(&emulator, row.region, "SMS", case).await;
            assert_eq!(
                sms["status"].as_str(),
                Some(VALID),
                "case {case}: expected the SMS contact method to stay valid, got {sms:?}"
            );
        }
    }

    // Each Region still holds its own destination: a send in one Region is not
    // a send in another.
    for row in ROWS {
        let method = contact_method(&emulator, row.region, "Email", row.case).await;
        assert_eq!(
            method["contactEndpoint"].as_str(),
            Some(row.email),
            "case {}: expected the region's own destination, got {method:?}",
            row.case
        );
    }
}

/// RULE, the other branch: a contact method that HAS been verified stays
/// verified — sending another verification request does not un-verify it —
/// while the call still answers its own terminal operation-log entry.
#[tokio::test]
async fn sending_a_verification_to_a_verified_contact_method_leaves_it_valid() {
    let case = "a verified email contact method";
    let emulator = Emulator::new();
    create(&emulator, None, "Email", "ops@example.com").await;
    // A verification is completed out of band — the holder of the address
    // follows a link Lightsail mails them, and this emulator sends no mail — so
    // the verified record this case is about is seeded through the service's own
    // public state API, in the one store its handlers read, rather than a copy.
    lightsail_state::with_state(emulator.engine(), |store| {
        store
            .contact_method_mut(DEFAULT_REGION, "Email")
            .expect("the suite created this contact method through CreateContactMethod")
            .status = lightsail_state::CONTACT_METHOD_STATUS_VALID.to_string();
    })
    .expect("the suite's engine is the shared in-memory one");

    let before = contact_method(&emulator, None, "Email", case).await;
    assert_eq!(
        before["status"].as_str(),
        Some(VALID),
        "case {case}: the seeded record must read back verified, got {before:?}"
    );

    let sent = emulator
        .ok(
            "SendContactMethodVerification",
            json!({"protocol": "Email"}),
        )
        .await;
    let operations = sent["operations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected an operations array, got {sent:?}"));
    assert_eq!(
        operations.len(),
        1,
        "case {case}: expected exactly one operation, got {sent:?}"
    );
    assert_eq!(
        (
            operations[0]["operationType"].as_str(),
            operations[0]["resourceName"].as_str(),
            operations[0]["isTerminal"].as_bool(),
        ),
        (
            Some("SendContactMethodVerification"),
            before["name"].as_str(),
            Some(true),
        ),
        "case {case}: expected one terminal entry naming the contact method, got {:?}",
        operations[0]
    );

    let after = contact_method(&emulator, None, "Email", case).await;
    assert_eq!(
        after["status"].as_str(),
        Some(VALID),
        "case {case}: a verified contact method stays verified, got {after:?}"
    );
    for member in ["arn", "supportCode", "createdAt", "contactEndpoint", "name"] {
        assert_eq!(
            before[member], after[member],
            "case {case}: expected {member} to survive the send, got {:?} then {:?}",
            before[member], after[member]
        );
    }
}

/// One verification case that addresses nothing: which Region the call is made
/// in, and what that Region holds instead.
#[derive(Debug, Clone, Copy)]
struct Missing {
    case: &'static str,
    region: Option<&'static str>,
    /// A contact method to register in `region` first, as `(protocol,
    /// endpoint)`, so the refusal is about the PROTOCOL rather than about an
    /// empty Region.
    holds: Option<(&'static str, &'static str)>,
}

/// The Regions that hold no email contact method: one holding nothing at all,
/// one holding only an SMS contact method, and a request with no credential
/// scope at all while the email contact method lives elsewhere.
const MISSING: &[Missing] = &[
    Missing {
        case: "a region that holds no contact method at all",
        region: Some("us-east-2"),
        holds: None,
    },
    Missing {
        case: "a region that holds only an SMS contact method",
        region: Some("us-west-2"),
        holds: Some(("SMS", "+12065550100")),
    },
    Missing {
        case: "a request with no credential scope while another region holds the email method",
        region: None,
        holds: None,
    },
];

/// RULE: a verification request can only go to a contact method that exists, so
/// a protocol the request's own Region holds no contact method for is refused
/// with Lightsail's `DoesNotExist` envelope and stores nothing.
#[tokio::test]
async fn sending_a_verification_request_for_a_protocol_the_region_lacks_is_refused() {
    let emulator = Emulator::new();
    // The email contact method lives in a Region none of the cases call in.
    create(&emulator, Some("eu-west-1"), "Email", "ops@example.com").await;

    for missing in MISSING {
        let case = missing.case;
        if let Some((protocol, endpoint)) = missing.holds {
            create(&emulator, missing.region, protocol, endpoint).await;
        }
        let before = contact_methods(&emulator, missing.region).await;

        let (status, body) = emulator
            .call_in(
                missing.region,
                "SendContactMethodVerification",
                json!({"protocol": "Email"}),
            )
            .await;
        // Lightsail answers a resource it cannot find with HTTP 400, the
        // `NotFoundException` shape and `DoesNotExist` in the body's `code`.
        assert_eq!(
            status, 400,
            "case {case}: expected the send to be refused with 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case}: expected the NotFoundException envelope, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case {case}: expected the DoesNotExist code, got {body:?}"
        );
        let after = contact_methods(&emulator, missing.region).await;
        assert_eq!(
            before, after,
            "case {case}: expected the refusal to store nothing, got {before:?} then {after:?}"
        );
    }

    // The contact method the other Region holds is untouched by every refusal.
    let elsewhere = contact_method(
        &emulator,
        Some("eu-west-1"),
        "Email",
        "the other region's method",
    )
    .await;
    assert_eq!(
        elsewhere["status"].as_str(),
        Some(PENDING_VERIFICATION),
        "case the other region's method: expected it to be unchanged, got {elsewhere:?}"
    );
}

/// RULE: the protocols a verification may be sent for are exactly the modelled
/// `ContactMethodVerificationProtocol` members (`Email` alone — "SMS contact
/// methods don't need to be verified"), so any other value is refused by the
/// boundary walk before the handler runs and nothing is written.
#[tokio::test]
async fn a_verification_protocol_the_model_does_not_admit_is_refused() {
    let emulator = Emulator::new();
    create(&emulator, None, "SMS", "+12065550100").await;

    // `SMS` is a `ContactProtocol` a contact method can be created under but not
    // a `ContactMethodVerificationProtocol`; the rest are outside both.
    for protocol in ["SMS", "Slack", "email", ""] {
        let case = format!("verifying the {protocol:?} protocol");
        let (status, body) = emulator
            .call(
                "SendContactMethodVerification",
                json!({"protocol": protocol}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the request to be refused with 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: expected the boundary walk's ValidationException, got {body:?}"
        );
    }

    let sms = contact_method(&emulator, None, "SMS", "the SMS method").await;
    assert_eq!(
        sms["status"].as_str(),
        Some(VALID),
        "case the SMS method: expected a refused verification to change nothing, got {sms:?}"
    );
}
