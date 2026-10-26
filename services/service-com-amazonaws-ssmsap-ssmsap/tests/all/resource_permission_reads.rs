// SPDX-License-Identifier: CC0-1.0

//! `GetResourcePermission` as a REPORTING operation, driven over the wire.
//!
//! The rule these cases pin: reading a resource's permissions reports the policy
//! projected from the grants the service already holds for the resource the ARN names,
//! and changes nothing a later caller can observe — however many reads arrive, in
//! whatever order, narrowed to an `ActionType` or not, aimed at a granted resource, an
//! ungranted one, or an ARN that names no resource of this account and Region at all.
//! So a read never brings a resource, a policy or a Region's partition into being, and
//! never turns a refusal into a success or a success into a refusal.
//!
//! `tests/all/resource_permissions.rs` pins WHAT the policy of a target contains, and the
//! write path of the Region partitions (`a_grant_for_a_region_holding_nothing_keeps_no_partition`,
//! `the_region_partition_ceiling_is_enforced_where_a_partition_is_minted`); this suite pins
//! that READING is free of consequence, which is the half of the contract a single read of
//! a single target cannot show — including the READ path of those same partitions.
//!
//! The rows span what the rule generalises over: the read form (unnarrowed, and narrowed
//! to each `PermissionActionType` the model declares —
//! `service_com_amazonaws_ssmsap_ssmsap::ops::types::PERMISSION_ACTION_TYPE_VALUES`), the
//! kind of target (a registered application's own ARN, a SAP HANA database of a
//! registration, a database of a registration nobody granted anything on, an ARN naming
//! nothing here, another service's resource), the endpoint the read reaches (the Region
//! holding the resource and one that does not), and the number and order of reads (three
//! rounds, alternately reversed). After every round every reporting call of the
//! registered slice — `GetApplication`, `ListApplications`, `ListDatabases` and
//! `GetResourcePermission` itself — must answer exactly what it answered before any read
//! was made.
//!
//! Every call goes through `MultiServiceRouter::dispatch` (the harness
//! `tests/all/application_lifecycle.rs` owns), so the restJson1 routing, the boundary
//! constraint walk, the handler and the response serialization all run exactly as they do
//! in the emulator binary. Nothing reaches into `crate::state` directly: a resource is
//! only made to exist by registering it, and a grant is only believed when a read reports
//! it back.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::json;

use crate::application_lifecycle::{rendered, settle, Emulator, Reply};
use crate::resource_permissions::{
    assert_not_found, assert_policy_grants_for_actions, get, get_in_region, policy, put,
    register_in_region,
};
use crate::resource_tags::{application_arn, database_arn, register, registration};
use service_com_amazonaws_ssmsap_ssmsap::ops::types::PERMISSION_ACTION_TYPE_VALUES;
use service_com_amazonaws_ssmsap_ssmsap::state::MAX_REGIONS;

/// The account every emulated ARN carries, from the core's single source of truth.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The Region this suite signs for, which is the Region every resource it registers
/// lives in.
const REGION: &str = "us-east-1";

/// A second regional endpoint, so a target can be read at a Region its resource was not
/// registered in.
const OTHER_REGION: &str = "eu-west-1";

/// The application whose own ARN and whose first HANA database both carry grants.
const GRANTED_APP: &str = "perm-read-granted";

/// A registered application nobody has granted anything on.
const BARE_APP: &str = "perm-read-bare";

/// The databases the granted registration brings with it; only the first is granted on.
const GRANTED_DATABASES: &[&str] = &["HDB", "SYSTEMDB"];

/// The single source granted on the application's own ARN.
fn application_sources() -> BTreeSet<String> {
    BTreeSet::from([format!(
        "arn:aws:ssm-sap:{REGION}:{ACCOUNT}:application/perm-read-source"
    )])
}

/// The sources granted on the granted registration's first HANA database: more than one,
/// and of more than one ARN form, so a read reports a document of several statements.
fn database_sources() -> BTreeSet<String> {
    BTreeSet::from([
        format!("arn:aws:ssm-sap:{REGION}:{ACCOUNT}:database/perm-read-source/HDB"),
        "arn:aws:s3:::sap-backint-bucket/restore".to_string(),
    ])
}

