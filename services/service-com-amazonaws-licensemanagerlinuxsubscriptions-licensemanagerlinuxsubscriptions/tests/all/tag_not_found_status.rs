// SPDX-License-Identifier: CC0-1.0

//! The HTTP STATUS this service's tag APIs (`/tags/{resourceArn}`) answer with, over the
//! real wire path: every call goes through the router's `dispatch`, so the status line a
//! client branches on is observed exactly as that client sees it.
//!
//! The rule under test: on this service's tagging surface a refusal stating that the
//! resource `resourceArn` addresses does not exist is reported with HTTP 404 and the code
//! `NotFoundException`, while a refusal stating the REQUEST is wrong (a `resourceArn`
//! outside the modelled grammar, a tag set past the modelled maximum) keeps HTTP 400 —
//! each refusal of that taxonomy carries the status that goes with its own code, rather
//! than one status for every client error. The model cannot decide this: the
//! `ResourceNotFoundException` it lists for these operations carries no `@httpError`
//! (`smithy_query(query="operation",operation="TagResource")` reports
//! `httpErrorDefaulted: true`), so its 400 is only Smithy's default for a client error.
//!
//! The tables span the class the rule covers rather than the one recorded request:
//!
//! * every tag operation of the service — `TagResource` (`PUT`), `ListTagsForResource`
//!   (`GET`) and `UntagResource` (`DELETE`);
//! * seven classes of well-formed ARN that address no resource of this account (never
//!   registered, deregistered, another region, another resource kind, another service,
//!   another partition, a one-character resource id), with a LIVE
//!   provider in the store throughout, so the refusal is about the ARN and not about an
//!   empty store;
//! * the payload the call carries (how many tag pairs a write names, how many keys a
//!   removal names), which the status must not depend on;
//! * a repeat of the same call, since the refusal is idempotent.
//!
//! An ARN in ANOTHER ACCOUNT is not a row of this rule: the surface decides authorization
//! from the account field before it resolves anything, so such an ARN is denied rather than
//! reported absent — `tag_plane_authorization.rs` owns that stage.
//!
//! `error_taxonomy.rs` owns the codes and texts of this taxonomy; what this suite adds is
//! the STATUS each of them carries, and the boundary of the rule: the non-tag surface's
//! own modelled `ResourceNotFoundException` keeps the model's 400.

use serde_json::{json, Map, Value};

use crate::harness::{
    deregister, get_provider, harness, list_tags, provider_arn_for, seed_provider, tag, untag,
    Reply, REGION, SIGNING_NAME,
};

/// A Secrets Manager ARN satisfying the modelled `SecretArn` pattern.
const SECRET_ARN: &str = "arn:aws:secretsmanager:us-west-2:000000000000:secret:rhel-token";

/// The refusal the tag APIs state for a `resourceArn` that addresses no resource of this
/// account: HTTP 404, because it reports an ABSENT resource.
const NOT_FOUND: (u16, &str, &str) = (404, "NotFoundException", "Resource was not found");

/// The refusal the same surface states for a `resourceArn` outside the modelled grammar:
/// the request itself is wrong, so HTTP 400.
const BAD_ARN: (u16, &str, &str) = (
    400,
    "BadRequestException",
    "Invalid input resource arn: bad syntax",
);

/// The refusal the same surface states for a tag set past the modelled `Tags` maximum
/// (`@length 0..50`): also a wrong request, so HTTP 400.
const TAG_LIMIT: (u16, &str, &str) = (
    400,
    "BadRequestException",
    "The resultant tag set must not have more than 50 user tags.",
);

/// The not-found refusal of the NON-tag surface: `GetRegisteredSubscriptionProvider` and
/// `DeregisterSubscriptionProvider` answer their own modelled `ResourceNotFoundException`,
/// which keeps the status the recorded service answers it with (400). The rule above is
/// about the tagging surface's taxonomy, and this row is what marks its edge.
const PROVIDER_NOT_FOUND: (u16, &str, &str) = (
    400,
    "ResourceNotFoundException",
    "Registered Subscription Provider not found",
);

