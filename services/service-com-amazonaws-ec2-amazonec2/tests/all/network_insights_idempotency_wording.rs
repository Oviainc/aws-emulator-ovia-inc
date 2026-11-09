// SPDX-License-Identifier: CC0-1.0

//! How EC2's Network Access Analyzer / Reachability Analyzer surface WORDS an idempotency
//! conflict, driven end to end over the wire the service speaks — `ec2Query`, through the
//! crate's shared [`super::ec2_query`] harness.
//!
//! The rule under test: a `NetworkInsights*` action given a `ClientToken` a previous call of
//! that action used, with materially different parameters, is refused HTTP 400 with the code
//! `IdempotentParameterMismatch` and that surface's own fixed sentence
//! (`NETWORK_INSIGHTS_IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE`) — while the older resource
//! creates of this service answer the same code with the sentence EC2 words there
//! (`IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE`). Neither sentence carries a value from the
//! request or from the store, so a row asserts the WHOLE message rather than a fragment of
//! it.
//!
//! Each row of the table is a different conflicting request along what that generalises
//! over: which operation of the surface, which member of the request differs (the content
//! paths, the tags, the analyzed scope, the analyzed path), and — for the contrast rows —
//! which operation OUTSIDE the surface, so neither one recorded call nor one operation is the
//! specification.

use ec2::state::{
    IDEMPOTENT_PARAMETER_MISMATCH, IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE,
    NETWORK_INSIGHTS_IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, emulator, error_code, error_message};
use super::wire::{param, Param};

/// The Region every call here is signed for. Nothing in the rule depends on it: the wording
/// of an idempotency conflict is a fact of the operation, not of the Region.
const REGION: &str = "us-west-2";

/// The HTTP status EC2 answers an idempotency conflict with.
const CONFLICT_STATUS: u16 = 400;

