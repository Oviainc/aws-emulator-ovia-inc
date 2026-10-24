// SPDX-License-Identifier: CC0-1.0

//! `PutResourcePermission` and `GetResourcePermission` end to end, over the wire.
//!
//! The rule these cases pin: a resource permission is the (target resource ARN,
//! `PermissionActionType`, source resource ARN) triple the service holds, and the
//! `Policy` either operation reports is PROJECTED from the triples held for that one
//! target — one `Allow` statement per granted source, in an order that does not depend
//! on the order the grants arrived, absent entirely for a target nobody has granted
//! anything on, and unchanged by re-granting a triple already held or by reading it.
//!
//! The table spans what that rule generalises over: the ARN form of the target
//! (both branches of the modelled `Arn` pattern — the colon-segment form and the
//! `.../path` form), the number of sources granted on one target, the ARN form of a
//! source, and every `PermissionActionType` the model declares
//! (`service_com_amazonaws_ssmsap_ssmsap::ops::types::PERMISSION_ACTION_TYPE_VALUES`).
//! A single HANA database target with one source — the shape the issue's reproduction
//! calls — is ONE row. A separate case pins that a grant is held per Region, the
//! partitioning `crate::region_scoping` pins for the rest of this service's state.
//!
//! Every call goes through `MultiServiceRouter::dispatch` (the harness
//! `tests/all/application_lifecycle.rs` owns), so the restJson1 routing, the boundary
//! constraint walk, the handler and the response serialization all run exactly as they
//! do in the emulator binary. Nothing reaches into `crate::state` directly: a grant is
//! only believed when a later `GetResourcePermission` reports it back.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::application_lifecycle::{Emulator, Reply};
use service_com_amazonaws_ssmsap_ssmsap::ops::types::PERMISSION_ACTION_TYPE_VALUES;
use service_com_amazonaws_ssmsap_ssmsap::state::{
    permission_policy_action, MAX_REGIONS, POLICY_EFFECT_ALLOW, POLICY_VERSION,
};

/// The account every emulated ARN carries, from the core's single source of truth.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The wire error code a refused request carries; this service models its own
/// `ValidationException` (HTTP 400).
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// One row: a label, the target resource ARN, and the source resource ARNs granted on
/// it in the order the test grants them.
struct Case {
    label: &'static str,
    resource: String,
    sources: Vec<String>,
}

/// The rows. Each row owns its OWN target ARN, so the last assertion can check that a
/// grant on one target never shows up in another's policy.
fn cases() -> Vec<Case> {
    vec![
        // The shape the issue's reproduction calls: one HANA database target, one
        // source database. `database/<application>/<database>` is the `.../path`
        // branch of the modelled `Arn` pattern.
        Case {
            label: "one source on a database target",
            resource: format!("arn:aws:ssm-sap:us-east-1:{ACCOUNT}:database/HDB-TEST/HDB"),
            sources: vec![format!(
                "arn:aws:ssm-sap:us-east-1:{ACCOUNT}:database/HDB-SOURCE/HDB"
            )],
        },
        // Three sources on one target: the statement count follows the grant count.
        Case {
            label: "three sources on a database target",
            resource: format!("arn:aws:ssm-sap:us-east-1:{ACCOUNT}:database/HDB-FANIN/HDB"),
            sources: vec![
                format!("arn:aws:ssm-sap:us-east-1:{ACCOUNT}:database/HDB-S3/HDB"),
                format!("arn:aws:ssm-sap:eu-west-1:{ACCOUNT}:database/HDB-S1/HDB"),
                format!("arn:aws:ssm-sap:us-east-1:{ACCOUNT}:database/HDB-S2/HDB"),
            ],
        },
        // A target in the colon-segment branch of the pattern (no `/`), with a source
        // in the other branch: neither ARN's form changes how a grant is stored.
        Case {
            label: "colon-segment target, mixed source forms",
            resource: format!("arn:aws:ssm-sap:us-east-1:{ACCOUNT}:HDB-COLON"),
            sources: vec![
                "arn:aws:s3:::sap-backint-bucket/backups".to_string(),
                format!("arn:aws:iam::{ACCOUNT}:role/sap-restore"),
            ],
        },
    ]
}

