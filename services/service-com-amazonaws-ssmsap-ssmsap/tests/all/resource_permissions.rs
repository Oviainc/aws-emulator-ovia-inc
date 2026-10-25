// SPDX-License-Identifier: CC0-1.0

//! `PutResourcePermission` and `GetResourcePermission` end to end, over the wire.
//!
//! The rules these cases pin:
//!
//! * A resource permission is the (target resource ARN, `PermissionActionType`, source
//!   resource ARN) triple the service holds, and the `Policy` either operation reports
//!   is PROJECTED from the triples held for that one target — one `Allow` statement per
//!   granted source, in an order that does not depend on the order the grants arrived,
//!   absent entirely for a target nobody has granted anything on, and unchanged by
//!   re-granting a triple already held or by reading it.
//! * The target is a resource this account and Region HOLDS: a registered SAP
//!   application, or a SAP HANA database of one of its registrations. An ARN that names
//!   no such resource is refused with the modelled `ResourceNotFoundException`
//!   (HTTP 404) naming that ARN, by the grant and by the read alike, and the refusal
//!   persists nothing. A grant therefore cannot outlive the registration its target came
//!   from.
//!
//! The tables span what those rules generalise over: the kind of resource the target ARN
//! names (the application itself, a database its credentials named, a database its
//! `DatabaseArn` named), the ARN form of the target (both branches of the modelled `Arn`
//! pattern — the colon-segment form and the `.../path` form), the number of sources
//! granted on one target, the ARN form of a source (a registered database of this Region,
//! a database of another Region or account, another service's resource), every
//! `PermissionActionType` the model declares
//! (`service_com_amazonaws_ssmsap_ssmsap::ops::types::PERMISSION_ACTION_TYPE_VALUES`),
//! and the ways an ARN can name nothing here (never registered, deregistered, another
//! Region, a database the registration never named, another service's resource). A single
//! HANA database target with one source — the shape the issue's reproduction calls — is
//! ONE row.
//!
//! Every call goes through `MultiServiceRouter::dispatch` (the harness
//! `tests/all/application_lifecycle.rs` owns), so the restJson1 routing, the boundary
//! constraint walk, the handler and the response serialization all run exactly as they
//! do in the emulator binary. Nothing reaches into `crate::state` directly: a grant is
//! only believed when a later `GetResourcePermission` reports it back.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::application_lifecycle::{settle, Emulator, Reply};
use crate::resource_tags::{application_arn, database_arn, register, registration};
use crate::validation_message_rendering::{message, VALIDATION_EXCEPTION};
use service_com_amazonaws_ssmsap_ssmsap::ops::types::PERMISSION_ACTION_TYPE_VALUES;
use service_com_amazonaws_ssmsap_ssmsap::state::{
    permission_policy_action, MAX_PERMISSION_SOURCES_PER_RESOURCE, POLICY_EFFECT_ALLOW,
    POLICY_VERSION,
};

/// The account every emulated ARN carries, from the core's single source of truth.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The Region this suite signs for, which is the Region every resource it registers
/// lives in.
const REGION: &str = "us-east-1";

/// A second regional endpoint, so a target can be named at a Region its resource was not
/// registered in.
const OTHER_REGION: &str = "us-west-2";

/// The modelled error code and status an ARN naming no resource is answered with
/// (`smithy_query(query="operation", operation="PutResourcePermission")`:
/// `ResourceNotFoundException` `@httpError 404`, declared for `GetResourcePermission`
/// too).
const RESOURCE_NOT_FOUND_EXCEPTION: &str = "ResourceNotFoundException";

/// One row: a label, the registration that brings the target into being, the target
/// resource ARN, and the source resource ARNs granted on it in the order the test grants
/// them.
struct Case {
    label: &'static str,
    /// The `RegisterApplication` body the row registers before granting anything.
    registration: Value,
    resource: String,
    sources: Vec<String>,
}

