// SPDX-License-Identifier: CC0-1.0

//! The DS Data FEATURE SWITCH, as every Directory Service Data operation sees it, end to
//! end over HTTP.
//!
//! The rule under test: this data plane answers for a directory only while the Directory
//! Service CONTROL plane has that directory's DS Data feature in service — the feature is
//! in service from the instant an `EnableDirectoryDataAccess` transition falls due until
//! the instant a `DisableDirectoryDataAccess` transition does — and every call against a
//! directory whose feature is out of service is refused with `AccessDeniedException`, HTTP
//! 403, `Reason` `DATA_DISABLED` and one fixed sentence
//! (`smithy_query(query="shape", shape="AccessDeniedException")`: `@error client`,
//! `@httpError` 403, members `Message` / `Reason`, the latter targeting `AccessDeniedReason`
//! whose values are `IAM_AUTH`, `DIRECTORY_AUTH`, `DATA_DISABLED`).
//!
//! The refusal is DIRECTORY-scoped, not operation-scoped, so the rows below cross every
//! route this service binds with every way a directory's feature can be out of service: an
//! object read, a write, an inventory listing and a search all carry the identical code and
//! the identical message. It precedes pagination-token parsing, `ClientToken` idempotency
//! and every object lookup, so a refused write persists nothing — and the objects that
//! existed before a disable are the same objects after the next enable, because the
//! directory's contents are not what a disable touches.
//!
//! The switch itself belongs to the control plane, which is another crate: these suites
//! record its decisions in the one inventory both planes share, exactly as that plane's
//! enable and disable handlers write them, each with the instant its transition falls due.
//! The cross-plane join — the control plane's own calls driving this gate — is
//! `core/router/tests/all/directory_data_access_cycle.rs`.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use super::directory_authorization::{
    emulator_over, every_operation, not_authorized, ACCESS_DENIED_EXCEPTION, IAM_AUTH,
    UNHELD_DIRECTORIES,
};
use super::validation_envelope::{call, Answer};

/// The `AccessDeniedReason` a directory whose DS Data feature is off is refused with: the
/// modelled value that names the directory's own data-access state.
const DATA_DISABLED: &str = "DATA_DISABLED";

/// The whole `Message` that refusal carries — fixed text, with nothing of the request in
/// it, so one sentence answers a read, a write and a listing of any directory alike.
const DATA_DISABLED_MESSAGE: &str = "DS Data feature is not enabled on this directory. \
     This feature must first be enabled by calling the EnableDirectoryDataAccess API.";

/// The error a request inside a served directory gets for an object that is not there, so a
/// served-but-empty answer is told from the gate's refusal.
const RESOURCE_NOT_FOUND_EXCEPTION: &str = "ResourceNotFoundException";

/// The scaffold's answer for an operation this emulator has not implemented yet, which is
/// also an answer the gate let through to the operation.
const NOT_IMPLEMENTED: &str = "NotImplemented";

/// A `DirectoryId` inside `^d-[0-9a-f]{10}$` the account holds in every case below; what
/// varies is the state of its DS Data feature.
const DIRECTORY: &str = "d-1234567890";

/// A second one, so no case can pass by answering for "the only directory".
const OTHER_DIRECTORY: &str = "d-00abcdef99";

/// Longer than any transition this emulator owes, for the cases about a toggle whose
/// transition has NOT fallen due yet.
const WELL_AHEAD: Duration = Duration::from_secs(3_600);

/// How the control plane's switch stands for one directory, as the inventory records it.
#[derive(Debug, Clone, Copy)]
enum Feature {
    /// Registered by a create and never enabled: a create opens nothing.
    NeverEnabled,
    /// Enabled, with the transition that enable owes still outstanding.
    Enabling,
    /// Enabled, that transition fallen due.
    Enabled,
    /// Disabled, with the transition that disable owes still outstanding — the directory is
    /// still served, as the control plane still reports its data access as `Disabling`.
    Disabling,
    /// Disabled, that transition fallen due.
    Disabled,
    /// Disabled and enabled again, the second transition fallen due.
    ReEnabled,
}

impl Feature {
    /// Whether this data plane answers for a directory whose switch stands this way.
    fn is_served(self) -> bool {
        matches!(self, Self::Enabled | Self::Disabling | Self::ReEnabled)
    }
}

