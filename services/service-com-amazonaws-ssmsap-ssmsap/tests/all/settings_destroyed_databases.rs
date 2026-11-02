// SPDX-License-Identifier: CC0-1.0

//! What a settled `UpdateApplicationSettings` leaves behind for the SAP HANA databases it
//! DESTROYS, driven over the wire.
//!
//! RULE under test: what this service holds ON a resource — its tags and its resource
//! permissions, both keyed by exactly the ARN the resource is reported under — cannot
//! outlive that resource. The SAP HANA databases of a registration are DERIVED from its
//! credentials and its SAP HANA connection, so a change that settles and leaves the
//! application no longer deriving one of them destroys it, and everything held on its ARN
//! goes at that same instant, exactly as it does when a whole registration ends. A
//! database derived again later under the same ARN is therefore a NEW resource carrying no
//! tag and no grant of the one that was destroyed, while a database the change leaves
//! derived keeps everything held on it.
//!
//! The table spans what the rule generalises over: how the update stops a database being
//! derived (the only credential naming it removed, several removed in one call, a removal
//! beside an addition, the SAP HANA connection replaced, every setting changed at once),
//! how many databases are involved, the form of the database name the ARN is keyed by
//! (one the `DatabaseId` form rewrites), the kind of ARN the destroyed database carries
//! (one this service mints and one the registration named itself), what is held on it
//! (tags and a grant), and the updates that destroy NOTHING — a credential removed for a
//! database the connection still names, an update that touches no credential at all, and a
//! database another registration of the Region still names.
//!
//! Every call goes through the real router (the harness
//! `tests/all/application_lifecycle.rs` owns), so the restJson1 routing, the boundary
//! constraint walk, the handler and the response serialization all run exactly as they do
//! in the emulator binary. The ONE thing read through `crate::state` is whether the
//! partition still holds anything keyed by a destroyed ARN: that is precisely the state no
//! API can observe — every call that takes an ARN refuses one that names nothing — so it
//! is checked where `tests/all/region_partition_reclamation.rs` checks what a partition
//! holds.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Map, Value};

use crate::application_lifecycle::{settle, tag_pairs, Emulator, Reply, REGION};
use crate::list_databases::{credentials, listed};
use crate::resource_permissions::{
    assert_invalid_database_arn, assert_not_found, assert_policy_grants, get as get_permission,
    policy, put as put_permission, COLON_SEGMENT_DATABASE_ARN, RESOURCE_ARN_MEMBER,
};
use crate::resource_tags::{database_arn, list_tags, register, registration};
use crate::tag_resource::{pairs, tag};
use crate::untag_resource::untag;
use service_com_amazonaws_ssmsap_ssmsap::ops::types::{
    BACKINT_MODE_VALUES, PERMISSION_ACTION_TYPE_VALUES,
};

/// The account every emulated ARN carries, from the core's single source of truth.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The `PermissionActionType` the grants below are made with — the model's own catalogue,
/// read rather than spelled (`service_com_amazonaws_ssmsap_ssmsap::ops::types::PERMISSION_ACTION_TYPE_VALUES`).
fn action() -> &'static str {
    PERMISSION_ACTION_TYPE_VALUES[0]
}

/// A `DatabaseArn` that is NOT the one `tests/all/resource_permissions.rs` registers, so a
/// row can replace one named ARN with another; it is in the colon-segment branch of the
/// modelled `Arn` pattern (`^arn:(.+:){2,4}.+$`) too.
const REPLACEMENT_DATABASE_ARN: &str = "arn:aws:rds:us-east-1:000000000000:db:sap-qas";

