// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHEN this service's organization gate is decided: an operation
//! restricted to the organization's management account is authorized from the CALLER and
//! the recorded organization alone, so that decision is made BEFORE the request's
//! modelled input constraints are validated.
//!
//! The rule, over the class these tables span: for EVERY operation this service gates on
//! the management account, a caller the store does not record as that account is refused
//! with HTTP 403 `AccessDeniedException` whatever the request body carries — a body that
//! breaks a modelled constraint earns exactly the refusal a conforming body earns, the
//! refusal quotes nothing from the request, and nothing is written. The same bodies sent
//! by the permitted caller are then refused by the boundary, in the modelled
//! `ValidationException` (400) — so the gate standing in front of validation does not
//! replace it.
//!
//! The rows differ along the gated OPERATION (`CreateApprovalTeam`,
//! `CreateIdentitySource` — the creates that set an organization's multi-party approval
//! up, i.e. every handler whose decision is
//! `state::is_management_account(state::caller_account_id())`), along WHICH modelled
//! constraint the body breaks and in what way (a `@pattern`, a `@length` maximum, a
//! `@length` minimum, a `@range` minimum, an absent `@required` member), and along WHERE
//! that member sits (top level, inside a nested structure, inside a list element, inside
//! the `ApprovalStrategy` union). Every bound in a comment is named with the modelled
//! member it comes from
//! (`smithy_query(query="operation",operation="CreateApprovalTeam,CreateIdentitySource")`
//! and `smithy_query(query="shape",shape="ApprovalTeamName,Description,Token,
//! MofNApprovalStrategy,PolicyReference,ApprovalTeamRequestApprovers,PoliciesReferences,
//! IdentitySourceParameters,IamIdentityCenter")`).
//!
//! The one overlap with `authorization_gate.rs` is deliberate: that suite's table spans
//! WHO the store records as the management account for a CONFORMING create, and the
//! conforming row below anchors the "same answer as a conforming body" half of this
//! rule. Neither row is redundant with the other.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the route, the URI bindings, the error
//! envelope and the HTTP status are all exercised.

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_team_lifecycle::{error_code, expected_arn, text, Harness, REGION};

/// The refusal a caller outside the organization's management account earns. The
/// requirement is the 403 and the `AccessDeniedException` code; this sentence is the one
/// the recorded service answers this condition with, and this service states it in one
/// place (`state::not_management_account`).
const MANAGEMENT_ACCOUNT_ONLY: &str =
    "Action can only be performed by an organization\u{2019}s management account.";

/// Name of the team a conforming `CreateApprovalTeam` body asks for. Satisfies
/// `ApprovalTeamName` (`@length(max: 64)`, `@pattern ^[a-zA-Z0-9._-]+$`).
const CONFORMING_NAME: &str = "gate-order-team";

/// One approver, satisfying `ApprovalTeamRequestApprover` (`PrimaryIdentityId` is
/// `@length(1..100)`, `PrimaryIdentitySourceArn` `@length(max: 1000)`).
fn approver(n: usize) -> Value {
    json!({
        "PrimaryIdentityId": format!("identity-{n:04}"),
        "PrimaryIdentitySourceArn": format!(
            "arn:aws:mpa:{REGION}:{}:identity-source/ssoins-{n:04}",
            engine::emulator_account_id()
        ),
    })
}

/// One policy reference, satisfying `PolicyReference$PolicyArn`'s modelled pattern
/// `^arn:.{1,63}:mpa:::aws:policy/[a-zA-Z0-9_\.-]{1,1023}/[a-zA-Z0-9_\.-]{1,1023}/(?:[\d]+|\$DEFAULT)$`.
fn policy(n: usize) -> Value {
    json!({
        "PolicyArn":
            format!("arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy{n}/$DEFAULT")
    })
}

/// A `CreateApprovalTeam` body every modelled constraint accepts.
fn conforming_team() -> Value {
    json!({
        "Name": CONFORMING_NAME,
        "Description": "organization gate ordering",
        "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 1 } },
        "Approvers": [approver(1)],
        "Policies": [policy(1)],
    })
}