/// What a read of one target must answer.
#[derive(Debug)]
enum Expect {
    /// HTTP 200 whose `Policy` allows exactly these sources on the target.
    Granted(BTreeSet<String>),
    /// HTTP 200 with no `Policy` member at all: the target exists and carries no grant.
    NoPolicy,
    /// The modelled `ResourceNotFoundException` (HTTP 404): the ARN names no resource of
    /// this account and Region.
    NotFound,
}

/// One read: where it is sent, what it names, how it is narrowed, and what it must
/// answer.
#[derive(Debug)]
struct Probe {
    label: String,
    region: &'static str,
    resource: String,
    action: Option<&'static str>,
    expect: Expect,
}

/// The action types a read reports on: the one it narrows to, or every type the model
/// declares when it narrows to none.
fn actions_in_scope(action: Option<&'static str>) -> Vec<&'static str> {
    match action {
        Some(action) => vec![action],
        None => PERMISSION_ACTION_TYPE_VALUES.to_vec(),
    }
}

/// The read forms: unnarrowed, then narrowed to each `PermissionActionType` the model
/// declares.
fn read_forms() -> Vec<Option<&'static str>> {
    std::iter::once(None)
        .chain(PERMISSION_ACTION_TYPE_VALUES.iter().copied().map(Some))
        .collect()
}

/// An emulator holding the two registrations, with every declared action type granted on
/// the granted application's own ARN and on its first HANA database.
async fn granted_emulator() -> Emulator {
    let emulator = Emulator::new();
    register(
        &emulator,
        GRANTED_APP,
        registration(GRANTED_APP, &[], Some(GRANTED_DATABASES)),
    )
    .await;
    register(
        &emulator,
        BARE_APP,
        registration(BARE_APP, &[], Some(&["HDB"])),
    )
    .await;

    let application = application_arn(REGION, GRANTED_APP);
    let database = database_arn(REGION, GRANTED_APP, GRANTED_DATABASES[0]);
    for action in PERMISSION_ACTION_TYPE_VALUES {
        for (target, sources) in [
            (&application, application_sources()),
            (&database, database_sources()),
        ] {
            for source in &sources {
                let case = format!("granting {action} on {target} to {source}");
                let reply = put(&emulator, target, action, source).await;
                assert_eq!(
                    reply.status, 200,
                    "case {case:?}: expected 200 from PutResourcePermission, got {reply:?}"
                );
            }
        }
    }

    // Wait out every status change the registrations owe, so each witness below already
    // reports terminal state: anything that differs after a read is then the read's
    // doing and not the clock's.
    settle().await;
    emulator
}

/// Every target of [`granted_emulator`] that EXISTS, in a stable order.
fn existing_targets() -> Vec<String> {
    let mut targets = vec![
        application_arn(REGION, GRANTED_APP),
        application_arn(REGION, BARE_APP),
        database_arn(REGION, BARE_APP, "HDB"),
    ];
    for database in GRANTED_DATABASES {
        targets.push(database_arn(REGION, GRANTED_APP, database));
    }
    targets
}

/// The reads of [`granted_emulator`], one per (read form, target kind) pair.
fn probes() -> Vec<Probe> {
    let mut rows = Vec::new();
    for action in read_forms() {
        let form = match action {
            Some(action) => format!("narrowed to {action}"),
            None => "unnarrowed".to_string(),
        };
        let mut row = |label: &str, region: &'static str, resource: String, expect: Expect| {
            rows.push(Probe {
                label: format!("{label} ({form})"),
                region,
                resource,
                action,
                expect,
            });
        };
        row(
            "the granted application's own ARN",
            REGION,
            application_arn(REGION, GRANTED_APP),
            Expect::Granted(application_sources()),
        );
        row(
            "the granted HANA database",
            REGION,
            database_arn(REGION, GRANTED_APP, GRANTED_DATABASES[0]),
            Expect::Granted(database_sources()),
        );
        row(
            "a database of the granted registration nobody granted on",
            REGION,
            database_arn(REGION, GRANTED_APP, GRANTED_DATABASES[1]),
            Expect::NoPolicy,
        );
        row(
            "the ungranted application's own ARN",
            REGION,
            application_arn(REGION, BARE_APP),
            Expect::NoPolicy,
        );
        row(
            "the ungranted application's HANA database",
            REGION,
            database_arn(REGION, BARE_APP, "HDB"),
            Expect::NoPolicy,
        );
        row(
            "an application that was never registered",
            REGION,
            application_arn(REGION, "perm-read-absent"),
            Expect::NotFound,
        );
        row(
            "another service's resource",
            REGION,
            "arn:aws:s3:::sap-backint-bucket/restore".to_string(),
            Expect::NotFound,
        );
        row(
            "the granted database at the endpoint of another Region",
            OTHER_REGION,
            database_arn(REGION, GRANTED_APP, GRANTED_DATABASES[0]),
            Expect::NotFound,
        );
    }
    rows
}