/// A database of the application under test, as the ARN what is held on it is keyed by.
#[derive(Debug, Clone, Copy)]
enum Resource {
    /// A database that is a SUB-RESOURCE of the application: its ARN is the one this
    /// service mints from the application id and the `DatabaseId`.
    Derived(&'static str),
    /// A database a `DatabaseArn` named, which carries exactly the ARN it was named by.
    Named(&'static str),
}

impl Resource {
    /// The ARN this resource is reported under for `application_id`.
    fn arn(self, application_id: &str) -> String {
        match self {
            Self::Derived(database_id) => database_arn(REGION, application_id, database_id),
            Self::Named(arn) => arn.to_string(),
        }
    }
}

/// One registration, one update of its settings, and the databases that update must
/// destroy and must leave alone.
struct Case {
    /// How an assertion names the row.
    name: &'static str,
    application_id: &'static str,
    /// The `DatabaseName` of every credential the REGISTRATION carries.
    registered: &'static [&'static str],
    /// The `DatabaseArn` the registration names, when it names one.
    registered_connection: Option<Resource>,
    /// The `DatabaseName`s the update's `CredentialsToAddOrUpdate` carries.
    add_or_update: &'static [&'static str],
    /// The `DatabaseName`s its `CredentialsToRemove` carries.
    remove: &'static [&'static str],
    /// The `DatabaseArn` the update names, when it names one.
    connection: Option<Resource>,
    /// Whether the update also carries a `Backint` configuration.
    backint: bool,
    /// The databases the settled update must DESTROY.
    destroyed: &'static [Resource],
    /// The databases it must leave derived, still holding what was held on them.
    kept: &'static [Resource],
}

impl Case {
    /// Every database the row holds tags and a grant on before the update: the ones it
    /// expects destroyed and the ones it expects kept.
    fn held_on(&self) -> impl Iterator<Item = &Resource> {
        self.destroyed.iter().chain(self.kept.iter())
    }
}

/// The rows: one registration and one update each.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "the only credential naming a database removed",
            application_id: "destroy-one",
            registered: &["SYSTEMDB", "TNT1"],
            registered_connection: None,
            add_or_update: &[],
            remove: &["TNT1"],
            connection: None,
            backint: false,
            destroyed: &[Resource::Derived("TNT1")],
            kept: &[Resource::Derived("SYSTEMDB")],
        },
        Case {
            name: "two of four credentials removed in one call",
            application_id: "destroy-two",
            registered: &["SYSTEMDB", "TNT1", "TNT2", "TNT3"],
            registered_connection: None,
            add_or_update: &[],
            remove: &["TNT1", "TNT3"],
            connection: None,
            backint: false,
            destroyed: &[Resource::Derived("TNT1"), Resource::Derived("TNT3")],
            kept: &[Resource::Derived("SYSTEMDB"), Resource::Derived("TNT2")],
        },
        Case {
            name: "a credential removed beside one added",
            application_id: "destroy-and-add",
            registered: &["SYSTEMDB", "TNT1"],
            registered_connection: None,
            add_or_update: &["TNT2"],
            remove: &["TNT1"],
            connection: None,
            backint: false,
            destroyed: &[Resource::Derived("TNT1")],
            kept: &[Resource::Derived("SYSTEMDB")],
        },
        // A name the `DatabaseId` form rewrites: every character the modelled `DatabaseId`
        // pattern does not admit becomes `_` and every ASCII letter is folded to upper case
        // (`service_com_amazonaws_ssmsap_ssmsap::state::database_id_from_name`), so the
        // ARN the tags and the grant are keyed by is not the name the credential carried.
        Case {
            name: "a database whose name the DatabaseId form rewrites",
            application_id: "destroy-rewritten-id",
            registered: &["SYSTEMDB", "tnt-1"],
            registered_connection: None,
            add_or_update: &[],
            remove: &["tnt-1"],
            connection: None,
            backint: false,
            destroyed: &[Resource::Derived("TNT_1")],
            kept: &[Resource::Derived("SYSTEMDB")],
        },
        Case {
            name: "the SAP HANA connection replaced by another ARN",
            application_id: "destroy-connection",
            registered: &["SYSTEMDB"],
            registered_connection: Some(Resource::Named(COLON_SEGMENT_DATABASE_ARN)),
            add_or_update: &[],
            remove: &[],
            connection: Some(Resource::Named(REPLACEMENT_DATABASE_ARN)),
            backint: false,
            destroyed: &[Resource::Named(COLON_SEGMENT_DATABASE_ARN)],
            kept: &[Resource::Derived("SYSTEMDB")],
        },
        Case {
            name: "every setting changed at once",
            application_id: "destroy-everything",
            registered: &["SYSTEMDB", "TNT1"],
            registered_connection: Some(Resource::Named(COLON_SEGMENT_DATABASE_ARN)),
            add_or_update: &["TNT2"],
            remove: &["TNT1"],
            connection: Some(Resource::Named(REPLACEMENT_DATABASE_ARN)),
            backint: true,
            destroyed: &[
                Resource::Derived("TNT1"),
                Resource::Named(COLON_SEGMENT_DATABASE_ARN),
            ],
            kept: &[Resource::Derived("SYSTEMDB")],
        },
        // The set of databases is what decides, not the credential list: a database the
        // connection still names is still derived once its credential is gone, so nothing
        // held on it is touched.
        Case {
            name: "a credential removed for a database the connection still names",
            application_id: "destroy-nothing-connected",
            registered: &["SYSTEMDB", "TNT1"],
            registered_connection: Some(Resource::Derived("TNT1")),
            add_or_update: &[],
            remove: &["TNT1"],
            connection: None,
            backint: false,
            destroyed: &[],
            kept: &[Resource::Derived("SYSTEMDB"), Resource::Derived("TNT1")],
        },
        Case {
            name: "an update that names no credential at all",
            application_id: "destroy-nothing-backint",
            registered: &["SYSTEMDB", "TNT1"],
            registered_connection: None,
            add_or_update: &[],
            remove: &[],
            connection: None,
            backint: true,
            destroyed: &[],
            kept: &[Resource::Derived("SYSTEMDB"), Resource::Derived("TNT1")],
        },
    ]
}

