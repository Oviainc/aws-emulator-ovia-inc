// SPDX-License-Identifier: CC0-1.0

//! What a `clientToken` already spent on a DIFFERENT request means for the Monitor
//! CREATES, driven over HTTP through the same `MultiServiceRouter` the emulator
//! boots with, so every assertion is about the wire behaviour a real client sees.
//!
//! The rule these rows pin: a `clientToken` suppresses re-execution only for a
//! REPEAT of the request it was spent on, and a repeat is recognised by the whole
//! request — not by the token alone. A request differing in any member it carries
//! is a different call, so a create carries it out and answers with the identifiers
//! of the resource IT minted: a create's answer carries nothing of the earlier call
//! but that call's resource, so answering from it would hand this caller a resource
//! it never asked for and mint nothing for the request it actually sent.
//!
//! The rows span what the rule generalises over: the resource kind (portal,
//! project, access policy), the answer shape (a minted id and ARN, with a lifecycle
//! status beside them), and the member the second request differs in — a required
//! name, an optional description the first call did not carry, the parent the
//! resource is created in, an enum permission, a nested identity structure and the
//! tags map.
//!
//! The HTTP harness (`call`, `router_with_engine`), the store counters (`Kind`,
//! `held`), the token helpers (`with_token`, `token_for`) and the portal create are
//! the ones the sibling suites define; all are reused rather than copied.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::access_policy_lifecycle::create_portal;
use crate::asset_lifecycle::{call, router_with_engine};
use crate::client_token_idempotency::with_token;
use crate::client_token_replay::{held, token_for, Kind};

