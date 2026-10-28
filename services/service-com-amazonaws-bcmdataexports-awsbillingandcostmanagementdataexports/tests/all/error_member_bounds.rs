// SPDX-License-Identifier: CC0-1.0

//! The modelled bound of the AUTHORIZATION refusal, over the awsJson1_1 wire.
//!
//! RULE: `AccessDeniedException$Message` is the same modelled `GenericString`,
//! `@length(0, 1024)`, that every other refusal of this service is worded into
//! (`smithy_query(query="shape",shape="AccessDeniedException")`), while the sentence it
//! carries ECHOES the resource the decision was made about — a modelled `Arn` of up to
//! 2048 characters (`smithy_query(query="shape",shape="Arn")`), twice what the message
//! admits. So the echoed resource takes the room the clause around it leaves: the message
//! stays inside the bound, and what a refused caller needs — the principal, the action and
//! the reason it was refused — is still stated, because an outsized ARN is the caller's
//! value while the clause is what tells it what to grant.
//!
//! The decision is the real one: the principal is made with IAM's and STS's own operations
//! through `crate::front_door_authorization::Emulator`, so the policies this service reads
//! are the ones those services wrote. The rows differ along how long the echoed ARN is —
//! its published ceiling, one character past the message's bound, exactly at it, and an
//! ordinary one — which is what the rule generalises over.

use serde_json::json;

use crate::export_lifecycle::{error_code, padded_arn, MAX_ARN_CHARS};
use crate::front_door_authorization::Emulator;

/// The `@length` ceiling of a `GenericString`, which every message member of every error
/// shape of this service targets (smithy_query shape `GenericString`: `@length(min: 0,
/// max: 1024)`). Stated from the MODEL, so the expectation is not read from the code under
/// test.
const MESSAGE_MAX_CHARS: usize = 1024;

/// The resource segment of the ARN this suite asks about (see
/// [`crate::export_lifecycle::padded_arn`]).
const BOUNDED_ACCESS_DENIED_RESOURCE: &str = "export/bounded_access_denied";

/// The action `UntagResource` is judged on, which the refusal names.
const UNTAG_ACTION: &str = "bcm-data-exports:UntagResource";

/// A well-formed ARN of this account, `chars` characters long, naming no export.
///
/// Built by the shared [`crate::export_lifecycle::padded_arn`] builder, which pads inside
/// the RESOURCE segment, so it satisfies the modelled `Arn` pattern and the boundary
/// validator hands it to the handler.
fn bounded_arn(chars: usize) -> String {
    padded_arn(BOUNDED_ACCESS_DENIED_RESOURCE, chars)
}

#[tokio::test]
async fn an_authorization_refusal_echoing_an_outsized_arn_stays_within_the_bound() {
    let emulator = Emulator::new();
    // A principal no policy grants anything to, so every call below is refused on the
    // authorization layer rather than on anything about the resource.
    let bare = emulator
        .assumed_role_key("bcmde-bounded-access-denied", None)
        .await;

    for chars in [
        MAX_ARN_CHARS,
        MESSAGE_MAX_CHARS + 1,
        MESSAGE_MAX_CHARS,
        MESSAGE_MAX_CHARS - 1,
        90,
    ] {
        let arn = bounded_arn(chars);
        let case = format!("an ARN of {chars} characters");
        let (status, body) = emulator
            .call(
                &bare,
                "UntagResource",
                &json!({ "ResourceArn": arn, "ResourceTagKeys": ["team"] }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (403, "AccessDeniedException"),
            "case {case:?}: a caller with no grant must be refused, got {status} with {body}"
        );
        let message = body["Message"]
            .as_str()
            .or_else(|| body["message"].as_str())
            .unwrap_or_else(|| panic!("case {case:?}: the refusal must carry a Message: {body}"));
        assert!(
            message.chars().count() <= MESSAGE_MAX_CHARS,
            "case {case:?}: Message must satisfy the modelled @length(0, {MESSAGE_MAX_CHARS}), got {} characters: {message:?}",
            message.chars().count()
        );
        // What the caller acts on survives the bound: who was refused, what for, and why.
        assert!(
            message.contains("is not authorized to perform")
                && message.contains(UNTAG_ACTION)
                && message.contains(&format!("policy allows the {UNTAG_ACTION} action")),
            "case {case:?}: the refusal must still name the principal, the action and the reason, got {message:?}"
        );
        // An ARN the message had room for is echoed whole, so the bound costs nothing
        // where nothing had to give way.
        if message.chars().count() < MESSAGE_MAX_CHARS {
            assert!(
                message.contains(&arn),
                "case {case:?}: an ARN the sentence had room for must be echoed whole, got {message:?}"
            );
        }
    }
}
