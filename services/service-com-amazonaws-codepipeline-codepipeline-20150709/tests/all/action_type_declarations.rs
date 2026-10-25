// SPDX-License-Identifier: CC0-1.0

//! The wording of the `ActionTypeNotFoundException` the action-type DECLARATION
//! operations state — `GetActionType` and `UpdateActionType`, the two that address an
//! `ActionTypeDeclaration` — over the wire the service actually speaks (`awsJson1_1`,
//! `POST /` with `X-Amz-Target: CodePipeline_20150709.<Operation>`).
//!
//! The round trip, the Region a definition belongs to and the store's bound are pinned
//! by `action_type_definitions.rs`; what these cases are about is the REFUSAL: the HTTP
//! status and the error code are the contract, and so is the message, which AWS states
//! over the request's own id — naming each of the four members and closing with the
//! sentence that tells a caller an action type is not something an ordinary account
//! defines for itself — so a caller can tell from it which member it has to correct.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so nothing here can pass by
//! reading a handler's internals.

use serde_json::{json, Value};

use service_com_amazonaws_codepipeline_codepipeline_20150709::ops::types::ActionTypeId;
use service_com_amazonaws_codepipeline_codepipeline_20150709::state::action_type_declaration_not_found_message;

use crate::pipeline_lifecycle::Emulator;

/// The operations under test: the two that address an action type's DEFINITION.
const GET: &str = "GetActionType";
const UPDATE: &str = "UpdateActionType";

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

        // The one template every operation that addresses an action type's DEFINITION
        // states a missing one with, applied to this request.
        let expected = action_type_declaration_not_found_message(requested);
        let message = response.get("message").and_then(Value::as_str);
        assert_eq!(
            message,
            Some(expected.as_str()),
            "case {case:?}: expected the refusal {expected:?}, got {response}"
        );

        // Built from the request: it names each member the caller asked about, so no
        // two rows share a refusal, and it closes with the sentence that says an
        // action type is not something an ordinary account defines for itself.
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
            expected.ends_with("To create an action type, contact the CodePipeline service team."),
            "case {case:?}: expected the refusal to close with the service-team sentence, \
             got {expected:?}"
        );
    }

    // No two identities are refused with the same text, which is what "built from the
    // request" means.
    let mut messages: Vec<String> = cases
        .iter()
        .map(action_type_declaration_not_found_message)
        .collect();
    let total = messages.len();
    messages.sort();
    messages.dedup();
    assert_eq!(
        messages.len(),
        total,
        "expected one refusal per identity, got {messages:?}"
    );
}

/// RULE: the wording belongs to the ERROR CLASS, so `UpdateActionType` states a
/// missing action type exactly as `GetActionType` does — over the request's own
/// `actionType.id`, in AWS's `owner`/`category`/`provider`/`version` order, with the
/// service-team sentence.
///
/// The rows differ along every member the rule generalises over: each `ActionCategory`
/// the model declares a provider for here, both owners `ActionTypeIdentifier$owner`
/// admits, and providers and versions at the ends of their modelled `@length`.
///
/// (WHICH identities the operation admits at all is `action_type_definitions.rs`; what
/// these rows pin is the wording of the refusal.)
#[tokio::test]
async fn update_action_type_states_a_missing_action_type_the_same_way() {
    let emulator = Emulator::new();

    let cases = [
        id("Build", "ThirdParty", "AbsentBuild", "1"),
        id("Source", "AWS", "AbsentSource", "1"),
        id("Deploy", "ThirdParty", "A", "123456789"),
        id("Test", "AWS", "Absent-Test_9", "2a"),
        id("Invoke", "ThirdParty", "Absent-Provider-0123456789012345678", "1"),
    ];

    for requested in &cases {
        let case = format!(
            "{}/{}/{}/{}",
            requested.category, requested.owner, requested.provider, requested.version
        );
        // The smallest `ActionTypeDeclaration` the boundary validator accepts: the
        // identity, an executor and both artifact-count bounds.
        let declaration = json!({
            "id": {
                "category": requested.category,
                "owner": requested.owner,
                "provider": requested.provider,
                "version": requested.version,
            },
            "executor": {
                "type": "JobWorker",
                "configuration": {
                    "jobWorkerExecutorConfiguration": {
                        "pollingAccounts": ["123456789012"],
                    },
                },
            },
            "inputArtifactDetails": {"minimumCount": 0, "maximumCount": 1},
            "outputArtifactDetails": {"minimumCount": 0, "maximumCount": 1},
        });
        let (status, response) = emulator
            .call(UPDATE, &json!({"actionType": declaration}))
            .await;
        assert_eq!(
            (status, response.get("__type").and_then(Value::as_str)),
            (NOT_FOUND_STATUS, Some(NOT_FOUND)),
            "case {case:?}: expected a {NOT_FOUND_STATUS} {NOT_FOUND}, got {status} {response}"
        );

        let expected = action_type_declaration_not_found_message(requested);
        assert_eq!(
            response.get("message").and_then(Value::as_str),
            Some(expected.as_str()),
            "case {case:?}: expected the refusal {expected:?}, got {response}"
        );

        // The same condition, through the other operation of the pair: one wording.
        let (status, read) = get(&emulator, requested).await;
        assert_eq!(
            (status, read.get("message").and_then(Value::as_str)),
            (NOT_FOUND_STATUS, Some(expected.as_str())),
            "case {case:?}: both declaration operations state it the same way, got {read}"
        );
    }
}