/// Assert `reply` is the HTTP status, wire error code and message of `refusal`.
fn assert_refused(reply: &Reply, case: &str, refusal: (u16, &str, &str)) {
    let (status, code, message) = refusal;
    reply.error(case, status, code);
    assert_eq!(
        reply.message(),
        message,
        "case {case:?}: expected message {message:?}, got {:?}",
        reply.message()
    );
}

/// `count` distinct pairs, as the `tags` map of a `TagResource` body.
fn tag_map(count: usize) -> Value {
    let pairs: Map<String, Value> = (0..count)
        .map(|index| (format!("key-{index:02}"), json!(format!("value-{index:02}"))))
        .collect();
    Value::Object(pairs)
}

/// `count` distinct keys, as a `UntagResource` `tagKeys` selection.
fn tag_keys(count: usize) -> Vec<String> {
    (0..count).map(|index| format!("key-{index:02}")).collect()
}

/// The ARN classes that are inside the modelled `resourceArn` grammar, name THIS account
/// and address no resource of it, given the `live` provider's ARN and one `gone` ARN whose
/// record was deregistered.
///
/// Every row carries the caller's own account: an ARN naming another account is denied
/// before it is resolved (`tag_plane_authorization.rs`), so it could not report absence.
fn unregistered(live: &str, gone: &str) -> Vec<(String, String)> {
    vec![
        (
            "an id nothing was ever registered under".to_string(),
            provider_arn_for("55555555-5555-4555-8555-555555555555"),
        ),
        ("a deregistered provider".to_string(), gone.to_string()),
        (
            "the live provider's id in another region".to_string(),
            live.replace(REGION, "eu-west-1"),
        ),
        (
            "another resource kind".to_string(),
            live.replace("subscription-provider/", "linux-subscription/"),
        ),
        (
            "another service's namespace".to_string(),
            live.replace(&format!(":{SIGNING_NAME}:"), ":license-manager:"),
        ),
        (
            "another partition".to_string(),
            live.replace("arn:aws:", "arn:aws-us-gov:"),
        ),
        (
            "a one-character resource id".to_string(),
            provider_arn_for("a"),
        ),
    ]
}

/// The rule: on every tag operation, an ARN that addresses no resource is reported with
/// HTTP 404 `NotFoundException` — whatever the ARN class, whatever the payload, and on a
/// repeat of the same call.
#[tokio::test]
async fn an_unregistered_resource_arn_is_reported_absent_with_http_404() {
    let h = harness();
    // One provider stays registered throughout: the refusals below must come from the ARN
    // addressing nothing, not from a store that holds nothing at all.
    let live = seed_provider(
        &h,
        "00000000-0000-4000-8000-00000000000a",
        SECRET_ARN,
        &[("env", "prod")],
    );
    let gone = seed_provider(&h, "00000000-0000-4000-8000-00000000000b", SECRET_ARN, &[]);
    deregister(&h, &gone)
        .await
        .ok("deregister the second provider");

    for (label, arn) in unregistered(&live, &gone) {
        for attempt in 0..2 {
            // A write: the status must not depend on how many pairs it names, so the row
            // walks one, two and the modelled maximum of 50. (A write naming NO pair is
            // refused as invalid input before the ARN is resolved, so it is not a row of
            // this rule — `tag_plane_authorization.rs` pins that refusal.)
            for pairs in [1_usize, 2, 50] {
                let case = format!("TagResource on {label} with {pairs} pair(s), attempt {attempt}");
                assert_refused(&tag(&h, &arn, &tag_map(pairs)).await, &case, NOT_FOUND);
            }
            // A removal: same, over the number of keys it names (the modelled `tagKeys`
            // maximum is 50).
            for keys in [1_usize, 2, 50] {
                let named = tag_keys(keys);
                let selection: Vec<&str> = named.iter().map(String::as_str).collect();
                let case =
                    format!("UntagResource on {label} naming {keys} key(s), attempt {attempt}");
                assert_refused(&untag(&h, &arn, &selection).await, &case, NOT_FOUND);
            }
            // And the read.
            let case = format!("ListTagsForResource on {label}, attempt {attempt}");
            assert_refused(&list_tags(&h, &arn).await, &case, NOT_FOUND);
        }
    }

    // The same three operations on a resource that DOES exist still succeed: the 404 is
    // the answer to an absent resource, not to the operation.
    tag(&h, &live, &tag_map(1))
        .await
        .ok("TagResource on the live provider");
    let listed = list_tags(&h, &live).await;
    let tags = listed
        .ok("ListTagsForResource on the live provider")
        .get("tags")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("no tags in {:?}", listed.body))
        .clone();
    assert_eq!(
        tags.get("key-00").and_then(Value::as_str),
        Some("value-00"),
        "the live provider must report the pair the accepted write merged in, got {tags:?}"
    );
    untag(&h, &live, &["key-00"])
        .await
        .ok("UntagResource on the live provider");
}

