// SPDX-License-Identifier: CC0-1.0

//! `DeleteResourcePermission` end to end, over the wire.
//!
//! The rules these cases pin:
//!
//! * A revocation removes exactly the grants the request NAMES. The grant is the
//!   (target resource ARN, `PermissionActionType`, source resource ARN) triple the
//!   service holds, `ActionType` and `SourceResourceArn` are both optional here, and
//!   each one the request carries narrows what is removed — so a call naming a source
//!   removes that source's grant alone, a call naming none removes every permission the
//!   target holds, and nothing held on another target or in another Region is touched.
//!   Removing a triple the target does not hold removes nothing and is not a failure.
//! * The `Policy` the call answers is PROJECTED from the grants that are left, narrowed
//!   to the `ActionType` the call named — the same projection `GetResourcePermission`
//!   reports next — and is ABSENT when nothing is left, rather than an empty document.
//! * The target is a SAP HANA DATABASE of this account and Region: these permissions are
//!   database-scoped, so an ARN naming no database discovered here — including a
//!   registered application's own ARN — is refused with the modelled
//!   `ValidationException` (HTTP 400) stating that the ARN is not a valid database ARN,
//!   exactly as the grant and the read refuse it, and the refusal changes nothing. A
//!   request outside the modelled constraints is refused with `ValidationException`
//!   (HTTP 400) and changes nothing either.
//!
//! The tables span what those rules generalise over: which optional members the delete
//! request carries (both, each alone, neither), whether the source it names is one the
//! target holds a grant for, the number of grants held on the target, how the target
//! database came to be (named by a registration's credentials, named by its `DatabaseArn`
//! — both branches of the modelled `Arn` pattern), every `PermissionActionType` the model
//! declares
//! (`service_com_amazonaws_ssmsap_ssmsap::ops::types::PERMISSION_ACTION_TYPE_VALUES`),
//! and the ways an ARN can name no database here (never registered, deregistered, the
//! application's own ARN, another Region, a Region holding nothing, another service's
//! resource).
//!
//! Every call goes through `MultiServiceRouter::dispatch` (the harness
//! `tests/all/application_lifecycle.rs` owns), so the restJson1 routing, the boundary
//! constraint walk, the handler and the response serialization all run exactly as they
//! do in the emulator binary. Nothing reaches into `crate::state` directly: a grant is
//! made and read back through `PutResourcePermission` / `GetResourcePermission`, and a
//! target is only made to exist by registering it through `RegisterApplication`.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

use crate::application_lifecycle::{settle, Emulator, Reply};
use crate::resource_permissions::{
    assert_invalid_database_arn, assert_policy_grants, get, get_in_region, policy, put,
    put_in_region, registered_by_arn, COLON_SEGMENT_DATABASE_ARN,
};
use crate::resource_tags::{application_arn, database_arn, register, registration};
use crate::validation_message_rendering::VALIDATION_EXCEPTION;
use service_com_amazonaws_ssmsap_ssmsap::ops::types::PERMISSION_ACTION_TYPE_VALUES;

/// The account every emulated ARN carries, from the core's single source of truth.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The Region this suite signs for, which is the Region every resource it registers
/// lives in.
const REGION: &str = "us-east-1";

/// A second regional endpoint, so a target can be named at a Region its resource was not
/// registered in, and a grant made at one endpoint can be revoked at another.
const OTHER_REGION: &str = "us-west-2";

/// `DeleteResourcePermission` at `region`'s endpoint, carrying only the optional members
/// the row names: an omitted member narrows nothing, which is the behaviour under test.
async fn delete_in_region(
    emulator: &Emulator,
    region: &str,
    resource: &str,
    action: Option<&str>,
    source: Option<&str>,
) -> Reply {
    let mut body = Map::new();
    body.insert("ResourceArn".to_string(), json!(resource));
    if let Some(action) = action {
        body.insert("ActionType".to_string(), json!(action));
    }
    if let Some(source) = source {
        body.insert("SourceResourceArn".to_string(), json!(source));
    }
    emulator
        .send_in_region(region, "/delete-resource-permission", Value::Object(body))
        .await
}

/// `DeleteResourcePermission` at [`REGION`]'s endpoint.
async fn delete(
    emulator: &Emulator,
    resource: &str,
    action: Option<&str>,
    source: Option<&str>,
) -> Reply {
    delete_in_region(emulator, REGION, resource, action, source).await
}

/// One target: a label, the registration that brings it into being, and its ARN.
struct Target {
    label: &'static str,
    registration: Value,
    arn: String,
}

