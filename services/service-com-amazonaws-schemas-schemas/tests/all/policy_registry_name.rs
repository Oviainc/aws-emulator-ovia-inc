// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ONE resolution every resource-policy operation makes
//! on the registry its request names.
//!
//! The rule: an operation scoped to a registry's resource-based policy resolves the
//! optional `registryName` query once — a request carrying none is refused 400
//! `BadRequestException` naming the member it needs, a name outside the service's
//! published constraint for a registry name is refused 400 `BadRequestException` naming
//! the parameter and the rule it broke, and a name the service accepts is served.
//!
//! The rule is asserted over a TABLE whose rows differ along what it generalises over:
//! the operation the request calls (and so the HTTP method, the body it carries and the
//! status a served call answers), and the form of the name the query carries — absent,
//! outside the accepted character set, past the published length bound, and accepted at
//! either end of that bound. Every operation drives every form, so no row can pass by
//! refusing everything or by serving everything.
//!
//! Every case drives the real wire path: a signed `restJson1` request through
//! `MultiServiceRouter::dispatch`, exactly as the production router serves it. The HTTP
//! harness is the crate's one harness, imported from `registry_schema_lifecycle`, and
//! the `/v1/policy` route from `resource_policy`.

use serde_json::json;
use service_com_amazonaws_schemas_schemas::state::{CODE_MALFORMED_REQUEST, REGISTRY_NAME};

use crate::registry_schema_lifecycle::{create_registry, Answer, Emulator};
use crate::resource_policy::policy_uri;

/// A policy document every call in this suite stores and reads back.
const POLICY: &str = r#"{"Version":"2012-10-17","Statement":[]}"#;

/// The sentence the service refuses a policy request that names no registry with.
/// Spelled out here rather than taken from the handler, so a message that changed would
/// fail the case.
const NO_REGISTRY_SENTENCE: &str = "RegistryName is required: a resource-based policy \
                                    belongs to one registry, so the request must name \
                                    the registry it applies to.";

/// One resource-policy operation: one row of the table, and one of the handlers that
/// used to carry its own copy of the require-then-check step.
#[derive(Debug, Clone, Copy)]
enum PolicyCall {
    /// `PutResourcePolicy`: attach a policy to the registry the query names.
    Put,
    /// `GetResourcePolicy`: report the policy of the registry the query names.
    Get,
    /// `DeleteResourcePolicy`: remove the policy of the registry the query names.
    Delete,
}

impl PolicyCall {
    /// Every row of the table.
    fn all() -> Vec<Self> {
        vec![Self::Put, Self::Get, Self::Delete]
    }

    /// The operation name, for the case label every assertion carries.
    fn case(self) -> &'static str {
        match self {
            Self::Put => "PutResourcePolicy",
            Self::Get => "GetResourcePolicy",
            Self::Delete => "DeleteResourcePolicy",
        }
    }

    /// The status this call answers once the registry its query names is one the caller
    /// owns and holds what the call needs.
    fn served_status(self) -> u16 {
        // Every method on the policy endpoint answers 200, the removal included.
        200
    }

    /// Send this call with `registry` as the `registryName` query, or with the query
    /// left out entirely when it is `None`.
    async fn send(self, emulator: &Emulator, registry: Option<&str>) -> Answer {
        let uri = policy_uri(registry);
        match self {
            Self::Put => {
                emulator
                    .call("PUT", &uri, Some(json!({"Policy": POLICY})))
                    .await
            }
            Self::Get => emulator.call("GET", &uri, None).await,
            Self::Delete => emulator.call("DELETE", &uri, None).await,
        }
    }

    /// Create `registry` as a registry THIS caller owns, holding what this row's call
    /// needs in order to be served: a policy for the read and the removal, and nothing
    /// but the registry for the first write of its policy.
    async fn seed_owned(self, emulator: &Emulator, registry: &str) {
        create_registry(emulator, registry).await;
        match self {
            Self::Put => {}
            Self::Get | Self::Delete => {
                let attached = Self::Put.send(emulator, Some(registry)).await;
                assert_eq!(
                    attached.status,
                    200,
                    "case {:?}: seeding a policy must succeed, got {} {:?}",
                    self.case(),
                    attached.status,
                    attached.body
                );
            }
        }
    }
}

/// Which of the two rules a name the service does not accept breaks, and the sentence
/// the service refuses it with.
#[derive(Debug, Clone, Copy)]
enum Broken {
    /// It is built from characters the service does not accept.
    Pattern,
    /// It is longer than the service accepts.
    Length,
}

impl Broken {
    fn sentence(self) -> String {
        match self {
            Self::Pattern => format!(
                "Parameter {} doesn't match pattern {}",
                REGISTRY_NAME.parameter,
                REGISTRY_NAME.pattern()
            ),
            Self::Length => format!(
                "Parameter {} must have length less than or equal to {}",
                REGISTRY_NAME.parameter, REGISTRY_NAME.max_length
            ),
        }
    }
}

