// SPDX-License-Identifier: CC0-1.0

//! What a registration's `DatabaseArn` NAMES, and what a completed deregistration
//! therefore forgets — driven over the wire.
//!
//! RULE under test: an ARN of this service names the resource its own resource type and
//! the ids that type carries identify, so a member documented to carry "the Amazon
//! Resource Name of the SAP HANA database" (`smithy_query(query="doc",
//! shape="com.amazonaws.ssmsap#RegisterApplicationInput", member="DatabaseArn")`) names a
//! database only when the ARN is one: an ssm-sap ARN of another KIND — a registered
//! application, a component — names no database of the registration that mentions it, and
//! an ARN of another service names a resource this store holds nothing of, which the
//! registration reports as its database under exactly that ARN. What is held PER RESOURCE
//! (its tags and the resource permissions granted on it, both keyed by the ARN the
//! resource is reported under) therefore belongs to the registration that OWNS that
//! resource: it goes when that registration's deregistration completes, whatever other
//! registration's `DatabaseArn` mentions the ARN, and a registration made afterwards
//! under the same `ApplicationId` is a new resource carrying neither. A DATABASE several
//! registrations name is ONE resource, so it keeps what is held on it while any of them
//! still names it.
//!
//! Every call goes through the real router (the harness
//! `tests/all/application_lifecycle.rs` owns), so the restJson1 routing, the boundary
//! constraint walk, the handler and the response serialization all run exactly as they do
//! in the emulator binary. Nothing reaches into `crate::state`: what a registration holds
//! is only believed when an API of this service reports it back.
//!
//! The tables span what the rule generalises over: the KIND of resource the ARN names (a
//! database of this service, a database of another service, a registered application, a
//! component), whose resource it is (another registration's, the naming registration's
//! own), the ARN's form (`/`-separated, `:`-separated, a resource type that merely begins
//! with the database one), and whether the resource is one several registrations name. The
//! issue's `VICTIM`/`KEEPER` pair is ONE row of the second table.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_ssmsap_ssmsap::ops::types::PERMISSION_ACTION_TYPE_VALUES;

use crate::application_lifecycle::{
    settle, tag_pairs, valid_registration, Emulator, Reply, REGION,
};
use crate::list_databases::listed;
use crate::resource_permissions::{
    assert_invalid_database_arn, assert_not_found, assert_policy_grants, get as get_permission,
    policy, put as put_permission, COLON_SEGMENT_DATABASE_ARN, RESOURCE_ARN_MEMBER,
};
use crate::resource_tags::{application_arn, database_arn, list_tags, register, registration};
use crate::tag_resource::{pairs, tag};
use crate::validation_message_rendering::{message, VALIDATION_EXCEPTION};

/// The account every emulated ARN carries, from the core's single source of truth.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The `ComponentType` of the SAP HANA component of an application's own SAP system: one
/// of the model's `ComponentType` values (`smithy_query(query="shape",
/// shape="ComponentType")`).
const HANA: &str = "HANA";

/// The `Sid` the owning registrations below carry, so each of them describes the SAP HANA
/// component of its own SAP system; it is of the modelled `SID` form (`^[A-Z][A-Z0-9]{2}$`).
const OWNER_SID: &str = "PRD";

/// The `DatabaseName` the owning registrations' credential names, so each of them has one
/// SAP HANA database of its own — reported under the sub-resource ARN this service mints
/// from the application and database ids.
const OWNER_DATABASE: &str = "SYSTEMDB";

/// The source resource a grant below is made to: a source is the counterparty a restore is
/// permitted for, so it need not be a resource of this service at all.
const SOURCE: &str = "arn:aws:s3:::a-bucket/an-object";

/// The `PermissionActionType` every grant below is made with — the model's own catalogue,
/// read rather than spelled.
fn action() -> &'static str {
    PERMISSION_ACTION_TYPE_VALUES[0]
}

/// The tag set every resource under test is tagged with, so a read reporting it is
/// reporting what was persisted on that resource.
fn tags() -> Vec<(String, String)> {
    pairs(&[("env", "prod")])
}

/// The `ComponentId` of a component declared with `sid` and `component_type`: a component
/// is identified by the System ID and the type it was declared with.
fn component_id(sid: &str, component_type: &str) -> String {
    format!("{sid}-{component_type}")
}

/// The ARN a component of `application_id` carries in [`REGION`]: a `ComponentId` is
/// unique only within its application, so the resource path carries both, under the same
/// `<kind>/<id>` form as the application's own ARN.
fn component_arn(application_id: &str, component_id: &str) -> String {
    format!("arn:aws:ssm-sap:{REGION}:{ACCOUNT}:component/{application_id}/{component_id}")
}

