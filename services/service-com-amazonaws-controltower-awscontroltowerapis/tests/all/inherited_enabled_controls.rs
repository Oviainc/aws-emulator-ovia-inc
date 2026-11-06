// SPDX-License-Identifier: CC0-1.0

//! `includeChildren`, `targetIdentifier`, `parentIdentifier` and
//! `filter.parentIdentifiers` on the enabled control listing, end to end over HTTP.
//!
//! Driven through the same [`MultiServiceRouter`] the production emulator dispatches with,
//! so what these tests exercise is the wire behaviour a client sees — status codes, error
//! codes and JSON bodies — not the handlers' Rust signatures.
//!
//! THE RULE under test, asserted over a TABLE of inheritance shapes rather than one case:
//! a listing reports the enablements it is SCOPED to — the ones on the `targetIdentifier`
//! the request names, or, when it names none, the ones that inherited from nothing — plus,
//! only when `includeChildren` is set, every enablement that inherited its configuration
//! from one of those, directly or through a chain. Which enablement is a child of which is
//! read off the `parentIdentifier` the records carry ("the ARN of the parent enabled
//! control from which this control inherits its configuration, if applicable"), the same
//! member `GetEnabledControl` reports, and `filter.parentIdentifiers` selects on that same
//! value.
//!
//! `EnableControl` applies a control DIRECTLY to the organizational unit a request names,
//! so every enablement it persists inherits from nothing: the rows below therefore apply
//! the OU-level enablements through the API and GRAFT the inherited ones — the deployments
//! into the accounts and child units a target contains, which take an organization
//! membership this service records nowhere — straight into the store, so the projection
//! rule is exercised on records of every shape it has to report. One row grafts nothing,
//! which is the whole store a caller can build through this API: there the flag widens the
//! listing by nothing and no entry reports a parent.
//!
//! The values written out by hand name their source: the account-ARN form
//! (`arn:<partition>:organizations::<management account>:account/<organization>/<account
//! id>`) is the one AWS Organizations publishes for an account and is built here from the
//! organizational-unit ARNs the sibling suites use; the modelled maximum length of
//! `filter.parentIdentifiers` (1) comes from `smithy_query(query="shape",
//! shape="EnabledControlFilter")`; the `maxResults` bounds (range 1..200) from
//! `smithy_query(query="operation", operation="ListEnabledControls")`; and the controls
//! come from `service_..._awscontroltowerapis::state::CONTROL_CATALOG`, which is what
//! `ListBaselines`' sibling `ListEnabledControls` resolves an identifier against.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_controltower_awscontroltowerapis::state;

use crate::enabled_control_lifecycle::{
    control, details, enable_with, entries, listed_arns, listed_entry, ou,
};
use crate::landing_zone_lifecycle::{
    call, emulator_with_engine, past_the_settle_span, set_up_account, Answer, SIGNED_REGION,
};

/// One OU-level enablement a row applies, with the inheritance hanging off it.
struct Governance {
    /// The catalogued control's name, as [`control`] spells its `ControlIdentifier`.
    control: &'static str,
    /// The organizational unit the enable names.
    target: String,
    /// Each entry is a CHAIN of targets: the first inherits from the OU-level enablement,
    /// the next from that one, and so on — so a row can carry inheritance deeper than one
    /// level.
    chains: Vec<Vec<String>>,
}

/// One row of the table.
struct Shape {
    case: &'static str,
    governances: Vec<Governance>,
}

/// The AWS Organizations ARN of an account as a member of the emulator's organization:
/// the partition, management account and organization the sibling suites' unit ARNs carry
/// ([`ou`]), with the account's own resource type and id.
fn account(account_id: &str) -> String {
    format!("arn:aws:organizations::000000000000:account/o-emulatororg/{account_id}")
}

