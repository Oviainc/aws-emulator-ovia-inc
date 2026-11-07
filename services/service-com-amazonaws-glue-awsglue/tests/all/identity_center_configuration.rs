// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue Identity Center configuration slice:
//! `CreateGlueIdentityCenterConfiguration`, `GetGlueIdentityCenterConfiguration` and
//! `DeleteGlueIdentityCenterConfiguration`.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: the configuration is an ACCOUNT SINGLETON — every one of the three
//! operations addresses it by the account alone — so an account that has none answers the
//! read and the delete with `EntityNotFoundException`, the first create registers it and
//! every later create is refused with `AlreadyExistsException` without replacing it, and a
//! delete returns the account to the not-configured state. What a create registered is
//! what the read reports, under the `ApplicationArn` the create minted for the Identity
//! Center instance it named.
//!
//! The rows below differ along what the rule generalises over: the Identity Center
//! instance the create names, how many scopes it sets (none, one, several), whether it
//! enables background sessions, how many creates race for the singleton, and how an
//! `InstanceArn` can fail to name an instance.
//!
//! Whether the instance a create names is one the ACCOUNT holds is a separate rule, pinned
//! by [`crate::identity_center_instance`]; the creates here name instances the account holds,
//! so what they exercise is the singleton.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::messages::IDENTITY_CENTER_CONFIGURATION_NOT_FOUND;
use service_com_amazonaws_ssoadmin_swbexternalservice::state::{
    instance_arn, ComAmazonawsSsoadminSwbexternalserviceState as IdentityCenterState, Instance,
    SEED_IDENTITY_STORE_ID, SEED_INSTANCE_NAME, SEED_INSTANCE_TYPE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, emulator_on, error_code, error_message, ok, ACCOUNT_ID, REGION};

/// The Identity Center instance id every create below names, as Identity Center mints one.
const INSTANCE_ID: &str = "ssoins-1234567890abcdef";

/// An Identity Center instance ARN, as Identity Center spells one:
/// `arn:aws:sso:::instance/<instance id>`.
const INSTANCE_ARN: &str = "arn:aws:sso:::instance/ssoins-1234567890abcdef";

/// An emulator whose account HOLDS the Identity Center instances `instance_ids` names.
///
/// A configuration is registered against an instance of the account
/// ([`crate::identity_center_instance`] pins that rule), so a suite about the SINGLETON
/// stands the instances it names up first — seeded into the Identity Center store the way
/// that service's own create writes them.
fn emulator_holding(instance_ids: &[&str]) -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let state = engine.state::<IdentityCenterState>();
        let mut store = state.lock();
        for instance_id in instance_ids {
            let instance = Instance::active(
                instance_id,
                SEED_IDENTITY_STORE_ID,
                SEED_INSTANCE_TYPE,
                Some(SEED_INSTANCE_NAME.to_string()),
                ::aws_smithy_types::DateTime::from_secs(1_700_000_000),
                REGION,
            );
            store.instances.insert(instance.instance_arn.clone(), instance);
        }
        // The account's instances are the ones seeded here, so no ambient instance is handed
        // out beside them.
        store.ambient_instance_enabled = true;
    }
    emulator_on(engine)
}

/// The `ApplicationArn` prefix a create mints under: an Identity Center application is a
/// resource of the account's own Identity Center, named inside the instance it belongs to.
fn application_arn_prefix(instance_arn: &str) -> String {
    let instance_id = instance_arn
        .rsplit('/')
        .next()
        .expect("an instance ARN carries an instance id");
    format!("arn:aws:sso::{ACCOUNT_ID}:application/{instance_id}/apl-")
}

/// Read the configuration, answering the HTTP status and body.
async fn read(router: &MultiServiceRouter) -> (u16, Value) {
    call(
        router,
        "GetGlueIdentityCenterConfiguration",
        &json!({}),
    )
    .await
}

/// Assert that the account reports no configuration, the way a fresh account does.
async fn assert_absent(router: &MultiServiceRouter, case: &str) {
    let (status, body) = read(router).await;
    assert_eq!(
        status, 400,
        "case {case}: an account with no configuration must refuse the read, got {status} \
         with {body}"
    );
    assert!(
        error_code(&body).ends_with("EntityNotFoundException"),
        "case {case}: expected EntityNotFoundException, got {} in {body}",
        error_code(&body)
    );
    assert_eq!(
        error_message(&body),
        IDENTITY_CENTER_CONFIGURATION_NOT_FOUND,
        "case {case}: the refusal must name the missing Identity Center application, got {body}"
    );
}