/// The rows. Each row owns its OWN target, so the last assertion can check that a grant
/// on one target never shows up in another's policy.
fn cases() -> Vec<Case> {
    vec![
        // The shape the issue's reproduction calls: one HANA database target, one source
        // database. `database/<application>/<database>` is the `.../path` branch of the
        // modelled `Arn` pattern.
        Case {
            label: "one source on the registration's HANA database",
            registration: registration("perm-hdb", &[], Some(&["HDB"])),
            resource: database_arn(REGION, "perm-hdb", "HDB"),
            sources: vec![format!(
                "arn:aws:ssm-sap:{REGION}:{ACCOUNT}:database/perm-source/HDB"
            )],
        },
        // Three sources on one of the three databases a registration named: the
        // statement count follows the grant count, and the source ARNs span a database
        // registered in this Region, one of another Region, and another service's
        // resource — a source is the counterparty a restore is permitted for, which need
        // not be a resource of this account or Region at all.
        Case {
            label: "three sources on one database of a three-database registration",
            registration: registration("perm-multi", &[], Some(&["SYSTEMDB", "TNT1", "TNT2"])),
            resource: database_arn(REGION, "perm-multi", "TNT1"),
            sources: vec![
                database_arn(REGION, "perm-hdb", "HDB"),
                format!("arn:aws:ssm-sap:eu-west-1:{ACCOUNT}:database/perm-far/HDB"),
                "arn:aws:s3:::sap-backint-bucket/backups".to_string(),
            ],
        },
        // The registered application itself as the target, with an IAM role as a source:
        // the rule is about what the ARN RESOLVES to here, not about which resource kind
        // or which service the ARN spells.
        Case {
            label: "two sources on the registered application",
            registration: registration("perm-app", &[], None),
            resource: application_arn(REGION, "perm-app"),
            sources: vec![
                format!("arn:aws:iam::{ACCOUNT}:role/sap-restore"),
                database_arn(REGION, "perm-app", "HDB"),
            ],
        },
        // A database the registration named by `DatabaseArn`: its ARN is the one the
        // registration gave it, which here is in the colon-segment branch of the modelled
        // `Arn` pattern and names another service entirely. It resolves because the
        // registration named it, which is the whole rule.
        Case {
            label: "the database a registration named by ARN",
            registration: registered_by_arn("perm-by-arn", COLON_SEGMENT_DATABASE_ARN),
            resource: COLON_SEGMENT_DATABASE_ARN.to_string(),
            sources: vec![format!(
                "arn:aws:ssm-sap:{REGION}:{ACCOUNT}:database/perm-source/HDB"
            )],
        },
    ]
}

/// A `DatabaseArn` in the colon-segment branch of the modelled `Arn` pattern
/// (`^arn:(.+:){2,4}.+$`), the form `tests/all/registered_database_arn.rs` registers too.
const COLON_SEGMENT_DATABASE_ARN: &str = "arn:aws:rds:us-east-1:000000000000:db:sap-prd";

/// A registration for `application_id` that names its SAP HANA database by
/// `DatabaseArn`, so the database carries exactly that ARN.
fn registered_by_arn(application_id: &str, database_arn: &str) -> Value {
    let mut body = registration(application_id, &[], None);
    body.as_object_mut()
        .expect("a registration body is a JSON object")
        .insert("DatabaseArn".to_string(), json!(database_arn));
    body
}

/// `PutResourcePermission` at `region`'s endpoint.
async fn put_in_region(
    emulator: &Emulator,
    region: &str,
    resource: &str,
    action: &str,
    source: &str,
) -> Reply {
    emulator
        .send_in_region(
            region,
            "/put-resource-permission",
            json!({
                "ResourceArn": resource,
                "ActionType": action,
                "SourceResourceArn": source,
            }),
        )
        .await
}