/// One row: a create served under a token, and a DIFFERENT create of the same
/// operation sent under that same, now spent, token.
struct Row {
    case: &'static str,
    kind: Kind,
    /// The create route, which is also the prefix every read of one of these
    /// resources is addressed under (`POST /projects`, `GET /projects/{id}`).
    path: &'static str,
    /// The status the operation models for a served create.
    status: u16,
    id_member: &'static str,
    arn_member: &'static str,
    /// The request the row's token is spent on.
    first: Value,
    /// The same create with ONE member different, which is a different call.
    second: Value,
    /// What a read of the resource the FIRST call minted reports — before the
    /// differing request, and unchanged after it.
    first_reads: Vec<(&'static str, Value)>,
    /// What a read of the resource the SECOND request minted reports: what THAT
    /// request asked for.
    second_reads: Vec<(&'static str, Value)>,
    /// The tags `ListTagsForResource` must report for the second resource, for the
    /// row whose requests differ only in their tags.
    second_tags: Option<Value>,
}

/// The id a create answered with.
fn minted(case: &str, body: &Value, id_member: &str) -> String {
    body[id_member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no {id_member} in {body}"))
        .to_string()
}

/// Assert a read of `uri` reports each of `members`.
async fn assert_reads(
    router: &MultiServiceRouter,
    case: &str,
    step: &str,
    uri: &str,
    members: &[(&'static str, Value)],
) {
    let (status, read) = call(router, "GET", uri, None).await;
    assert_eq!(
        status, 200,
        "case {case:?} {step}: the read of {uri} got {status} {read}"
    );
    for (member, value) in members {
        assert_eq!(
            &read[*member], value,
            "case {case:?} {step}: expected {member} {value}, got {read}"
        );
    }
}

/// The rows, with the portals the project and access-policy rows need created
/// first. Two portals, because one row's second request differs exactly in the
/// portal the project is created in.
async fn rows(router: &MultiServiceRouter) -> Vec<Row> {
    let home = create_portal(router, "spent-token-create-home").await;
    let elsewhere = create_portal(router, "spent-token-create-elsewhere").await;
    let role_arn = "arn:aws:iam::123456789012:role/service-role-monitor";
    let viewer = json!({ "user": { "id": "u-0123456789abcdef" } });
    let another_identity = json!({ "group": { "id": "g-fedcba9876543210" } });
    let resource = json!({ "portal": { "id": home.clone() } });

    vec![
        Row {
            case: "CreatePortal differing in the portal name",
            kind: Kind::Portal,
            path: "/portals",
            status: 202,
            id_member: "portalId",
            arn_member: "portalArn",
            first: json!({
                "portalName": "SpentTokenFirstPortal",
                "portalContactEmail": "admin@example.com",
                "roleArn": role_arn,
            }),
            second: json!({
                "portalName": "SpentTokenSecondPortal",
                "portalContactEmail": "admin@example.com",
                "roleArn": role_arn,
            }),
            first_reads: vec![("portalName", json!("SpentTokenFirstPortal"))],
            second_reads: vec![("portalName", json!("SpentTokenSecondPortal"))],
            second_tags: None,
        },
        Row {
            case: "CreatePortal differing in the description and the email",
            kind: Kind::Portal,
            path: "/portals",
            status: 202,
            id_member: "portalId",
            arn_member: "portalArn",
            first: json!({
                "portalName": "SpentTokenDescribedPortal",
                "portalContactEmail": "first@example.com",
                "roleArn": role_arn,
            }),
            second: json!({
                "portalName": "SpentTokenDescribedPortal",
                "portalContactEmail": "second@example.com",
                "portalDescription": "a description the first call did not carry",
                "roleArn": role_arn,
            }),
            first_reads: vec![
                ("portalContactEmail", json!("first@example.com")),
                // The first call carried no description, so the record holds none.
                ("portalDescription", json!(null)),
            ],
            second_reads: vec![
                ("portalContactEmail", json!("second@example.com")),
                (
                    "portalDescription",
                    json!("a description the first call did not carry"),
                ),
            ],
            second_tags: None,
        },
        Row {
            case: "CreateProject differing in the project name",
            kind: Kind::Project,
            path: "/projects",
            status: 201,
            id_member: "projectId",
            arn_member: "projectArn",
            first: json!({ "portalId": home.clone(), "projectName": "SpentTokenFirstProject" }),
            second: json!({ "portalId": home.clone(), "projectName": "SpentTokenSecondProject" }),
            first_reads: vec![("projectName", json!("SpentTokenFirstProject"))],
            second_reads: vec![("projectName", json!("SpentTokenSecondProject"))],
            second_tags: None,
        },
        Row {
            case: "CreateProject differing in the portal it is created in",
            kind: Kind::Project,
            path: "/projects",
            status: 201,
            id_member: "projectId",
            arn_member: "projectArn",
            first: json!({ "portalId": home.clone(), "projectName": "SpentTokenMovedProject" }),
            second: json!({
                "portalId": elsewhere.clone(),
                "projectName": "SpentTokenMovedProject",
            }),
            first_reads: vec![("portalId", json!(home.clone()))],
            second_reads: vec![("portalId", json!(elsewhere.clone()))],
            second_tags: None,
        },
        Row {
            case: "CreateProject differing only in the tags",
            kind: Kind::Project,
            path: "/projects",
            status: 201,
            id_member: "projectId",
            arn_member: "projectArn",
            first: json!({ "portalId": home.clone(), "projectName": "SpentTokenTaggedProject" }),
            second: json!({
                "portalId": home.clone(),
                "projectName": "SpentTokenTaggedProject",
                "tags": { "stage": "test" },
            }),
            first_reads: vec![("projectName", json!("SpentTokenTaggedProject"))],
            second_reads: vec![("projectName", json!("SpentTokenTaggedProject"))],
            second_tags: Some(json!({ "stage": "test" })),
        },
        Row {
            case: "CreateAccessPolicy differing in the permission",
            kind: Kind::AccessPolicy,
            path: "/access-policies",
            status: 201,
            id_member: "accessPolicyId",
            arn_member: "accessPolicyArn",
            first: json!({
                "accessPolicyIdentity": viewer.clone(),
                "accessPolicyResource": resource.clone(),
                "accessPolicyPermission": "VIEWER",
            }),
            second: json!({
                "accessPolicyIdentity": viewer.clone(),
                "accessPolicyResource": resource.clone(),
                "accessPolicyPermission": "ADMINISTRATOR",
            }),
            first_reads: vec![("accessPolicyPermission", json!("VIEWER"))],
            second_reads: vec![("accessPolicyPermission", json!("ADMINISTRATOR"))],
            second_tags: None,
        },
        Row {
            case: "CreateAccessPolicy differing in the identity it is about",
            kind: Kind::AccessPolicy,
            path: "/access-policies",
            status: 201,
            id_member: "accessPolicyId",
            arn_member: "accessPolicyArn",
            first: json!({
                "accessPolicyIdentity": viewer.clone(),
                "accessPolicyResource": resource.clone(),
                "accessPolicyPermission": "VIEWER",
            }),
            second: json!({
                "accessPolicyIdentity": another_identity.clone(),
                "accessPolicyResource": resource.clone(),
                "accessPolicyPermission": "VIEWER",
            }),
            first_reads: vec![("accessPolicyIdentity", viewer)],
            second_reads: vec![("accessPolicyIdentity", another_identity)],
            second_tags: None,
        },
    ]
}

#[tokio::test]
async fn a_create_differing_from_the_call_its_token_names_mints_a_resource_of_its_own() {
    let (router, engine) = router_with_engine();

    for row in rows(&router).await {
        let Row {
            case,
            kind,
            path,
            status: modelled,
            id_member,
            arn_member,
            first,
            second,
            first_reads,
            second_reads,
            second_tags,
        } = row;
        let token = token_for(case);

        let before = held(&engine, kind);
        let (status, served) = call(&router, "POST", path, Some(with_token(&first, &token))).await;
        assert_eq!(
            status, modelled,
            "case {case:?}: the first create got {status} {served}"
        );
        let after_first = held(&engine, kind);
        assert_eq!(
            after_first,
            before + 1,
            "case {case:?}: the create must mint exactly one resource, {before} -> {after_first}"
        );
        let first_id = minted(case, &served, id_member);
        let first_uri = format!("{path}/{first_id}");
        assert_reads(
            &router,
            case,
            "after the first create",
            &first_uri,
            &first_reads,
        )
        .await;

        // The RETRY: the SAME request under the same token answers from the call it
        // repeats and mints nothing.
        let (status, replayed) = call(&router, "POST", path, Some(with_token(&first, &token))).await;
        assert_eq!(
            status, modelled,
            "case {case:?}: a retry got {status} {replayed}"
        );
        assert_eq!(
            replayed[id_member], served[id_member],
            "case {case:?}: a retry must replay {id_member}, got {replayed} after {served}"
        );
        let after_retry = held(&engine, kind);
        assert_eq!(
            after_retry, after_first,
            "case {case:?}: a retry must mint nothing, {after_first} -> {after_retry}"
        );

        // The DIFFERING request under that same, spent, token: a different call,
        // carried out as the create it is.
        let (status, answered) = call(&router, "POST", path, Some(with_token(&second, &token))).await;
        assert_eq!(
            status, modelled,
            "case {case:?}: a create differing from the call its token names must be served, \
             got {status} {answered}"
        );
        let after_second = held(&engine, kind);
        assert_eq!(
            after_second,
            after_first + 1,
            "case {case:?}: that create must mint one resource of its own, {after_first} -> \
             {after_second}"
        );
        for member in [id_member, arn_member] {
            assert_ne!(
                answered[member], served[member],
                "case {case:?}: the differing create must answer with its OWN {member}, got \
                 {answered} after {served}"
            );
        }

        // The resource it minted holds what THIS request asked for, and the first
        // one is untouched by it.
        let second_id = minted(case, &answered, id_member);
        assert_reads(
            &router,
            case,
            "reading the resource the differing create minted",
            &format!("{path}/{second_id}"),
            &second_reads,
        )
        .await;
        assert_reads(
            &router,
            case,
            "reading the first resource again",
            &first_uri,
            &first_reads,
        )
        .await;

        if let Some(expected) = second_tags {
            let arn = answered[arn_member]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no {arn_member} in {answered}"));
            let (status, tags) = call(&router, "GET", &format!("/tags?resourceArn={arn}"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: ListTagsForResource got {status} {tags}"
            );
            assert_eq!(
                tags["tags"], expected,
                "case {case:?}: expected the tags {expected} this create carried, got {tags}"
            );
        }
    }
}