/// Record `directory` as held by the account with its DS Data feature standing as `feature`
/// says, the way the control plane's create, enable and disable handlers write it.
fn record(engine: &engine::InMemoryEngine, directory: &str, feature: Feature) {
    let inventory = service_com_amazonaws_directoryservicedata_directoryservicedata::state::
        ad_directory_inventory(engine)
        .expect("the shared engine holds the AD directory inventory");
    let mut recorded = inventory.lock();
    let now = SystemTime::now();
    // Every state starts from the create, which registers the directory and enables nothing.
    recorded.register(directory);
    match feature {
        Feature::NeverEnabled => {}
        Feature::Enabling => recorded.enable_data_access(directory, now, now + WELL_AHEAD),
        Feature::Enabled => recorded.enable_data_access(directory, now, now),
        Feature::Disabling => {
            recorded.enable_data_access(directory, now, now);
            recorded.disable_data_access(directory, now, now + WELL_AHEAD);
        }
        Feature::Disabled => {
            recorded.enable_data_access(directory, now, now);
            recorded.disable_data_access(directory, now, now);
        }
        Feature::ReEnabled => {
            recorded.enable_data_access(directory, now, now);
            recorded.disable_data_access(directory, now, now);
            recorded.enable_data_access(directory, now, now);
        }
    }
}

/// An emulator whose one directory's feature stands as `feature` says, over an engine the
/// caller can keep driving.
fn emulator_with(
    feature: Feature,
) -> (Arc<engine::InMemoryEngine>, smithy_server_runtime::MultiServiceRouter) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(&engine);
    record(&engine, DIRECTORY, feature);
    record(&engine, OTHER_DIRECTORY, feature);
    (engine, router)
}

/// Assert one call was refused because the directory's DS Data feature is out of service:
/// 403 `AccessDeniedException` in the body and the header, `Reason` `DATA_DISABLED`, the one
/// fixed sentence, and nothing of the request — the directory or the object it named —
/// anywhere in the body.
fn assert_data_disabled(case: &str, answer: &Answer, directory: &str) {
    assert_eq!(
        answer.status, 403,
        "case {case:?}: expected HTTP 403, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        ACCESS_DENIED_EXCEPTION,
        "case {case:?}: expected error code {ACCESS_DENIED_EXCEPTION}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.error_type, ACCESS_DENIED_EXCEPTION,
        "case {case:?}: expected x-amzn-errortype {ACCESS_DENIED_EXCEPTION}, got {:?}",
        answer.error_type
    );
    assert_eq!(
        answer.body.get("Reason").and_then(Value::as_str),
        Some(DATA_DISABLED),
        "case {case:?}: expected Reason {DATA_DISABLED}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.message(),
        Some(DATA_DISABLED_MESSAGE),
        "case {case:?}: expected Message {DATA_DISABLED_MESSAGE:?}, got {:?}",
        answer.body
    );
    let rendered = answer.body.to_string();
    assert!(
        !rendered.contains(directory),
        "case {case:?}: the refusal carries no variable part, got {rendered:?}"
    );
}

/// RULE: every operation refuses a directory whose DS Data feature is out of service, with
/// one identical refusal — whichever operation, and however the feature came to be off.
///
/// The rows are the whole surface (17 routes) crossed with every out-of-service state of the
/// switch and two directories, so neither a per-operation answer nor one keyed to how the
/// feature was last written passes.
#[tokio::test]
async fn every_operation_refuses_a_directory_whose_feature_is_out_of_service() {
    for feature in [Feature::NeverEnabled, Feature::Enabling, Feature::Disabled] {
        assert!(
            !feature.is_served(),
            "case {feature:?}: this row is about a feature that is OUT of service"
        );
        let (_engine, router) = emulator_with(feature);
        for (route, body) in every_operation() {
            for directory in [DIRECTORY, OTHER_DIRECTORY] {
                let answer = call(&router, route, directory, &body).await;
                assert_data_disabled(
                    &format!("{route} on {directory} with the feature {feature:?}"),
                    &answer,
                    directory,
                );
            }
        }
    }
}

/// The SAME requests are served once the feature is in service, so the refusal above is the
/// switch's doing and not the request's: the directory is served from the instant the
/// enable's transition falls due, stays served while a disable's transition is outstanding,
/// and is served again after a re-enable.
#[tokio::test]
async fn every_operation_is_served_while_the_feature_is_in_service() {
    for feature in [Feature::Enabled, Feature::Disabling, Feature::ReEnabled] {
        assert!(
            feature.is_served(),
            "case {feature:?}: this row is about a feature that IS in service"
        );
        let (_engine, router) = emulator_with(feature);
        for (route, body) in every_operation() {
            let answer = call(&router, route, DIRECTORY, &body).await;
            // Every row's request is well formed for a directory that holds no object of
            // its own, so the answer is the operation's own: a success, the modelled
            // not-found for the object it named, or — for an operation this emulator has
            // not implemented yet, which is that operation's own stub issue — the scaffold's
            // 501. Never the gate's refusal.
            let served = answer.status == 200
                || (answer.status == 404 && answer.error_code() == RESOURCE_NOT_FOUND_EXCEPTION)
                || (answer.status == 501 && answer.error_code() == NOT_IMPLEMENTED);
            assert!(
                served,
                "route {route:?} with the feature {feature:?}: an in-service directory must be \
                 answered by the operation itself, got {} with {:?}",
                answer.status, answer.body
            );
            assert_ne!(
                answer.error_code(),
                ACCESS_DENIED_EXCEPTION,
                "route {route:?} with the feature {feature:?}: an in-service directory must \
                 not be refused, got {:?}",
                answer.body
            );
        }
    }
}

