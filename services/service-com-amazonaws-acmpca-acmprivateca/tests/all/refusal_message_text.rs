// SPDX-License-Identifier: CC0-1.0

//! The prose this service refuses with, for the two refusal classes whose wording a
//! caller discriminates on, driven over the wire through the same router the emulator
//! boots.
//!
//! The rules under test, neither of them about one particular request:
//!
//! * a mutation refused because the private CA it names is soft-deleted answers ONE
//!   sentence of its own — the CA is in the DELETED state and has to be restored before
//!   the action can complete — whichever mutation it was, whichever status the CA was
//!   deleted from and whatever restoration window the deletion opened; it is not the
//!   sentence `UpdateCertificateAuthority` refuses the same deleted CA with, because the
//!   two are different refusals under one error code and a caller tells them apart by the
//!   text;
//! * a pagination token a listing did not issue is refused with the service's own
//!   sentence for that class, whose only variable part is the operation that mints the
//!   listing's tokens — not the error shape's API-reference description, and never the
//!   rejected token echoed back.
//!
//! Every message below is FIXED text for its class: it carries no ARN, id, timestamp or
//! day count, so each row of each table expects the same sentence.
//!
//! The harness (the engine, the router, the awsJson1_1 call) is
//! [`crate::private_ca_lifecycle::Emulator`].

use serde_json::{json, Value};
use service_com_amazonaws_acmpca_acmprivateca::state::{
    encode_page_token, STATUS_DELETED, STATUS_DISABLED, STATUS_EXPIRED, STATUS_PENDING_CERTIFICATE,
};

use crate::delete_certificate_authority::force_status;
use crate::private_ca_lifecycle::{root_ca_request, string, wait_for_the_clock, Emulator, ACCOUNT};
use crate::resource_policy::error_message;

/// What Amazon Web Services Private CA answers a mutation that names a soft-deleted CA.
const DELETED_STATE_MESSAGE: &str =
    "The certificate authority is in the DELETED state and must be restored to complete \
     this action.";

/// What it answers an `UpdateCertificateAuthority` of a CA whose status does not allow an
/// update — the sibling refusal of the one above: same error code, same HTTP status, its
/// own sentence.
const UPDATE_STATE_MESSAGE: &str =
    "The certificate authority must be in the ACTIVE or DISABLED state to be updated";

/// What it answers a `NextToken` the listing paged by `operation` never minted.
fn invalid_next_token_message(operation: &str) -> String {
    format!("The NextToken you provided is invalid. Use a token returned by {operation}.")
}

/// The tag every CA in this suite is created with, so a refused write is observably a
/// write that did not happen.
const BORN_WITH: (&str, &str) = ("owner", "platform");

/// A resource-based policy of `arn` that `PutPolicy` would accept on a CA that is not
/// deleted, so a refusal of it is about the CA's state and nothing else.
fn policy_of(arn: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"AWS": ACCOUNT},
            "Action": "acm-pca:IssueCertificate",
            "Resource": arn
        }]
    })
    .to_string()
}

/// A CA created through the wire carrying [`BORN_WITH`], named after `label`.
async fn created_ca(emulator: &Emulator, case: &str, label: &str) -> String {
    let mut body = root_ca_request(label);
    body["Tags"] = json!([{"Key": BORN_WITH.0, "Value": BORN_WITH.1}]);
    let created = emulator
        .ok(case, "CreateCertificateAuthority", &body)
        .await;
    string(&created, "CertificateAuthorityArn", case)
}