/// The table: rows differ along what the rule generalises over — how many enablements are
/// in scope, how many inherit from each, how DEEP the inheritance runs, whether the
/// inheriting target is an account or a child unit, and whether two enablements in scope
/// share a target or a control.
fn shapes() -> Vec<Shape> {
    vec![
        Shape {
            case: "one enablement applied directly to its unit, nothing inheriting from it",
            governances: vec![Governance {
                control: "AWS-GR_ENCRYPTED_VOLUMES",
                target: ou("1a01"),
                chains: Vec::new(),
            }],
        },
        Shape {
            case: "one account inheriting the unit's enablement",
            governances: vec![Governance {
                control: "AWS-GR_RESTRICT_ROOT_USER",
                target: ou("1a02"),
                chains: vec![vec![account("111111111111")]],
            }],
        },
        Shape {
            case: "two accounts and a child unit whose own account inherits in turn",
            governances: vec![Governance {
                control: "AWS-GR_ENCRYPTED_VOLUMES",
                target: ou("1a03"),
                chains: vec![
                    vec![account("222222222222")],
                    vec![account("333333333333")],
                    vec![ou("1a04"), account("444444444444")],
                ],
            }],
        },
        Shape {
            case: "two units, each with its own control and one inheriting account",
            governances: vec![
                Governance {
                    control: "AWS-GR_ENCRYPTED_VOLUMES",
                    target: ou("1a05"),
                    chains: vec![vec![account("555555555555")]],
                },
                Governance {
                    control: "AWS-GR_RESTRICT_ROOT_USER",
                    target: ou("1a06"),
                    chains: vec![vec![account("666666666666")]],
                },
            ],
        },
        Shape {
            case: "two controls on one unit, one of them inherited by an account",
            governances: vec![
                Governance {
                    control: "AWS-GR_ENCRYPTED_VOLUMES",
                    target: ou("1a07"),
                    chains: vec![vec![account("777777777777")]],
                },
                Governance {
                    control: "AWS-GR_RESTRICT_ROOT_USER",
                    target: ou("1a07"),
                    chains: Vec::new(),
                },
            ],
        },
    ]
}

/// One enablement of a built shape, as the store holds it.
struct Enablement {
    arn: String,
    target: String,
    /// The enablement this one inherited its configuration from, absent for an OU-level
    /// one.
    parent: Option<String>,
}

/// Graft onto the store the enablement a control's deployment into `target` — an account
/// or a child unit of the target `parent_arn` is enabled on — would land as: the parent's
/// own record, on the contained target, inheriting its configuration from it.
///
/// `EnableControl` applies a control directly to the unit a request names and this service
/// records no organization membership, so no call it serves can create such a record; it
/// is written here because what the listing does with one is exactly the rule under test.
fn inherit(engine: &Arc<engine::InMemoryEngine>, parent_arn: &str, target: &str) -> String {
    let service_state = state::service_state(&**engine).expect("the Control Tower store");
    let mut store = service_state.lock();
    let parent = store
        .enabled_controls
        .get(parent_arn)
        .expect("the enablement the grafted one inherits from")
        .clone();
    let arn = state::enabled_control_arn(SIGNED_REGION, &state::new_enabled_control_id());
    store.enabled_controls.insert(
        arn.clone(),
        state::EnabledControl {
            arn: arn.clone(),
            target_identifier: target.to_string(),
            parent_identifier: Some(parent_arn.to_string()),
            ..parent
        },
    );
    arn
}

/// Apply one row: every OU-level enablement through `EnableControl`, then the inheritance
/// its chains describe.
async fn apply(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    shape: &Shape,
) -> Vec<Enablement> {
    let mut applied: Vec<Enablement> = Vec::new();
    for governance in &shape.governances {
        let (arn, _) = enable_with(
            router,
            &control(governance.control),
            &governance.target,
            None,
        )
        .await;
        applied.push(Enablement {
            arn: arn.clone(),
            target: governance.target.clone(),
            parent: None,
        });
        for chain in &governance.chains {
            let mut parent = arn.clone();
            for target in chain {
                let child = inherit(engine, &parent, target);
                applied.push(Enablement {
                    arn: child.clone(),
                    target: target.clone(),
                    parent: Some(parent),
                });
                parent = child;
            }
        }
    }
    applied
}

/// `arns`, sorted, so a case compares SETS rather than the order ARNs were minted in.
fn sorted(arns: Vec<String>) -> Vec<String> {
    let mut arns = arns;
    arns.sort();
    arns
}

/// The enablements of `applied` that inherited from nothing: the scope of a listing that
/// names no target.
fn applied_directly(applied: &[Enablement]) -> Vec<String> {
    sorted(
        applied
            .iter()
            .filter(|enablement| enablement.parent.is_none())
            .map(|enablement| enablement.arn.clone())
            .collect(),
    )
}

/// The enablements of `applied` that are ON `target`.
fn on_target(applied: &[Enablement], target: &str) -> Vec<String> {
    sorted(
        applied
            .iter()
            .filter(|enablement| enablement.target == target)
            .map(|enablement| enablement.arn.clone())
            .collect(),
    )
}