/// A `CreateIdentitySource` body every modelled constraint accepts: `InstanceArn`
/// satisfies `IdcInstanceArn`'s `@pattern ^arn:.+:sso:::instance/(?:sso)?ins-[a-zA-Z0-9-.]{16}$`
/// and its `@length(20..2048)`.
fn conforming_identity_source() -> Value {
    json!({
        "IdentitySourceParameters": {
            "IamIdentityCenter": {
                "InstanceArn": "arn:aws:sso:::instance/ssoins-1234567890abcdef",
                "Region": REGION,
            }
        }
    })
}

/// `base` with one change applied — the one modelled constraint a row breaks.
fn with(base: fn() -> Value, mutate: impl FnOnce(&mut Value)) -> Value {
    let mut body = base();
    mutate(&mut body);
    body
}

/// Drop a top-level member, for a row that omits an `@required` one.
fn without(base: fn() -> Value, member: &str) -> Value {
    with(base, |body| {
        body.as_object_mut()
            .expect("the create body is a JSON object")
            .remove(member);
    })
}

/// `CreateApprovalTeam` bodies that break exactly one modelled constraint of
/// `CreateApprovalTeamRequest`, one row per kind of constraint and per place it is
/// declared.
fn team_bodies_the_model_refuses() -> Vec<(&'static str, Value)> {
    vec![
        // `ApprovalTeamName`'s `@pattern ^[a-zA-Z0-9._-]+$`: a space is not in the class.
        (
            "Name breaks the modelled pattern (a space)",
            with(conforming_team, |body| {
                body["Name"] = json!("gate order team");
            }),
        ),
        // The same pattern, broken by a different character — the rule is the pattern,
        // not one recorded value.
        (
            "Name breaks the modelled pattern (a colon)",
            with(conforming_team, |body| {
                body["Name"] = json!("gate:order:team");
            }),
        ),
        // The pattern needs one or more characters, so the empty string breaks it.
        (
            "Name is empty, which the modelled pattern admits no value of",
            with(conforming_team, |body| body["Name"] = json!("")),
        ),
        // `ApprovalTeamName` is `@length(max: 64)`: one past the bound.
        (
            "Name is one character past the modelled length maximum (64)",
            with(conforming_team, |body| {
                body["Name"] = json!("n".repeat(65));
            }),
        ),
        // `Description` is `@length(min: 1, max: 256)`: one past each bound.
        (
            "Description is one character past the modelled length maximum (256)",
            with(conforming_team, |body| {
                body["Description"] = json!("x".repeat(257));
            }),
        ),
        (
            "Description is empty, below the modelled length minimum (1)",
            with(conforming_team, |body| body["Description"] = json!("")),
        ),
        // `Approvers` is `@length(min: 1, max: 20)`.
        (
            "Approvers is empty, below the modelled length minimum (1)",
            with(conforming_team, |body| body["Approvers"] = json!([])),
        ),
        (
            "Approvers holds one more than the modelled length maximum (20)",
            with(conforming_team, |body| {
                body["Approvers"] = Value::Array((1..=21).map(approver).collect());
            }),
        ),
        // `Policies` is `@length(min: 1, max: 10)`.
        (
            "Policies is empty, below the modelled length minimum (1)",
            with(conforming_team, |body| body["Policies"] = json!([])),
        ),
        (
            "Policies holds one more than the modelled length maximum (10)",
            with(conforming_team, |body| {
                body["Policies"] = Value::Array((1..=11).map(policy).collect());
            }),
        ),
        // A constraint on a member nested inside a list element:
        // `PolicyReference$PolicyArn`'s pattern.
        (
            "a Policies element's PolicyArn breaks the modelled pattern",
            with(conforming_team, |body| {
                body["Policies"] = json!([{ "PolicyArn": "not-a-policy-arn" }]);
            }),
        ),
        // `PrimaryIdentityId` is `@length(min: 1, max: 100)`, inside an element of
        // `Approvers`.
        (
            "an Approvers element's PrimaryIdentityId is past the modelled length \
             maximum (100)",
            with(conforming_team, |body| {
                body["Approvers"][0]["PrimaryIdentityId"] = json!("i".repeat(101));
            }),
        ),
        // A constraint inside the `ApprovalStrategy` union:
        // `MofNApprovalStrategy$MinApprovalsRequired` is `@range(min: 1)`.
        (
            "MinApprovalsRequired is below the modelled range minimum (1)",
            with(conforming_team, |body| {
                body["ApprovalStrategy"] = json!({ "MofN": { "MinApprovalsRequired": 0 } });
            }),
        ),
        // `ClientToken` is `@length(max: 4096)`.
        (
            "ClientToken is one character past the modelled length maximum (4096)",
            with(conforming_team, |body| {
                body["ClientToken"] = json!("t".repeat(4097));
            }),
        ),
        // The `@required` members, absent — found while READING the request rather than
        // while walking what was read, and still the boundary's refusal.
        (
            "the required member Name is absent",
            without(conforming_team, "Name"),
        ),
        (
            "the required member Description is absent",
            without(conforming_team, "Description"),
        ),
        (
            "the required member ApprovalStrategy is absent",
            without(conforming_team, "ApprovalStrategy"),
        ),
        (
            "the required member Approvers is absent",
            without(conforming_team, "Approvers"),
        ),
        (
            "the required member Policies is absent",
            without(conforming_team, "Policies"),
        ),
    ]
}