/// The `RegisterApplication` body of `case`: a HANA application whose credentials name
/// its registered databases, and whose SAP HANA connection names the row's ARN when it
/// names one.
fn registration_body(case: &Case) -> Value {
    let mut body = registration(case.application_id, &[], Some(case.registered));
    if let Some(connection) = case.registered_connection {
        body.as_object_mut()
            .expect("a registration body is a JSON object")
            .insert(
                "DatabaseArn".to_string(),
                json!(connection.arn(case.application_id)),
            );
    }
    body
}

/// The `UpdateApplicationSettings` body of `case`, carrying exactly the settings the row
/// names.
fn update_body(case: &Case) -> Value {
    let mut body = Map::new();
    body.insert("ApplicationId".to_string(), json!(case.application_id));
    if !case.add_or_update.is_empty() {
        body.insert(
            "CredentialsToAddOrUpdate".to_string(),
            credentials(case.add_or_update),
        );
    }
    if !case.remove.is_empty() {
        body.insert("CredentialsToRemove".to_string(), credentials(case.remove));
    }
    if let Some(connection) = case.connection {
        body.insert(
            "DatabaseArn".to_string(),
            json!(connection.arn(case.application_id)),
        );
    }
    if case.backint {
        body.insert(
            "Backint".to_string(),
            json!({
                "BackintMode": BACKINT_MODE_VALUES[0],
                "EnsureNoBackupInProcess": false,
            }),
        );
    }
    Value::Object(body)
}

/// The tag set every resource under test is tagged with, so a read that reports it is
/// reporting what was persisted on that resource.
fn tags() -> Vec<(String, String)> {
    pairs(&[("env", "dev"), ("owner", "sap-team")])
}

/// The source resource the `index`-th grant of a row is made to, so each resource's policy
/// is distinguishable from every other's.
fn source_arn(index: usize) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/sap-restore-{index}")
}

/// `source` as the set [`assert_policy_grants`] compares a document against.
fn one_source(source: &str) -> BTreeSet<String> {
    BTreeSet::from([source.to_string()])
}

/// `UpdateApplicationSettings` for `body`, asserting the update itself was accepted.
async fn update(emulator: &Emulator, case: &str, body: Value) {
    let reply = emulator.send("/update-application-settings", body).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from UpdateApplicationSettings, got {reply:?}"
    );
}

/// `DeleteResourcePermission` for every grant held on `resource_arn`.
async fn delete_permission(emulator: &Emulator, resource_arn: &str) -> Reply {
    emulator
        .send(
            "/delete-resource-permission",
            json!({ "ResourceArn": resource_arn }),
        )
        .await
}

