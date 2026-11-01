// SPDX-License-Identifier: CC0-1.0

//! What this service's tagging surface (`/tags/{resourceArn}`) settles BEFORE it resolves
//! the ARN it was addressed with, over the real wire path: every call goes through the
//! router's `dispatch`, so the status line, the wire error code and the message a client
//! branches on are observed exactly as that client sees them.
//!
//! Two rules, both about the ORDER a tag request is answered in:
//!
//! 1. **Authorization is decided from the `resourceArn`'s account field alone.** An ARN whose
//!    account is not the one the request is served for names a resource no document of this
//!    account can grant anything over, so the request is refused HTTP 403 with the IAM denial
//!    naming the calling principal — on every operation of the surface, and without the
//!    store being consulted, so the answer cannot depend on whether this account happens to
//!    hold a resource under the same id. A client has to be able to tell "you may not touch
//!    this" from "this does not exist": a 403 is permanent, while a 404 invites a
//!    create-then-retry or is swallowed as an idempotent miss.
//! 2. **`TagResource` validates the tag map it was given before resolving the resource.** A
//!    write that assigns no pair is refused HTTP 400 with the tagging surface's own
//!    `BadRequestException`, whether or not the named ARN addresses a registration: a wrong
//!    request is the caller's to fix, and must not be misreported as missing state.
//!
//! Both are recorded behaviour rather than model text: the Smithy model binds these
//! operations only `ResourceNotFoundException`, `ValidationException` and
//! `InternalServerException` and no authorization error at all
//! (`smithy_query(query="operation",operation="TagResource,ListTagsForResource,UntagResource")`),
//! and `TagResourceRequest$tags` is `@required` with `@length(min: 0, max: 50)`, so an empty
//! map is model-legal.
//!
//! The tables span what the rules generalise over rather than one request: every operation of
//! the surface (`PUT`, `GET`, `DELETE`), several foreign account ids, several ARN shapes
//! carrying one (the live resource's own ARN with only the account changed, an id nothing was
//! registered under, another Region, another resource kind, another partition, another
//! service's namespace), the size of the payload the call carries, and a repeat of each call
//! since a refusal is idempotent. The BOUNDARY of each rule is a row too: the same ARN shapes
//! in the caller's OWN account keep the surface's 404, the live resource is still served, and
//! a write naming a single pair is still accepted.
//!
//! `tag_not_found_status.rs` owns the surface's absent-resource status and
//! `tag_selection_limit.rs` its size rule; what this suite adds is the two stages that sit
//! AHEAD of them.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::harness::{
    harness, list_tags, provider_arn_for, reported_tags_of, seed_provider, stored_provider_count,
    tag, untag, Harness, Reply, ACCOUNT, REGION, SIGNING_NAME,
};

/// A Secrets Manager ARN satisfying the modelled `SecretArn` pattern.
const SECRET_ARN: &str = "arn:aws:secretsmanager:us-west-2:000000000000:secret:rhel-token-auth";

/// The tag set the live resource carries throughout, so every refusal below can be shown to
/// have left it alone.
const STORED: &[(&str, &str)] = &[("env", "prod"), ("owner", "platform")];

/// The HTTP status and wire error code the surface denies an unauthorized request with.
const DENIED: (u16, &str) = (403, "AccessDeniedException");

/// The status, code and sentence the surface refuses an ARN addressing no resource of this
/// account with — the answer the rows inside the caller's own account keep.
const NOT_FOUND: (u16, &str, &str) = (404, "NotFoundException", "Resource was not found");

/// The status, code and sentence `TagResource` refuses a tag map naming no pair with: the
/// tagging surface's own code, the one it states every wrong request with. Fixed text: it
/// has no variable part.
const EMPTY_TAGS: (u16, &str, &str) = (
    400,
    "BadRequestException",
    "Invalid request input, expected non-empty tags map",
);

/// Account ids other than the one this emulator serves. Twelve decimal digits, which is what
/// an ARN's account field holds; the values differ along their digits only, because the rule
/// is about the field not matching rather than about any particular account.
const FOREIGN_ACCOUNTS: &[&str] = &["123456789012", "999999999999", "111122223333"];

/// The IAM principal every request to this emulator speaks for — read from the shared
/// definition of that identity, so this suite states the principal rather than a literal ARN.
fn caller_principal() -> String {
    service_common::caller_principal::ambient_user_arn()
}

/// The sentence the denial carries: AWS's IAM denial template for a resource the caller may
/// not reach at all — the fixed text `User: `, the calling principal, and the fixed text
/// ` is not authorized to access this resource`.
fn denial_message() -> String {
    format!(
        "User: {} is not authorized to access this resource",
        caller_principal()
    )
}