/// The kinds of SAP HANA database a target ARN can name here, spanning both branches of
/// the modelled `Arn` pattern.
fn targets() -> Vec<Target> {
    vec![
        Target {
            label: "a SAP HANA database of the registration",
            registration: registration("del-hdb", &[], Some(&["HDB"])),
            arn: database_arn(REGION, "del-hdb", "HDB"),
        },
        Target {
            label: "the system database of the registration",
            registration: registration("del-app", &[], Some(&["SYSTEMDB"])),
            arn: database_arn(REGION, "del-app", "SYSTEMDB"),
        },
        Target {
            label: "a database the registration named by ARN",
            registration: registered_by_arn("del-by-arn", COLON_SEGMENT_DATABASE_ARN),
            arn: COLON_SEGMENT_DATABASE_ARN.to_string(),
        },
    ]
}

/// The three sources a row grants on its target before revoking, spanning a database of
/// this Region, a resource of another Region, and another service's resource.
fn sources() -> Vec<String> {
    vec![
        format!("arn:aws:ssm-sap:{REGION}:{ACCOUNT}:database/del-source/HDB"),
        format!("arn:aws:ssm-sap:eu-west-1:{ACCOUNT}:database/del-far/HDB"),
        "arn:aws:s3:::sap-backint-bucket/restore".to_string(),
    ]
}

/// A source ARN no row ever grants, so a delete naming it names a grant the target does
/// not hold.
fn ungranted_source() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/del-never-granted")
}

/// How one row narrows its delete request, and which of the granted sources that leaves.
struct Narrowing {
    label: &'static str,
    /// Whether the request carries `ActionType`.
    names_action: bool,
    /// The `SourceResourceArn` the request carries: an index into [`sources`], the
    /// ungranted source, or nothing at all.
    source: SourceChoice,
}

enum SourceChoice {
    /// The source at this index of [`sources`] — one the target holds a grant for.
    Granted(usize),
    /// A source the target holds no grant for.
    Ungranted,
    /// No `SourceResourceArn` at all.
    Omitted,
}

/// The rows: every combination of which optional members the request carries, plus a
/// source the target holds nothing for.
fn narrowings() -> Vec<Narrowing> {
    vec![
        Narrowing {
            label: "ActionType and SourceResourceArn",
            names_action: true,
            source: SourceChoice::Granted(0),
        },
        Narrowing {
            label: "SourceResourceArn alone",
            names_action: false,
            source: SourceChoice::Granted(1),
        },
        Narrowing {
            label: "the last granted source",
            names_action: true,
            source: SourceChoice::Granted(2),
        },
        Narrowing {
            label: "ActionType alone",
            names_action: true,
            source: SourceChoice::Omitted,
        },
        Narrowing {
            label: "neither optional member",
            names_action: false,
            source: SourceChoice::Omitted,
        },
        Narrowing {
            label: "a source the target holds no grant for",
            names_action: true,
            source: SourceChoice::Ungranted,
        },
        Narrowing {
            label: "a source the target holds no grant for, no ActionType",
            names_action: false,
            source: SourceChoice::Ungranted,
        },
    ]
}

/// Assert `reply` is a 200 carrying NO `Policy` — what a target with no grant left
/// answers.
fn assert_no_policy(reply: &Reply, case: &str) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected HTTP 200, got {reply:?}"
    );
    assert!(
        reply.member("Policy").is_none(),
        "case {case:?}: a target holding no matching grant must carry no Policy, got {reply:?}"
    );
}

/// Assert the policy of `target` is exactly `expected` — the projection of what is left,
/// absent when nothing is.
fn assert_remaining(
    reply: &Reply,
    target: &str,
    action: &str,
    expected: &BTreeSet<String>,
    case: &str,
) {
    if expected.is_empty() {
        assert_no_policy(reply, case);
    } else {
        assert_policy_grants(&policy(reply, case), target, action, expected, case);
    }
}

