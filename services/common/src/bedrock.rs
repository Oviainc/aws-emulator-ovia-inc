// SPDX-License-Identifier: CC0-1.0

//! The request-claim rule for the SIX emulated services whose SigV4 signing name is
//! `bedrock`, stated once here and read by every one of them.
//!
//! `aws.auth#sigv4` names `bedrock` on all six Bedrock-family models, while each one
//! serves its own endpoint (`smithy_query(query="service_info")` on each model gives the
//! endpoint prefixes `bedrock`, `bedrock-runtime`, `bedrock-agent`,
//! `bedrock-agent-runtime`, `bedrock-data-automation` and
//! `bedrock-data-automation-runtime` against that ONE signing name). The router resolves
//! a request's routing key from the SigV4 credential scope first, so all six see the key
//! `bedrock`: a service that claims on the signing name ALONE claims every sibling's
//! traffic too, the router finds six claimants and — refusing to serve a request by an
//! arbitrary one — answers `500 AmbiguousService`, a code no AWS model carries, for every
//! Bedrock request. AWS separates them by endpoint HOST, which an emulator cannot read: a
//! client configured with one local endpoint sends the same `Host` for all six.
//!
//! The claim is therefore [`crate::claims_request`]'s rule — the signing name AND the
//! request's own modelled wire surface, which is the first URI path segment for a REST
//! model and the service the `X-Amz-Target` names for the `awsJson1_1` one — narrowed by
//! the split below wherever MORE THAN ONE of these models publishes the same first path
//! segment. Source for every segment named here: `smithy_query(query="digest")` on each
//! of the six models.

use smithy_server_runtime::ErasedServiceDefinition;

use crate::shared_path::{shared_path_arn, SharedPathArn};

/// The `aws.api#service$arnNamespace` every Bedrock-family model shares
/// (`smithy_query(query="service_info")` on each of the six reports `bedrock`), and so
/// the only service field an ARN any of them addresses can carry.
pub const ARN_NAMESPACE: &str = "bedrock";

/// The Smithy shape id of each emulated service that signs as `bedrock`, as
/// `definition().identity().shape_id()` reports it. A member names itself by its shape id
/// rather than by a flag the call site passes, so the ownership table below cannot
/// disagree with the service reading it.
pub const CONTROL_PLANE: &str = "com.amazonaws.bedrock#AmazonBedrockControlPlaneService";
/// The `bedrock-runtime` data plane (`Converse`, `InvokeModel`, `ApplyGuardrail`).
pub const RUNTIME: &str = "com.amazonaws.bedrockruntime#AmazonBedrockFrontendService";
/// The `bedrock-agent` build-time control plane (agents, knowledge bases, flows, prompts).
pub const AGENT: &str = "com.amazonaws.bedrockagent#AmazonBedrockAgentBuildTimeLambda";
/// The `bedrock-agent-runtime` data plane (`InvokeAgent`, `Retrieve`, `InvokeFlow`).
pub const AGENT_RUNTIME: &str =
    "com.amazonaws.bedrockagentruntime#AmazonBedrockAgentRunTimeService";
/// The `bedrock-data-automation` build-time control plane (blueprints, projects).
pub const DATA_AUTOMATION: &str =
    "com.amazonaws.bedrockdataautomation#AmazonBedrockKeystoneBuildTimeService";
/// The `bedrock-data-automation-runtime` data plane — the one `awsJson1_1` model of the
/// six, which publishes no `smithy.api#http` bindings and so posts every operation to `/`.
pub const DATA_AUTOMATION_RUNTIME: &str =
    "com.amazonaws.bedrockdataautomationruntime#AmazonBedrockKeystoneRuntimeService";