/// One call signed for [`REGION`].
async fn api(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// One call that has to succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = api(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// The form fields of a Network Access Scope create whose single `MatchPaths` entry starts at
/// resources of `resource_type`.
fn scope_params(client_token: &str, resource_type: &str, tag: Option<&str>) -> Vec<Param> {
    let mut params = vec![
        param("ClientToken", client_token),
        param(
            "MatchPath.1.Source.ResourceStatement.ResourceType.1",
            resource_type,
        ),
    ];
    if let Some(value) = tag {
        params.push(param(
            "TagSpecification.1.ResourceType",
            ec2::state::RESOURCE_TYPE_NETWORK_INSIGHTS_ACCESS_SCOPE,
        ));
        params.push(param("TagSpecification.1.Tag.1.Key", "Name"));
        params.push(param("TagSpecification.1.Tag.1.Value", value));
    }
    params
}

/// A Reachability Analyzer path of the account, minted through
/// `CreateNetworkInsightsPath`, as its id.
async fn create_path(router: &MultiServiceRouter, client_token: &str, source: &str) -> String {
    let body = ok(
        client_token,
        router,
        "CreateNetworkInsightsPath",
        &[
            param("ClientToken", client_token),
            param("Source", source),
            param("Destination", "i-0123456789abcdef0"),
            param("Protocol", "tcp"),
        ],
    )
    .await;
    element(&body, "networkInsightsPathId")
        .unwrap_or_else(|| panic!("a created path must report its id, got {body}"))
}

/// A Network Access Scope of the account, as its id.
async fn create_scope(router: &MultiServiceRouter, client_token: &str) -> String {
    let body = ok(
        client_token,
        router,
        "CreateNetworkInsightsAccessScope",
        &scope_params(client_token, "AWS::EC2::VPC", None),
    )
    .await;
    element(&body, "networkInsightsAccessScopeId")
        .unwrap_or_else(|| panic!("a created scope must report its id, got {body}"))
}

/// One row: an operation, the request a token is first given with, the DIFFERENT request the
/// same token is then replayed with, and the whole sentence that operation refuses the replay
/// with.
struct Conflict {
    case: &'static str,
    action: &'static str,
    first: Vec<Param>,
    replayed: Vec<Param>,
    message: &'static str,
}

/// The two sentences are different texts: a row asserting one would otherwise pass against
/// the other, so the table below could not tell the surfaces apart.
#[test]
fn the_two_idempotency_sentences_are_distinct() {
    assert_ne!(
        NETWORK_INSIGHTS_IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE, IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE,
        "the Network Insights sentence and the sentence the older creates answer must differ, \
         both are {NETWORK_INSIGHTS_IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE:?}"
    );
}

/// Every operation of the Network Insights surface that reads a `ClientToken` refuses a
/// replay carrying a different request with that surface's sentence, and the older resource
/// creates of this service keep their own — same code, same status, different text.
#[tokio::test]
async fn an_idempotency_conflict_is_worded_by_the_operation_s_surface() {
    let (router, _engine) = emulator();

    // What the Network Insights analyses are started on: two scopes and two paths, so a
    // token can be replayed naming the OTHER one.
    let scope_a = create_scope(&router, "scope-a").await;
    let scope_b = create_scope(&router, "scope-b").await;
    assert_ne!(
        scope_a, scope_b,
        "two creates under different tokens must mint two scopes, both got {scope_a:?}"
    );
    let path_a = create_path(&router, "path-a", "igw-0123456789abcdef0").await;
    let path_b = create_path(&router, "path-b", "i-0fedcba9876543210").await;
    assert_ne!(
        path_a, path_b,
        "two creates under different tokens must mint two paths, both got {path_a:?}"
    );

    // The Availability Zone a `CreateVolume` contrast row names, read from the Region's own
    // zones rather than written out here.
    let zone = ec2::state::availability_zones_in_region(REGION)[0]
        .zone_name
        .clone();

    let conflicts = vec![
        Conflict {
            case: "a scope create replayed with different match paths",
            action: "CreateNetworkInsightsAccessScope",
            first: scope_params("scope-paths", "AWS::EC2::VPC", None),
            replayed: scope_params("scope-paths", "AWS::EC2::Subnet", None),
            message: NETWORK_INSIGHTS_IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE,
        },
        Conflict {
            case: "a scope create replayed with different tags",
            action: "CreateNetworkInsightsAccessScope",
            first: scope_params("scope-tags", "AWS::EC2::VPC", Some("first")),
            replayed: scope_params("scope-tags", "AWS::EC2::VPC", Some("second")),
            message: NETWORK_INSIGHTS_IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE,
        },
        Conflict {
            case: "a scope analysis replayed naming another scope",
            action: "StartNetworkInsightsAccessScopeAnalysis",
            first: vec![
                param("ClientToken", "scope-analysis"),
                param("NetworkInsightsAccessScopeId", &scope_a),
            ],
            replayed: vec![
                param("ClientToken", "scope-analysis"),
                param("NetworkInsightsAccessScopeId", &scope_b),
            ],
            message: NETWORK_INSIGHTS_IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE,
        },
        Conflict {
            case: "a path analysis replayed naming another path",
            action: "StartNetworkInsightsAnalysis",
            first: vec![
                param("ClientToken", "path-analysis"),
                param("NetworkInsightsPathId", &path_a),
            ],
            replayed: vec![
                param("ClientToken", "path-analysis"),
                param("NetworkInsightsPathId", &path_b),
            ],
            message: NETWORK_INSIGHTS_IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE,
        },
        // Outside the surface: the older creates answer the same code with their own
        // sentence, so the wording above is the operation's and not the service's.
        Conflict {
            case: "a volume create replayed with a different size",
            action: "CreateVolume",
            first: vec![
                param("ClientToken", "volume-token"),
                param("AvailabilityZone", &zone),
                param("Size", "8"),
            ],
            replayed: vec![
                param("ClientToken", "volume-token"),
                param("AvailabilityZone", &zone),
                param("Size", "16"),
            ],
            message: IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE,
        },
        Conflict {
            case: "an IPAM create replayed with a different description",
            action: "CreateIpam",
            first: vec![
                param("ClientToken", "ipam-token"),
                param("Description", "the first request"),
            ],
            replayed: vec![
                param("ClientToken", "ipam-token"),
                param("Description", "another request"),
            ],
            message: IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE,
        },
    ];

    for conflict in &conflicts {
        let Conflict {
            case,
            action,
            first,
            replayed,
            message,
        } = conflict;
        ok(case, &router, action, first).await;

        // The same token with the same request is idempotent, so what the row below asserts
        // is about the DIFFERENT request and not about the token being seen twice.
        let (status, body) = api(&router, action, first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an identical replay of {action} must be served, got {status} with \
             body {body}"
        );

        let (status, body) = api(&router, action, replayed).await;
        assert_eq!(
            status, CONFLICT_STATUS,
            "case {case:?}: expected HTTP {CONFLICT_STATUS} from {action}, got {status} with \
             body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, IDEMPOTENT_PARAMETER_MISMATCH,
            "case {case:?}: expected the code {IDEMPOTENT_PARAMETER_MISMATCH:?} from {action}, \
             got {code:?} in {body}"
        );
        let answered = error_message(&body);
        assert_eq!(
            answered, *message,
            "case {case:?}: expected the message {message:?} from {action}, got {answered:?}"
        );
    }
}