/// The other half of the rule: a refusal about the REQUEST keeps HTTP 400, so a client
/// branching on the status can tell "absent" from "malformed", and the non-tag surface's
/// modelled not-found keeps the status the model and the recording give it.
#[tokio::test]
async fn a_bad_request_on_the_tag_apis_keeps_http_400() {
    let h = harness();
    let live = seed_provider(
        &h,
        "00000000-0000-4000-8000-00000000000c",
        SECRET_ARN,
        &[("env", "prod")],
    );

    // Stage 1 — FORMAT. The rows span the ways a value misses the modelled grammar:
    // nothing ARN-shaped, too few segments, no resource id, a character outside the
    // segment class.
    let unparseable = [
        "not-an-arn-at-all",
        "arn:aws:license-manager-linux-subscriptions:us-west-2:000000000000:subscription-provider",
        "arn:aws:license-manager-linux-subscriptions:us-west-2:000000000000",
        "arn:aws:license-manager-linux-subscriptions:us-west-2:000000000000:SUBSCRIPTION/abc",
    ];
    for arn in unparseable {
        let case = format!("TagResource with {arn}");
        assert_refused(&tag(&h, arn, &tag_map(1)).await, &case, BAD_ARN);
        let case = format!("ListTagsForResource with {arn}");
        assert_refused(&list_tags(&h, arn).await, &case, BAD_ARN);
        let case = format!("UntagResource with {arn}");
        assert_refused(&untag(&h, arn, &["env"]).await, &case, BAD_ARN);
    }

    // A tag set one past the modelled maximum is a wrong request on BOTH a resource that
    // exists and one that does not: the size of the request is settled before the ARN is
    // resolved, so this row also fixes the ORDER of the two refusals.
    let over_limit = tag_map(51);
    for (label, arn) in [
        ("the live provider", live.clone()),
        (
            "an unregistered provider",
            provider_arn_for("77777777-7777-4777-8777-777777777777"),
        ),
    ] {
        let case = format!("TagResource with 51 pairs on {label}");
        assert_refused(&tag(&h, &arn, &over_limit).await, &case, TAG_LIMIT);
    }

    // The edge of the rule: the subscription-provider operations answer their OWN
    // modelled `ResourceNotFoundException`, whose status the recorded service leaves at
    // the model's 400 — the tagging surface's 404 belongs to the tagging surface's code.
    let gone = seed_provider(&h, "00000000-0000-4000-8000-00000000000d", SECRET_ARN, &[]);
    deregister(&h, &gone).await.ok("deregister that provider");
    for (label, arn) in unregistered(&live, &gone) {
        let case = format!("GetRegisteredSubscriptionProvider on {label}");
        assert_refused(&get_provider(&h, &arn).await, &case, PROVIDER_NOT_FOUND);
        let case = format!("DeregisterSubscriptionProvider on {label}");
        assert_refused(&deregister(&h, &arn).await, &case, PROVIDER_NOT_FOUND);
    }
}