/// `PutResourcePermission` at [`REGION`]'s endpoint.
async fn put(emulator: &Emulator, resource: &str, action: &str, source: &str) -> Reply {
    put_in_region(emulator, REGION, resource, action, source).await
}

/// `GetResourcePermission` at `region`'s endpoint, with an `ActionType` when `action` is
/// given.
async fn get_in_region(
    emulator: &Emulator,
    region: &str,
    resource: &str,
    action: Option<&str>,
) -> Reply {
    let body = match action {
        Some(action) => json!({ "ResourceArn": resource, "ActionType": action }),
        None => json!({ "ResourceArn": resource }),
    };
    emulator
        .send_in_region(region, "/get-resource-permission", body)
        .await
}

/// `GetResourcePermission` at [`REGION`]'s endpoint.
async fn get(emulator: &Emulator, resource: &str, action: Option<&str>) -> Reply {
    get_in_region(emulator, REGION, resource, action).await
}

/// Register `body` at `region`'s endpoint, asserting the registration was accepted.
async fn register_in_region(emulator: &Emulator, case: &str, region: &str, body: Value) {
    let reply = emulator
        .send_in_region(region, "/register-application", body)
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from RegisterApplication, got {reply:?}"
    );
}

/// The `Policy` member of a reply, which must be present.
fn policy(reply: &Reply, case: &str) -> String {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected HTTP 200, got {reply:?}"
    );
    reply
        .member("Policy")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a Policy member, got {reply:?}"))
        .to_string()
}

/// One statement of a policy document, reduced to what the assertions read.
#[derive(Debug, PartialEq, Eq)]
struct Statement {
    effect: String,
    principal: String,
    action: String,
    resource: String,
}

/// The statements of `document`, in the order the document lists them, after checking
/// the envelope the policy language requires.
fn statements(document: &str, case: &str) -> Vec<Statement> {
    let parsed: Value = serde_json::from_str(document).unwrap_or_else(|error| {
        panic!("case {case:?}: Policy must be a JSON document, got {document:?} ({error})")
    });
    assert_eq!(
        parsed.get("Version").and_then(Value::as_str),
        Some(POLICY_VERSION),
        "case {case:?}: expected policy Version {POLICY_VERSION:?}, got {parsed:?}"
    );
    let list = parsed
        .get("Statement")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a Statement list, got {parsed:?}"));
    list.iter()
        .map(|statement| Statement {
            effect: statement
                .get("Effect")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            principal: statement
                .pointer("/Principal/AWS")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            action: statement
                .get("Action")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            resource: statement
                .get("Resource")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        })
        .collect()
}

/// Assert that `document` is exactly the policy of `resource` granting `action` to
/// `expected_sources` — no more statements, no fewer, each one well formed.
fn assert_policy_grants(
    document: &str,
    resource: &str,
    action: &str,
    expected_sources: &BTreeSet<String>,
    case: &str,
) {
    let statements = statements(document, case);
    assert_eq!(
        statements.len(),
        expected_sources.len(),
        "case {case:?}: expected {} statement(s) for {:?}, got {statements:?}",
        expected_sources.len(),
        expected_sources
    );
    let expected_action = permission_policy_action(action);
    let mut seen = BTreeSet::new();
    for statement in &statements {
        assert_eq!(
            statement.effect, POLICY_EFFECT_ALLOW,
            "case {case:?}: a granted permission must allow, got {statement:?}"
        );
        assert_eq!(
            statement.action, expected_action,
            "case {case:?}: expected Action {expected_action:?}, got {statement:?}"
        );
        assert_eq!(
            statement.resource, resource,
            "case {case:?}: expected Resource {resource:?}, got {statement:?}"
        );
        assert!(
            seen.insert(statement.principal.clone()),
            "case {case:?}: principal {:?} appears twice in {statements:?}",
            statement.principal
        );
    }
    assert_eq!(
        &seen, expected_sources,
        "case {case:?}: expected the granted sources as principals, got {seen:?}"
    );
}

