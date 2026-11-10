// SPDX-License-Identifier: CC0-1.0

//! The two INDEPENDENT stages an identity source's Amazon Cognito user pool
//! reference is resolved in, and the sentence each of them is refused with.
//!
//! The rule under test, one sentence: a class of refusal this service words
//! ITSELF states that wording as the exception's `message`, so a `userPoolArn`
//! the model's own constraints refuse is answered with the constraint-violation
//! envelope naming the member and the requirement the value broke, while one
//! whose shape the model admits and that still resolves to no user pool is
//! answered with `User pool <poolId> does not exist.` — the pool id read out of
//! the request's own ARN — and neither of them with the service's generic
//! summary, which would leave the two indistinguishable.
//!
//! The rows differ along WHICH stage refuses them and why: the shape rows break
//! the member's `@pattern` in three different segments of the ARN or run one byte
//! past its `@length` maximum, and the existence rows carry an ARN the model
//! admits — including one padded to exactly that maximum, the published bound the
//! row past it fails — naming a pool nothing minted, a pool that was deleted, or
//! one whose id is mixed case. Every row is driven through BOTH writers of an
//! identity source, which spell the member that carries the configuration
//! differently, so the pointer the shape stage addresses follows the request
//! while the sentence the existence stage states does not.
//!
//! Every pool a row references is created — and for one row deleted — through the
//! Amazon Cognito user-pool service mounted on the same engine, so an accepted
//! reference names an id that service really minted.
//!
//! Every assertion goes over the wire through the shared [`super::harness`].

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_verifiedpermissions_verifiedpermissions::ops::INVALID_INPUT_MESSAGE;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, cognito, error_code, fault_message, field_list, ok, router, store_with_validation_off,
    user_pool, user_pool_arn, SIGNING_REGION,
};
use crate::identity_source_refusal_pointers::POOL_ARN_IN_CONFIGURATION;

/// `UserPoolArn`'s `@pattern`, as the model writes it — anchors included
/// (`smithy_query(query="shape", shape="UserPoolArn")`).
const USER_POOL_ARN_PATTERN: &str =
    r"^arn:[a-zA-Z0-9-]+:cognito-idp:(([a-zA-Z0-9-]+:\d{12}:userpool/[\w-]+_[0-9a-zA-Z]+))$";

/// `UserPoolArn`'s `@length` maximum (`smithy_query(query="shape",
/// shape="UserPoolArn")`).
const USER_POOL_ARN_MAX_LENGTH: usize = 255;

/// `CreateIdentitySourceInput`'s provider member.
const CONFIGURATION: &str = "/configuration";

/// `UpdateIdentitySourceInput`'s provider member.
const UPDATE_CONFIGURATION: &str = "/updateConfiguration";

/// The entity type every row names, so no row is refused for an absent
/// `principalEntityType` instead of the pool reference it is about.
const PRINCIPAL_ENTITY_TYPE: &str = "MyCorp::User";

/// A Cognito provider description naming `arn`, which is the only thing a row
/// varies.
fn cognito_provider(arn: &str) -> Value {
    json!({ "cognitoUserPoolConfiguration": { "userPoolArn": arn } })
}

/// A user pool ARN whose POOL ID is padded until the whole ARN is `bytes` long:
/// padding the id keeps the value inside the member's pattern, so the only
/// constraint such a value can break is the length maximum.
fn user_pool_arn_of(bytes: usize) -> String {
    let base = user_pool_arn(SIGNING_REGION, &format!("{SIGNING_REGION}_"));
    format!(
        "{base}{}",
        "a".repeat(bytes.saturating_sub(base.chars().count()))
    )
}

/// The pool id a user pool ARN carries, which is the only variable part of the
/// sentence the existence stage states.
fn pool_id_of(arn: &str) -> String {
    arn.rsplit_once("userpool/")
        .map(|(_, pool)| pool.to_string())
        .unwrap_or_default()
}

/// Delete a user pool through the service that owns it, so a reference to it
/// resolves to nothing afterwards.
async fn delete_user_pool(router: &MultiServiceRouter, pool: &str) {
    cognito(router, "DeleteUserPool", &json!({ "UserPoolId": pool })).await;
}

/// Which of the two stages refuses a row, and what that stage's sentence is
/// filled from. Written out HERE rather than derived from the code under test.
enum Stage {
    /// The model's own constraint on the member: the sentence is the
    /// constraint-violation envelope, stating the member as a JSON pointer
    /// rooted at the request body and the requirement the value broke.
    Shape(String),
    /// The existence lookup, which runs only on a value the model admits: the
    /// sentence names the pool id the request's ARN carried.
    Existence(String),
}