/// `PutResourcePermission` over the wire.
async fn put(emulator: &Emulator, resource: &str, action: &str, source: &str) -> Reply {
    emulator
        .send(
            "/put-resource-permission",
            json!({
                "ResourceArn": resource,
                "ActionType": action,
                "SourceResourceArn": source,
            }),
        )
        .await
}

/// `GetResourcePermission` over the wire, with an `ActionType` when `action` is given.
async fn get(emulator: &Emulator, resource: &str, action: Option<&str>) -> Reply {
    let body = match action {
        Some(action) => json!({ "ResourceArn": resource, "ActionType": action }),
        None => json!({ "ResourceArn": resource }),
    };
    emulator.send("/get-resource-permission", body).await
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

#[tokio::test]
async fn granted_permissions_are_projected_as_the_targets_policy() {
    // Every `PermissionActionType` the model declares, on a fresh emulator each time,
    // so one action type's grants cannot be read as another's.
    for action in PERMISSION_ACTION_TYPE_VALUES {
        let emulator = Emulator::new();
        let rows = cases();

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

#[tokio::test]
async fn a_target_with_no_grant_has_no_policy() {
    let emulator = Emulator::new();
    let ungranted = format!("arn:aws:ssm-sap:us-east-1:{ACCOUNT}:database/HDB-NONE/HDB");

    for action in PERMISSION_ACTION_TYPE_VALUES {
        for (label, requested) in [
            ("no ActionType", None),
            ("with ActionType", Some(*action)),
        ] {
            let case = format!("ungranted target / {action} / {label}");
            let reply = get(&emulator, &ungranted, requested).await;
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

    // A grant on ANOTHER target does not give this one a policy.
    let other = format!("arn:aws:ssm-sap:us-east-1:{ACCOUNT}:database/HDB-OTHER/HDB");
    let source = format!("arn:aws:ssm-sap:us-east-1:{ACCOUNT}:database/HDB-SRC/HDB");
    let action = PERMISSION_ACTION_TYPE_VALUES[0];
    let reply = put(&emulator, &other, action, &source).await;
    assert_eq!(
        reply.status, 200,
        "case \"grant on another target\": expected HTTP 200, got {reply:?}"
    );
    let reply = get(&emulator, &ungranted, None).await;
    assert!(
        reply.member("Policy").is_none(),
        "case \"grant on another target\": the ungranted target must still carry no Policy, got {reply:?}"
    );
}

#[tokio::test]
async fn the_document_does_not_depend_on_the_order_grants_arrived() {
    let emulator = Emulator::new();
    let action = PERMISSION_ACTION_TYPE_VALUES[0];
    let sources = [
        format!("arn:aws:ssm-sap:us-east-1:{ACCOUNT}:database/HDB-Z/HDB"),
        format!("arn:aws:ssm-sap:us-east-1:{ACCOUNT}:database/HDB-A/HDB"),
        "arn:aws:s3:::sap-backint-bucket/restore".to_string(),
    ];

    // Two targets, the same three sources, granted in opposite orders.
    let forward = format!("arn:aws:ssm-sap:us-east-1:{ACCOUNT}:database/HDB-FORWARD/HDB");
    let backward = format!("arn:aws:ssm-sap:us-east-1:{ACCOUNT}:database/HDB-BACKWARD/HDB");
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

#[tokio::test]
async fn grants_are_held_per_region() {
    // The service is regional, so a grant made at one regional endpoint is neither
    // read nor reported through another; the same target ARN is free in every other
    // Region. (The harness signs each call for the Region it names.)
    let emulator = Emulator::new();
    let action = PERMISSION_ACTION_TYPE_VALUES[0];
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2"];
    let target = format!("arn:aws:ssm-sap:us-east-1:{ACCOUNT}:database/HDB-REGIONS/HDB");

    for granted_in in regions {
        let source = format!("arn:aws:ssm-sap:{granted_in}:{ACCOUNT}:database/HDB-SRC/HDB");
        let case = format!("granted in {granted_in}");
        let reply = emulator
            .send_in_region(
                granted_in,
                "/put-resource-permission",
                json!({
                    "ResourceArn": target,
                    "ActionType": action,
                    "SourceResourceArn": source,
                }),
            )
            .await;
        let document = policy(&reply, &case);
        let expected: BTreeSet<String> = BTreeSet::from([source.clone()]);
        assert_policy_grants(&document, &target, action, &expected, &case);

        for read_in in regions {
            let case = format!("granted in {granted_in}, read in {read_in}");
            let reply = emulator
                .send_in_region(
                    read_in,
                    "/get-resource-permission",
                    json!({ "ResourceArn": target }),
                )
                .await;
            if read_in == granted_in {
                assert_policy_grants(&policy(&reply, &case), &target, action, &expected, &case);
            } else {
                // Another Region either holds its own single grant or none at all —
                // never the one made here.
                assert_eq!(
                    reply.status, 200,
                    "case {case:?}: expected HTTP 200, got {reply:?}"
                );
                let document = reply
                    .member("Policy")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                assert!(
                    !document.contains(&source),
                    "case {case:?}: the grant made in {granted_in} must not be reported in {read_in}, got {document:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_region_partition_ceiling_is_enforced() {
    // The Region comes from the caller's credential scope, so a grant must not mint a
    // partition for every Region a caller invents: past `MAX_REGIONS` the grant is
    // refused, and the refusal persists nothing.
    let emulator = Emulator::new();
    let action = PERMISSION_ACTION_TYPE_VALUES[0];
    let target = format!("arn:aws:ssm-sap:us-east-1:{ACCOUNT}:database/HDB-CEILING/HDB");
    let source = format!("arn:aws:ssm-sap:us-east-1:{ACCOUNT}:database/HDB-SRC/HDB");
    let body = json!({
        "ResourceArn": target,
        "ActionType": action,
        "SourceResourceArn": source,
    });

    for index in 0..MAX_REGIONS {
        let region = format!("us-test-{index}");
        let case = format!("region {region} (partition {}/{MAX_REGIONS})", index + 1);
        let reply = emulator
            .send_in_region(&region, "/put-resource-permission", body.clone())
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200 within the ceiling, got {reply:?}"
        );
    }

    let over = format!("us-test-{MAX_REGIONS}");
    let case = format!("region {over} (one past {MAX_REGIONS})");
    let reply = emulator
        .send_in_region(&over, "/put-resource-permission", body.clone())
        .await;
    assert_eq!(
        reply.status, 400,
        "case {case:?}: expected HTTP 400 past the ceiling, got {reply:?}"
    );
    assert_eq!(
        reply.code(),
        VALIDATION_EXCEPTION,
        "case {case:?}: expected {VALIDATION_EXCEPTION}, got {reply:?}"
    );
    let read = emulator
        .send_in_region(&over, "/get-resource-permission", json!({"ResourceArn": target}))
        .await;
    assert!(
        read.member("Policy").is_none(),
        "case {case:?}: a refused grant must persist nothing, got {read:?}"
    );

    // Every Region within the ceiling still holds its grant.
    let expected: BTreeSet<String> = BTreeSet::from([source.clone()]);
    for index in 0..MAX_REGIONS {
        let region = format!("us-test-{index}");
        let case = format!("region {region} after the refusal");
        let reply = emulator
            .send_in_region(&region, "/get-resource-permission", json!({"ResourceArn": target}))
            .await;
        assert_policy_grants(&policy(&reply, &case), &target, action, &expected, &case);
    }
}

#[tokio::test]
async fn a_request_outside_the_modelled_constraints_is_refused() {
    let emulator = Emulator::new();
    let action = PERMISSION_ACTION_TYPE_VALUES[0];
    let good = format!("arn:aws:ssm-sap:us-east-1:{ACCOUNT}:database/HDB-REFUSE/HDB");

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