/// RULE, layer order: the data-access refusal precedes pagination-token parsing and
/// `ClientToken` semantics, and a directory the account does not hold at all is still the
/// authorization layer's `IAM_AUTH` denial rather than this one.
#[tokio::test]
async fn the_refusal_precedes_token_handling_and_follows_the_accounts_inventory() {
    let (_engine, router) = emulator_with(Feature::Disabled);

    // (case, route, body) — each carries a member a layer BEHIND the gate owns.
    let rows: [(&str, &str, Value); 3] = [
        (
            "a NextToken this listing never issued",
            "/Users/ListUsers",
            json!({"NextToken": "not-a-token-this-listing-issued"}),
        ),
        (
            "a fabricated NextToken on a group listing",
            "/Groups/ListGroups",
            json!({"NextToken": "also-not-a-token"}),
        ),
        (
            "a well-formed ClientToken on a create",
            "/Users/CreateUser",
            json!({"SAMAccountName": "sim2real", "ClientToken": "a-client-token-01"}),
        ),
    ];
    for (case, route, body) in rows {
        let refused = call(&router, route, DIRECTORY, &body).await;
        assert_data_disabled(case, &refused, DIRECTORY);
    }

    // A value outside a modelled constraint is still refused BEFORE the gate.
    let invalid = call(
        &router,
        "/Users/ListUsers",
        DIRECTORY,
        &json!({"MaxResults": 251}),
    )
    .await;
    assert_eq!(
        invalid.error_code(),
        super::validation_envelope::VALIDATION_EXCEPTION,
        "case a MaxResults past its @range ceiling: the constraint layer must answer before \
         the gate, got {:?}",
        invalid.body
    );

    // The two refusals are distinct and ordered: an id the account does not hold is the
    // authorization layer's denial, whatever its data access would have been.
    for unheld in UNHELD_DIRECTORIES {
        let denied = call(&router, "/Users/ListUsers", unheld, &json!({})).await;
        assert_eq!(
            denied.body.get("Reason").and_then(Value::as_str),
            Some(IAM_AUTH),
            "case the unheld directory {unheld}: expected Reason {IAM_AUTH}, got {:?}",
            denied.body
        );
        assert_eq!(
            denied.message(),
            Some(not_authorized().as_str()),
            "case the unheld directory {unheld}: expected the principal-only message, got {:?}",
            denied.body
        );
    }
}