/// `CreateIdentitySource` bodies that break exactly one modelled constraint of
/// `CreateIdentitySourceRequest`.
fn identity_source_bodies_the_model_refuses() -> Vec<(&'static str, Value)> {
    vec![
        // `IdcInstanceArn`'s `@pattern ^arn:.+:sso:::instance/(?:sso)?ins-[a-zA-Z0-9-.]{16}$`.
        (
            "InstanceArn breaks the modelled pattern (not an IAM Identity Center \
             instance)",
            with(conforming_identity_source, |body| {
                body["IdentitySourceParameters"]["IamIdentityCenter"]["InstanceArn"] =
                    json!("arn:aws:iam::123456789012:user/not-an-instance");
            }),
        ),
        // The same pattern fixes the instance id at 16 characters; 15 breaks it.
        (
            "InstanceArn's instance id is one character short of the modelled pattern's \
             16",
            with(conforming_identity_source, |body| {
                body["IdentitySourceParameters"]["IamIdentityCenter"]["InstanceArn"] =
                    json!("arn:aws:sso:::instance/ssoins-1234567890abcde");
            }),
        ),
        // `IdcInstanceArn` is `@length(min: 20, max: 2048)`: below the minimum.
        (
            "InstanceArn is shorter than the modelled length minimum (20)",
            with(conforming_identity_source, |body| {
                body["IdentitySourceParameters"]["IamIdentityCenter"]["InstanceArn"] =
                    json!("arn:aws:sso:::i");
            }),
        ),
        // `IamIdentityCenter$Region` is `@length(max: 1000)`.
        (
            "Region is one character past the modelled length maximum (1000)",
            with(conforming_identity_source, |body| {
                body["IdentitySourceParameters"]["IamIdentityCenter"]["Region"] =
                    json!("r".repeat(1001));
            }),
        ),
        // The `@required` members of the nested structure, absent.
        (
            "the required member InstanceArn is absent",
            with(conforming_identity_source, |body| {
                body["IdentitySourceParameters"]["IamIdentityCenter"]
                    .as_object_mut()
                    .expect("IamIdentityCenter is a JSON object")
                    .remove("InstanceArn");
            }),
        ),
        (
            "the required member Region is absent",
            with(conforming_identity_source, |body| {
                body["IdentitySourceParameters"]["IamIdentityCenter"]
                    .as_object_mut()
                    .expect("IamIdentityCenter is a JSON object")
                    .remove("Region");
            }),
        ),
        // The `@required` top-level member, absent.
        (
            "the required member IdentitySourceParameters is absent",
            without(conforming_identity_source, "IdentitySourceParameters"),
        ),
        // `ClientToken` is `@length(max: 4096)`.
        (
            "ClientToken is one character past the modelled length maximum (4096)",
            with(conforming_identity_source, |body| {
                body["ClientToken"] = json!("t".repeat(4097));
            }),
        ),
    ]
}