/// The ARN of every database `ListDatabases` reports for `application_id`.
async fn database_arns(emulator: &Emulator, case: &str, application_id: &str) -> BTreeSet<String> {
    let reply = emulator
        .send("/list-databases", json!({"ApplicationId": application_id}))
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from ListDatabases, got {reply:?}"
    );
    listed(&reply)
        .into_iter()
        .filter_map(|summary| summary.arn)
        .collect()
}

/// Attach the tag set and one `RESTORE` grant to `arn`, asserting each write was served
/// and reports back what it persisted — the state the rule is about the fate of.
async fn hold(emulator: &Emulator, case: &str, arn: &str, source: &str) {
    let reply = tag(emulator, arn, &tags()).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from TagResource for {arn}, got {reply:?}"
    );
    let reply = put_permission(emulator, arn, action(), source).await;
    let document = policy(&reply, case);
    assert_policy_grants(&document, arn, action(), &one_source(source), case);
}

/// Assert `arn` still holds exactly what [`hold`] placed on it.
async fn assert_still_held(emulator: &Emulator, case: &str, arn: &str, source: &str) {
    let reply = list_tags(emulator, arn).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from ListTagsForResource for {arn}, got {reply:?}"
    );
    let reported = tag_pairs(reply.member("tags"));
    let expected: BTreeMap<String, String> = tags().into_iter().collect();
    assert_eq!(
        reported, expected,
        "case {case:?}: expected {arn} to still hold {expected:?}, got {reported:?}"
    );
    let reply = get_permission(emulator, arn, Some(action())).await;
    let document = policy(&reply, case);
    assert_policy_grants(&document, arn, action(), &one_source(source), case);
}

/// Assert `arn` names NOTHING: every call that takes a resource ARN refuses it — the read
/// and the delete of its tags as the not-found class the tagging routes carry, and the
/// read, the grant and the revocation of its permissions as the modelled
/// `ValidationException` the database-scoped permission calls answer for an ARN
/// that names no database.
async fn assert_gone(emulator: &Emulator, case: &str, arn: &str) {
    let label = format!("{case} / {arn}");
    assert_not_found(&list_tags(emulator, arn).await, arn, &label);
    let keys: Vec<String> = tags().into_iter().map(|(key, _)| key).collect();
    assert_not_found(&untag(emulator, arn, &keys).await, arn, &label);
    assert_invalid_database_arn(
        &get_permission(emulator, arn, Some(action())).await,
        None,
        arn,
        &label,
    );
    assert_invalid_database_arn(&delete_permission(emulator, arn).await, None, arn, &label);
    assert_invalid_database_arn(
        &put_permission(emulator, arn, action(), &source_arn(0)).await,
        Some(RESOURCE_ARN_MEMBER),
        arn,
        &label,
    );
}

/// Whether the partition still holds tags, or grants, keyed by `arn`.
///
/// Read through the store because an entry left for a resource that no longer exists is
/// exactly what no API can report: every call that takes an ARN refuses one that names
/// nothing, so a leak is invisible over the wire — and it is what keeps the Region
/// partition from being reclaimed.
fn still_in_store(emulator: &Emulator, arn: &str) -> (bool, bool) {
    let mut held = (false, false);
    emulator.with_state(|state| {
        let region = state.region(REGION);
        held = (
            region.tags_by_arn.contains_key(arn),
            region.resource_permissions.contains_key(arn),
        );
    });
    held
}