// ---------------------------------------------------------------------------
// The singleton's lifecycle
// ---------------------------------------------------------------------------

/// The configuration is absent before a create, reports what the create registered after
/// it, and is absent again after the delete — which is the state a further create is
/// accepted from.
///
/// The rows differ along what the create names: the scopes it sets (none, one, several) and
/// whether it enables background sessions (absent, false, true).
#[tokio::test]
async fn the_configuration_is_absent_until_created_and_absent_again_after_delete() {
    // (case, the members the create carries beside InstanceArn)
    let cases: [(&str, Value); 4] = [
        ("no scopes and no background sessions", json!({})),
        (
            "one scope",
            json!({ "Scopes": ["glue:read"], "UserBackgroundSessionsEnabled": false }),
        ),
        (
            "several scopes, background sessions enabled",
            json!({
                "Scopes": ["glue:read", "glue:write", "lakeformation:getDataAccess"],
                "UserBackgroundSessionsEnabled": true
            }),
        ),
        (
            "another Identity Center instance",
            json!({ "Scopes": ["glue:read"] }),
        ),
    ];

    for (index, (case, extra)) in cases.iter().enumerate() {
        let instance_id = format!("{INSTANCE_ID}{index}");
        let router = emulator_holding(&[&instance_id]);
        let instance_arn = instance_arn(&instance_id);
        assert_absent(&router, case).await;

        // The delete of an absent singleton is the same miss as the read's.
        let (status, body) = call(
            &router,
            "DeleteGlueIdentityCenterConfiguration",
            &json!({}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: deleting what is not there must be refused, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException from the delete, got {}",
            error_code(&body)
        );

        let mut create = json!({ "InstanceArn": instance_arn });
        for (member, value) in extra.as_object().expect("the row carries an object") {
            create[member] = value.clone();
        }
        let created = ok(&router, "CreateGlueIdentityCenterConfiguration", &create).await;
        let minted = created["ApplicationArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: the create must mint an ApplicationArn"))
            .to_string();
        let prefix = application_arn_prefix(&instance_arn);
        assert!(
            minted.starts_with(&prefix),
            "case {case}: the application ARN must name the instance it was registered \
             with; expected the prefix {prefix}, got {minted}"
        );

        let (status, read_back) = read(&router).await;
        assert_eq!(
            status, 200,
            "case {case}: the registered configuration must be readable, got {status} with \
             {read_back}"
        );
        assert_eq!(
            read_back["ApplicationArn"].as_str(),
            Some(minted.as_str()),
            "case {case}: the read must report the application the create minted, got \
             {read_back}"
        );
        assert_eq!(
            read_back["InstanceArn"].as_str(),
            Some(instance_arn.as_str()),
            "case {case}: the read must report the instance the create named, got {read_back}"
        );
        assert_eq!(
            read_back["Scopes"], create["Scopes"],
            "case {case}: the read must report the scopes the create set, got {read_back}"
        );
        assert_eq!(
            read_back["UserBackgroundSessionsEnabled"], create["UserBackgroundSessionsEnabled"],
            "case {case}: the read must report the background-session setting the create \
             made, got {read_back}"
        );
        // A read changes nothing: the same call twice answers the same configuration.
        let (_, again) = read(&router).await;
        assert_eq!(
            again, read_back,
            "case {case}: a read must not change what a later read sees, got {again} after \
             {read_back}"
        );

        ok(
            &router,
            "DeleteGlueIdentityCenterConfiguration",
            &json!({}),
        )
        .await;
        assert_absent(&router, case).await;

        // The account is back in the state a create is accepted from, and the create that
        // follows mints an application of its own.
        let recreated = ok(&router, "CreateGlueIdentityCenterConfiguration", &create).await;
        let second = recreated["ApplicationArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: the second create must mint an ARN"));
        assert_ne!(
            second, minted,
            "case {case}: a configuration created after a delete is a new application, got \
             {second} twice"
        );
    }
}

// ---------------------------------------------------------------------------
// Only one create wins
// ---------------------------------------------------------------------------