/// Assert the call was refused as the modelled `ResourceNotFoundException`, naming the
/// ARN the request carried.
fn assert_not_found(reply: &Reply, arn: &str, case: &str) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: expected HTTP 404 for an ARN naming no resource, got {reply:?}"
    );
    assert_eq!(
        reply.code(),
        RESOURCE_NOT_FOUND_EXCEPTION,
        "case {case:?}: expected {RESOURCE_NOT_FOUND_EXCEPTION}, got {reply:?}"
    );
    let stated = message(reply);
    assert!(
        stated.contains(arn),
        "case {case:?}: expected the refusal to name {arn:?}, got {stated:?}"
    );
}

/// RULE: the grants held for a target are projected as that target's policy — one
/// statement per granted source, persisted for every later read.
#[tokio::test]
async fn granted_permissions_are_projected_as_the_targets_policy() {
    // Every `PermissionActionType` the model declares, on a fresh emulator each time,
    // so one action type's grants cannot be read as another's.
    for action in PERMISSION_ACTION_TYPE_VALUES {
        let emulator = Emulator::new();
        let rows = cases();

        // Every row's target is a resource this Region holds, which is what the
        // registrations bring into being.
        for row in &rows {
            register(&emulator, row.label, row.registration.clone()).await;
        }

        for row in &rows {
            let mut granted: BTreeSet<String> = BTreeSet::new();
            for source in &row.sources {
                let case = format!("{} / {action} / {source}", row.label);
                let reply = put(&emulator, &row.resource, action, source).await;
                let document = policy(&reply, &case);
                granted.insert(source.clone());
                // The response carries the policy the target has NOW: every source
                // granted so far, including the one this call added.
                assert_policy_grants(&document, &row.resource, action, &granted, &case);

                // The grant is persisted, not just echoed: a later read renders the
                // same document, and reading it twice does not change it.
                let read = get(&emulator, &row.resource, Some(action)).await;
                let read_document = policy(&read, &case);
                assert_eq!(
                    read_document, document,
                    "case {case:?}: GetResourcePermission must report the stored policy, got {read_document:?}"
                );
                let reread = get(&emulator, &row.resource, Some(action)).await;
                assert_eq!(
                    policy(&reread, &case),
                    read_document,
                    "case {case:?}: a read must not change the policy a later read sees"
                );

                // Omitting the optional `ActionType` reports the whole policy held for
                // the target, which is this action's statements and nothing else.
                let unfiltered = get(&emulator, &row.resource, None).await;
                assert_eq!(
                    policy(&unfiltered, &case),
                    read_document,
                    "case {case:?}: an unfiltered read must report the target's whole policy"
                );
            }

            // Re-granting a triple already held is the same single grant: the document
            // does not grow and does not change.
            let case = format!("{} / {action} / repeat grant", row.label);
            let before = policy(&get(&emulator, &row.resource, Some(action)).await, &case);
            for source in &row.sources {
                let repeat = put(&emulator, &row.resource, action, source).await;
                assert_eq!(
                    policy(&repeat, &case),
                    before,
                    "case {case:?}: re-granting {source:?} must leave the policy unchanged"
                );
            }
            let after = policy(&get(&emulator, &row.resource, Some(action)).await, &case);
            assert_eq!(
                after, before,
                "case {case:?}: expected {before:?} after repeat grants, got {after:?}"
            );
        }

        // Grants are held per target: every row still reports exactly its own sources,
        // after every other row has been granted too.
        for row in &rows {
            let case = format!("{} / {action} / isolation", row.label);
            let expected: BTreeSet<String> = row.sources.iter().cloned().collect();
            let document = policy(&get(&emulator, &row.resource, Some(action)).await, &case);
            assert_policy_grants(&document, &row.resource, action, &expected, &case);
        }
    }
}