/// `count` distinct pairs, as the `tags` map of a `TagResource` body.
fn tag_map(count: usize) -> Value {
    let pairs: Map<String, Value> = (0..count)
        .map(|index| (format!("key-{index:02}"), json!(format!("value-{index:02}"))))
        .collect();
    Value::Object(pairs)
}

/// `count` distinct keys, as an `UntagResource` `tagKeys` selection.
fn tag_keys(count: usize) -> Vec<String> {
    (0..count).map(|index| format!("key-{index:02}")).collect()
}

/// The ARN shapes a row is built in: `live`'s own ARN with the account replaced, and the
/// same replacement on an id nothing was registered under, on another Region, on another
/// resource kind, on another partition and on another service's namespace — so the verdict is
/// shown to come from the account field and not from the rest of the ARN.
fn shapes(live: &str, account: &str) -> Vec<(String, String)> {
    let in_account = |arn: &str| arn.replace(ACCOUNT, account);
    vec![
        (
            "the live resource's own ARN".to_string(),
            in_account(live),
        ),
        (
            "an id nothing was registered under".to_string(),
            in_account(&provider_arn_for("44444444-4444-4444-8444-444444444444")),
        ),
        (
            "another region".to_string(),
            in_account(&live.replace(REGION, "eu-west-1")),
        ),
        (
            "another resource kind".to_string(),
            in_account(&live.replace("subscription-provider/", "linux-subscription/")),
        ),
        (
            "another partition".to_string(),
            in_account(&live.replace("arn:aws:", "arn:aws-us-gov:")),
        ),
        (
            "another service's namespace".to_string(),
            in_account(&live.replace(&format!(":{SIGNING_NAME}:"), ":license-manager:")),
        ),
    ]
}

/// Every call of the surface against `arn`, as `(label, reply)` rows: a write naming one pair
/// and one naming the modelled maximum, a removal naming one key and one naming that maximum,
/// and the read — so the answer is shown not to depend on the payload the call carries.
async fn every_tag_call(h: &Harness, arn: &str) -> Vec<(String, Reply)> {
    let mut answered = Vec::new();
    for pairs in [1_usize, 50] {
        answered.push((
            format!("TagResource with {pairs} pair(s)"),
            tag(h, arn, &tag_map(pairs)).await,
        ));
    }
    for keys in [1_usize, 50] {
        let named = tag_keys(keys);
        let selection: Vec<&str> = named.iter().map(String::as_str).collect();
        answered.push((
            format!("UntagResource naming {keys} key(s)"),
            untag(h, arn, &selection).await,
        ));
    }
    answered.push(("ListTagsForResource".to_string(), list_tags(h, arn).await));
    answered
}

/// Assert `reply` is the refusal `expected` states: its HTTP status, its wire error code and
/// its message.
fn assert_refused(reply: &Reply, case: &str, expected: (u16, &str, &str)) {
    let (status, code, message) = expected;
    reply.error(case, status, code);
    assert_eq!(
        reply.message(),
        message,
        "case {case:?}: expected message {message:?}, got {:?}",
        reply.message()
    );
}