/// Every enablement of `applied` that inherits from one of `arns`, directly or through a
/// chain, beside `arns` themselves.
fn with_inherited(applied: &[Enablement], arns: &[String]) -> Vec<String> {
    let mut reached: Vec<String> = arns.to_vec();
    loop {
        let inherited: Vec<String> = applied
            .iter()
            .filter(|enablement| {
                !reached.contains(&enablement.arn)
                    && enablement
                        .parent
                        .as_ref()
                        .is_some_and(|parent| reached.contains(parent))
            })
            .map(|enablement| enablement.arn.clone())
            .collect();
        if inherited.is_empty() {
            return sorted(reached);
        }
        reached.extend(inherited);
    }
}

/// The enablements of `applied` that inherit DIRECTLY from `arn`: what a
/// `parentIdentifiers` filter naming it selects.
fn children_of(applied: &[Enablement], arn: &str) -> Vec<String> {
    sorted(
        applied
            .iter()
            .filter(|enablement| enablement.parent.as_deref() == Some(arn))
            .map(|enablement| enablement.arn.clone())
            .collect(),
    )
}

/// POST `/list-enabled-controls` with `body`, asserting the call succeeded.
async fn list(router: &MultiServiceRouter, case: &str, body: serde_json::Value) -> Answer {
    let listed = call(router, "/list-enabled-controls", body.clone()).await;
    assert_eq!(
        listed.status, 200,
        "case {case:?} ({body}): list-enabled-controls expected 200, got {} body {:?}",
        listed.status, listed.body
    );
    listed
}

/// RULE: the listing holds the enablements it is scoped to — the ones on the named target,
/// or the ones inheriting from nothing when no target is named — plus, only when
/// `includeChildren` is set, the ones that inherited from those, directly or through a
/// chain.
#[tokio::test]
async fn include_children_widens_the_listing_to_the_inherited_enablements() {
    for shape in shapes() {
        let case = shape.case;
        let (router, engine) = emulator_with_engine();
        set_up_account(&router).await;
        let applied = apply(&router, &engine, &shape).await;

        // Past the settle span, so every enablement is terminal and the cases below do not
        // race the clock.
        tokio::time::sleep(past_the_settle_span()).await;

        let directly = applied_directly(&applied);
        let everything = with_inherited(&applied, &directly);
        let mut cases: Vec<(String, serde_json::Value, Vec<String>)> = vec![
            (
                "no target, includeChildren omitted (the modelled default is false)"
                    .to_string(),
                serde_json::json!({}),
                directly.clone(),
            ),
            (
                "no target, includeChildren false".to_string(),
                serde_json::json!({ "includeChildren": false }),
                directly.clone(),
            ),
            (
                "no target, includeChildren true".to_string(),
                serde_json::json!({ "includeChildren": true }),
                everything.clone(),
            ),
            (
                "a unit nothing is enabled on, includeChildren false".to_string(),
                serde_json::json!({ "targetIdentifier": ou("9999"), "includeChildren": false }),
                Vec::new(),
            ),
            (
                "a unit nothing is enabled on, includeChildren true".to_string(),
                serde_json::json!({ "targetIdentifier": ou("9999"), "includeChildren": true }),
                Vec::new(),
            ),
        ];

        // Every target the shape holds an enablement on, scoped either way: with the flag
        // unset the listing is what is enabled ON that target — an inherited enablement
        // included, since that is a control enabled on the target rather than on a child
        // of it — and with it set, also everything that inherits from those.
        for enablement in &applied {
            let at_target = on_target(&applied, &enablement.target);
            cases.push((
                format!(
                    "target {}, includeChildren false",
                    enablement.target.clone()
                ),
                serde_json::json!({
                    "targetIdentifier": enablement.target.clone(),
                    "includeChildren": false
                }),
                at_target.clone(),
            ));
            cases.push((
                format!("target {}, includeChildren true", enablement.target.clone()),
                serde_json::json!({
                    "targetIdentifier": enablement.target.clone(),
                    "includeChildren": true
                }),
                with_inherited(&applied, &at_target),
            ));

            // `parentIdentifiers` selects on the parent an entry reports — "filters
            // enabled controls by their parent control identifiers, allowing you to find
            // child controls of specific parent controls" — so it keeps the enablements
            // that inherit from the one it names, and nothing at all while the children
            // are out of the listing's scope. One value, which is the member's modelled
            // maximum length.
            cases.push((
                format!(
                    "filter.parentIdentifiers {}, includeChildren true",
                    enablement.arn.clone()
                ),
                serde_json::json!({
                    "includeChildren": true,
                    "filter": { "parentIdentifiers": [enablement.arn.clone()] }
                }),
                children_of(&applied, &enablement.arn),
            ));
            cases.push((
                format!(
                    "filter.parentIdentifiers {}, includeChildren false",
                    enablement.arn.clone()
                ),
                serde_json::json!({
                    "filter": { "parentIdentifiers": [enablement.arn.clone()] }
                }),
                Vec::new(),
            ));
        }

        for (name, body, expected) in cases {
            let listed = list(&router, case, body).await;
            assert_eq!(
                listed_arns(&listed),
                expected,
                "case {case:?}, {name}: expected {expected:?}, got {:?}",
                listed.body
            );
        }
    }
}