/// The names the query carries that the service does not accept.
///
/// Source for the accepted set and the bound: `crate::state::REGISTRY_NAME`, the
/// constraint the service publishes for a registry's name — letters, digits, `.`, `-`
/// and `_`, up to 64 characters. The rows span both ways of breaking it: a character
/// outside the set (including the `@` only a SCHEMA name may carry), and the length
/// bound, one character past it and far past it.
fn refused_names() -> Vec<(&'static str, String, Broken)> {
    vec![
        ("a space", "my%20registry".to_string(), Broken::Pattern),
        ("an at sign", "my@registry".to_string(), Broken::Pattern),
        ("a slash", "my%2Fregistry".to_string(), Broken::Pattern),
        (
            "a non-ASCII letter",
            "caf%C3%A9".to_string(),
            Broken::Pattern,
        ),
        (
            "one character past the longest accepted name",
            "r".repeat(REGISTRY_NAME.max_length + 1),
            Broken::Length,
        ),
        (
            "far past the longest accepted name",
            "r".repeat(REGISTRY_NAME.max_length * 3),
            Broken::Length,
        ),
    ]
}

/// The names the query carries that the service accepts: one per accepted character
/// class, every class at once, and the longest name the published bound admits.
fn accepted_names() -> Vec<(&'static str, String)> {
    vec![
        ("a single letter", "r".to_string()),
        ("a single digit", "7".to_string()),
        ("every accepted class at once", "Svc.4-policy_X".to_string()),
        (
            "the longest accepted name",
            "r".repeat(REGISTRY_NAME.max_length),
        ),
    ]
}

/// Assert `answer` is the service's parameter-validation refusal carrying `sentence`.
fn assert_refused(case: &str, answer: &Answer, sentence: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected 400, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_type(),
        "BadRequestException",
        "case {case:?}: expected the BadRequestException shape, got {:?}",
        answer.header("x-amzn-errortype")
    );
    assert_eq!(
        answer.error_code(),
        "BadRequestException",
        "case {case:?}: expected the BadRequestException wire code, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("Code"),
        CODE_MALFORMED_REQUEST,
        "case {case:?}: expected the {CODE_MALFORMED_REQUEST} condition code, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("Message"),
        sentence,
        "case {case:?}: wrong refusal sentence, got {:?}",
        answer.body
    );
}

/// RULE: every operation scoped to a registry's resource-based policy resolves its
/// `registryName` query once — absent is refused 400 naming the member it needs, a name
/// outside the published constraint is refused 400 naming the parameter and the rule it
/// broke, and a name the service accepts is served.
#[tokio::test]
async fn every_policy_operation_resolves_its_registry_name_the_same_way() {
    for call in PolicyCall::all() {
        let case = call.case();

        // Step one: no `registryName` at all names no resource to act on. A registry the
        // caller owns and its policy exist in this store, so what the refusal reports is
        // the request's own form and not a missing resource.
        let emulator = Emulator::new();
        let present = "policy-holder";
        PolicyCall::Delete.seed_owned(&emulator, present).await;
        let unnamed = call.send(&emulator, None).await;
        assert_refused(
            &format!("{case}/no registryName"),
            &unnamed,
            NO_REGISTRY_SENTENCE,
        );

        // And the refusal left the stored policy exactly as it was: a request refused on
        // its form reaches no resource.
        let still_there = PolicyCall::Get.send(&emulator, Some(present)).await;
        assert_eq!(
            still_there.status,
            200,
            "case {case:?}: the refusal must leave the stored policy alone, got {} {:?}",
            still_there.status,
            still_there.body
        );
        assert_eq!(
            still_there.str_at("Policy"),
            POLICY,
            "case {case:?}: the refusal must leave the stored policy alone, got {:?}",
            still_there.body
        );

        // Step two: a name outside the service's constraint names no registry anywhere,
        // whichever of the two rules it breaks.
        for (label, name, broken) in refused_names() {
            let refused = call.send(&emulator, Some(&name)).await;
            assert_refused(
                &format!("{case}/{label}"),
                &refused,
                &broken.sentence(),
            );
        }

        // And the same call with a name the service accepts is SERVED: the resolution
        // refuses the requests that name no registry, not every request.
        for (label, name) in accepted_names() {
            let label = format!("{case}/{label}");
            let emulator = Emulator::new();
            call.seed_owned(&emulator, &name).await;
            let served = call.send(&emulator, Some(&name)).await;
            assert_eq!(
                served.status,
                call.served_status(),
                "case {label:?}: expected {} from a name the service accepts, got {} {:?}",
                call.served_status(),
                served.status,
                served.body
            );
        }
    }
}
