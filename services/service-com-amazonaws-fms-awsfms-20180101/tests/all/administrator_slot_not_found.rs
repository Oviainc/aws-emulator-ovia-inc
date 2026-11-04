// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ONE not-found this service answers about the
//! caller's own Firewall Manager administrator association — the account's
//! single administrator slot, which no request carries a key for.
//!
//! The rule: where Firewall Manager reports that the caller's account has no
//! administrator association, the HTTP 400 `ResourceNotFoundException` carries NO
//! `Message` member at all, so a client reading the exception's message off the
//! body reads nothing; the service's constant not-found text belongs to the
//! lookups that NAME what they looked for (a policy id, a list id, a
//! resource-set id, a resource ARN, the administrators managing the caller).
//! That empty message is what tells "Firewall Manager onboarding never happened"
//! apart from "the resource you named is not there", and the two classes are
//! asserted here against each other.
//!
//! Asserted over a table rather than one request: the slot miss is driven in
//! several Regions and read twice in each, because the empty message is a
//! property of the error class and must not move with the request the way the
//! administrator-authorization refusal's message does (that one is asserted by
//! the `error_message_members` suite); and the contrast table spans the three
//! kinds of lookup key the keyed not-found is recorded on.
//!
//! The wire harness is `policy_lifecycle`'s, so these assertions are about the
//! same deserialize → handle → serialize walk a client drives.

use serde_json::{json, Value};
use service_com_amazonaws_fms_awsfms_20180101::state;

use crate::administrator_gated_operations::associate_admin;
use crate::error_message_members::message_member;
use crate::policy_lifecycle::{call, call_in, emulator, error_code, ACCOUNT, REGION};

/// Regions the slot miss is driven in: the suite's own Region, two others, and
/// the emulator's default, so an answer composed from the request could not pass
/// the same assertion in all four.
const REGIONS: &[&str] = &[REGION, "eu-central-1", "ap-southeast-4", "us-east-1"];

/// A `PolicyId` (`com.amazonaws.fms#PolicyId`: 36 characters matching
/// `^[a-z0-9A-Z-]{36}$`) that names no policy.
const UNKNOWN_POLICY_ID: &str = "3f5e2a71-9c84-4a1b-bd06-7e215c9f4a83";

/// A `ListId` (`com.amazonaws.fms#ListId`: 36 characters matching
/// `^[a-z0-9A-Z-]{36}$`) that names no list.
const UNKNOWN_LIST_ID: &str = "cccccccc-dddd-eeee-ffff-000000000000";

/// A `Base62Id` (`com.amazonaws.fms#Base62Id`: 22 characters matching
/// `^[a-z0-9A-Z]{22}$`) that names no resource set.
const UNKNOWN_RESOURCE_SET_ID: &str = "QwErTyUiOpAsDfGhJkLzXc";

/// An `fms` ARN that names no stored resource.
fn unknown_arn(resource: &str) -> String {
    format!("arn:aws:fms:{REGION}:{ACCOUNT}:{resource}")
}

/// Assert `body` is the modelled not-found with NO message content, under any
/// spelling of the member — which is what botocore's
/// `response["Error"].get("Message", "")` reads as the empty string.
fn assert_not_found_without_message(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(body),
        "ResourceNotFoundException",
        "case {case}: expected the modelled not-found error, got {body}"
    );
    assert_eq!(
        message_member(body),
        None,
        "case {case}: the administrator-slot miss carries no message, got {body}"
    );
    assert!(
        body.get("message").is_none(),
        "case {case}: the body must carry no message member under any spelling, got {body}"
    );
    assert!(
        !body
            .to_string()
            .contains(state::RESOURCE_NOT_FOUND_MESSAGE),
        "case {case}: the keyed-lookup not-found text must not reach this error, got {body}"
    );
}

