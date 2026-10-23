// SPDX-License-Identifier: CC0-1.0

//! The wording of `GetActionType`'s `ActionTypeNotFoundException`, over the wire the
//! service actually speaks (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: CodePipeline_20150709.<Operation>`).
//!
//! The round trip, the Region a definition belongs to and the store's bound are pinned
//! by `action_type_definitions.rs`; what these cases are about is the REFUSAL: the HTTP
//! status and the error code are the contract, and the message the service states is
//! built from the request rather than naming one particular action type, so a caller
//! can tell from it which of the four identifying members it has to correct.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so nothing here can pass by
//! reading a handler's internals.

use serde_json::{json, Value};

use service_com_amazonaws_codepipeline_codepipeline_20150709::ops::types::ActionTypeId;
use service_com_amazonaws_codepipeline_codepipeline_20150709::state::action_type_not_found_message;

use crate::pipeline_lifecycle::{Emulator, ACCOUNT_ID};

/// The operation under test.
const GET: &str = "GetActionType";

/// The error this operation models for an action type it holds no definition for
/// ("The specified action type cannot be found"), with the status the model defaults
/// it to.
const NOT_FOUND: &str = "ActionTypeNotFoundException";
const NOT_FOUND_STATUS: u16 = 400;

/// The identity a request names an action type by, as `ActionTypeId` declares the four
/// members.
fn id(category: &str, owner: &str, provider: &str, version: &str) -> ActionTypeId {
    ActionTypeId {
        category: category.to_string(),
        owner: owner.to_string(),
        provider: provider.to_string(),
        version: version.to_string(),
    }
}

/// `GetActionType` for the four members `id` names.
async fn get(emulator: &Emulator, id: &ActionTypeId) -> (u16, Value) {
    emulator
        .call(
            GET,
            &json!({
                "category": id.category,
                "owner": id.owner,
                "provider": id.provider,
                "version": id.version,
            }),
        )
        .await
}

/// RULE: an identity no definition is held for is refused with the modelled not-found
/// error, and the refusal is stated over the REQUEST's own four members — so the same
/// rule produces a different text for every identity rather than one canned sentence.
#[tokio::test]
async fn the_not_found_refusal_is_stated_over_the_requested_identity() {
    let emulator = Emulator::new();

    // The rows differ along every member the rule generalises over: each
    // `ActionCategory` the model declares, both owners `ActionTypeIdentifier$owner`
    // admits (`^AWS|ThirdParty$`), and providers and versions at the ends of their
    // modelled `@length` (`ActionProvider` 1..35, `Version` 1..9).
    let cases = [
        id("Source", "ThirdParty", "AbsentSource", "1"),
        id("Build", "AWS", "AbsentBuild", "123456789"),
        id("Deploy", "ThirdParty", "A", "1"),
        id("Test", "AWS", "Absent-Test_9", "2a"),
        id("Invoke", "ThirdParty", "Absent-Provider-0123456789012345678", "1"),
        id("Approval", "AWS", "AbsentApproval", "1"),
        id("Compute", "ThirdParty", "AbsentCompute", "9"),
    ];

    for requested in &cases {
        let case = format!(
            "{}/{}/{}/{}",
            requested.category, requested.owner, requested.provider, requested.version
        );
        let (status, response) = get(&emulator, requested).await;
        assert_eq!(
            (status, response.get("__type").and_then(Value::as_str)),
            (NOT_FOUND_STATUS, Some(NOT_FOUND)),
            "case {case:?}: expected a {NOT_FOUND_STATUS} {NOT_FOUND}, got {status} {response}"
        );

        // The one template every operation of this service states a missing action
        // type with, applied to this request.
        let expected = action_type_not_found_message(requested);
        let message = response.get("message").and_then(Value::as_str);
        assert_eq!(
            message,
            Some(expected.as_str()),
            "case {case:?}: expected the refusal {expected:?}, got {response}"
        );

        // Built from the request: it names each member the caller asked about, and the
        // account the lookup was scoped to, so no two rows share a refusal.
        for member in [
            &requested.category,
            &requested.owner,
            &requested.provider,
            &requested.version,
        ] {
            assert!(
                expected.contains(member.as_str()),
                "case {case:?}: expected the refusal to name {member:?}, got {expected:?}"
            );
        }
        assert!(
            expected.contains(ACCOUNT_ID),
            "case {case:?}: expected the refusal to name the account, got {expected:?}"
        );
    }

    // No two identities are refused with the same text, which is what "built from the
    // request" means.
    let mut messages: Vec<String> = cases.iter().map(action_type_not_found_message).collect();
    let total = messages.len();
    messages.sort();
    messages.dedup();
    assert_eq!(
        messages.len(),
        total,
        "expected one refusal per identity, got {messages:?}"
    );
}