/// Several creates for one account leave exactly ONE configuration: the first is served
/// and every later one is refused with `AlreadyExistsException`, leaving the registration
/// the winner made untouched.
///
/// The rows differ along how many creates follow the first, and along whether they name the
/// same instance and scopes as the winner or different ones.
#[tokio::test]
async fn only_the_first_create_registers_the_configuration() {
    // (case, the creates that follow the first one)
    let cases: [(&str, Vec<Value>); 3] = [
        (
            "one identical create",
            vec![json!({ "InstanceArn": INSTANCE_ARN })],
        ),
        (
            "two creates naming other instances",
            vec![
                json!({ "InstanceArn": "arn:aws:sso:::instance/ssoins-aaaaaaaaaaaaaaaa" }),
                json!({ "InstanceArn": "arn:aws:sso:::instance/ssoins-bbbbbbbbbbbbbbbb" }),
            ],
        ),
        (
            "a create naming other scopes",
            vec![json!({
                "InstanceArn": INSTANCE_ARN,
                "Scopes": ["glue:write"],
                "UserBackgroundSessionsEnabled": true
            })],
        ),
    ];

    for (case, losers) in cases {
        let router = emulator_holding(&[
            INSTANCE_ID,
            "ssoins-aaaaaaaaaaaaaaaa",
            "ssoins-bbbbbbbbbbbbbbbb",
        ]);
        let winner = ok(
            &router,
            "CreateGlueIdentityCenterConfiguration",
            &json!({ "InstanceArn": INSTANCE_ARN, "Scopes": ["glue:read"] }),
        )
        .await;
        let registered = read(&router).await.1;

        for loser in &losers {
            let (status, body) = call(&router, "CreateGlueIdentityCenterConfiguration", loser).await;
            assert_eq!(
                status, 400,
                "case {case}: a second create must be refused, got {status} with {body}"
            );
            assert!(
                error_code(&body).ends_with("AlreadyExistsException"),
                "case {case}: expected AlreadyExistsException, got {} in {body}",
                error_code(&body)
            );
        }

        let (status, after) = read(&router).await;
        assert_eq!(
            status, 200,
            "case {case}: the winner's configuration must still be readable, got {status} \
             with {after}"
        );
        assert_eq!(
            after, registered,
            "case {case}: a refused create must change nothing; expected {registered}, got \
             {after}"
        );
        assert_eq!(
            after["ApplicationArn"], winner["ApplicationArn"],
            "case {case}: the stored application must be the one the winning create minted, \
             got {after}"
        );
    }
}

// ---------------------------------------------------------------------------
// The instance the application is registered with
// ---------------------------------------------------------------------------

/// A create whose `InstanceArn` names no Identity Center INSTANCE is refused with
/// `InvalidInputException`, and registers nothing: the application a create mints is a
/// resource of the instance it names, so a value naming none registers it nowhere.
///
/// The rows differ along how the value fails to name an instance: no ARN at all, an ARN of
/// another kind of Identity Center resource, an instance segment with no id after it, and
/// an id carrying a further path segment.
#[tokio::test]
async fn an_instance_arn_that_names_no_instance_is_refused() {
    let refused = [
        ("a bare instance id", "ssoins-1234567890abcdef"),
        (
            "an ARN of another resource kind",
            "arn:aws:sso::000000000000:application/apl-1234567890abcdef",
        ),
        ("an instance segment with no id", "arn:aws:sso:::instance/"),
        (
            "an instance id with a further path segment",
            "arn:aws:sso:::instance/ssoins-1234567890abcdef/extra",
        ),
        ("a value that is no ARN at all", "not an arn at all"),
    ];

    for (case, instance_arn) in refused {
        let router = emulator();
        let (status, body) = call(
            &router,
            "CreateGlueIdentityCenterConfiguration",
            &json!({ "InstanceArn": instance_arn }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: {instance_arn} names no instance and must be refused, got {status} \
             with {body}"
        );
        assert!(
            error_code(&body).ends_with("InvalidInputException"),
            "case {case}: expected InvalidInputException, got {} in {body}",
            error_code(&body)
        );
        assert!(
            error_message(&body).contains(instance_arn),
            "case {case}: the refusal must name the value the request sent, got {body}"
        );
        // Nothing was registered, so the account is still the not-configured one.
        assert_absent(&router, case).await;
    }
}