/// RULE: a target that EXISTS but has been granted nothing carries no policy — an absent
/// `Policy` member on a 200, not an invented document and not a refusal.
#[tokio::test]
async fn a_target_that_exists_with_no_grant_has_no_policy() {
    let emulator = Emulator::new();
    register(
        &emulator,
        "an ungranted registration",
        registration("perm-none", &[], Some(&["HDB"])),
    )
    .await;
    register(
        &emulator,
        "another registration to grant on",
        registration("perm-other", &[], Some(&["HDB"])),
    )
    .await;

    // Rows: the kinds of resource a registration brings with it, each granted nothing.
    let ungranted = [
        ("the application", application_arn(REGION, "perm-none")),
        (
            "its HANA database",
            database_arn(REGION, "perm-none", "HDB"),
        ),
    ];

    for action in PERMISSION_ACTION_TYPE_VALUES {
        for (resource_label, resource) in &ungranted {
            for (label, requested) in [("no ActionType", None), ("with ActionType", Some(*action))] {
                let case = format!("{resource_label} / {action} / {label}");
                let reply = get(&emulator, resource, requested).await;
                assert_eq!(
                    reply.status, 200,
                    "case {case:?}: expected HTTP 200, got {reply:?}"
                );
                assert!(
                    reply.member("Policy").is_none(),
                    "case {case:?}: a target with no grant must carry no Policy, got {reply:?}"
                );
            }
        }
    }

    // A grant on ANOTHER target does not give these ones a policy.
    let other = database_arn(REGION, "perm-other", "HDB");
    let source = database_arn(REGION, "perm-none", "HDB");
    let action = PERMISSION_ACTION_TYPE_VALUES[0];
    let reply = put(&emulator, &other, action, &source).await;
    assert_eq!(
        reply.status, 200,
        "case \"grant on another target\": expected HTTP 200, got {reply:?}"
    );
    for (resource_label, resource) in &ungranted {
        let reply = get(&emulator, resource, None).await;
        assert!(
            reply.member("Policy").is_none(),
            "case {resource_label:?}: the ungranted target must still carry no Policy, got \
             {reply:?}"
        );
    }
}

/// RULE: an ARN that names no resource of this account and Region is refused with the
/// modelled `ResourceNotFoundException` (HTTP 404) naming that ARN — by the grant and by
/// the read alike — and the refused grant persists nothing.
#[tokio::test]
async fn an_arn_naming_no_resource_here_is_refused_and_persists_nothing() {
    let emulator = Emulator::new();
    let action = PERMISSION_ACTION_TYPE_VALUES[0];

    register(
        &emulator,
        "a registration that stays",
        registration("perm-present", &[], Some(&["HDB"])),
    )
    .await;
    register(
        &emulator,
        "a registration to deregister",
        registration("perm-gone", &[], Some(&["HDB"])),
    )
    .await;
    let reply = emulator
        .send(
            "/deregister-application",
            json!({ "ApplicationId": "perm-gone" }),
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
            application_arn(REGION, "perm-never-registered"),
        ),
        (
            "a database of an application never registered",
            REGION,
            database_arn(REGION, "perm-never-registered", "HDB"),
        ),
        (
            "an application that was deregistered",
            REGION,
            application_arn(REGION, "perm-gone"),
        ),
        (
            "a database of an application that was deregistered",
            REGION,
            database_arn(REGION, "perm-gone", "HDB"),
        ),
        (
            "a database the registration never named",
            REGION,
            database_arn(REGION, "perm-present", "NOPE"),
        ),
        (
            "a registered resource named at another regional endpoint",
            OTHER_REGION,
            database_arn(REGION, "perm-present", "HDB"),
        ),
        (
            "an ARN carrying another Region",
            REGION,
            database_arn(OTHER_REGION, "perm-present", "HDB"),
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
    ];

    let source = database_arn(REGION, "perm-present", "HDB");
    for (label, region, arn) in rows {
        let grant = put_in_region(&emulator, region, &arn, action, &source).await;
        assert_not_found(&grant, &arn, label);

        // The read refuses the same ARN in the same way, rather than reporting an empty
        // policy for a resource that does not exist.
        let read = get_in_region(&emulator, region, &arn, None).await;
        assert_not_found(&read, &arn, label);
        let narrowed = get_in_region(&emulator, region, &arn, Some(action)).await;
        assert_not_found(&narrowed, &arn, label);
    }

    // The resource that does exist was untouched by every refusal: it still carries no
    // policy, and a grant on it is still served.
    let target = database_arn(REGION, "perm-present", "HDB");
    let reply = get(&emulator, &target, None).await;
    assert!(
        reply.member("Policy").is_none(),
        "case \"the registered database\": a refused call persists nothing, got {reply:?}"
    );
    let reply = put(&emulator, &target, action, &source).await;
    let expected: BTreeSet<String> = BTreeSet::from([source.clone()]);
    assert_policy_grants(
        &policy(&reply, "the registered database"),
        &target,
        action,
        &expected,
        "the registered database",
    );
}

