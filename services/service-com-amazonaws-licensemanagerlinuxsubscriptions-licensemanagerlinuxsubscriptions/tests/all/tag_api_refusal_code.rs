// SPDX-License-Identifier: CC0-1.0

//! The CODE this service's tagging surface refuses with, over the real wire path: every
//! call goes through the router's `dispatch`, so what is asserted is the status line, the
//! wire error code and the message an SDK client branches on.
//!
//! The rule: a refusal of a request ADDRESSED at the tagging surface
//! (`/tags/{resourceArn}`) — the label it addresses the resource with, the size of the tag
//! selection it carries, and a required REQUEST PARAMETER it does not carry — is stated in
//! that surface's own taxonomy, HTTP 400 `BadRequestException` with the sentence stating
//! that class, and never with a code the called operation does not model
//! (`smithy_query(query="operation",operation="UntagResource")` lists
//! `InternalServerException` and `ResourceNotFoundException` and no `ValidationException`
//! at all). A request on a route this surface serves is never answered as unimplemented
//! for a parameter it left out.
//!
//! The table spans the classes of refusal the surface states itself, which is what the
//! rule generalises over:
//!
//! * a required REQUEST PARAMETER the request does not carry (`UntagResource`'s `tagKeys`
//!   is `@required` and `@httpQuery`), reported by naming the parameter to add — the
//!   class the REST router itself refuses, by not matching the route at all;
//! * a selection past the modelled tag maximum, reported with the sentence stating that
//!   limit, through the removal's key LIST and the write's tag MAP alike;
//! * a `resourceArn` outside the label's modelled grammar, reported with the sentence
//!   stating that;
//!
//! each against a resource that exists and one that does not, since such a refusal is
//! settled before the ARN is resolved — and each with the stored tag set checked
//! afterwards, because a refused request changes nothing.
//!
//! The BOUNDARY of the rule is a row too: a required member of a request BODY
//! (`TagResource`'s `tags`) is not a request parameter and is not one of the refusals this
//! surface states, so it keeps the service's modelled `ValidationException` envelope —
//! which `TagResource` declares
//! (`smithy_query(query="operation",operation="TagResource")`).
//!
//! `tag_selection_limit.rs` owns the SIZE rule and `error_taxonomy.rs` the two ARN stages;
//! what this suite adds is the absent-`tagKeys` class — the one row those suites skip —
//! and where the surface's taxonomy stops.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::harness::{
    harness, list_tags, percent_encode, provider_arn_for, seed_provider, send, tag, untag,
    Harness, Reply, REGION,
};

/// The code the refusals this surface states itself name, with the HTTP status that goes
/// with it.
const SURFACE_REFUSAL: (u16, &str) = (400, "BadRequestException");

/// The code the service's modelled constraint envelope names, with its status: what a
/// class the surface states no refusal of its own for keeps.
const MODELLED_REFUSAL: (u16, &str) = (400, "ValidationException");

/// The code a refusal of `UntagResource` must never name: that operation models no
/// `ValidationException`, so a client branching on its modelled errors cannot see it.
const UNDECLARED_CODE: &str = "ValidationException";

/// The sentence stating a required request parameter the request did not carry: the fixed
/// text, over the list of parameter names to add — here the one `@httpQuery` parameter
/// `UntagResource` requires.
const MISSING_TAG_KEYS: &str = "Missing required request parameters: [tagKeys]";

/// The sentence stating a required BODY member the payload did not carry: the service's
/// modelled constraint envelope, which names the member and what it failed.
const MISSING_TAGS: &str = "1 validation error detected: Value null at 'tags' failed to \
                            satisfy constraint: Member must not be null";

/// The sentence stating a tag selection past the modelled maximum (`@length 0..50` on
/// every member a selection travels in).
const TAG_LIMIT: &str = "The resultant tag set must not have more than 50 user tags.";

/// The sentence stating a `resourceArn` outside the label's modelled grammar.
const BAD_ARN: &str = "Invalid input resource arn: bad syntax";

/// The tag set every seeded resource carries, so a refusal can be checked to have left it
/// alone.
const STORED: &[(&str, &str)] = &[("owner", "platform"), ("env", "test")];