/// Send one probe at the endpoint it names.
async fn read(emulator: &Emulator, probe: &Probe) -> Reply {
    get_in_region(emulator, probe.region, &probe.resource, probe.action).await
}

/// Assert `reply` is what `probe` must answer.
fn assert_probe(reply: &Reply, probe: &Probe, case: &str) {
    match &probe.expect {
        Expect::Granted(sources) => {
            // A read reports one `Allow` statement per (action type in scope, granted
            // source) pair, which is the one projection check this target keeps.
            let document = policy(reply, case);
            assert_policy_grants_for_actions(
                &document,
                &probe.resource,
                &actions_in_scope(probe.action),
                sources,
                case,
            );
        }
        Expect::NoPolicy => {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected HTTP 200 for a target that exists, got {reply:?}"
            );
            assert!(
                reply.member("Policy").is_none(),
                "case {case:?}: a target granted nothing must carry no Policy, got {reply:?}"
            );
        }
        Expect::NotFound => assert_not_found(reply, &probe.resource, case),
    }
}

/// What a later caller can observe, rendered so two snapshots compare as values: every
/// reporting call of the registered slice, plus the whole policy of every target that
/// exists.
///
/// Comparing whole bodies holds because the caller waited the clock out before the first
/// snapshot ([`granted_emulator`] ends in `settle()`), so nothing a response carries is
/// still owed a change: a transition added with a longer span has to be waited out here
/// the same way. A witness whose body ever legitimately carries a value computed per call
/// belongs out of this map, named here with the member that made it so — not in it with a
/// weaker comparison, which would stop the map detecting the writes it exists to catch.
async fn witnesses(emulator: &Emulator) -> BTreeMap<String, String> {
    let mut snapshot = BTreeMap::new();
    for application in [GRANTED_APP, BARE_APP] {
        let reply = emulator
            .send("/get-application", json!({ "ApplicationId": application }))
            .await;
        snapshot.insert(format!("GetApplication {application}"), rendered(&reply));
        let reply = emulator
            .send("/list-databases", json!({ "ApplicationId": application }))
            .await;
        snapshot.insert(format!("ListDatabases {application}"), rendered(&reply));
    }
    let reply = emulator.send("/list-applications", json!({})).await;
    snapshot.insert("ListApplications".to_string(), rendered(&reply));
    for target in existing_targets() {
        let reply = get(emulator, &target, None).await;
        snapshot.insert(format!("GetResourcePermission {target}"), rendered(&reply));
    }
    snapshot
}

/// RULE: reading a resource's permissions reports the grants already held and changes
/// nothing a later caller sees — whatever the read's form, target, endpoint, order or
/// count.
#[tokio::test]
async fn reading_a_resource_policy_changes_nothing_a_later_caller_sees() {
    let emulator = granted_emulator().await;
    let before = witnesses(&emulator).await;
    let probes = probes();

    for round in 0..3u32 {
        // The same reads in the opposite order on alternate rounds: the order they
        // arrive in is as much part of the rule as how many there are.
        let mut ordered: Vec<&Probe> = probes.iter().collect();
        if round % 2 == 1 {
            ordered.reverse();
        }
        for probe in ordered {
            let case = format!("round {round} / {}", probe.label);
            let reply = read(&emulator, probe).await;
            assert_probe(&reply, probe, &case);
        }

        let after = witnesses(&emulator).await;
        for (witness, expected) in &before {
            let actual = after.get(witness);
            assert_eq!(
                actual,
                Some(expected),
                "case {witness:?} after round {round}: expected {expected:?}, got {actual:?}"
            );
        }
        assert_eq!(
            after.len(),
            before.len(),
            "case \"round {round}\": expected the same witnesses as before any read, got {:?}",
            after.keys().collect::<Vec<_>>()
        );
    }
}