/// How a first path segment that more than one Bedrock-family model publishes is split
/// between its sharers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shared {
    /// The sharers bind DIFFERENT routes under this segment, so the whole route — method
    /// plus URI pattern — tells them apart: each one claims only a request its own model
    /// binds a route for ([`crate::addresses_modeled_route`]).
    ///
    /// A path under such a segment that NO model binds therefore reaches nobody, and the
    /// router answers it `404 UnknownService` rather than one service's protocol routing
    /// error. That is the narrow cost of not guessing: no AWS operation lives on such a
    /// path, while handing the whole segment to one sharer would make every one of the
    /// other's operations in it unreachable.
    ByRoute,
    /// The sharers bind the SAME routes under this segment, so no part of the request
    /// tells them apart (AWS tells them apart by endpoint host, which is not on the
    /// wire here). The named service answers the whole segment; its sharer's operations
    /// there are unreachable, which keeps the owner's working instead of refusing both
    /// with `500 AmbiguousService`.
    OwnedBy(&'static str),
    /// The sharers bind the SAME routes under this segment, and the `@httpLabel` in the
    /// path is a resource ARN — so the request DOES tell them apart: a resource belongs
    /// to exactly one service, the one whose own model's taggable-resource ARN pattern
    /// admits that resource type and whose own operations mint it. The resource TYPE
    /// therefore decides the claim ([`crate::shared_path::shared_path_arn`]).
    ///
    /// `prefix` is the literal segments leading to the label, `owners` the resource
    /// types one sharer's model claims, and `default_owner` the sharer every other type
    /// — and a label that names no resource in the family's ARN namespace — belongs to,
    /// so a request on this space always reaches exactly one claimant.
    ByArnResourceType {
        prefix: &'static [&'static str],
        owners: &'static [(&'static str, &'static str)],
        default_owner: &'static str,
    },
}

/// The resource types the `bedrock-agent-runtime` model's own `TaggableResourcesArn`
/// admits under `/tags/{resourceArn}`: the SESSION, and nothing else
/// (`smithy_query(query="operation",operation="TagResource")` on that model ->
/// `resourceArn`'s pattern is `…:(session)/<uuid>$`). Every other `bedrock` resource type
/// — the build-time plane's `agent`, `agent-alias`, `knowledge-base`, `flow`, `prompt`, …
/// — is named only by the `bedrock-agent` model's own pattern, so it stays with that
/// plane.
const TAGS_BY_RESOURCE_TYPE: &[(&str, &str)] = &[("session", AGENT_RUNTIME)];

/// Every first URI path segment more than one Bedrock-family model publishes, and how it
/// is split. Each sharer reads this one table, so the claims cannot drift into a path
/// with two claimants (the `AmbiguousService` refusal) or none.
///
/// * `agents`, `flows`, `knowledgebases` — `bedrock-agent` (build time) and
///   `bedrock-agent-runtime` both publish them and bind different routes underneath
///   (`PUT /knowledgebases/` and `POST /knowledgebases/{knowledgeBaseId}/datasources/`
///   against `POST /knowledgebases/{knowledgeBaseId}/retrieve`), so the route decides.
/// * `tags` — the same two both bind `GET`, `POST` and `DELETE /tags/{resourceArn}`, the
///   identical three routes, so neither the method nor the URI template decides; the ARN
///   in the path does, because the two models' taggable-resource patterns name DISJOINT
///   resource types (`session` against `agent`/`agent-alias`/`knowledge-base`/`flow`/
///   `prompt`/…). A session ARN is the data plane's, every other `bedrock` resource — and
///   a label that is no `bedrock` resource ARN at all — the build-time plane's, which owns
///   the resources being tagged.
/// * `listTagsForResource`, `tagResource`, `untagResource` — the `bedrock` control plane
///   and `bedrock-data-automation` both bind `POST` on each of these three, again
///   identically, and the control plane is the member every Bedrock tagging flow uses.
const SHARED_SEGMENTS: &[(&str, Shared)] = &[
    ("agents", Shared::ByRoute),
    ("flows", Shared::ByRoute),
    ("knowledgebases", Shared::ByRoute),
    ("listTagsForResource", Shared::OwnedBy(CONTROL_PLANE)),
    ("tagResource", Shared::OwnedBy(CONTROL_PLANE)),
    (
        "tags",
        Shared::ByArnResourceType {
            prefix: &["tags"],
            owners: TAGS_BY_RESOURCE_TYPE,
            default_owner: AGENT,
        },
    ),
    ("untagResource", Shared::OwnedBy(CONTROL_PLANE)),
];

/// Whether the Bedrock-family service whose generated `ServiceDefinition` is `definition`
/// claims `parts`.
///
/// * `path_segments` — that service's generated `crate::ROUTE_PATH_SEGMENTS`, the first
///   URI path segments its model declares.
/// * `rpc_service_shape_id` — `Some(definition.identity().shape_id())` for the one model
///   of the six that declares an RPC protocol (`awsJson1_1`), whose wire form names its
///   service out of band; `None` for the five `restJson1` models, which are discriminated
///   by their path space.
///
/// The signing name is read off `definition`, so a member cannot claim under a name its
/// model does not declare.
pub fn claims_request(
    parts: &http::request::Parts,
    definition: &dyn ErasedServiceDefinition,
    path_segments: &[&str],
    rpc_service_shape_id: Option<&str>,
) -> bool {
    let identity = definition.identity();
    if !crate::claims_request(
        parts,
        identity.signing_name(),
        path_segments,
        rpc_service_shape_id,
    ) {
        return false;
    }
    match shared_split(parts.uri.path()) {
        None => true,
        Some(Shared::ByRoute) => crate::addresses_modeled_route(parts, definition),
        Some(Shared::OwnedBy(owner)) => identity.shape_id() == owner,
        Some(Shared::ByArnResourceType {
            prefix,
            owners,
            default_owner,
        }) => identity.shape_id() == arn_resource_owner(parts, prefix, owners, default_owner),
    }
}

