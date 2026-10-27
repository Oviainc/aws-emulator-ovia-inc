// SPDX-License-Identifier: CC0-1.0

//! Who may define an action type, over the wire the service actually speaks
//! (`awsJson1_1`, `POST /` with `X-Amz-Target: CodePipeline_20150709.<Operation>`).
//!
//! The rule under test: an action type's definition belongs to the account that
//! PROVIDES that action type, so `UpdateActionType` serves only the provider — the
//! partner the action type is registered to — and every other identity is refused with
//! `ActionTypeNotFoundException` carrying the one message the declaration operations
//! state a missing action type with, built from the request's own `actionType.id`.
//! Being in the catalog `ListActionTypes` reports is not provision: an action type the
//! SERVICE provides is as undefinable by a caller as an identity nothing provides at
//! all.
//!
//! The rows span what the rule generalises over: the owner (`AWS` and `ThirdParty`),
//! the category, whether the identity is in the catalog, whether anything provides it,
//! and which account provides it. `tests/all/action_type_definitions.rs` pins the
//! definition round trip and the store's bounds; what these cases add is the CODE and
//! the MESSAGE of the refusal for an action type that exists but is not the caller's,
//! and that the refusal leaves the catalog entry itself exactly as it was.

use serde_json::{json, Value};

use service_com_amazonaws_codepipeline_codepipeline_20150709::ops::types::ActionTypeId;
use service_com_amazonaws_codepipeline_codepipeline_20150709::state::{
    action_type_declaration_not_found_message, BUILT_IN_ACTION_TYPES,
};
use service_com_amazonaws_codepipeline_codepipeline_20150709::ComAmazonawsCodepipelineCodepipeline20150709State as ServiceState;

use crate::pipeline_lifecycle::{Emulator, ACCOUNT_ID};

/// The operation under test, and the read that reports what it stored.
const UPDATE: &str = "UpdateActionType";
const GET: &str = "GetActionType";

/// The error the operation models for an action type the caller has none of ("The
/// specified action type cannot be found"), with the status the model defaults it to.
const NOT_FOUND: &str = "ActionTypeNotFoundException";
const NOT_FOUND_STATUS: u16 = 400;

/// Another account, for the row an identity is provided by someone else in.
const OTHER_ACCOUNT: &str = "123456789012";

/// A `lambdaFunctionArn` satisfying `LambdaFunctionArn`'s modelled pattern.
const LAMBDA_ARN: &str = "arn:aws:lambda:us-east-1:123456789012:function:cp-action";

/// The smallest `ActionTypeDeclaration` the boundary validator accepts for `id`: the
/// identity, an executor carrying its own integration model's configuration, and both
/// artifact-count bounds.
fn declaration(id: &ActionTypeId) -> Value {
    json!({
        "id": {
            "category": id.category,
            "owner": id.owner,
            "provider": id.provider,
            "version": id.version,
        },
        "executor": {
            "type": "Lambda",
            "configuration": {
                "lambdaExecutorConfiguration": {"lambdaFunctionArn": LAMBDA_ARN},
            },
        },
        "inputArtifactDetails": {"minimumCount": 0, "maximumCount": 1},
        "outputArtifactDetails": {"minimumCount": 0, "maximumCount": 1},
    })
}

/// The four members a request names an action type by.
fn id(category: &str, owner: &str, provider: &str, version: &str) -> ActionTypeId {
    ActionTypeId {
        category: category.to_string(),
        owner: owner.to_string(),
        provider: provider.to_string(),
        version: version.to_string(),
    }
}

/// `UpdateActionType` for the definition of `id`.
async fn update(emulator: &Emulator, id: &ActionTypeId) -> (u16, Value) {
    emulator
        .call(UPDATE, &json!({"actionType": declaration(id)}))
        .await
}

/// `GetActionType` for the four members of `id`.
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

/// Register `account` as the partner that provides `id` — the registration AWS makes
/// when a partner onboards, which this API models no operation for — in the shared
/// store the handlers read. Every assertion still goes through the router.
fn register_partner(emulator: &Emulator, id: &ActionTypeId, account: &str) {
    assert!(
        emulator
            .engine()
            .state::<ServiceState>()
            .lock()
            .register_third_party_partner(id, account),
        "expected {}/{}/{}/{} to take a partner registration",
        id.category,
        id.owner,
        id.provider,
        id.version
    );
}

/// Whether `ListActionTypes` reports an action type with this identity.
async fn is_listed(emulator: &Emulator, id: &ActionTypeId) -> bool {
    let listed = emulator.ok("ListActionTypes", &json!({})).await;
    listed["actionTypes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .any(|action_type| {
            action_type["id"]
                == json!({
                    "category": id.category,
                    "owner": id.owner,
                    "provider": id.provider,
                    "version": id.version,
                })
        })
}