/// With nothing associated, `GetAdminAccount`'s not-found carries no message —
/// in every Region, and on every read.
#[tokio::test]
async fn the_administrator_slot_miss_answers_a_not_found_with_no_message() {
    for region in REGIONS {
        // One emulator per Region, so each case really starts from an account
        // that has never associated an administrator.
        let router = emulator();
        for round in 1..=2 {
            let (status, body) = call_in(&router, "GetAdminAccount", &json!({}), region).await;
            assert_not_found_without_message(
                &format!("GetAdminAccount in {region}, read {round}"),
                status,
                &body,
            );
        }
    }
}

/// The empty message is specific to the administrator slot: in the SAME store,
/// every lookup that names what it looked for still answers the service's
/// constant not-found text, and never the looked-up key.
#[tokio::test]
async fn a_lookup_that_names_its_target_keeps_the_constant_not_found_text() {
    let router = emulator();
    let (status, body) = call(&router, "GetAdminAccount", &json!({})).await;
    assert_not_found_without_message("GetAdminAccount with no association", status, &body);

    // The three kinds of lookup key the keyed not-found is recorded on — a
    // policy UUID, a list/resource-set id and a resource ARN — plus the
    // keyless-but-plural listing of the administrators managing the caller,
    // whose recorded not-found carries that same text.
    let named: Vec<(&str, &str, Value, Option<String>)> = vec![
        (
            "GetPolicy with an unknown policy id",
            "GetPolicy",
            json!({ "PolicyId": UNKNOWN_POLICY_ID }),
            Some(UNKNOWN_POLICY_ID.to_string()),
        ),
        (
            "ListTagsForResource with an applications-list ARN naming nothing",
            "ListTagsForResource",
            json!({ "ResourceArn": unknown_arn(&format!("applications-list/{UNKNOWN_LIST_ID}")) }),
            Some(unknown_arn(&format!(
                "applications-list/{UNKNOWN_LIST_ID}"
            ))),
        ),
        (
            "ListTagsForResource with a resource-set ARN naming nothing",
            "ListTagsForResource",
            json!({ "ResourceArn": unknown_arn(&format!("resource-set/{UNKNOWN_RESOURCE_SET_ID}")) }),
            Some(unknown_arn(&format!(
                "resource-set/{UNKNOWN_RESOURCE_SET_ID}"
            ))),
        ),
        (
            "ListAdminsManagingAccount when no administrator manages the caller",
            "ListAdminsManagingAccount",
            json!({}),
            None,
        ),
    ];

    for (case, operation, request, lookup) in named {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case}: expected the modelled not-found error, got {body}"
        );
        assert_eq!(
            message_member(&body),
            Some(state::RESOURCE_NOT_FOUND_MESSAGE),
            "case {case}: expected the constant not-found text, got {body}"
        );
        if let Some(key) = lookup {
            let message = message_member(&body).unwrap_or_default();
            assert!(
                !message.contains(key.as_str()),
                "case {case}: the message must not restate the looked-up key {key:?}, \
                 got {message:?}"
            );
        }
    }
}

/// Once an administrator IS associated the slot is a resource again, so the same
/// request is answered rather than refused — and reading it twice answers
/// identically, so nothing about the empty-message path is a one-shot.
#[tokio::test]
async fn an_associated_administrator_is_reported_instead_of_the_empty_message_refusal() {
    let router = emulator();
    associate_admin(&router).await;

    let mut answers: Vec<Value> = Vec::new();
    for round in 1..=2 {
        let (status, body) = call(&router, "GetAdminAccount", &json!({})).await;
        assert_eq!(
            status, 200,
            "case GetAdminAccount after the association, read {round}: expected 200, \
             got {status} with {body}"
        );
        assert_eq!(
            body.get("AdminAccount").and_then(Value::as_str),
            Some(ACCOUNT),
            "case GetAdminAccount after the association, read {round}: expected the \
             associated account, got {body}"
        );
        assert!(
            body.get("RoleStatus")
                .and_then(Value::as_str)
                .is_some_and(|status| !status.is_empty()),
            "case GetAdminAccount after the association, read {round}: expected a role \
             status, got {body}"
        );
        answers.push(body);
    }
    assert_eq!(
        answers[0], answers[1],
        "case GetAdminAccount read twice: a read must not change what the next caller \
         sees, got {:?} then {:?}",
        answers[0], answers[1]
    );
}