/// The (key, value) pairs `ListTags` reports for `arn`.
async fn listed_tags(emulator: &Emulator, case: &str, arn: &str) -> Vec<(String, String)> {
    let answer = emulator
        .ok(case, "ListTags", &json!({"CertificateAuthorityArn": arn}))
        .await;
    answer
        .get("Tags")
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
        .iter()
        .map(|tag| {
            (
                string(tag, "Key", case),
                tag.get("Value")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// The request body `operation` carries for the CA at `arn`.
fn mutation_request(operation: &str, arn: &str) -> Value {
    match operation {
        "TagCertificateAuthority" => json!({
            "CertificateAuthorityArn": arn,
            "Tags": [{"Key": "stage", "Value": "refused"}],
        }),
        "UntagCertificateAuthority" => json!({
            "CertificateAuthorityArn": arn,
            "Tags": [{"Key": BORN_WITH.0, "Value": BORN_WITH.1}],
        }),
        "PutPolicy" => json!({"ResourceArn": arn, "Policy": policy_of(arn)}),
        other => panic!("no request shape for {other}"),
    }
}

/// Every mutation this emulator refuses against a soft-deleted CA, crossed with the
/// status the CA was deleted from and the restoration window the deletion opened: one
/// sentence for all of them, and the CA left exactly as it was.
#[tokio::test]
async fn a_mutation_against_a_soft_deleted_ca_is_refused_with_the_restore_sentence() {
    let emulator = Emulator::new();

    // The mutations that write to what a CA holds and so are gated on its not being
    // deleted (`is_deleted` in `src/ops/`).
    let mutations = [
        "TagCertificateAuthority",
        "UntagCertificateAuthority",
        "PutPolicy",
    ];
    // Statuses a deletion is allowed from and that open a restoration window
    // (`DELETABLE_STATUSES` minus CREATING and FAILED, which assign none).
    let deleted_from = [STATUS_PENDING_CERTIFICATE, STATUS_DISABLED, STATUS_EXPIRED];
    // `PermanentDeletionTimeInDays` is `@range(7..30)`: both published bounds and the
    // absent member the 30-day default covers.
    let windows = [None, Some(7), Some(30)];

    // Every CA is created first, then settled by ONE wait for the clock.
    let mut rows: Vec<(String, &str, String, Option<i32>, &str)> = Vec::new();
    for (index, operation) in mutations.iter().enumerate() {
        for (offset, status) in deleted_from.iter().enumerate() {
            let window = windows[(index + offset) % windows.len()];
            let case = format!("{operation} on a CA deleted from {status} with {window:?} days");
            let label = format!("deleted-{index}-{offset}");
            let arn = created_ca(&emulator, &case, &label).await;
            rows.push((case, operation, arn, window, status));
        }
    }
    wait_for_the_clock();

    for (case, operation, arn, window, status) in &rows {
        let case = case.as_str();
        // The CA reaches the status the row deletes it from: PENDING_CERTIFICATE is where
        // the clock has already left it, the other two are reached through the store the
        // handlers read, since the operation that moves a CA into them is a stub.
        if *status != STATUS_PENDING_CERTIFICATE {
            force_status(&emulator, arn, status, case);
        }

        let mut delete = json!({"CertificateAuthorityArn": arn});
        if let Some(days) = window {
            delete["PermanentDeletionTimeInDays"] = json!(days);
        }
        emulator.ok(case, "DeleteCertificateAuthority", &delete).await;
        let described = emulator
            .ok(
                case,
                "DescribeCertificateAuthority",
                &json!({"CertificateAuthorityArn": arn}),
            )
            .await;
        let described = described
            .get("CertificateAuthority")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: no CertificateAuthority in {described:?}"));
        assert_eq!(
            string(&described, "Status", case),
            STATUS_DELETED,
            "case {case:?}: the row needs a soft-deleted CA, got {described:?}"
        );

        let body = emulator
            .refused(
                case,
                operation,
                &mutation_request(operation, arn),
                "InvalidStateException",
            )
            .await;
        let message = error_message(&body, case);
        assert_eq!(
            message, DELETED_STATE_MESSAGE,
            "case {case:?}: expected {DELETED_STATE_MESSAGE:?}, got {message:?}"
        );

        // The refusal wrote nothing: the CA still holds exactly the tag it was born with.
        let tags = listed_tags(&emulator, case, arn).await;
        assert_eq!(
            tags,
            vec![(BORN_WITH.0.to_string(), BORN_WITH.1.to_string())],
            "case {case:?}: a refused mutation must leave the CA as it was, got {tags:?}"
        );

        // The sibling refusal under the same error code and status keeps its own
        // sentence: an update of the same deleted CA is refused about the statuses an
        // update is allowed from, not about the restore.
        let body = emulator
            .refused(
                case,
                "UpdateCertificateAuthority",
                &json!({"CertificateAuthorityArn": arn, "Status": STATUS_DISABLED}),
                "InvalidStateException",
            )
            .await;
        let sibling = error_message(&body, case);
        assert_eq!(
            sibling, UPDATE_STATE_MESSAGE,
            "case {case:?}: expected the update-from-this-status sentence, got {sibling:?}"
        );
        assert_ne!(
            sibling, DELETED_STATE_MESSAGE,
            "case {case:?}: two refusals under one error code must not share one sentence"
        );
    }
}

/// Every paginated listing of this service, crossed with the ways a `NextToken` fails to
/// be one of its cursors: one sentence per listing, naming the operation whose tokens
/// resume it, with the rejected token never echoed.
#[tokio::test]
async fn a_next_token_a_listing_never_minted_is_refused_with_the_services_own_sentence() {
    let emulator = Emulator::new();
    let case = "the CA the listings page over";
    let arn = created_ca(&emulator, case, "invalid-next-token").await;
    // A tag and a grant so each listing has something to page, which is not what decides
    // the refusal: a token is judged before any of it is read.
    emulator
        .ok(
            case,
            "CreatePermission",
            &json!({
                "CertificateAuthorityArn": arn,
                "Principal": "acm.amazonaws.com",
                "Actions": ["IssueCertificate", "GetCertificate", "ListPermissions"],
            }),
        )
        .await;

    // The members each listing needs beside `NextToken`.
    let listings: [(&str, Value); 3] = [
        ("ListCertificateAuthorities", json!({})),
        (
            "ListTags",
            json!({"CertificateAuthorityArn": arn.clone()}),
        ),
        (
            "ListPermissions",
            json!({"CertificateAuthorityArn": arn.clone()}),
        ),
    ];

    // The rows differ along HOW the token fails to be a cursor of the listing it is
    // handed to: not this service's encoding at all, a well-formed base64 payload it
    // never minted, hex that is not a whole number of bytes, hex that decodes to no
    // cursor, a payload carrying no cursor separator, and a cursor of another listing.
    let tokens: [(&str, String); 6] = [
        ("a token that is not this service's encoding", "not-a-token-at-all".to_string()),
        (
            "a base64 payload the service never minted",
            aws_smithy_types::base64::encode("behavioral-acm-pca-bogus-cursor".as_bytes()),
        ),
        ("a token of odd hex length", "abc".to_string()),
        ("hex that decodes to no cursor at all", "deadbeef".to_string()),
        ("a payload carrying no cursor separator", hex::encode("certificate-authorities")),
        ("a cursor of another listing", encode_page_token("another-listing", &arn)),
    ];

    for (operation, members) in &listings {
        let expected = invalid_next_token_message(operation);
        for (token_case, token) in &tokens {
            let case = format!("{operation} with {token_case}");
            let mut body = members.clone();
            body["NextToken"] = json!(token);
            let answer = emulator
                .refused(&case, operation, &body, "InvalidNextTokenException")
                .await;
            let message = error_message(&answer, &case);
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
            assert!(
                !message.contains(token.as_str()),
                "case {case:?}: the rejected token must not be echoed, got {message:?}"
            );
            for member in ["CertificateAuthorities", "Tags", "Permissions", "NextToken"] {
                assert!(
                    answer.get(member).is_none(),
                    "case {case:?}: a refusal must report no page, got {answer:?}"
                );
            }
        }
    }
}