/// The one service that owns the resource the ARN in `parts`' path names: the sharer
/// whose model claims that resource TYPE, else `default_owner`.
fn arn_resource_owner(
    parts: &http::request::Parts,
    prefix: &'static [&'static str],
    owners: &'static [(&'static str, &'static str)],
    default_owner: &'static str,
) -> &'static str {
    match shared_path_arn(parts.uri.path(), prefix, ARN_NAMESPACE) {
        SharedPathArn::Resource(kind) => owners
            .iter()
            .find(|(resource_type, _)| *resource_type == kind)
            .map_or(default_owner, |(_, owner)| *owner),
        // Not an ARN this family's models name a resource with: the declared owner
        // answers it, so the request reaches one claimant and gets that service's own
        // modelled refusal rather than a `404 UnknownService`.
        SharedPathArn::NotShared | SharedPathArn::Unrecognised => default_owner,
    }
}

/// How the first segment of `path` is split, or `None` when only one Bedrock-family model
/// publishes it (the overwhelmingly common case, where the first segment already names
/// exactly one member). Matching is exact segment equality, as the framework's own
/// discriminator is.
fn shared_split(path: &str) -> Option<Shared> {
    let segment = super::first_path_segment(path)?;
    SHARED_SEGMENTS
        .iter()
        .find(|(shared, _)| *shared == segment)
        .map(|(_, split)| *split)
}

#[cfg(test)]
mod tests {
    //! The parts of the rule that need no built service: which segments are shared, and
    //! how each one is split.

    use super::*;