/// RULE: a revocation removes exactly the grants the request names, the `Policy` it
/// answers is the projection of what is left (absent when nothing is), and every later
/// read reports the same thing.
#[tokio::test]
async fn a_revocation_removes_exactly_the_grants_the_request_names() {
    // Every `PermissionActionType` the model declares, so one action type's grants are
    // never revoked as another's.
    for action in PERMISSION_ACTION_TYPE_VALUES {
        for row in narrowings() {
            for target in targets() {
                // A fresh emulator per row: the row's own grants are the only ones held.
                let emulator = Emulator::new();
                let case = format!("{} / {action} / {}", row.label, target.label);
                register(&emulator, &case, target.registration.clone()).await;
                // A second target, granted the same sources, so a revocation on one
                // target can be shown to leave the other alone.
                register(
                    &emulator,
                    &case,
                    registration("del-bystander", &[], Some(&["HDB"])),
                )
                .await;
                let bystander = database_arn(REGION, "del-bystander", "HDB");

                let granted = sources();
                let mut remaining: BTreeSet<String> = BTreeSet::new();
                for source in &granted {
                    let reply = put(&emulator, &target.arn, action, source).await;
                    assert_eq!(
                        reply.status, 200,
                        "case {case:?}: expected 200 from PutResourcePermission, got {reply:?}"
                    );
                    let reply = put(&emulator, &bystander, action, source).await;
                    assert_eq!(
                        reply.status, 200,
                        "case {case:?}: expected 200 granting on the bystander, got {reply:?}"
                    );
                    remaining.insert(source.clone());
                }

                // What the row's request names, and what it therefore leaves behind.
                let ungranted = ungranted_source();
                let named_source: Option<String> = match row.source {
                    SourceChoice::Granted(index) => Some(granted[index].clone()),
                    SourceChoice::Ungranted => Some(ungranted.clone()),
                    SourceChoice::Omitted => None,
                };
                match row.source {
                    SourceChoice::Granted(index) => {
                        remaining.remove(&granted[index]);
                    }
                    // A source nothing was granted to removes nothing.
                    SourceChoice::Ungranted => {}
                    // No source named: every grant of what the request DOES name goes.
                    SourceChoice::Omitted => remaining.clear(),
                }

                let names_action = if row.names_action { Some(*action) } else { None };
                let reply = delete(
                    &emulator,
                    &target.arn,
                    names_action,
                    named_source.as_deref(),
                )
                .await;
                assert_remaining(&reply, &target.arn, action, &remaining, &case);

                // The revocation is persisted, not just reported: the read agrees, with
                // and without the optional `ActionType`.
                let read = get(&emulator, &target.arn, Some(action)).await;
                assert_remaining(&read, &target.arn, action, &remaining, &case);
                let unfiltered = get(&emulator, &target.arn, None).await;
                assert_remaining(&unfiltered, &target.arn, action, &remaining, &case);

                // Repeating the very same request removes nothing more and answers the
                // same document: the grant is a triple a set holds at most once.
                let repeat = delete(
                    &emulator,
                    &target.arn,
                    names_action,
                    named_source.as_deref(),
                )
                .await;
                assert_remaining(&repeat, &target.arn, action, &remaining, &case);

                // The bystander still holds every source it was granted: a revocation
                // reaches the target it names and no other resource.
                let others: BTreeSet<String> = granted.iter().cloned().collect();
                let reply = get(&emulator, &bystander, None).await;
                assert_policy_grants(
                    &policy(&reply, &case),
                    &bystander,
                    action,
                    &others,
                    &format!("{case} / bystander"),
                );
            }
        }
    }
}

/// RULE: a target that holds no grant at all is still a database this Region holds, so
/// revoking on it is served with an absent `Policy` rather than refused — and a grant
/// made after a revocation is held again, so revoking removes the grant and nothing more.
#[tokio::test]
async fn revoking_what_is_not_held_is_served_and_leaves_the_target_grantable() {
    let emulator = Emulator::new();
    let action = PERMISSION_ACTION_TYPE_VALUES[0];
    register(
        &emulator,
        "a registration to revoke on",
        registration("del-regrant", &[], Some(&["HDB"])),
    )
    .await;
    let target = database_arn(REGION, "del-regrant", "HDB");
    let source = format!("arn:aws:ssm-sap:{REGION}:{ACCOUNT}:database/del-source/HDB");

    // Nothing has been granted yet, so every form of the request answers no policy.
    for (label, named_action, named_source) in [
        ("both members", Some(action), Some(source.as_str())),
        ("ActionType alone", Some(action), None),
        ("neither member", None, None),
        ("SourceResourceArn alone", None, Some(source.as_str())),
    ] {
        let case = format!("ungranted target / {label}");
        let reply = delete(&emulator, &target, named_action, named_source).await;
        assert_no_policy(&reply, &case);
        assert_no_policy(&get(&emulator, &target, None).await, &case);
    }

    // Granting, revoking and granting again: the target holds exactly what its last
    // grant made, so a revocation leaves the resource itself untouched.
    for round in 0..3 {
        let case = format!("round {round}");
        let reply = put(&emulator, &target, action, &source).await;
        let expected: BTreeSet<String> = BTreeSet::from([source.clone()]);
        assert_policy_grants(&policy(&reply, &case), &target, action, &expected, &case);

        let reply = delete(&emulator, &target, Some(action), Some(&source)).await;
        assert_no_policy(&reply, &case);
        assert_no_policy(&get(&emulator, &target, Some(action)).await, &case);
    }
}