/// RULE: what is held ON a database — its tags and its resource permissions — goes with
/// that database when a settled update leaves the application no longer deriving it, and
/// a database the update leaves derived keeps everything held on it.
#[tokio::test]
async fn what_is_held_on_a_database_an_update_destroys_goes_with_it() {
    let emulator = Emulator::new();

    for case in cases() {
        register(&emulator, case.name, registration_body(&case)).await;
        // The registration is asynchronous: waiting it out leaves the application owed
        // nothing, so the update below is accepted rather than refused as a conflict.
        settle().await;

        // Everything the row names — destroyed or kept — is tagged and granted on BEFORE
        // the update, so what the update does to it is the only thing under test.
        let mut sources: BTreeMap<String, String> = BTreeMap::new();
        for (index, resource) in case.held_on().enumerate() {
            let arn = resource.arn(case.application_id);
            let source = source_arn(index);
            hold(&emulator, case.name, &arn, &source).await;
            sources.insert(arn, source);
        }

        update(&emulator, case.name, update_body(&case)).await;
        // The settings are asynchronous too: they fall due on the clock, and so does what
        // the databases they destroy were holding.
        settle().await;

        let derived = database_arns(&emulator, case.name, case.application_id).await;

        for resource in case.destroyed {
            let arn = resource.arn(case.application_id);
            assert!(
                !derived.contains(&arn),
                "case {:?}: expected the update to stop {arn} being derived, ListDatabases \
                 still reports it among {derived:?}",
                case.name
            );
            assert_gone(&emulator, case.name, &arn).await;
            let (tagged, granted) = still_in_store(&emulator, &arn);
            assert!(
                !tagged && !granted,
                "case {:?}: nothing may be held for the destroyed {arn}, and the partition \
                 still holds tags: {tagged}, grants: {granted}",
                case.name
            );
        }

        for resource in case.kept {
            let arn = resource.arn(case.application_id);
            assert!(
                derived.contains(&arn),
                "case {:?}: expected {arn} to still be derived, ListDatabases reports only \
                 {derived:?}",
                case.name
            );
            let source = sources
                .get(&arn)
                .unwrap_or_else(|| panic!("case {:?}: {arn} was granted above", case.name));
            assert_still_held(&emulator, case.name, &arn, source).await;
        }
    }
}

/// RULE: the resource is what stops existing, so a database ANOTHER registration of the
/// Region still names is not destroyed by an update that stops one application deriving
/// it — and everything held on it survives.
#[tokio::test]
async fn a_database_another_registration_still_names_keeps_what_is_held_on_it() {
    let emulator = Emulator::new();
    let case = "a database two registrations name by ARN";

    // Both applications point their SAP HANA connection at the SAME database, which is
    // what a `DatabaseArn` naming a resource of its own makes possible.
    for application_id in ["shared-one", "shared-two"] {
        let shared = Case {
            name: case,
            application_id,
            registered: &["SYSTEMDB"],
            registered_connection: Some(Resource::Named(COLON_SEGMENT_DATABASE_ARN)),
            add_or_update: &[],
            remove: &[],
            connection: None,
            backint: false,
            destroyed: &[],
            kept: &[],
        };
        register(&emulator, case, registration_body(&shared)).await;
    }
    settle().await;

    let source = source_arn(0);
    hold(&emulator, case, COLON_SEGMENT_DATABASE_ARN, &source).await;

    // One of them points its connection elsewhere: that application stops deriving the
    // shared database, the other does not, so the resource still exists.
    update(
        &emulator,
        case,
        json!({
            "ApplicationId": "shared-one",
            "DatabaseArn": REPLACEMENT_DATABASE_ARN,
        }),
    )
    .await;
    settle().await;

    let derived = database_arns(&emulator, case, "shared-two").await;
    assert!(
        derived.contains(COLON_SEGMENT_DATABASE_ARN),
        "case {case:?}: expected the other registration to still derive \
         {COLON_SEGMENT_DATABASE_ARN}, ListDatabases reports only {derived:?}"
    );
    assert_still_held(&emulator, case, COLON_SEGMENT_DATABASE_ARN, &source).await;
}

/// How a destroyed database is derived again: the update that destroys it and the update
/// that brings it back under the SAME ARN.
struct Revival {
    /// How an assertion names the row.
    name: &'static str,
    application_id: &'static str,
    /// The `DatabaseName` of every credential the registration carries.
    registered: &'static [&'static str],
    /// The `DatabaseArn` the registration names, when it names one.
    registered_connection: Option<Resource>,
    /// The database that is destroyed and then derived again.
    resource: Resource,
    /// The update that destroys it.
    destroy: fn(&str) -> Value,
    /// The update that derives it again.
    revive: fn(&str) -> Value,
}

