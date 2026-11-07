// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ADMISSION of the `InstanceArn` a Glue Identity Center
//! configuration is registered against.
//!
//! The rule under test: the application `CreateGlueIdentityCenterConfiguration` registers is
//! a resource OF the Identity Center instance the request names, so the create is admitted
//! only for an instance THIS ACCOUNT holds — the instances the account's own Identity Center
//! holds, plus the ambient instance that service hands an account whose enablement nothing
//! has decided yet — and is otherwise refused with `InvalidInputException` (HTTP 400),
//! registering nothing. An instance nothing can be shown about is not refused: an emulator
//! with no Identity Center to ask refuses nothing, which is why
//! [`crate::identity_center_configuration`], which mounts Glue alone, still registers
//! configurations against an instance no Identity Center record backs.
//!
//! Because a refused create registers nothing, the account stays unconfigured: the read and
//! the DELETE of the configuration are then the account's own not-found refusal, which the
//! last suite below asserts.
//!
//! The rows differ along what the rule generalises over: which instance the ARN names (the
//! ambient one, one the account's Identity Center holds, one it does not, one it held and
//! deleted), and how an ARN can fail to name an instance at all. Every call goes over the
//! wire through the shared [`crate::wire`] harness, so what is asserted is HTTP behaviour
//! and not a direct call into a handler.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state::{
    instance_arn, ComAmazonawsSsoadminSwbexternalserviceState as IdentityCenterState, Instance,
    SEED_IDENTITY_STORE_ID, SEED_INSTANCE_ID, SEED_INSTANCE_NAME, SEED_INSTANCE_TYPE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, error_code, error_message, ok, REGION};

/// The whole `Message` a create naming an instance the account does not hold is refused with
/// — written out here rather than read from the crate's own constant so this suite pins the
/// wording.
const INSTANCE_NOT_IN_ACCOUNT: &str = "Provided identityCenterInstanceArn is invalid. Please \
                                      check IdentityCenter instance Arn in your account.";

/// The whole `Message` the read and the delete of an unconfigured account are refused with,
/// written out for the same reason.
const CONFIGURATION_NOT_FOUND: &str =
    "Idc application not found. Need to create glue application first.";

/// An emulator with Glue AND IAM Identity Center mounted on one shared engine, plus that
/// engine: the instance resolution reads the account's instances out of that service's own
/// store, so it has to be mounted for the account to hold any.
fn emulator_with_identity_center() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_glue_awsglue::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_ssoadmin_swbexternalservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    (router, engine)
}

/// Seed one ACTIVE instance under `instance_id` into the shared engine's Identity Center
/// store — the record `CreateInstance` writes once its activation is due — and answer the ARN
/// that service holds it under.
fn seed_instance(engine: &Arc<engine::InMemoryEngine>, instance_id: &str) -> String {
    let state = engine.state::<IdentityCenterState>();
    let mut store = state.lock();
    let instance = Instance::active(
        instance_id,
        SEED_IDENTITY_STORE_ID,
        SEED_INSTANCE_TYPE,
        Some(SEED_INSTANCE_NAME.to_string()),
        ::aws_smithy_types::DateTime::from_secs(1_700_000_000),
        REGION,
    );
    let arn = instance.instance_arn.clone();
    // A caller's own create decides the account's enablement, so no ambient instance is
    // handed out beside it.
    store.instances.insert(arn.clone(), instance);
    store.ambient_instance_enabled = true;
    arn
}

/// Record that the account's Identity Center enablement has been DECIDED while the store
/// holds no instance: the state an account is in once the instance it had was deleted.
fn forget_the_ambient_instance(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<IdentityCenterState>();
    state.lock().ambient_instance_enabled = true;
}

/// The create body for one instance ARN.
fn create_body(instance_arn: &str) -> Value {
    json!({ "InstanceArn": instance_arn })
}

/// An Identity Center instance id of the form the service mints: `ssoins-` plus 16
/// characters (`^arn:aws(-[a-z]{1,5}){0,3}:sso:::instance/(sso)?ins-[a-zA-Z0-9-.]{16}$`, the
/// modelled `InstanceArn` pattern).
const OTHER_INSTANCE_ID: &str = "ssoins-0123456789abcdef";