/// The ARN of the SAP HANA component of `application_id`'s own SAP system.
fn hana_component_arn(application_id: &str) -> String {
    component_arn(application_id, &component_id(OWNER_SID, HANA))
}

/// A registration that OWNS resources: a HANA application with a System ID, so it has a
/// SAP HANA component, and one credential, so it has one SAP HANA database of its own.
///
/// `database_arn` is the SAP HANA connection it names, for the rows where the owning
/// registration names the same ARN another one does.
fn owner_registration(application_id: &str, database_arn: Option<&str>) -> Value {
    let mut body = registration(application_id, &[], Some(&[OWNER_DATABASE]));
    let members = body
        .as_object_mut()
        .expect("a registration body is a JSON object");
    members.insert("Sid".to_string(), json!(OWNER_SID));
    if let Some(arn) = database_arn {
        members.insert("DatabaseArn".to_string(), json!(arn));
    }
    body
}

/// A registration that names `database_arn` as its SAP HANA connection and nothing else:
/// an ABAP application requires no System ID and no credentials, so every database it has
/// is one that ARN names.
fn naming_registration(application_id: &str, database_arn: &str) -> Value {
    let mut body = valid_registration(application_id, "SAP_ABAP");
    body.as_object_mut()
        .expect("a registration body is a JSON object")
        .insert("DatabaseArn".to_string(), json!(database_arn));
    body
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

/// Attach the tag set to `arn`, and one grant too when the ARN names a SAP HANA database
/// — the resource-permission calls are database-scoped, so an ARN of another kind holds
/// tags and no grant at all. Each write is asserted served and reporting back what it
/// persisted: the state the rule is about the fate of.
async fn hold(emulator: &Emulator, case: &str, arn: &str, names_a_database: bool) {
    let reply = tag(emulator, arn, &tags()).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from TagResource for {arn}, got {reply:?}"
    );
    if !names_a_database {
        // Granting on it is refused rather than held, because the ARN names no database.
        assert_invalid_database_arn(
            &put_permission(emulator, arn, action(), SOURCE).await,
            Some(RESOURCE_ARN_MEMBER),
            arn,
            case,
        );
        return;
    }
    let reply = put_permission(emulator, arn, action(), SOURCE).await;
    let document = policy(&reply, case);
    assert_policy_grants(&document, arn, action(), &one_source(), case);
}

/// [`SOURCE`] as the set [`assert_policy_grants`] compares a document against.
fn one_source() -> BTreeSet<String> {
    BTreeSet::from([SOURCE.to_string()])
}

/// Assert `arn` still holds exactly what [`hold`] placed on it.
async fn assert_still_held(emulator: &Emulator, case: &str, arn: &str) {
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
    assert_policy_grants(&document, arn, action(), &one_source(), case);
}

/// Assert nothing is held on `arn` any more: the tag read refuses it with the not-found
/// class the tagging routes carry and the permission read as the modelled
/// `ValidationException` for an ARN that names no database, because the resource it named
/// has stopped existing.
async fn assert_gone(emulator: &Emulator, case: &str, arn: &str) {
    assert_not_found(&list_tags(emulator, arn).await, arn, case);
    assert_invalid_database_arn(
        &get_permission(emulator, arn, Some(action())).await,
        None,
        arn,
        case,
    );
}

/// Deregister `application_id`, asserting the deregistration itself was accepted.
async fn deregister(emulator: &Emulator, case: &str, application_id: &str) {
    let reply = emulator
        .send(
            "/deregister-application",
            json!({ "ApplicationId": application_id }),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from DeregisterApplication, got {reply:?}"
    );
}

/// `GetApplication` for `application_id`.
async fn get_application(emulator: &Emulator, application_id: &str) -> Reply {
    emulator
        .send("/get-application", json!({"ApplicationId": application_id}))
        .await
}

/// The `ApplicationId` of the registration whose resources the first table's ARNs name.
const KIND_OWNER: &str = "arn-kind-owner";

/// One registration naming an ARN as its SAP HANA connection, and whether that ARN names
/// a database at all.
struct Named {
    /// How an assertion names the row.
    name: &'static str,
    /// The `ApplicationId` the naming registration is made under.
    application_id: &'static str,
    /// The `DatabaseArn` it carries, built from the id it is registered under so a row can
    /// name the naming registration's OWN resources.
    arn: fn(&str) -> String,
    /// Whether that ARN names a SAP HANA database, so the registration has that database.
    names_a_database: bool,
}