/// RULE: an entry reports as its `parentIdentifier` the enablement it inherited its
/// configuration from — the same member `GetEnabledControl` reports off the same record —
/// and absent for one applied directly to its target.
#[tokio::test]
async fn every_entry_reports_the_enablement_it_inherits_from() {
    for shape in shapes() {
        let case = shape.case;
        let (router, engine) = emulator_with_engine();
        set_up_account(&router).await;
        let applied = apply(&router, &engine, &shape).await;
        tokio::time::sleep(past_the_settle_span()).await;

        let widest = list(&router, case, serde_json::json!({ "includeChildren": true })).await;
        for enablement in &applied {
            let entry = listed_entry(&widest, &enablement.arn);
            assert_eq!(
                entry
                    .get("parentIdentifier")
                    .and_then(serde_json::Value::as_str),
                enablement.parent.as_deref(),
                "case {case:?}: the listing must report {:?} as the parent of {}, got {entry:?}",
                enablement.parent,
                enablement.arn
            );
            assert_eq!(
                entry
                    .get("targetIdentifier")
                    .and_then(serde_json::Value::as_str),
                Some(enablement.target.as_str()),
                "case {case:?}: the listing must report {} on target {}, got {entry:?}",
                enablement.arn,
                enablement.target
            );

            // The same two members through `GetEnabledControl`, which reads the same
            // record: an inherited enablement is a resource in its own right, so it
            // resolves there too.
            let detail = details(&router, &enablement.arn).await;
            assert_eq!(
                detail
                    .get("parentIdentifier")
                    .and_then(serde_json::Value::as_str),
                enablement.parent.as_deref(),
                "case {case:?}: GetEnabledControl must report {:?} as the parent of {}, got \
                 {detail:?}",
                enablement.parent,
                enablement.arn
            );
            assert_eq!(
                detail
                    .get("targetIdentifier")
                    .and_then(serde_json::Value::as_str),
                Some(enablement.target.as_str()),
                "case {case:?}: GetEnabledControl must report {} on target {}, got {detail:?}",
                enablement.arn,
                enablement.target
            );
        }
    }
}

/// RULE: `includeChildren` narrows WHAT is listed, so a page token is bound to it — a
/// walk of the widened listing returns every enablement exactly once, and a token minted
/// from it resumes no listing selected the other way.
#[tokio::test]
async fn a_walk_of_the_widened_listing_returns_every_enablement_once() {
    for shape in shapes() {
        let case = shape.case;
        let (router, engine) = emulator_with_engine();
        set_up_account(&router).await;
        let applied = apply(&router, &engine, &shape).await;
        tokio::time::sleep(past_the_settle_span()).await;

        let expected = with_inherited(&applied, &applied_directly(&applied));

        // Pages of one — the published minimum of `maxResults` — so the walk crosses a
        // boundary for every enablement there is.
        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut body = serde_json::json!({ "includeChildren": true, "maxResults": 1 });
            if let Some(token) = &next_token {
                body["nextToken"] = serde_json::json!(token);
            }
            let listed = list(&router, case, body).await;
            walked.extend(listed_arns(&listed));
            next_token = listed
                .get("nextToken")
                .as_str()
                .map(std::string::ToString::to_string);
            if next_token.is_none() {
                break;
            }
            assert!(
                walked.len() <= expected.len(),
                "case {case:?}: the walk must end within the {} enablements the listing \
                 holds, got {walked:?}",
                expected.len()
            );
        }
        assert_eq!(
            sorted(walked.clone()),
            expected,
            "case {case:?}: expected the walk to return every enablement exactly once, got \
             {walked:?}"
        );

        // A token minted from the widened listing names a position in THAT selection, so
        // the same walk with the flag unset is a different listing and the token is
        // refused. Only shapes whose widened listing has a second page mint one.
        let first = list(
            &router,
            case,
            serde_json::json!({ "includeChildren": true, "maxResults": 1 }),
        )
        .await;
        if let Some(token) = first.get("nextToken").as_str() {
            let resumed = call(
                &router,
                "/list-enabled-controls",
                serde_json::json!({ "maxResults": 1, "nextToken": token }),
            )
            .await;
            assert_eq!(
                resumed.status, 400,
                "case {case:?}: a token from the widened listing must not resume the \
                 listing without includeChildren, got {} body {:?}",
                resumed.status, resumed.body
            );
            assert_eq!(
                resumed.error_code.as_deref(),
                Some("ValidationException"),
                "case {case:?}: expected ValidationException, got {:?} body {:?}",
                resumed.error_code,
                resumed.body
            );
        }
    }
}