/// RULE: a database derived again under the ARN of one a settled update destroyed is a NEW
/// resource — the ARNs of this service are a function of the ids they embed, so it carries
/// no tag and no grant of the database that was destroyed.
#[tokio::test]
async fn a_database_derived_again_carries_nothing_of_the_destroyed_one() {
    let emulator = Emulator::new();

    // Rows: how the database comes back — as a credential named again (with another
    // secret, since the secret is not what identifies it), and as the application's SAP
    // HANA connection pointed back at the ARN it had named.
    let rows = [
        Revival {
            name: "the credential re-added",
            application_id: "revived-credential",
            registered: &["SYSTEMDB", "TNT1"],
            registered_connection: None,
            resource: Resource::Derived("TNT1"),
            destroy: |application_id| {
                json!({
                    "ApplicationId": application_id,
                    "CredentialsToRemove": credentials(&["TNT1"]),
                })
            },
            revive: |application_id| {
                json!({
                    "ApplicationId": application_id,
                    "CredentialsToAddOrUpdate": [{
                        "DatabaseName": "TNT1",
                        "CredentialType": "ADMIN",
                        "SecretId": "sap/tnt1/admin-rotated",
                    }],
                })
            },
        },
        Revival {
            name: "the connection pointed back",
            application_id: "revived-connection",
            registered: &["SYSTEMDB"],
            registered_connection: Some(Resource::Named(COLON_SEGMENT_DATABASE_ARN)),
            resource: Resource::Named(COLON_SEGMENT_DATABASE_ARN),
            destroy: |application_id| {
                json!({
                    "ApplicationId": application_id,
                    "DatabaseArn": REPLACEMENT_DATABASE_ARN,
                })
            },
            revive: |application_id| {
                json!({
                    "ApplicationId": application_id,
                    "DatabaseArn": COLON_SEGMENT_DATABASE_ARN,
                })
            },
        },
    ];

    for row in rows {
        let case = Case {
            name: row.name,
            application_id: row.application_id,
            registered: row.registered,
            registered_connection: row.registered_connection,
            add_or_update: &[],
            remove: &[],
            connection: None,
            backint: false,
            destroyed: &[],
            kept: &[],
        };
        register(&emulator, row.name, registration_body(&case)).await;
        settle().await;

        let arn = row.resource.arn(row.application_id);
        hold(&emulator, row.name, &arn, &source_arn(0)).await;

        update(&emulator, row.name, (row.destroy)(row.application_id)).await;
        settle().await;
        assert_gone(&emulator, row.name, &arn).await;

        update(&emulator, row.name, (row.revive)(row.application_id)).await;
        settle().await;

        // The database is derived again, under exactly the ARN it had before.
        let derived = database_arns(&emulator, row.name, row.application_id).await;
        assert!(
            derived.contains(&arn),
            "case {:?}: expected {arn} to be derived again, ListDatabases reports only \
             {derived:?}",
            row.name
        );

        // And it is a new resource: no tag and no policy at all.
        let reply = list_tags(&emulator, &arn).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200 from ListTagsForResource for the re-derived {arn}, got \
             {reply:?}",
            row.name
        );
        let reported = tag_pairs(reply.member("tags"));
        assert!(
            reported.is_empty(),
            "case {:?}: a freshly derived database carries no tag, got {reported:?}",
            row.name
        );

        let reply = get_permission(&emulator, &arn, Some(action())).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200 from GetResourcePermission for the re-derived {arn}, \
             got {reply:?}",
            row.name
        );
        let rendered = reply.member("Policy").cloned().unwrap_or(Value::Null);
        assert!(
            rendered.is_null(),
            "case {:?}: a freshly derived database holds no grant, expected no Policy, got \
             {rendered:?}",
            row.name
        );

        // The listing reports it with no tags either, which is the same source of truth.
        let reply = emulator
            .send(
                "/list-databases",
                json!({"ApplicationId": row.application_id}),
            )
            .await;
        let tagged: Vec<String> = listed(&reply)
            .into_iter()
            .filter(|summary| summary.tagged)
            .filter_map(|summary| summary.arn)
            .collect();
        assert!(
            tagged.is_empty(),
            "case {:?}: expected no database of the application to report Tags, got \
             {tagged:?}",
            row.name
        );
    }
}