/// The rows: one naming registration each, spanning the kinds of resource an ARN can name
/// and the forms an ARN can take.
fn named_rows() -> Vec<Named> {
    vec![
        // A database of ANOTHER service: this store holds no resource of it, so the
        // registration's claim that it is its SAP HANA database stands, under that ARN.
        Named {
            name: "another service's database ARN",
            application_id: "arn-kind-foreign",
            arn: |_| COLON_SEGMENT_DATABASE_ARN.to_string(),
            names_a_database: true,
        },
        // A database ARN of this service belonging to another registration: one database,
        // named by both.
        Named {
            name: "a database ARN of another registration",
            application_id: "arn-kind-shared-database",
            arn: |_| database_arn(REGION, KIND_OWNER, OWNER_DATABASE),
            names_a_database: true,
        },
        // A database ARN of this service in the naming registration's own namespace.
        Named {
            name: "a database ARN of the naming registration itself",
            application_id: "arn-kind-own-database",
            arn: |application_id| database_arn(REGION, application_id, "TNT1"),
            names_a_database: true,
        },
        // The same kind, spelled in the `:`-separated branch of the modelled `SsmSapArn`
        // pattern (`^arn:(.+:){2,4}.+$`): the resource type is the field after the
        // account however the resource path is separated.
        Named {
            name: "a colon-separated database ARN of this service",
            application_id: "arn-kind-colon-database",
            arn: |_| {
                format!("arn:aws:ssm-sap:{REGION}:{ACCOUNT}:database:{KIND_OWNER}:{OWNER_DATABASE}")
            },
            names_a_database: true,
        },
        // The ARN of ANOTHER registration: an application is not a database, so the
        // registration naming it has no database at all.
        Named {
            name: "the application ARN of another registration",
            application_id: "arn-kind-other-application",
            arn: |_| application_arn(REGION, KIND_OWNER),
            names_a_database: false,
        },
        // Not even its own application ARN names a database.
        Named {
            name: "the naming registration's own application ARN",
            application_id: "arn-kind-own-application",
            arn: |application_id| application_arn(REGION, application_id),
            names_a_database: false,
        },
        // A component of another registration is not a database either.
        Named {
            name: "a component ARN of another registration",
            application_id: "arn-kind-other-component",
            arn: |_| hana_component_arn(KIND_OWNER),
            names_a_database: false,
        },
        // The resource type is a WHOLE segment, not a prefix: a type that merely begins
        // with the database one is another kind of resource.
        Named {
            name: "a resource type that only begins with the database one",
            application_id: "arn-kind-prefix",
            arn: |_| format!("arn:aws:ssm-sap:{REGION}:{ACCOUNT}:databases/{KIND_OWNER}/HDB"),
            names_a_database: false,
        },
    ]
}

/// RULE: a `DatabaseArn` names one of the registration's SAP HANA databases only when it
/// names a DATABASE — an ARN of this service naming a resource of another kind names no
/// database, and `ListDatabases` and `GetDatabase` agree about which databases there are.
#[tokio::test]
async fn only_an_arn_that_names_a_database_names_a_database_of_a_registration() {
    let emulator = Emulator::new();

    // The registration whose resources the rows point at: its own ARN, the ARN of the SAP
    // HANA component of its SAP system, and the ARN of the database its credential names.
    register(
        &emulator,
        "the owning registration",
        owner_registration(KIND_OWNER, None),
    )
    .await;
    let rows = named_rows();
    for row in &rows {
        register(
            &emulator,
            row.name,
            naming_registration(row.application_id, &(row.arn)(row.application_id)),
        )
        .await;
    }
    // One wait for every registration: each is asynchronous, and nothing below depends on
    // when a particular one of them settled.
    settle().await;

    for row in &rows {
        let arn = (row.arn)(row.application_id);
        let reported = database_arns(&emulator, row.name, row.application_id).await;
        let expected: BTreeSet<String> = if row.names_a_database {
            BTreeSet::from([arn.clone()])
        } else {
            BTreeSet::new()
        };
        assert_eq!(
            reported, expected,
            "case {:?}: for the DatabaseArn {arn} expected ListDatabases to report \
             {expected:?}, got {reported:?}",
            row.name
        );

        // The same set, resolved one database at a time: a `GetDatabase` by that very ARN
        // answers exactly when the listing reports a database under it.
        let reply = emulator
            .send("/get-database", json!({"DatabaseArn": arn}))
            .await;
        if row.names_a_database {
            assert_eq!(
                reply.status, 200,
                "case {:?}: expected GetDatabase to resolve {arn}, got {reply:?}",
                row.name
            );
        } else {
            // The modelled refusal for a reference no database of the Region answers:
            // `GetDatabase` models no `ResourceNotFoundException`, so the reference itself
            // is refused with `ValidationException` (HTTP 400), naming it.
            assert_eq!(
                reply.status, 400,
                "case {:?}: {arn} names no database, expected 400 from GetDatabase, got \
                 {reply:?}",
                row.name
            );
            assert_eq!(
                reply.code(),
                VALIDATION_EXCEPTION,
                "case {:?}: expected {VALIDATION_EXCEPTION} for {arn}, got {reply:?}",
                row.name
            );
            assert!(
                reply.member("Database").is_none(),
                "case {:?}: a refusal must report no database, got {reply:?}",
                row.name
            );
            let stated = message(&reply);
            assert!(
                stated.contains(&arn),
                "case {:?}: expected the refusal to name {arn:?}, got {stated:?}",
                row.name
            );
        }
    }
}