/// The tag set the live resource is expected to carry: exactly what it was seeded with.
fn stored_set() -> BTreeMap<String, String> {
    STORED
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// Rule 1: every operation of the surface denies an ARN in another account, with the IAM
/// denial naming the calling principal and nothing of the request — whatever the ARN's shape,
/// whatever the payload, and on a repeat.
#[tokio::test]
async fn a_resource_arn_in_another_account_is_denied_by_every_tag_operation() {
    let h = harness();
    // One registered resource throughout: a denial must come from the ARN's account rather
    // than from a store that holds nothing at all.
    let live = seed_provider(&h, "00000000-0000-4000-8000-0000000000a1", SECRET_ARN, STORED);
    let registrations = stored_provider_count(&h);
    let (status, code) = DENIED;
    let expected = denial_message();

    for account in FOREIGN_ACCOUNTS {
        for (shape, arn) in shapes(&live, account) {
            for attempt in 0..2 {
                for (call, reply) in every_tag_call(&h, &arn).await {
                    let case = format!("{call} on {shape} in {account}, attempt {attempt}");
                    reply.error(&case, status, code);
                    assert_eq!(
                        reply.message(),
                        expected,
                        "case {case:?}: expected the IAM denial {expected:?}, got {:?}",
                        reply.message()
                    );
                    // The denial names the caller's own principal and nothing it was asked
                    // about: not the ARN, not the account in it, and none of the tag
                    // selection (every member one travels in is `@sensitive`).
                    for leaked in [arn.as_str(), account, "key-00"] {
                        assert!(
                            !reply.message().contains(leaked),
                            "case {case:?}: the denial must carry nothing from the request, \
                             got {:?} carrying {leaked:?}",
                            reply.message()
                        );
                    }
                }
            }
        }
    }

    // No denied call reached the store: the live resource still carries exactly its seeded
    // set, and nothing was registered or removed.
    let left = reported_tags_of(&h, &live, "the live resource after the denials").await;
    assert_eq!(
        left,
        stored_set(),
        "expected the live resource's seeded set {:?}, got {left:?}",
        stored_set()
    );
    assert_eq!(
        stored_provider_count(&h),
        registrations,
        "expected the store untouched at {registrations} record(s), got {}",
        stored_provider_count(&h)
    );
}

/// The boundary of rule 1: the gate is keyed on the ACCOUNT field, so the same ARN shapes in
/// the caller's own account keep the surface's absent-resource refusal, and the resource that
/// does exist is still served by all three operations.
#[tokio::test]
async fn the_same_arn_shapes_in_the_callers_account_keep_the_surfaces_not_found() {
    let h = harness();
    let live = seed_provider(&h, "00000000-0000-4000-8000-0000000000a2", SECRET_ARN, STORED);

    // `ACCOUNT` is the account this emulator serves, so these rows are the shapes above with
    // the account left as it is: every one of them addresses no resource, and none is denied.
    for (shape, arn) in shapes(&live, ACCOUNT) {
        if arn == live {
            continue;
        }
        for (call, reply) in every_tag_call(&h, &arn).await {
            let case = format!("{call} on {shape} in the caller's account");
            assert_refused(&reply, &case, NOT_FOUND);
        }
    }

    // And the resource that exists is served: the write merges, the read reports and the
    // removal removes, so the two refusals above are about the ARN rather than the surface.
    tag(&h, &live, &tag_map(1))
        .await
        .ok("TagResource on the live resource");
    let after_write = reported_tags_of(&h, &live, "the live resource after the write").await;
    assert_eq!(
        after_write.get("key-00").map(String::as_str),
        Some("value-00"),
        "expected the pair the accepted write merged in, got {after_write:?}"
    );
    untag(&h, &live, &["key-00"])
        .await
        .ok("UntagResource on the live resource");
    let after_removal = reported_tags_of(&h, &live, "the live resource after the removal").await;
    assert_eq!(
        after_removal,
        stored_set(),
        "expected the seeded set back after the removal, got {after_removal:?}"
    );
}

/// Rule 2: a write assigning no pair is refused as invalid input, ahead of the resolution of
/// the ARN it names — so the same answer on a resource that exists and on one that does not —
/// and it stores nothing. A foreign-account ARN is still denied first: authorization is
/// settled from the ARN before the body is read at all.
#[tokio::test]
async fn a_write_with_an_empty_tags_map_is_refused_before_the_resource_is_resolved() {
    let h = harness();
    let live = seed_provider(&h, "00000000-0000-4000-8000-0000000000a3", SECRET_ARN, STORED);
    let unregistered = provider_arn_for("55555555-5555-4555-8555-555555555555");
    let registrations = stored_provider_count(&h);
    let empty = json!({});

    for (label, arn) in [
        ("a resource this account holds", live.as_str()),
        ("an ARN addressing no resource", unregistered.as_str()),
    ] {
        for attempt in 0..2 {
            let case = format!("TagResource with an empty tags map on {label}, attempt {attempt}");
            let reply = tag(&h, arn, &empty).await;
            assert_refused(&reply, &case, EMPTY_TAGS);
            assert!(
                !reply.message().contains(arn),
                "case {case:?}: the refusal states the condition and nothing of the request, \
                 got {:?}",
                reply.message()
            );
        }
    }

    // Authorization is the first stage, so a foreign-account ARN is denied rather than
    // answered about the body it carries.
    let (status, code) = DENIED;
    let foreign = live.replace(ACCOUNT, FOREIGN_ACCOUNTS[0]);
    let case = "TagResource with an empty tags map on an ARN in another account";
    let reply = tag(&h, &foreign, &empty).await;
    reply.error(case, status, code);
    assert_eq!(
        reply.message(),
        denial_message(),
        "case {case:?}: expected the IAM denial, got {:?}",
        reply.message()
    );

    // The other side of the rule: naming a single pair is accepted, so what is refused is
    // the EMPTY map and not the operation.
    tag(&h, &live, &tag_map(1))
        .await
        .ok("TagResource naming one pair");
    let mut want = stored_set();
    want.insert("key-00".to_string(), "value-00".to_string());
    let left = reported_tags_of(&h, &live, "the live resource after the refusals").await;
    assert_eq!(
        left, want,
        "expected only the accepted write's pair added to the seeded set, got {left:?}"
    );
    assert_eq!(
        stored_provider_count(&h),
        registrations,
        "expected the store untouched at {registrations} record(s), got {}",
        stored_provider_count(&h)
    );
}