impl Stage {
    /// The `message` this stage states for a request whose provider member is at
    /// `root`.
    fn message(&self, root: &str) -> String {
        match self {
            Self::Shape(requirement) => format!(
                "1 validation error detected. Value at \
                 '{root}{POOL_ARN_IN_CONFIGURATION}' failed to satisfy constraint: {requirement}"
            ),
            Self::Existence(pool_id) => format!("User pool {pool_id} does not exist."),
        }
    }
}

/// The requirement clause a value outside the member's `@pattern` breaks.
fn pattern_requirement() -> String {
    format!("Member must satisfy regular expression pattern: {USER_POOL_ARN_PATTERN}")
}

/// The requirement clause a value past the member's `@length` maximum breaks.
fn length_requirement() -> String {
    format!("Member must have length less than or equal to {USER_POOL_ARN_MAX_LENGTH}")
}

/// One row: what a failing assertion calls it, the `userPoolArn` both writers
/// carry, and the stage that has to refuse it.
struct Row {
    case: &'static str,
    arn: String,
    stage: Stage,
}

/// RULE: a `userPoolArn` the model's constraints refuse is answered with the
/// constraint-violation envelope naming the member and the requirement, and one
/// the model admits that resolves to no pool is answered with `User pool <poolId>
/// does not exist.` — never with the generic summary and never with each other's
/// wording — by either writer of an identity source, and neither refusal leaves
/// anything behind.
#[tokio::test]
async fn each_stage_of_a_pool_reference_states_its_own_sentence() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;

    // The pool the positive control references: one the user-pool service really
    // minted, in the region the harness signs for.
    let live = user_pool(&router, "stages-live-pool").await;
    let live_arn = user_pool_arn(SIGNING_REGION, &live);
    // A pool that existed and then stopped existing, so a row reads live state
    // rather than admitting anything well formed.
    let deleted = user_pool(&router, "stages-deleted-pool").await;
    delete_user_pool(&router, &deleted).await;

    // The ARN exactly at the member's length maximum — the published bound, which
    // the shape stage admits — and the one byte past it, which it refuses.
    let at_maximum = user_pool_arn_of(USER_POOL_ARN_MAX_LENGTH);
    let past_maximum = user_pool_arn_of(USER_POOL_ARN_MAX_LENGTH + 1);

    let rows = vec![
        Row {
            // Not a user pool ARN at all: the service segment is another one's.
            case: "an ARN of another service entirely",
            arn: format!(
                "arn:aws:iam::{}:role/stages-not-a-user-pool",
                engine::emulator_account_id()
            ),
            stage: Stage::Shape(pattern_requirement()),
        },
        Row {
            // A pool id carries the region it was created in, separated by `_`.
            case: "a pool id with no region separator",
            arn: user_pool_arn(SIGNING_REGION, "useast1stagespool"),
            stage: Stage::Shape(pattern_requirement()),
        },
        Row {
            case: "an account id that is not twelve digits",
            arn: format!(
                "arn:aws:cognito-idp:{SIGNING_REGION}:12345:userpool/{SIGNING_REGION}_1a2b3c4d5"
            ),
            stage: Stage::Shape(pattern_requirement()),
        },
        Row {
            case: "an ARN one byte past the length maximum",
            arn: past_maximum.clone(),
            stage: Stage::Shape(length_requirement()),
        },
        Row {
            case: "a pool id nothing ever minted",
            arn: user_pool_arn(SIGNING_REGION, &format!("{SIGNING_REGION}_0a1b2c3d4")),
            stage: Stage::Existence(format!("{SIGNING_REGION}_0a1b2c3d4")),
        },
        Row {
            case: "a pool that was minted and then deleted",
            arn: user_pool_arn(SIGNING_REGION, &deleted),
            stage: Stage::Existence(deleted.clone()),
        },
        Row {
            case: "a mixed-case pool id suffix",
            arn: user_pool_arn(SIGNING_REGION, &format!("{SIGNING_REGION}_AbCdEf123")),
            stage: Stage::Existence(format!("{SIGNING_REGION}_AbCdEf123")),
        },
        Row {
            // The bound itself: the model admits it, so the existence stage is
            // what refuses it, naming the long pool id the ARN carried.
            case: "an ARN exactly at the length maximum",
            arn: at_maximum.clone(),
            stage: Stage::Existence(pool_id_of(&at_maximum)),
        },
    ];

    // The positive control: a reference that resolves is created, so every row
    // below is refused for the reference it carries and not for the operation.
    // It is also the source the update rows aim at, which makes "nothing was
    // written" observable.
    let control = ok(
        &router,
        "CreateIdentitySource",
        &json!({
            "policyStoreId": store_id,
            "principalEntityType": PRINCIPAL_ENTITY_TYPE,
            "configuration": cognito_provider(&live_arn),
        }),
    )
    .await["identitySourceId"]
        .as_str()
        .expect("identitySourceId is required on the CreateIdentitySource response")
        .to_string();

    // Every sentence the existence rows earned, to show the set carries one per
    // referenced pool rather than one constant.
    let mut existence_messages: BTreeSet<String> = BTreeSet::new();
    let mut existence_pools: BTreeSet<String> = BTreeSet::new();

    for Row { case, arn, stage } in &rows {
        for (operation, root) in [
            ("CreateIdentitySource", CONFIGURATION),
            ("UpdateIdentitySource", UPDATE_CONFIGURATION),
        ] {
            let request = if operation == "CreateIdentitySource" {
                json!({
                    "policyStoreId": store_id,
                    "principalEntityType": PRINCIPAL_ENTITY_TYPE,
                    "configuration": cognito_provider(arn),
                })
            } else {
                json!({
                    "policyStoreId": store_id,
                    "identitySourceId": control,
                    "principalEntityType": PRINCIPAL_ENTITY_TYPE,
                    "updateConfiguration": cognito_provider(arn),
                })
            };
            let (status, body) = call(&router, operation, &request).await;

            assert_eq!(
                status, 400,
                "case {case:?} on {operation}: expected HTTP 400 for {arn}, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?} on {operation}: expected a ValidationException, got {body}"
            );
            let expected = stage.message(root);
            let message = fault_message(&body);
            assert_eq!(
                message, expected,
                "case {case:?} on {operation}: expected the message {expected:?}, got {message:?}"
            );
            // Neither stage falls back to the summary that would make the two
            // refusals indistinguishable.
            assert_ne!(
                message, INVALID_INPUT_MESSAGE,
                "case {case:?} on {operation}: expected a worded refusal rather than the generic \
                 summary, got {message:?}"
            );

            let fields = field_list(&body);
            match stage {
                Stage::Shape(_) => {
                    // A constraint the model declares is refused before the
                    // handler runs: the member is in the message's pointer, and
                    // the shape refusal never borrows the existence wording.
                    assert!(
                        !message.contains("does not exist"),
                        "case {case:?} on {operation}: expected the constraint envelope rather \
                         than the existence sentence, got {message:?}"
                    );
                    assert!(
                        fields.is_empty(),
                        "case {case:?} on {operation}: expected no fieldList on a constraint \
                         refusal, got {fields:?}"
                    );
                }
                Stage::Existence(pool_id) => {
                    // The existence refusal is the service's own, so it states
                    // its sentence and still points at the member in
                    // `fieldList`.
                    assert!(
                        !message.contains("validation error"),
                        "case {case:?} on {operation}: expected the existence sentence rather \
                         than the constraint envelope, got {message:?}"
                    );
                    assert!(
                        !message.contains(arn.as_str()),
                        "case {case:?} on {operation}: the sentence must name the pool, not the \
                         whole ARN {arn:?}, got {message:?}"
                    );
                    let expected_path = format!("{root}{POOL_ARN_IN_CONFIGURATION}");
                    assert_eq!(
                        fields.first().map(|(path, _)| path.as_str()),
                        Some(expected_path.as_str()),
                        "case {case:?} on {operation}: expected the refusal to address \
                         {expected_path:?}, got {fields:?}"
                    );
                    existence_messages.insert(message);
                    existence_pools.insert(pool_id.clone());
                }
            }

            // The refused write changed nothing: the control still carries the
            // pool it was created with, and no row minted a second source.
            let aimed_at = ok(
                &router,
                "GetIdentitySource",
                &json!({ "policyStoreId": store_id, "identitySourceId": control }),
            )
            .await;
            assert_eq!(
                aimed_at["configuration"]["cognitoUserPoolConfiguration"]["userPoolArn"].as_str(),
                Some(live_arn.as_str()),
                "case {case:?} on {operation}: expected the source to still carry {live_arn}, \
                 got {aimed_at}"
            );
            let listed = ok(
                &router,
                "ListIdentitySources",
                &json!({ "policyStoreId": store_id }),
            )
            .await;
            assert_eq!(
                listed["identitySources"]
                    .as_array()
                    .map(|sources| sources
                        .iter()
                        .filter_map(|source| source["identitySourceId"].as_str())
                        .collect::<Vec<_>>()),
                Some(vec![control.as_str()]),
                "case {case:?} on {operation}: expected the store to still hold only the \
                 control, got {listed}"
            );
        }
    }

    // The existence sentence is a template filled from the request: as many
    // distinct sentences as the rows named distinct pools.
    assert_eq!(
        existence_messages.len(),
        existence_pools.len(),
        "expected one sentence per referenced pool ({}), got {existence_messages:?}",
        existence_pools.len()
    );
}