/// RULE: a grant is held ON a resource, so it cannot outlive the registration that
/// resource came from.
#[tokio::test]
async fn a_grant_does_not_outlive_the_registration_its_target_came_from() {
    let emulator = Emulator::new();
    let action = PERMISSION_ACTION_TYPE_VALUES[0];
    let source = format!("arn:aws:ssm-sap:{REGION}:{ACCOUNT}:database/perm-source/HDB");

    // Rows: the kinds of resource a registration brings with it, each granted on.
    let databases: &[&str] = &["HDB", "SYSTEMDB"];
    register(
        &emulator,
        "a registration to deregister",
        registration("perm-lifetime", &[], Some(databases)),
    )
    .await;

    let mut targets = vec![application_arn(REGION, "perm-lifetime")];
    targets.extend(
        databases
            .iter()
            .map(|database_id| database_arn(REGION, "perm-lifetime", database_id)),
    );
    for target in &targets {
        let reply = put(&emulator, target, action, &source).await;
        let expected: BTreeSet<String> = BTreeSet::from([source.clone()]);
        assert_policy_grants(&policy(&reply, target), target, action, &expected, target);
    }

    let reply = emulator
        .send(
            "/deregister-application",
            json!({ "ApplicationId": "perm-lifetime" }),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "expected 200 from DeregisterApplication, got {reply:?}"
    );
    settle().await;

    // The resources are gone, so both calls refuse their ARNs.
    for target in &targets {
        let case = format!("{target} after deregistration");
        assert_not_found(&get(&emulator, target, None).await, target, &case);
        assert_not_found(
            &put(&emulator, target, action, &source).await,
            target,
            &case,
        );
    }

    // The same application registered again is a NEW registration: its resources carry
    // the grants its own callers made, which is none of the old ones.
    register(
        &emulator,
        "the registration that replaces it",
        registration("perm-lifetime", &[], Some(databases)),
    )
    .await;
    for target in &targets {
        let reply = get(&emulator, target, None).await;
        assert_eq!(
            reply.status, 200,
            "case {target:?}: expected HTTP 200 after re-registration, got {reply:?}"
        );
        assert!(
            reply.member("Policy").is_none(),
            "case {target:?}: the grants of the old registration are gone, got {reply:?}"
        );
    }
}