/// RULE: the service is regional, so a grant is revoked at the endpoint it was made at;
/// at any other endpoint the target is a resource that Region does not hold.
#[tokio::test]
async fn a_revocation_reaches_only_the_region_it_was_made_in() {
    let emulator = Emulator::new();
    let action = PERMISSION_ACTION_TYPE_VALUES[0];
    let regions = [REGION, OTHER_REGION, "ap-southeast-2"];

    // The SAME application id registered at each regional endpoint: each registration's
    // database carries its own Region's ARN.
    for region in regions {
        let reply = emulator
            .send_in_region(
                region,
                "/register-application",
                registration("del-regions", &[], Some(&["HDB"])),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {region:?}: expected 200 from RegisterApplication, got {reply:?}"
        );
    }

    let source = format!("arn:aws:ssm-sap:{REGION}:{ACCOUNT}:database/del-source/HDB");
    let expected: BTreeSet<String> = BTreeSet::from([source.clone()]);
    for region in regions {
        let target = database_arn(region, "del-regions", "HDB");
        let reply = put_in_region(&emulator, region, &target, action, &source).await;
        assert_eq!(
            reply.status, 200,
            "case {region:?}: expected 200 from PutResourcePermission, got {reply:?}"
        );
    }

    for (index, revoked_in) in regions.iter().enumerate() {
        let target = database_arn(revoked_in, "del-regions", "HDB");
        // The target belongs to `revoked_in`, so every OTHER endpoint holds no such
        // database and refuses the revocation.
        for called_in in regions {
            let case = format!("target of {revoked_in}, revoked in {called_in}");
            if called_in != *revoked_in {
                let reply =
                    delete_in_region(&emulator, called_in, &target, Some(action), None).await;
                assert_invalid_database_arn(&reply, None, &target, &case);
            }
        }
        // Still granted: the refusals revoked nothing.
        let case = format!("target of {revoked_in} after the refusals");
        let reply = get_in_region(&emulator, revoked_in, &target, None).await;
        assert_policy_grants(&policy(&reply, &case), &target, action, &expected, &case);

        // At its own endpoint the revocation is served, and only this Region's target
        // loses its grant: a Region revoked in an earlier round holds nothing, and one
        // not yet revoked still holds exactly what it was granted.
        let reply = delete_in_region(&emulator, revoked_in, &target, Some(action), None).await;
        assert_no_policy(&reply, &case);
        for (other_index, other) in regions.iter().enumerate() {
            if other_index == index {
                continue;
            }
            let other_target = database_arn(other, "del-regions", "HDB");
            let case = format!("target of {other} after revoking in {revoked_in}");
            let reply = get_in_region(&emulator, other, &other_target, None).await;
            if other_index > index {
                assert_policy_grants(
                    &policy(&reply, &case),
                    &other_target,
                    action,
                    &expected,
                    &case,
                );
            } else {
                assert_no_policy(&reply, &case);
            }
        }
    }
}