/// Which boundary refusal a row drives the surface into.
#[derive(Debug, Clone, Copy)]
enum Refusal {
    /// `DELETE /tags/{resourceArn}` carrying no `tagKeys` query parameter at all.
    NoTagKeysParameter,
    /// `PUT /tags/{resourceArn}` whose payload carries no `tags` member: the BOUNDARY of
    /// the rule — a required member of a request BODY is not a request parameter, so this
    /// one keeps the service's modelled constraint envelope, which `TagResource` declares.
    NoTagsMember,
    /// A removal naming more keys than the modelled maximum.
    RemovalPastTheLimit,
    /// A write whose tag map is past the modelled maximum.
    WritePastTheLimit,
}

impl Refusal {
    /// The status, code and sentence this class is stated with: the surface's own taxonomy
    /// for the refusals it states, and the service's modelled envelope for the class it
    /// does not.
    fn expected(self) -> (u16, &'static str, &'static str) {
        let (status, code) = match self {
            Self::NoTagsMember => MODELLED_REFUSAL,
            _ => SURFACE_REFUSAL,
        };
        let message = match self {
            Self::NoTagKeysParameter => MISSING_TAG_KEYS,
            Self::NoTagsMember => MISSING_TAGS,
            Self::RemovalPastTheLimit | Self::WritePastTheLimit => TAG_LIMIT,
        };
        (status, code, message)
    }

    /// The operation the row reaches the surface through.
    fn operation(self) -> &'static str {
        match self {
            Self::NoTagKeysParameter | Self::RemovalPastTheLimit => "UntagResource",
            Self::NoTagsMember | Self::WritePastTheLimit => "TagResource",
        }
    }

    /// Send the row's request against `arn`.
    async fn send(self, h: &Harness, arn: &str) -> Reply {
        match self {
            Self::NoTagKeysParameter => untag(h, arn, &[]).await,
            // The harness's `tag` always wraps a map in the `tags` member, which is the
            // member this row leaves out — so the payload is built here.
            Self::NoTagsMember => {
                send(
                    h,
                    "PUT",
                    REGION,
                    &format!("/tags/{}", percent_encode(arn)),
                    Some(&json!({})),
                )
                .await
            }
            Self::RemovalPastTheLimit => {
                let keys: Vec<String> = (0..51).map(|index| format!("k-{index:03}")).collect();
                let borrowed: Vec<&str> = keys.iter().map(String::as_str).collect();
                untag(h, arn, &borrowed).await
            }
            Self::WritePastTheLimit => {
                let map: serde_json::Map<String, Value> = (0..51)
                    .map(|index| (format!("k-{index:03}"), json!(format!("v-{index:03}"))))
                    .collect();
                tag(h, arn, &Value::Object(map)).await
            }
        }
    }
}

/// The tag set `arn` reports, as a sorted map.
async fn reported_tags(h: &Harness, arn: &str, case: &str) -> BTreeMap<String, String> {
    let reply = list_tags(h, arn).await;
    let body = reply.ok(case);
    body.get("tags")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("case {case:?}: no tags map in {body:?}"))
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: tag {key:?} is not a string"))
                    .to_string(),
            )
        })
        .collect()
}

/// Assert `reply` is the refusal `expected` states — status, wire code and sentence —
/// and that a refusal of `operation` never names a code that operation does not model.
fn assert_refusal(
    reply: &Reply,
    case: &str,
    operation: &str,
    expected: (u16, &str, &str),
) {
    let (status, code, message) = expected;
    reply.error(case, status, code);
    assert_eq!(
        reply.message(),
        message,
        "case {case:?}: expected {message:?}, got {:?}",
        reply.message()
    );
    let answered = reply.error_type.clone().unwrap_or_default();
    assert!(
        operation != "UntagResource" || !answered.contains(UNDECLARED_CODE),
        "case {case:?}: {operation} models no {UNDECLARED_CODE}, got {answered:?}"
    );
}

/// The rule, over every class of refusal the surface states itself plus the boundary row
/// that keeps the modelled envelope, against a resource that exists and one that does
/// not — and with the stored tag set unchanged by each refusal.
#[tokio::test]
async fn a_refused_tag_request_is_stated_in_the_surfaces_own_taxonomy() {
    let h = harness();
    let live = seed_provider(
        &h,
        "00000000-0000-4000-8000-00000000a001",
        "arn:aws:secretsmanager:us-west-2:000000000000:secret:rhel-token-code",
        STORED,
    );
    // Inside the label's modelled grammar, but addressing no resource of this account: a
    // refusal of the request itself is settled before the ARN is resolved, so the answer
    // is the same.
    let unregistered = provider_arn_for("00000000-0000-4000-8000-00000000b002");
    let stored: BTreeMap<String, String> = STORED
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();

    for refusal in [
        Refusal::NoTagKeysParameter,
        Refusal::NoTagsMember,
        Refusal::RemovalPastTheLimit,
        Refusal::WritePastTheLimit,
    ] {
        for (label, arn) in [
            ("a registered provider", live.as_str()),
            ("an unregistered provider", unregistered.as_str()),
        ] {
            let case = format!("{refusal:?} through {} on {label}", refusal.operation());
            let reply = refusal.send(&h, arn).await;
            assert_refusal(&reply, &case, refusal.operation(), refusal.expected());

            let left = reported_tags(&h, &live, &case).await;
            assert_eq!(
                left, stored,
                "case {case:?}: a refused request must leave the stored set as it was, \
                 got {left:?}"
            );
        }
    }
}