/// One operation this service gates on the organization's management account, with
/// everything needed to drive it and to see whether it recorded anything.
struct GatedOperation {
    operation: &'static str,
    /// The create's own `@http` URI (`POST`).
    uri: &'static str,
    /// That resource's list operation (`POST <uri>/?List`) and the member it answers in.
    list_uri: &'static str,
    list_member: &'static str,
    /// How many of that resource the store holds.
    stored: fn(&Harness) -> usize,
    /// A body every modelled constraint accepts.
    conforming: fn() -> Value,
    /// Bodies that each break exactly one modelled constraint.
    refused_by_the_model: fn() -> Vec<(&'static str, Value)>,
}

/// Every operation this service gates on the management account — the same set
/// `crate::service`'s gate table holds, which is every handler whose decision is
/// `state::is_management_account(state::caller_account_id())`.
fn gated_operations() -> Vec<GatedOperation> {
    vec![
        GatedOperation {
            operation: "CreateApprovalTeam",
            uri: "/approval-teams",
            list_uri: "/approval-teams/?List",
            list_member: "ApprovalTeams",
            stored: |harness| {
                let store = state::service_state(&*harness.engine).expect("the service store");
                let count = store.lock().approval_teams.len();
                count
            },
            conforming: conforming_team,
            refused_by_the_model: team_bodies_the_model_refuses,
        },
        GatedOperation {
            operation: "CreateIdentitySource",
            uri: "/identity-sources",
            list_uri: "/identity-sources/?List",
            list_member: "IdentitySources",
            stored: |harness| {
                let store = state::service_state(&*harness.engine).expect("the service store");
                let count = store.lock().identity_sources.len();
                count
            },
            conforming: conforming_identity_source,
            refused_by_the_model: identity_source_bodies_the_model_refuses,
        },
    ]
}

/// The gate comes FIRST: for a caller the store does not record as the organization's
/// management account, every body — conforming or not, however it breaks the model —
/// earns the same 403 `AccessDeniedException`, the refusal quotes nothing from the
/// request, and nothing is recorded. Driven over every gated operation.
#[tokio::test]
async fn the_organization_gate_is_decided_before_the_body_is_validated() {
    // (case, who the store records as the organization's management account)
    let callers: [(&str, Option<&str>); 3] = [
        ("no organization recorded", None),
        (
            "another account is the management account",
            Some("999999999999"),
        ),
        (
            "an account one digit from the caller's is the management account",
            Some("000000000001"),
        ),
    ];

    for gated in gated_operations() {
        let bodies: Vec<(&'static str, Value)> = (gated.refused_by_the_model)()
            .into_iter()
            .chain([(
                "a body every modelled constraint accepts",
                (gated.conforming)(),
            )])
            .collect();

        for (caller, management) in callers {
            for (case, body) in &bodies {
                let operation = gated.operation;
                let harness = Harness::without_organization();
                if let Some(account) = management {
                    harness.record_management_account(account);
                }

                let (status, answer) = harness.call("POST", gated.uri, Some(body.clone())).await;
                assert_eq!(
                    status, 403,
                    "{operation} / caller {caller:?} / body {case:?}: expected HTTP 403, \
                     got {status} with {answer:?}"
                );
                assert_eq!(
                    error_code(&answer),
                    "AccessDeniedException",
                    "{operation} / caller {caller:?} / body {case:?}: expected error code \
                     AccessDeniedException, got {answer:?}"
                );
                let message = text(&answer, "Message");
                assert_eq!(
                    message, MANAGEMENT_ACCOUNT_ONLY,
                    "{operation} / caller {caller:?} / body {case:?}: wrong refusal \
                     message, got {message:?}"
                );
                // The authorization decision does not depend on the body, so the refusal
                // says nothing about it: a caller that is refused cannot tell from the
                // answer whether its body would also have been refused.
                assert!(
                    !message.to_lowercase().contains("constraint")
                        && !message.to_lowercase().contains("validation"),
                    "{operation} / caller {caller:?} / body {case:?}: the refusal reports \
                     a constraint failure: {message:?}"
                );

                // A refused create wrote NOTHING: nothing in the store, and nothing for
                // the resource's list operation to report.
                let stored = (gated.stored)(&harness);
                assert_eq!(
                    stored, 0,
                    "{operation} / caller {caller:?} / body {case:?}: the store holds \
                     {stored} resource(s) after a refusal"
                );
                let (status, listed) = harness.call("POST", gated.list_uri, None).await;
                assert_eq!(
                    status, 200,
                    "{operation} / caller {caller:?} / body {case:?}: list returned \
                     {listed:?}"
                );
                let listed_count = listed
                    .get(gated.list_member)
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                assert_eq!(
                    listed_count, 0,
                    "{operation} / caller {caller:?} / body {case:?}: list reported \
                     {listed_count} resource(s) after a refusal"
                );
            }
        }
    }
}

/// Standing in front of validation does not replace it: the SAME bodies, sent by the
/// caller the store records as the management account, are refused by the boundary with
/// the modelled `ValidationException` (400) and record nothing — and a conforming body
/// from that caller is created and reported back out of state.
#[tokio::test]
async fn the_permitted_caller_still_has_its_body_validated() {
    for gated in gated_operations() {
        let operation = gated.operation;
        for (case, body) in (gated.refused_by_the_model)() {
            let harness = Harness::new();

            let (status, answer) = harness.call("POST", gated.uri, Some(body)).await;
            assert_eq!(
                status, 400,
                "{operation} / body {case:?}: expected HTTP 400, got {status} with \
                 {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "{operation} / body {case:?}: expected error code ValidationException, \
                 got {answer:?}"
            );
            let stored = (gated.stored)(&harness);
            assert_eq!(
                stored, 0,
                "{operation} / body {case:?}: the store holds {stored} resource(s) after \
                 a refused create"
            );
        }

        // The conforming body from the permitted caller is created, recorded, and
        // reported back.
        let harness = Harness::new();
        let (status, created) = harness
            .call("POST", gated.uri, Some((gated.conforming)()))
            .await;
        assert_eq!(
            status, 200,
            "{operation}: a conforming create returned {created:?}"
        );
        let stored = (gated.stored)(&harness);
        assert_eq!(
            stored, 1,
            "{operation}: the store holds {stored} resource(s) after a conforming create"
        );
        let (status, listed) = harness.call("POST", gated.list_uri, None).await;
        assert_eq!(status, 200, "{operation}: list returned {listed:?}");
        let listed_count = listed
            .get(gated.list_member)
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        assert_eq!(
            listed_count, 1,
            "{operation}: list reported {listed_count} resource(s) after a conforming \
             create"
        );
    }

    // The approval team also reads back through `GetApprovalTeam` — the round trip the
    // identity source cannot make yet, `GetIdentitySource` still being a stub (its own
    // issue).
    let harness = Harness::new();
    let (status, created) = harness.create(&conforming_team()).await;
    assert_eq!(status, 200, "a conforming create returned {created:?}");
    let arn = text(&created, "Arn");
    assert_eq!(
        arn,
        expected_arn(CONFORMING_NAME),
        "a conforming create minted the wrong Arn: {created:?}"
    );
    let (status, team) = harness.get(&arn).await;
    assert_eq!(status, 200, "the created team reads back as {team:?}");
    assert_eq!(
        text(&team, "Name"),
        CONFORMING_NAME,
        "the created team reads back with the wrong Name: {team:?}"
    );
}