#[tokio::test]
async fn only_the_account_that_provides_an_action_type_can_define_it() {
    let emulator = Emulator::new();

    // One identity the catalog carries for each owner it publishes, so the rows differ
    // in the owner, the category and the provider: these are real, currently-listed
    // action types, and not one of them is the caller's to define.
    let catalog: Vec<ActionTypeId> = BUILT_IN_ACTION_TYPES
        .iter()
        .map(|entry| id(entry.category, entry.owner, entry.provider, entry.version))
        .collect();
    assert!(
        catalog.iter().any(|entry| entry.owner == "AWS")
            && catalog.iter().any(|entry| entry.owner == "ThirdParty"),
        "expected the catalog to publish both owners, got {catalog:?}"
    );

    // An identity another account provides, and one nothing provides at all.
    let elsewhere = id("Build", "ThirdParty", "Partner-Elsewhere", "1");
    register_partner(&emulator, &elsewhere, OTHER_ACCOUNT);
    let nowhere = id("Invoke", "ThirdParty", "Partner-Nowhere", "7");

    let mut refused: Vec<(String, ActionTypeId)> = catalog
        .iter()
        .map(|entry| (format!("the catalog's {}/{}", entry.owner, entry.provider), entry.clone()))
        .collect();
    refused.push(("an identity another account provides".to_string(), elsewhere));
    refused.push(("an identity nothing provides".to_string(), nowhere));

    for (case, requested) in &refused {
        let listed_before = is_listed(&emulator, requested).await;
        let (status, response) = update(&emulator, requested).await;
        assert_eq!(
            (status, response["__type"].as_str()),
            (NOT_FOUND_STATUS, Some(NOT_FOUND)),
            "case {case:?}: expected a {NOT_FOUND_STATUS} {NOT_FOUND}, got {status} {response}"
        );
        // The one wording the declaration operations state a missing action type with,
        // over the request's own four members.
        let expected = action_type_declaration_not_found_message(requested);
        assert_eq!(
            response["message"].as_str(),
            Some(expected.as_str()),
            "case {case:?}: expected the refusal {expected:?}, got {response}"
        );
        // That wording written out here, so a reword of the helper fails this suite
        // rather than following it.
        assert_eq!(
            expected,
            format!(
                "An action type with owner=[{}], category=[{}], provider=[{}] and \
                 version=[{}] doesn't exist. To create an action type, contact the \
                 CodePipeline service team.",
                requested.owner, requested.category, requested.provider, requested.version
            ),
            "case {case:?}: the refusal states what AWS states for this class"
        );

        // The refusal stored nothing, and it changed nothing about the action type
        // itself: a listed one is still listed.
        let (status, read) = get(&emulator, requested).await;
        assert_eq!(
            (status, read["__type"].as_str()),
            (NOT_FOUND_STATUS, Some(NOT_FOUND)),
            "case {case:?}: a refused definition is no definition, got {status} {read}"
        );
        assert_eq!(
            is_listed(&emulator, requested).await,
            listed_before,
            "case {case:?}: the refusal left the catalog as it was"
        );
    }

    // The same call for an action type the CALLER provides is served, and what it
    // stored is what the read reports: the refusals above are the entitlement and not
    // the operation.
    let own = id("Test", "ThirdParty", "Partner-Own", "1");
    register_partner(&emulator, &own, ACCOUNT_ID);
    let (status, answered) = update(&emulator, &own).await;
    assert_eq!(
        status, 200,
        "an action type the caller provides is definable, got {status} {answered}"
    );
    let (status, read) = get(&emulator, &own).await;
    assert_eq!(
        status, 200,
        "expected the stored definition, got {status} {read}"
    );
    assert_eq!(
        read["actionType"]["id"],
        json!({
            "category": own.category,
            "owner": own.owner,
            "provider": own.provider,
            "version": own.version,
        }),
        "expected the definition to be reported for the identity it was stored under, \
         got {read}"
    );
    // The declaration is reported as it was stated, member for member.
    assert_eq!(
        read["actionType"]["executor"]["configuration"]["lambdaExecutorConfiguration"]
            ["lambdaFunctionArn"],
        json!(LAMBDA_ARN),
        "expected the executor the definition stated, got {read}"
    );

    // And the identity is the key: the same provider at another version is still not
    // provided to this caller.
    let other_version = id(&own.category, &own.owner, &own.provider, "2");
    let (status, response) = update(&emulator, &other_version).await;
    assert_eq!(
        (status, response["message"].as_str()),
        (
            NOT_FOUND_STATUS,
            Some(action_type_declaration_not_found_message(&other_version).as_str())
        ),
        "a registration names one version, got {status} {response}"
    );
}