/// The FORMAT stage of the same rule, and the ORDER the two refusals of one request
/// stand in: the required request parameter has to be there for the removal to be served
/// at all, so a request that leaves it out is answered by naming it — and once it is
/// carried, a `resourceArn` outside the modelled grammar is the surface's refusal of the
/// label.
#[tokio::test]
async fn a_bad_resource_arn_is_the_surfaces_refusal_once_the_parameter_is_carried() {
    let h = harness();
    // The ways a value can miss the modelled grammar: nothing ARN-shaped, too few
    // segments, no resource id, a character outside the segment class.
    let unparseable = [
        "not-an-arn-at-all",
        "arn:aws:license-manager-linux-subscriptions:us-west-2:000000000000:subscription-provider",
        "arn:aws:license-manager-linux-subscriptions:us-west-2:000000000000",
        "arn:aws:license-manager-linux-subscriptions:us-west-2:000000000000:SUBSCRIPTION/abc",
    ];
    let (status, code) = SURFACE_REFUSAL;
    for arn in unparseable {
        // (what the request carries, the refusal that states it)
        for (label, keys, expected) in [
            ("no tagKeys", &[][..], (status, code, MISSING_TAG_KEYS)),
            ("one tagKey", &["owner"][..], (status, code, BAD_ARN)),
        ] {
            let case = format!("UntagResource with {arn} and {label}");
            let reply = untag(&h, arn, keys).await;
            assert_refusal(&reply, &case, "UntagResource", expected);
        }
    }
}

/// The other side of the rule: a removal that DOES carry the parameter is served, so the
/// refusal above is about the absent parameter rather than about the operation.
#[tokio::test]
async fn a_removal_carrying_the_required_parameter_is_served() {
    let h = harness();
    let arn = seed_provider(
        &h,
        "00000000-0000-4000-8000-00000000c003",
        "arn:aws:secretsmanager:us-west-2:000000000000:secret:rhel-token-served",
        STORED,
    );
    let case = "UntagResource naming one stored key";
    untag(&h, &arn, &["owner"]).await.ok(case);
    let left = reported_tags(&h, &arn, case).await;
    assert_eq!(
        left,
        BTreeMap::from([("env".to_string(), "test".to_string())]),
        "case {case:?}: expected the key the request did not name to remain, got {left:?}"
    );
}

/// And the bound of the rule: a request on a route this service's model does not publish
/// is NOT one of the surface's refusals, however much it looks like one — the tag
/// surface's answer is for the tag surface's routes.
#[tokio::test]
async fn a_request_outside_the_tag_routes_is_not_the_surfaces_refusal() {
    let h = harness();
    let (_, code) = SURFACE_REFUSAL;
    // Rows differ along what makes the route unpublished: the method, the first path
    // segment, and the number of segments the tag URI has.
    let rows = [
        ("a DELETE on an unmodelled path", "DELETE", "/subscription/Nope"),
        ("a DELETE on the tag prefix alone", "DELETE", "/tags"),
        ("a DELETE one segment too deep", "DELETE", "/tags/one/two"),
        // An interior empty segment is a third segment to the router, so this is not the
        // tag route either.
        ("a DELETE with an empty segment", "DELETE", "/tags//abc"),
        ("an unmodelled method on the tag route", "PATCH", "/tags/abc"),
    ];
    for (case, method, path) in rows {
        let reply = send(&h, method, REGION, path, None).await;
        let answered = reply.error_type.clone().unwrap_or_default();
        assert!(
            !answered.ends_with(code),
            "case {case:?}: a route this model does not publish must not answer {code}, \
             got {answered:?} with {:?}",
            reply.body
        );
    }
}