/// RULE, what a refused call leaves behind and what a disable does NOT touch: a write
/// refused by the gate creates nothing, while the objects stored before a disable are the
/// same objects after the next enable.
///
/// This is the disable → refuse → re-enable cycle, driven over the one switch: the user, the
/// group and the membership edge created while the feature was in service come back
/// unchanged in both projections, and the name a refused create used is absent from the
/// inventory.
#[tokio::test]
async fn a_refused_write_creates_nothing_and_a_disable_keeps_the_directorys_contents() {
    let (engine, router) = emulator_with(Feature::Enabled);
    let user = "s2rSurvivor";
    let group = "s2rGroupSurvivor";
    let refused_name = "s2rNeverCreated";

    // The objects that exist before the disable, created while the feature is in service.
    let created_user = call(
        &router,
        "/Users/CreateUser",
        DIRECTORY,
        &json!({
            "SAMAccountName": user,
            "GivenName": "Sim",
            "Surname": "Survivor",
            "EmailAddress": "s2rSurvivor@corp.example.com",
        }),
    )
    .await;
    assert_eq!(
        created_user.status, 200,
        "CreateUser on an in-service directory must be served, got {} with {:?}",
        created_user.status, created_user.body
    );
    let created_group = call(
        &router,
        "/Groups/CreateGroup",
        DIRECTORY,
        &json!({"SAMAccountName": group}),
    )
    .await;
    assert_eq!(
        created_group.status, 200,
        "CreateGroup on an in-service directory must be served, got {} with {:?}",
        created_group.status, created_group.body
    );
    let joined = call(
        &router,
        "/GroupMemberships/AddGroupMember",
        DIRECTORY,
        &json!({"GroupName": group, "MemberName": user}),
    )
    .await;
    assert_eq!(
        joined.status, 200,
        "AddGroupMember on an in-service directory must be served, got {} with {:?}",
        joined.status, joined.body
    );

    // What the directory reports while it is served, to compare the cycle against.
    let before = call(
        &router,
        "/Users/DescribeUser",
        DIRECTORY,
        &json!({"SAMAccountName": user}),
    )
    .await;
    let before_group = call(
        &router,
        "/Groups/DescribeGroup",
        DIRECTORY,
        &json!({"SAMAccountName": group}),
    )
    .await;

    // The control plane withdraws the directory from this API.
    record(&engine, DIRECTORY, Feature::Disabled);

    // A read, a write and a listing are refused with one identical answer.
    let read = call(
        &router,
        "/Users/DescribeUser",
        DIRECTORY,
        &json!({"SAMAccountName": user}),
    )
    .await;
    let write = call(
        &router,
        "/Users/CreateUser",
        DIRECTORY,
        &json!({"SAMAccountName": refused_name, "GivenName": "Sim", "Surname": "Nope"}),
    )
    .await;
    let listing = call(&router, "/Users/ListUsers", DIRECTORY, &json!({})).await;
    for (case, answer) in [
        ("DescribeUser", &read),
        ("CreateUser", &write),
        ("ListUsers", &listing),
    ] {
        assert_data_disabled(case, answer, DIRECTORY);
    }
    assert_eq!(
        read.body, write.body,
        "a read and a write of a withdrawn directory must carry the identical refusal, got \
         {:?} and {:?}",
        read.body, write.body
    );
    assert_eq!(
        read.body, listing.body,
        "a read and a listing of a withdrawn directory must carry the identical refusal, got \
         {:?} and {:?}",
        read.body, listing.body
    );

    // The control plane puts it back in service: the directory's contents come back as the
    // same AD objects, and the refused write left nothing.
    record(&engine, DIRECTORY, Feature::ReEnabled);

    let after = call(
        &router,
        "/Users/DescribeUser",
        DIRECTORY,
        &json!({"SAMAccountName": user}),
    )
    .await;
    assert_eq!(
        after.body, before.body,
        "case the user across the cycle: expected the same AD object, got {:?} before and \
         {:?} after",
        before.body, after.body
    );
    let after_group = call(
        &router,
        "/Groups/DescribeGroup",
        DIRECTORY,
        &json!({"SAMAccountName": group}),
    )
    .await;
    assert_eq!(
        after_group.body, before_group.body,
        "case the group across the cycle: expected the same AD object, got {:?} before and \
         {:?} after",
        before_group.body, after_group.body
    );

    // The membership edge survives in BOTH projections.
    let members = call(
        &router,
        "/GroupMemberships/ListGroupMembers",
        DIRECTORY,
        &json!({"SAMAccountName": group}),
    )
    .await;
    let member_names: Vec<&str> = members.body["Members"]
        .as_array()
        .map(|members| {
            members
                .iter()
                .filter_map(|member| member["SAMAccountName"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        member_names.contains(&user),
        "case ListGroupMembers after the cycle: expected the membership edge to {user:?} among \
         the members, got {member_names:?}"
    );
    let memberships = call(
        &router,
        "/GroupMemberships/ListGroupsForMember",
        DIRECTORY,
        &json!({"SAMAccountName": user}),
    )
    .await;
    let group_names: Vec<&str> = memberships.body["Groups"]
        .as_array()
        .map(|groups| {
            groups
                .iter()
                .filter_map(|held| held["SAMAccountName"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        group_names.contains(&group),
        "case ListGroupsForMember after the cycle: expected the membership edge to {group:?} \
         among the groups, got {group_names:?}"
    );

    // The name the refused create used is in no projection of the directory.
    let listed = call(&router, "/Users/ListUsers", DIRECTORY, &json!({})).await;
    let listed_names: Vec<&str> = listed.body["Users"]
        .as_array()
        .map(|users| {
            users
                .iter()
                .filter_map(|listed| listed["SAMAccountName"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        listed_names.contains(&user) && !listed_names.contains(&refused_name),
        "case the inventory after the cycle: expected {user:?} and not {refused_name:?}, got \
         {listed_names:?}"
    );
    let described = call(
        &router,
        "/Users/DescribeUser",
        DIRECTORY,
        &json!({"SAMAccountName": refused_name}),
    )
    .await;
    assert_eq!(
        described.error_code(),
        RESOURCE_NOT_FOUND_EXCEPTION,
        "case the refused create's name: expected {RESOURCE_NOT_FOUND_EXCEPTION}, got {:?}",
        described.body
    );
}