/// RULE: the statement order of a document does not depend on the order the grants
/// arrived, so two reads of equally granted targets render the same statements.
#[tokio::test]
async fn the_document_does_not_depend_on_the_order_grants_arrived() {
    let emulator = Emulator::new();
    let action = PERMISSION_ACTION_TYPE_VALUES[0];
    let sources = [
        format!("arn:aws:ssm-sap:{REGION}:{ACCOUNT}:database/HDB-Z/HDB"),
        format!("arn:aws:ssm-sap:{REGION}:{ACCOUNT}:database/HDB-A/HDB"),
        "arn:aws:s3:::sap-backint-bucket/restore".to_string(),
    ];

    // Two targets, the same three sources, granted in opposite orders.
    register(
        &emulator,
        "the forward target",
        registration("perm-forward", &[], Some(&["HDB"])),
    )
    .await;
    register(
        &emulator,
        "the backward target",
        registration("perm-backward", &[], Some(&["HDB"])),
    )
    .await;
    let forward = database_arn(REGION, "perm-forward", "HDB");
    let backward = database_arn(REGION, "perm-backward", "HDB");
    for source in &sources {
        put(&emulator, &forward, action, source).await;
    }
    for source in sources.iter().rev() {
        put(&emulator, &backward, action, source).await;
    }

    let case = "insertion order";
    let forward_principals: Vec<String> =
        statements(&policy(&get(&emulator, &forward, None).await, case), case)
            .into_iter()
            .map(|statement| statement.principal)
            .collect();
    let backward_principals: Vec<String> =
        statements(&policy(&get(&emulator, &backward, None).await, case), case)
            .into_iter()
            .map(|statement| statement.principal)
            .collect();
    assert_eq!(
        forward_principals, backward_principals,
        "case {case:?}: statement order must not depend on the order grants arrived: {forward_principals:?} vs {backward_principals:?}"
    );
    assert_eq!(
        forward_principals.len(),
        sources.len(),
        "case {case:?}: expected {} statements, got {forward_principals:?}",
        sources.len()
    );
}

/// RULE: the service is regional, so a grant is made on the resource of THIS Region and
/// is neither read nor reported through another endpoint.
#[tokio::test]
async fn grants_are_held_per_region() {
    let emulator = Emulator::new();
    let action = PERMISSION_ACTION_TYPE_VALUES[0];
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2"];

    // The SAME application id registered at each regional endpoint: an id in use in one
    // Region is free in every other, and each registration's database carries its own
    // Region's ARN.
    for region in regions {
        register_in_region(
            &emulator,
            region,
            region,
            registration("perm-regions", &[], Some(&["HDB"])),
        )
        .await;
    }

    for granted_in in regions {
        let target = database_arn(granted_in, "perm-regions", "HDB");
        let source = format!("arn:aws:ssm-sap:{granted_in}:{ACCOUNT}:database/perm-source/HDB");
        let case = format!("granted in {granted_in}");
        let reply = put_in_region(&emulator, granted_in, &target, action, &source).await;
        let expected: BTreeSet<String> = BTreeSet::from([source.clone()]);
        assert_policy_grants(&policy(&reply, &case), &target, action, &expected, &case);

        for read_in in regions {
            let case = format!("granted in {granted_in}, read in {read_in}");
            let reply = get_in_region(&emulator, read_in, &target, None).await;
            if read_in == granted_in {
                assert_policy_grants(&policy(&reply, &case), &target, action, &expected, &case);
            } else {
                // The target is a resource of ANOTHER Region, so this endpoint holds no
                // such resource at all — it cannot report the grant made there.
                assert_not_found(&reply, &target, &case);
            }
        }
    }
}