    /// Every segment in the table is split exactly one way, and a segment only one model
    /// publishes is not in it at all.
    #[test]
    fn the_shared_segments_are_declared_once_each() {
        // (case, path, expected split)
        let rows: [(&str, &str, Option<Shared>); 12] = [
            ("the agent pair's agents", "/agents/", Some(Shared::ByRoute)),
            (
                "a deep path under it",
                "/agents/AGENT1/agentversions/1/knowledgebases/",
                Some(Shared::ByRoute),
            ),
            ("flows", "/flows/f-1/aliases", Some(Shared::ByRoute)),
            (
                "knowledgebases",
                "/knowledgebases/kb-1/retrieve",
                Some(Shared::ByRoute),
            ),
            (
                "the agent pair's identical tagging routes",
                "/tags/arn%3Aaws%3Abedrock%3A%3A%3Aagent%2Fa1",
                Some(Shared::ByArnResourceType {
                    prefix: &["tags"],
                    owners: TAGS_BY_RESOURCE_TYPE,
                    default_owner: AGENT,
                }),
            ),
            (
                "the control plane's tagging routes",
                "/tagResource",
                Some(Shared::OwnedBy(CONTROL_PLANE)),
            ),
            (
                "untagResource",
                "/untagResource",
                Some(Shared::OwnedBy(CONTROL_PLANE)),
            ),
            (
                "listTagsForResource",
                "/listTagsForResource",
                Some(Shared::OwnedBy(CONTROL_PLANE)),
            ),
            // Segments exactly one of the six models publishes: not shared, so the first
            // segment alone already names one claimant.
            ("the control plane's guardrails", "/guardrails", None),
            ("the data plane's model invocations", "/model/m-1/invoke", None),
            ("a near miss on a shared segment", "/agentsextra", None),
            ("the root path", "/", None),
        ];

        for (case, path, expected) in rows {
            let actual = shared_split(path);
            assert_eq!(
                actual, expected,
                "case {case:?}: {path:?} expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// A shared segment owned by one member is owned by a member of this family, and no
    /// segment is declared twice.
    #[test]
    fn every_owner_is_a_member_and_no_segment_repeats() {
        let members = [
            CONTROL_PLANE,
            RUNTIME,
            AGENT,
            AGENT_RUNTIME,
            DATA_AUTOMATION,
            DATA_AUTOMATION_RUNTIME,
        ];
        let mut seen: Vec<&str> = Vec::new();
        for (segment, split) in SHARED_SEGMENTS {
            assert!(
                !seen.contains(segment),
                "segment {segment:?} is declared more than once"
            );
            seen.push(segment);
            match split {
                Shared::OwnedBy(owner) => assert!(
                    members.contains(owner),
                    "segment {segment:?} is owned by {owner:?}, which is not a member of \
                     the bedrock family"
                ),
                Shared::ByArnResourceType {
                    owners,
                    default_owner,
                    ..
                } => {
                    assert!(
                        members.contains(default_owner),
                        "segment {segment:?} falls back to {default_owner:?}, which is not a \
                         member of the bedrock family"
                    );
                    for (resource_type, owner) in *owners {
                        assert!(
                            members.contains(owner),
                            "segment {segment:?} gives {resource_type:?} to {owner:?}, which \
                             is not a member of the bedrock family"
                        );
                    }
                }
                Shared::ByRoute => {}
            }
        }
    }

    /// RULE: on a path space the sharers bind identically, whose `@httpLabel` is a
    /// resource ARN, the resource TYPE the ARN names decides the claim — the service
    /// whose own model's taggable-resource pattern admits that type — and every label
    /// that names no resource of this family falls to the declared owner, so exactly one
    /// service claims any such request.
    #[test]
    fn the_tagging_path_is_claimed_by_the_service_that_owns_the_arns_resource() {
        /// The label as a client that percent-encodes a non-greedy `@httpLabel` sends it.
        fn encoded(arn: &str) -> String {
            arn.replace(':', "%3A").replace('/', "%2F")
        }

        const SESSION: &str =
            "arn:aws:bedrock:us-west-2:000000000000:session/0cabde93-58b4-4d68-b230-90df328e1acb";

        // The rows differ along the resource type the ARN names, the partition, the
        // spelling of the label's separators, and the forms that name no resource at all.
        let rows: Vec<(&str, String, &str)> = vec![
            (
                "a session ARN, percent-encoded as an SDK sends it",
                format!("/tags/{}", encoded(SESSION)),
                AGENT_RUNTIME,
            ),
            (
                "the same session ARN sent raw",
                format!("/tags/{SESSION}"),
                AGENT_RUNTIME,
            ),
            (
                "a session ARN in another partition",
                format!(
                    "/tags/{}",
                    encoded(
                        "arn:aws-us-gov:bedrock:us-gov-west-1:000000000000:session/\
                         11111111-2222-3333-4444-555555555555"
                    )
                ),
                AGENT_RUNTIME,
            ),
            (
                "a non-canonically cased session ARN",
                format!(
                    "/tags/{}",
                    encoded(
                        "arn:aws:Bedrock:us-west-2:000000000000:Session/\
                         0cabde93-58b4-4d68-b230-90df328e1acb"
                    )
                ),
                AGENT_RUNTIME,
            ),
            (
                "an agent ARN",
                format!(
                    "/tags/{}",
                    encoded("arn:aws:bedrock:us-east-1:000000000000:agent/AGENT12345")
                ),
                AGENT,
            ),
            (
                "a knowledge-base ARN",
                format!(
                    "/tags/{}",
                    encoded("arn:aws:bedrock:us-east-1:000000000000:knowledge-base/KB12345678")
                ),
                AGENT,
            ),
            (
                "a prompt ARN",
                format!(
                    "/tags/{}",
                    encoded("arn:aws:bedrock:us-east-1:000000000000:prompt/PROMPT1234")
                ),
                AGENT,
            ),
            (
                "another service's ARN",
                format!(
                    "/tags/{}",
                    encoded("arn:aws:sqs:us-east-1:000000000000:session/q")
                ),
                AGENT,
            ),
            (
                "a label that is not an ARN",
                "/tags/not-an-arn".to_string(),
                AGENT,
            ),
            ("no label at all", "/tags".to_string(), AGENT),
        ];

        for (case, path, expected) in rows {
            let parts = parts_for(&path);
            let actual = arn_resource_owner(&parts, &["tags"], TAGS_BY_RESOURCE_TYPE, AGENT);
            assert_eq!(
                actual, expected,
                "case {case:?} ({path}): expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// The request parts for one path, which is all the claim rule reads here.
    fn parts_for(path: &str) -> http::request::Parts {
        http::Request::builder()
            .method("POST")
            .uri(path)
            .body(())
            .expect("the test request builds")
            .into_parts()
            .0
    }
}