/// One owning registration, the ARN of one of its resources that ANOTHER registration
/// names as its SAP HANA connection, and what must become of what is held on that ARN once
/// the owning registration's deregistration completes.
struct Owned {
    /// How an assertion names the row.
    name: &'static str,
    /// The `ApplicationId` of the registration the resource belongs to.
    owner: &'static str,
    /// The `ApplicationId` of the registration that names the resource's ARN.
    namer: &'static str,
    /// The ARN both registrations name, built from the owner's id.
    arn: fn(&str) -> String,
    /// Whether the OWNING registration names that ARN as its SAP HANA connection too,
    /// which is how two registrations come to name one resource of another service.
    owner_names_it: bool,
    /// Whether what is held on the ARN survives the owner's deregistration — true exactly
    /// when the resource itself does, because the ARN names a DATABASE the other
    /// registration still names.
    survives: bool,
}

/// The rows: the kinds of resource an ARN can name, each named by a second registration.
fn owned_rows() -> Vec<Owned> {
    vec![
        // The issue's pair: the other registration's `DatabaseArn` is the owner's own
        // application ARN, which names no database and cannot keep the application's tags
        // or grants alive.
        Owned {
            name: "the owner's application ARN named by another registration",
            owner: "held-app-owner",
            namer: "held-app-namer",
            arn: application_arn_of,
            owner_names_it: false,
            survives: false,
        },
        Owned {
            name: "the owner's component ARN named by another registration",
            owner: "held-component-owner",
            namer: "held-component-namer",
            arn: hana_component_arn_of,
            owner_names_it: false,
            survives: false,
        },
        // A DATABASE two registrations name is ONE resource: the other registration still
        // has it, so it still exists and keeps everything held on it.
        Owned {
            name: "a database of this service two registrations name",
            owner: "held-database-owner",
            namer: "held-database-namer",
            arn: owner_database_arn_of,
            owner_names_it: false,
            survives: true,
        },
        Owned {
            name: "a database of another service two registrations name",
            owner: "held-foreign-owner",
            namer: "held-foreign-namer",
            arn: |_| COLON_SEGMENT_DATABASE_ARN.to_string(),
            owner_names_it: true,
            survives: true,
        },
    ]
}

/// The application ARN of `application_id` in [`REGION`], as a row's ARN builder.
fn application_arn_of(application_id: &str) -> String {
    application_arn(REGION, application_id)
}

/// The ARN of the SAP HANA component of `application_id`, as a row's ARN builder.
fn hana_component_arn_of(application_id: &str) -> String {
    hana_component_arn(application_id)
}

/// The ARN of the database `application_id`'s credential names, as a row's ARN builder.
fn owner_database_arn_of(application_id: &str) -> String {
    database_arn(REGION, application_id, OWNER_DATABASE)
}