/// RULE: an ARN that names no SAP HANA database of this account and Region is refused
/// with the modelled `ValidationException` (HTTP 400) stating that the ARN is not a valid
/// database ARN, and the refusal changes nothing.
#[tokio::test]
async fn an_arn_naming_no_database_here_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let action = PERMISSION_ACTION_TYPE_VALUES[0];

    register(
        &emulator,
        "a registration that stays",
        registration("del-present", &[], Some(&["HDB"])),
    )
    .await;
    register(
        &emulator,
        "a registration to deregister",
        registration("del-gone", &[], Some(&["HDB"])),
    )
    .await;
    let kept = database_arn(REGION, "del-present", "HDB");
    let source = format!("arn:aws:ssm-sap:{REGION}:{ACCOUNT}:database/del-source/HDB");
    let reply = put(&emulator, &kept, action, &source).await;
    assert_eq!(
        reply.status, 200,
        "expected 200 from PutResourcePermission, got {reply:?}"
    );

    let reply = emulator
        .send(
            "/deregister-application",
            json!({ "ApplicationId": "del-gone" }),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "expected 200 from DeregisterApplication, got {reply:?}"
    );
    // Deregistration is asynchronous: the application stops resolving when the removal
    // the clock owes it falls due.
    settle().await;

    // (label, the Region the call is made in, the ARN it names)
    let rows: Vec<(&str, &str, String)> = vec![
        (
            "an application never registered",
            REGION,
            application_arn(REGION, "del-never-registered"),
        ),
        (
            "a database of an application never registered",
            REGION,
            database_arn(REGION, "del-never-registered", "HDB"),
        ),
        (
            "the own ARN of a registered application",
            REGION,
            application_arn(REGION, "del-present"),
        ),
        (
            "an application that was deregistered",
            REGION,
            application_arn(REGION, "del-gone"),
        ),
        (
            "a database of an application that was deregistered",
            REGION,
            database_arn(REGION, "del-gone", "HDB"),
        ),
        (
            "a database the registration never named",
            REGION,
            database_arn(REGION, "del-present", "NOPE"),
        ),
        (
            "a registered resource named at another regional endpoint",
            OTHER_REGION,
            kept.clone(),
        ),
        (
            "an ARN carrying another Region",
            REGION,
            database_arn(OTHER_REGION, "del-present", "HDB"),
        ),
        (
            "another service's resource",
            REGION,
            "arn:aws:s3:::a-bucket/an-object".to_string(),
        ),
        (
            "a colon-segment ARN no registration named",
            REGION,
            COLON_SEGMENT_DATABASE_ARN.to_string(),
        ),
        (
            "a Region holding nothing at all",
            "us-test-nothing",
            database_arn("us-test-nothing", "del-present", "HDB"),
        ),
    ];

    for (label, region, arn) in rows {
        for (narrowing, named_action, named_source) in [
            ("both members", Some(action), Some(source.as_str())),
            ("neither member", None, None),
        ] {
            let case = format!("{label} / {narrowing}");
            let reply = delete_in_region(&emulator, region, &arn, named_action, named_source).await;
            assert_invalid_database_arn(&reply, None, &arn, &case);
        }
    }

    // The resource that does exist still holds exactly the grant it was given: every
    // refusal changed nothing.
    let expected: BTreeSet<String> = BTreeSet::from([source.clone()]);
    let case = "the registered database after the refusals";
    let reply = get(&emulator, &kept, None).await;
    assert_policy_grants(&policy(&reply, case), &kept, action, &expected, case);
}

/// RULE: a request outside what the model declares is refused with the modelled
/// `ValidationException` (HTTP 400) before anything is resolved or changed.
#[tokio::test]
async fn a_request_outside_the_modelled_constraints_is_refused() {
    let emulator = Emulator::new();
    let action = PERMISSION_ACTION_TYPE_VALUES[0];
    register(
        &emulator,
        "a registration to revoke on",
        registration("del-refuse", &[], Some(&["HDB"])),
    )
    .await;
    let good = database_arn(REGION, "del-refuse", "HDB");
    let source = format!("arn:aws:ssm-sap:{REGION}:{ACCOUNT}:database/del-source/HDB");
    let reply = put(&emulator, &good, action, &source).await;
    assert_eq!(
        reply.status, 200,
        "expected 200 from PutResourcePermission, got {reply:?}"
    );

    // Each row is one way the request falls outside what the model declares: the
    // `@required` `ResourceArn` omitted, an ARN that is not an ARN on either member, and
    // an `ActionType` outside the `PermissionActionType` enum.
    let rows = vec![
        (
            "ResourceArn omitted",
            json!({"ActionType": action, "SourceResourceArn": source}),
        ),
        (
            "ResourceArn is not an ARN",
            json!({"ResourceArn": "not-an-arn", "ActionType": action}),
        ),
        (
            "SourceResourceArn is not an ARN",
            json!({"ResourceArn": good, "ActionType": action, "SourceResourceArn": "nope"}),
        ),
        (
            "ActionType outside the enum",
            json!({"ResourceArn": good, "ActionType": "BACKUP"}),
        ),
    ];

    for (case, body) in rows {
        let reply = emulator.send("/delete-resource-permission", body).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            VALIDATION_EXCEPTION,
            "case {case:?}: expected {VALIDATION_EXCEPTION}, got {reply:?}"
        );
    }

    // A refused request changed nothing: the grant the target held is still held.
    let expected: BTreeSet<String> = BTreeSet::from([source.clone()]);
    let case = "refused requests change nothing";
    let reply = get(&emulator, &good, None).await;
    assert_policy_grants(&policy(&reply, case), &good, action, &expected, case);
}