/// RULE, on the store a caller can build through this API alone: `EnableControl` applies a
/// control directly to the unit a request names, so no enablement inherits from another —
/// the listing is then the same whichever way `includeChildren` is sent, no entry reports
/// a parent, a `parentIdentifiers` filter selects nothing, and no enablement is reported
/// for an account.
#[tokio::test]
async fn an_enablement_applied_directly_to_its_target_inherits_from_nothing() {
    let (router, _) = emulator_with_engine();
    set_up_account(&router).await;
    let units = [ou("2a01"), ou("2a02")];
    let mut enabled: Vec<String> = Vec::new();
    for unit in &units {
        for name in ["AWS-GR_ENCRYPTED_VOLUMES", "AWS-GR_RESTRICT_ROOT_USER"] {
            let (arn, _) = enable_with(&router, &control(name), unit, None).await;
            enabled.push(arn);
        }
    }
    let expected = sorted(enabled.clone());

    let mut bodies: Vec<(&str, serde_json::Value)> = Vec::new();
    for (name, body) in [
        ("includeChildren omitted", serde_json::json!({})),
        (
            "includeChildren false",
            serde_json::json!({ "includeChildren": false }),
        ),
        (
            "includeChildren true",
            serde_json::json!({ "includeChildren": true }),
        ),
    ] {
        let listed = list(&router, "directly applied", body.clone()).await;
        assert_eq!(
            listed_arns(&listed),
            expected,
            "{name}: expected exactly the enablements EnableControl minted, got {:?}",
            listed.body
        );
        for entry in entries(listed.get("enabledControls")) {
            assert!(
                entry.get("parentIdentifier").is_none(),
                "{name}: an enablement applied directly to its target inherits from \
                 nothing, so it reports no parentIdentifier, got {entry:?}"
            );
        }
        bodies.push((name, body));
    }
    let first = list(&router, "directly applied", bodies[0].1.clone()).await;
    for (name, body) in &bodies[1..] {
        let listed = list(&router, "directly applied", body.clone()).await;
        assert_eq!(
            listed.body, first.body,
            "{name} must report the same listing as {} while no enablement inherits from \
             another, got {:?}",
            bodies[0].0, listed.body
        );
    }

    // Every value a caller could send `parentIdentifiers` — an enablement's ARN, a unit,
    // an account in the organization — selects nothing while no enablement reports a
    // parent, with the flag either way; and an account target lists nothing, because no
    // enablement is recorded for one.
    let mut values: Vec<String> = enabled.clone();
    values.extend(units.iter().cloned());
    values.push(account("888888888888"));
    for include_children in [false, true] {
        for value in &values {
            let listed = list(
                &router,
                "directly applied",
                serde_json::json!({
                    "includeChildren": include_children,
                    "filter": { "parentIdentifiers": [value] }
                }),
            )
            .await;
            assert!(
                listed_arns(&listed).is_empty(),
                "includeChildren {include_children}: parentIdentifiers {value} must select \
                 nothing while no enablement reports a parent, got {:?}",
                listed.body
            );
        }
        let listed = list(
            &router,
            "directly applied",
            serde_json::json!({
                "targetIdentifier": account("888888888888"),
                "includeChildren": include_children
            }),
        )
        .await;
        assert!(
            listed_arns(&listed).is_empty(),
            "includeChildren {include_children}: no enablement is recorded for an account, \
             so none may be reported for one, got {:?}",
            listed.body
        );
    }
}