/// RULE: a read answers for any Region a caller names without making that Region one the
/// store holds, so reading cannot consume the ceiling that bounds the Regions written to.
#[tokio::test]
async fn reading_at_a_region_holding_nothing_mints_no_partition() {
    let emulator = Emulator::new();

    // More Regions than the store keeps partitions for, each read at before anything is
    // registered anywhere: the Region comes from the caller's credential scope, so
    // nothing stops a caller naming as many as it likes.
    for index in 0..=MAX_REGIONS {
        let region = format!("us-read-{index}");
        let target = database_arn(&region, GRANTED_APP, "HDB");
        for action in read_forms() {
            let case = format!(
                "read {} of {} in {region} ({action:?})",
                index + 1,
                MAX_REGIONS + 1
            );
            let reply = get_in_region(&emulator, &region, &target, action).await;
            assert_not_found(&reply, &target, &case);
        }
    }

    // Every partition the store keeps is therefore still free: a registration at each of
    // MAX_REGIONS Regions just read at is accepted, which it could not be had the reads
    // minted partitions of their own.
    for index in 0..MAX_REGIONS {
        let region = format!("us-read-{index}");
        register_in_region(
            &emulator,
            &format!("Region {} of {MAX_REGIONS} after the reads", index + 1),
            &region,
            registration(GRANTED_APP, &[], Some(&["HDB"])),
        )
        .await;
    }

    // And the resource those registrations brought into being now reads as a target that
    // exists and carries no grant, at the same endpoint that answered not found before.
    for index in 0..MAX_REGIONS {
        let region = format!("us-read-{index}");
        let target = database_arn(&region, GRANTED_APP, "HDB");
        let case = format!("the registered database in {region}");
        let reply = get_in_region(&emulator, &region, &target, None).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200 once the Region holds the resource, got {reply:?}"
        );
        assert!(
            reply.member("Policy").is_none(),
            "case {case:?}: a target granted nothing must carry no Policy, got {reply:?}"
        );
    }
}

/// RULE: the policy a read reports is the one the grants project, so it changes exactly
/// when the grants do and not when anything is read: a target read many times while
/// ungranted reports one statement — not one per read — once a single grant is made.
#[tokio::test]
async fn a_policy_appears_only_when_a_grant_is_made() {
    let emulator = granted_emulator().await;
    let action = PERMISSION_ACTION_TYPE_VALUES[0];
    let target = database_arn(REGION, GRANTED_APP, GRANTED_DATABASES[1]);
    let source = format!("arn:aws:ssm-sap:{REGION}:{ACCOUNT}:database/perm-read-late/HDB");

    for round in 0..4u32 {
        let case = format!("read {round} before the grant");
        let reply = get(&emulator, &target, Some(action)).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200 for a target that exists, got {reply:?}"
        );
        assert!(
            reply.member("Policy").is_none(),
            "case {case:?}: reading must not bring a policy into being, got {reply:?}"
        );
    }

    let granted = put(&emulator, &target, action, &source).await;
    assert_eq!(
        granted.status, 200,
        "case \"the grant\": expected 200 from PutResourcePermission, got {granted:?}"
    );

    let expected = Expect::Granted(BTreeSet::from([source]));
    let probe = Probe {
        label: "the late grant".to_string(),
        region: REGION,
        resource: target.clone(),
        action: Some(action),
        expect: expected,
    };
    for round in 0..4u32 {
        let case = format!("read {round} after the grant");
        let reply = read(&emulator, &probe).await;
        assert_probe(&reply, &probe, &case);
    }

    // The grant reaches that one target alone: its sibling database, read just as often,
    // still carries nothing.
    let sibling = database_arn(REGION, BARE_APP, "HDB");
    let reply = get(&emulator, &sibling, None).await;
    assert_eq!(
        reply.status, 200,
        "case \"the sibling target\": expected HTTP 200, got {reply:?}"
    );
    assert!(
        reply.member("Policy").is_none(),
        "case \"the sibling target\": a grant on one target must not reach another, got {reply:?}"
    );
}