/// The configuration is registered against an instance the account holds, and refused for
/// one it does not.
///
/// The rows differ along which instance the ARN names and how the account came to hold it.
#[tokio::test]
async fn a_configuration_is_registered_only_against_an_instance_this_account_holds() {
    // (case, whether the account's Identity Center holds the instance the create names,
    // whether the create must be admitted)
    enum Holding {
        /// The ambient instance, which the account holds until something decides its
        /// enablement.
        Ambient,
        /// An instance seeded into the Identity Center store, as a create there writes it.
        Seeded,
        /// An instance of the right form that the account holds no record of.
        Unheld,
        /// The ambient instance's ARN, after the account's enablement has been decided and
        /// the store holds no instance — the state a deleted instance leaves behind.
        AmbientWithdrawn,
    }

    let rows: Vec<(&str, Holding, bool)> = vec![
        (
            "the account's ambient Identity Center instance",
            Holding::Ambient,
            true,
        ),
        (
            "an instance the account's Identity Center holds",
            Holding::Seeded,
            true,
        ),
        (
            "a well-formed instance ARN the account holds no instance for",
            Holding::Unheld,
            false,
        ),
        (
            "the ambient instance's ARN once the account's enablement was decided without it",
            Holding::AmbientWithdrawn,
            false,
        ),
    ];

    for (case, holding, admitted) in rows {
        let (router, engine) = emulator_with_identity_center();
        let instance = match holding {
            Holding::Ambient => instance_arn(SEED_INSTANCE_ID),
            Holding::Seeded => seed_instance(&engine, OTHER_INSTANCE_ID),
            Holding::Unheld => instance_arn(OTHER_INSTANCE_ID),
            Holding::AmbientWithdrawn => {
                forget_the_ambient_instance(&engine);
                instance_arn(SEED_INSTANCE_ID)
            }
        };

        let (status, body) = call(
            &router,
            "CreateGlueIdentityCenterConfiguration",
            &create_body(&instance),
        )
        .await;
        if admitted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the create to be admitted, got {status} with {body}"
            );
            // Round trip: the configuration the create registered is readable, carrying the
            // instance it was registered against and the application the create minted.
            let read = ok(&router, "GetGlueIdentityCenterConfiguration", &json!({})).await;
            assert_eq!(
                read["InstanceArn"],
                json!(instance),
                "case {case:?}: expected the stored configuration to carry the instance it was \
                 registered against, got {read}"
            );
            assert_eq!(
                read["ApplicationArn"], body["ApplicationArn"],
                "case {case:?}: expected the read to report the application the create minted, \
                 got {read} after {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected the create to be refused, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("InvalidInputException"),
            "case {case:?}: expected InvalidInputException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            INSTANCE_NOT_IN_ACCOUNT,
            "case {case:?}: expected Message {INSTANCE_NOT_IN_ACCOUNT:?}, got {:?}",
            error_message(&body)
        );
    }
}

/// An `InstanceArn` that names no Identity Center INSTANCE at all is refused for its form,
/// before the account's instances are consulted — so the two refusals of this member stay
/// distinguishable.
///
/// The rows differ along how the value fails to name an instance: another Identity Center
/// resource, an ARN of another service, a value that is no ARN, and an instance segment
/// carrying no id.
#[tokio::test]
async fn an_instance_arn_that_names_no_instance_is_refused_for_its_form() {
    let (router, _engine) = emulator_with_identity_center();

    let rows: Vec<(&str, String)> = vec![
        (
            "an Identity Center application ARN",
            format!("arn:aws:sso::000000000000:application/{SEED_INSTANCE_ID}/apl-0123456789abcdef"),
        ),
        (
            "an ARN of another service",
            "arn:aws:iam::000000000000:role/glue-service-role".to_string(),
        ),
        ("a bare instance id", SEED_INSTANCE_ID.to_string()),
        (
            "an instance segment carrying no id",
            "arn:aws:sso:::instance/".to_string(),
        ),
    ];

    for (case, instance) in rows {
        let (status, body) = call(
            &router,
            "CreateGlueIdentityCenterConfiguration",
            &create_body(&instance),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the create to be refused, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("InvalidInputException"),
            "case {case:?}: expected InvalidInputException, got {body}"
        );
        assert_ne!(
            error_message(&body),
            INSTANCE_NOT_IN_ACCOUNT,
            "case {case:?}: a value that names no instance is refused for its form, not for the \
             account's holdings, got {body}"
        );
    }
}

/// A refused create registers NOTHING, so the account is still unconfigured: its read and
/// its delete are the account's own not-found refusal.
///
/// The rows differ along which refusal the create met.
#[tokio::test]
async fn nothing_is_registered_by_a_refused_create() {
    for (case, instance) in [
        (
            "an instance the account holds no record of",
            instance_arn(OTHER_INSTANCE_ID),
        ),
        (
            "a value that names no instance at all",
            "not-an-arn".to_string(),
        ),
    ] {
        let (router, _engine) = emulator_with_identity_center();
        let (status, body) = call(
            &router,
            "CreateGlueIdentityCenterConfiguration",
            &create_body(&instance),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the create to be refused, got {status} with {body}"
        );

        for operation in [
            "GetGlueIdentityCenterConfiguration",
            "DeleteGlueIdentityCenterConfiguration",
        ] {
            let (status, body) = call(&router, operation, &json!({})).await;
            assert_eq!(
                status, 400,
                "case {case:?}: {operation} must refuse an unconfigured account, got {status} \
                 with {body}"
            );
            assert!(
                error_code(&body).ends_with("EntityNotFoundException"),
                "case {case:?}: expected EntityNotFoundException from {operation}, got {body}"
            );
            assert_eq!(
                error_message(&body),
                CONFIGURATION_NOT_FOUND,
                "case {case:?}: expected Message {CONFIGURATION_NOT_FOUND:?} \
                 from {operation}, got {:?}",
                error_message(&body)
            );
        }
    }
}