/// RULE: the store holds at most [`MAX_PERMISSION_SOURCES_PER_RESOURCE`] sources for one
/// (target, action type) pair; a grant past that ceiling is refused with the modelled
/// `ValidationException` (HTTP 400) and persists nothing.
#[tokio::test]
async fn the_per_resource_source_ceiling_is_refused_and_persists_nothing() {
    let emulator = Emulator::new();
    let action = PERMISSION_ACTION_TYPE_VALUES[0];
    register(
        &emulator,
        "a registration to fill up",
        registration("perm-ceiling", &[], Some(&["HDB"])),
    )
    .await;
    let target = database_arn(REGION, "perm-ceiling", "HDB");

    let mut granted: BTreeSet<String> = BTreeSet::new();
    for index in 0..MAX_PERMISSION_SOURCES_PER_RESOURCE {
        let source = format!("arn:aws:ssm-sap:{REGION}:{ACCOUNT}:database/perm-src-{index}/HDB");
        let case = format!("source {} of {MAX_PERMISSION_SOURCES_PER_RESOURCE}", index + 1);
        let reply = put(&emulator, &target, action, &source).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200 within the ceiling, got {reply:?}"
        );
        granted.insert(source);
    }
    let at_ceiling = policy(&get(&emulator, &target, Some(action)).await, "at the ceiling");
    assert_policy_grants(&at_ceiling, &target, action, &granted, "at the ceiling");

    // One past the ceiling is refused, and the document is exactly what it was.
    let over = format!("arn:aws:ssm-sap:{REGION}:{ACCOUNT}:database/perm-src-over/HDB");
    let case = format!("one past {MAX_PERMISSION_SOURCES_PER_RESOURCE}");
    let reply = put(&emulator, &target, action, &over).await;
    assert_eq!(
        reply.status, 400,
        "case {case:?}: expected HTTP 400 past the ceiling, got {reply:?}"
    );
    assert_eq!(
        reply.code(),
        VALIDATION_EXCEPTION,
        "case {case:?}: expected {VALIDATION_EXCEPTION}, got {reply:?}"
    );
    let after = policy(&get(&emulator, &target, Some(action)).await, &case);
    assert_eq!(
        after, at_ceiling,
        "case {case:?}: a refused grant persists nothing, got {after:?}"
    );

    // A source already held is still accepted at the ceiling: it adds nothing.
    let repeat = granted
        .iter()
        .next()
        .expect("the ceiling held at least one source")
        .clone();
    let reply = put(&emulator, &target, action, &repeat).await;
    assert_eq!(
        policy(&reply, "repeat at the ceiling"),
        at_ceiling,
        "case \"repeat at the ceiling\": re-granting a held source must change nothing"
    );
}

/// RULE: a request outside what the model declares is refused with the modelled
/// `ValidationException` (HTTP 400) before anything is resolved or stored.
#[tokio::test]
async fn a_request_outside_the_modelled_constraints_is_refused() {
    let emulator = Emulator::new();
    let action = PERMISSION_ACTION_TYPE_VALUES[0];
    register(
        &emulator,
        "a registration to grant on",
        registration("perm-refuse", &[], Some(&["HDB"])),
    )
    .await;
    let good = database_arn(REGION, "perm-refuse", "HDB");

    // Each row is one way the request falls outside what the model declares: an ARN
    // that is not an ARN on either member, an omitted `@required` member, and an
    // `ActionType` outside the `PermissionActionType` enum.
    let rows = vec![
        (
            "ResourceArn is not an ARN",
            json!({"ResourceArn": "not-an-arn", "ActionType": action, "SourceResourceArn": good}),
        ),
        (
            "SourceResourceArn is not an ARN",
            json!({"ResourceArn": good, "ActionType": action, "SourceResourceArn": "nope"}),
        ),
        (
            "ActionType omitted",
            json!({"ResourceArn": good, "SourceResourceArn": good}),
        ),
        (
            "SourceResourceArn omitted",
            json!({"ResourceArn": good, "ActionType": action}),
        ),
        (
            "ActionType outside the enum",
            json!({"ResourceArn": good, "ActionType": "BACKUP", "SourceResourceArn": good}),
        ),
    ];

    for (case, body) in rows {
        let reply = emulator.send("/put-resource-permission", body).await;
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

    // A refused request persists nothing: the target it named still has no policy.
    let reply = get(&emulator, &good, None).await;
    assert!(
        reply.member("Policy").is_none(),
        "case \"refused requests persist nothing\": expected no Policy, got {reply:?}"
    );
}