/// RULE: what is held on a resource belongs to the registration that OWNS it, so a
/// completed deregistration forgets the tags and the grants held on the ARNs of ITS
/// resources however another registration's `DatabaseArn` mentions them — while a database
/// another registration still names is one resource that still exists and keeps them.
#[tokio::test]
async fn what_is_held_on_a_resource_goes_with_the_registration_that_owns_it() {
    let emulator = Emulator::new();
    let rows = owned_rows();

    for row in &rows {
        let arn = (row.arn)(row.owner);
        register(
            &emulator,
            row.name,
            owner_registration(row.owner, row.owner_names_it.then_some(arn.as_str())),
        )
        .await;
        register(&emulator, row.name, naming_registration(row.namer, &arn)).await;
    }
    settle().await;

    // Everything the rows name is tagged and granted on BEFORE any deregistration, so
    // what the deregistration does to it is the only thing under test. An ARN that names
    // no database holds tags alone, since the permission calls refuse it.
    for row in &rows {
        hold(&emulator, row.name, &(row.arn)(row.owner), row.survives).await;
    }

    for row in &rows {
        deregister(&emulator, row.name, row.owner).await;
    }
    // Deregistration is asynchronous: the removal falls due on the clock, and so does
    // what the resources it destroys were holding.
    settle().await;

    for row in &rows {
        let arn = (row.arn)(row.owner);
        let fetched = get_application(&emulator, row.owner).await;
        assert!(
            fetched.status >= 400 && fetched.member("Application").is_none(),
            "case {:?}: expected the deregistered {} to be gone, got {fetched:?}",
            row.name,
            row.owner
        );

        let named = database_arns(&emulator, row.name, row.namer).await;
        if row.survives {
            assert!(
                named.contains(&arn),
                "case {:?}: expected the other registration to still report the database \
                 {arn}, ListDatabases reports only {named:?}",
                row.name
            );
            assert_still_held(&emulator, row.name, &arn).await;
        } else {
            assert!(
                named.is_empty(),
                "case {:?}: {arn} names no database, expected the other registration to \
                 report none, got {named:?}",
                row.name
            );
            assert_gone(&emulator, row.name, &arn).await;
        }
    }
}

/// One registration whose resource another registration names, registered AGAIN under the
/// same `ApplicationId` once its deregistration has completed.
struct Reborn {
    /// How an assertion names the row.
    name: &'static str,
    /// The `ApplicationId` of the registration the resource belongs to, which is
    /// registered twice.
    owner: &'static str,
    /// The `ApplicationId` of the registration that names the resource's ARN.
    namer: &'static str,
    /// The ARN under test, built from the owner's id.
    arn: fn(&str) -> String,
}

/// The rows: the resources whose ARN a registration under the same `ApplicationId` mints
/// again — the application itself, and the component its System ID describes.
fn reborn_rows() -> Vec<Reborn> {
    vec![
        Reborn {
            name: "the application ARN",
            owner: "reborn-app-owner",
            namer: "reborn-app-namer",
            arn: application_arn_of,
        },
        Reborn {
            name: "the component ARN",
            owner: "reborn-component-owner",
            namer: "reborn-component-namer",
            arn: hana_component_arn_of,
        },
    ]
}

/// RULE: the ARNs of this service are a function of the ids they embed, so a resource
/// derived again under the ARN of one that stopped existing is a NEW resource: it carries
/// neither the tags nor the grants of the one that was destroyed, whatever another
/// registration's `DatabaseArn` mentioned in the meantime.
#[tokio::test]
async fn a_registration_under_the_same_id_inherits_nothing_held_on_the_previous_one() {
    let emulator = Emulator::new();

    let rows = reborn_rows();
    for row in &rows {
        register(&emulator, row.name, owner_registration(row.owner, None)).await;
        register(
            &emulator,
            row.name,
            naming_registration(row.namer, &(row.arn)(row.owner)),
        )
        .await;
    }
    settle().await;
    for row in &rows {
        // Every row's ARN names a resource of another kind than a database (the
        // application itself, its SAP HANA component), so each holds tags alone.
        hold(&emulator, row.name, &(row.arn)(row.owner), false).await;
    }
    for row in &rows {
        deregister(&emulator, row.name, row.owner).await;
    }
    settle().await;

    // The SAME id registered again, carrying no tag of its own.
    for row in &rows {
        register(&emulator, row.name, owner_registration(row.owner, None)).await;
    }
    settle().await;

    for row in &rows {
        let (name, owner) = (row.name, row.owner);
        let arn = (row.arn)(owner);
        let reply = list_tags(&emulator, &arn).await;
        assert_eq!(
            reply.status, 200,
            "case {name:?}: expected 200 from ListTagsForResource for the fresh {arn}, got \
             {reply:?}"
        );
        let reported = tag_pairs(reply.member("tags"));
        assert!(
            reported.is_empty(),
            "case {name:?}: the fresh {arn} carries no tag of the destroyed resource, got \
             {reported:?}"
        );

        // The same answer through the application's own read, which joins on the same ARN.
        let fetched = get_application(&emulator, owner).await;
        let by_get_application = tag_pairs(fetched.member("Tags"));
        assert!(
            by_get_application.is_empty(),
            "case {name:?}: expected GetApplication to report no tags for the fresh {owner}, \
             got {by_get_application:?}"
        );

        // The permission calls refuse the fresh ARN for what it is — a resource of another
        // kind than a database — so nothing of the destroyed resource can be reported for
        // it either.
        assert_invalid_database_arn(
            &get_permission(&emulator, &arn, Some(action())).await,
            None,
            &arn,
            name,
        );
    }
}
