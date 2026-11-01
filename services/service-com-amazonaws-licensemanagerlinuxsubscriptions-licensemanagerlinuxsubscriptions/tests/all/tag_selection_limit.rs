// SPDX-License-Identifier: CC0-1.0

//! The SIZE refusal this service's tagging surface answers, over the real wire path:
//! every call goes through the router's `dispatch`, so what is asserted is the status
//! line, the wire error code and the message an SDK client branches on.
//!
//! The rule: a request naming more tags than a resource may carry is refused by the tag
//! surface's own taxonomy — HTTP 400 `BadRequestException` with the one sentence stating
//! that limit — whichever member the selection travels in, and never with the generic
//! `ValidationException`, which `UntagResource` does not model at all
//! (`smithy_query(query="operation",operation="UntagResource")` lists only
//! `InternalServerException` and `ResourceNotFoundException`). A map names the tags a
//! write leaves and a list names the tags a removal takes away, but each selects out of
//! the one tag set whose size the limit bounds, so one refusal covers both.
//!
//! The table spans the class the rule generalises over rather than one request:
//!
//! * the member the selection travels in, with the shape and binding it has there: the
//!   `tagKeys` LIST of `UntagResource` (`@httpQuery`), the `tags` MAP of `TagResource`
//!   (body) and the `Tags` map of `RegisterSubscriptionProvider` (body of the create);
//! * the SIZE of the selection: the modelled maximum and below it are accepted, one past
//!   it and far past it are refused;
//! * whether the addressed resource exists, since the size of the request is settled
//!   before the ARN is resolved;
//! * what the store holds afterwards: a refused request writes, removes and registers
//!   nothing.
//!
//! `error_taxonomy.rs` owns the two ARN stages of this taxonomy and the over-limit `Tags`
//! map of a registration; what this suite adds is that the SIZE refusal is the surface's
//! own for every member a tag selection travels in — the `tagKeys` list included.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::harness::{
    harness, provider_arn_for, register, reported_tags_of, seed_provider, stored_provider_count,
    tag, untag, Harness, Reply, ACCOUNT, RED_HAT,
};

/// The most tags one request may select: the `@length` maximum the model gives every
/// member a tag selection travels in (`smithy_query(query="operation",
/// operation="UntagResource")` reports `tagKeys` as `length {min: 0, max: 50}`, and
/// `TagResource`'s `tags` and `RegisterSubscriptionProvider`'s `Tags` carry the same
/// bound), which is also the largest tag set a resource may hold.
const MAX_TAG_SELECTION: usize = 50;

/// The refusal the tagging surface states for a selection past that maximum: its own
/// code, with the HTTP status that goes with it.
const TAG_LIMIT: (u16, &str, &str) = (
    400,
    "BadRequestException",
    "The resultant tag set must not have more than 50 user tags.",
);

/// The code the surface must NOT answer on this path: `UntagResource` models no
/// `ValidationException`, so a client branching on its modelled errors cannot see it.
const UNMODELLED_CODE: &str = "ValidationException";

/// The prefix of every generated key, so an assertion can check that the refusal echoes
/// none of the selection — each member one travels in is `@sensitive`.
const KEY_PREFIX: &str = "sel-key";

/// A Secrets Manager ARN satisfying the modelled `SecretArn` pattern, distinct per label:
/// a registration naming a (source, secret) pair this account already holds resolves to
/// THAT registration, so every row that means to address its own resource names its own
/// secret.
fn secret_arn(label: &str) -> String {
    format!("arn:aws:secretsmanager:us-west-2:{ACCOUNT}:secret:rhel-token-{label}")
}

/// `count` distinct generated keys.
fn keys(count: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("{KEY_PREFIX}-{index:03}"))
        .collect()
}

/// The same keys with their generated values.
fn pairs(count: usize) -> Vec<(String, String)> {
    keys(count)
        .into_iter()
        .map(|key| {
            let value = format!("value-of-{key}");
            (key, value)
        })
        .collect()
}

/// `count` distinct generated pairs, as the JSON tag map a write or a create carries.
fn tag_map(count: usize) -> Value {
    Value::Object(
        pairs(count)
            .into_iter()
            .map(|(key, value)| (key, Value::String(value)))
            .collect::<Map<String, Value>>(),
    )
}

/// The expected tag set, as a sorted map.
fn expected(count: usize) -> BTreeMap<String, String> {
    pairs(count).into_iter().collect()
}

/// Seed one provider carrying `count` generated pairs and answer its ARN.
fn seed(h: &Harness, label: &str, count: usize) -> String {
    let stored = pairs(count);
    let borrowed: Vec<(&str, &str)> = stored
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    seed_provider(
        h,
        &format!("00000000-0000-4000-8000-{:012x}", provider_index(label)),
        &secret_arn(label),
        &borrowed,
    )
}

/// A provider id derived from a row's label, so two rows never seed the same ARN.
fn provider_index(label: &str) -> usize {
    label.bytes().fold(1_usize, |acc, byte| {
        acc.wrapping_mul(31).wrapping_add(byte as usize) % 0xffff_ffff
    })
}

/// Which member of the tag surface a selection travels in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Selection {
    /// `UntagResource`'s `tagKeys`: a LIST, bound to the query string.
    RemovalKeys,
    /// `TagResource`'s `tags`: a MAP, in the request body.
    WriteMap,
    /// `RegisterSubscriptionProvider`'s `Tags`: the same map on the create.
    CreateMap,
}

impl Selection {
    /// The operation the selection reaches the service through.
    fn operation(self) -> &'static str {
        match self {
            Self::RemovalKeys => "UntagResource",
            Self::WriteMap => "TagResource",
            Self::CreateMap => "RegisterSubscriptionProvider",
        }
    }

    /// The member path the selection occupies.
    fn member(self) -> &'static str {
        match self {
            Self::RemovalKeys => "tagKeys",
            Self::WriteMap => "tags",
            Self::CreateMap => "Tags",
        }
    }

    /// Send one request carrying a selection of `count` tags. `arn` is the resource the
    /// request addresses, and `secret` the secret a create names; each operation uses the
    /// one it has.
    async fn send(self, h: &Harness, arn: &str, secret: &str, count: usize) -> Reply {
        match self {
            Self::RemovalKeys => {
                let named = keys(count);
                let borrowed: Vec<&str> = named.iter().map(String::as_str).collect();
                untag(h, arn, &borrowed).await
            }
            Self::WriteMap => tag(h, arn, &tag_map(count)).await,
            Self::CreateMap => {
                register(
                    h,
                    &json!({
                        "SubscriptionProviderSource": RED_HAT,
                        "SecretArn": secret,
                        "Tags": tag_map(count),
                    }),
                )
                .await
            }
        }
    }
}

/// Assert `reply` is the surface's size refusal, and that it states nothing of the
/// request beyond the limit.
fn assert_tag_limit(reply: &Reply, case: &str) {
    let (status, code, message) = TAG_LIMIT;
    reply.error(case, status, code);
    assert_eq!(
        reply.message(),
        message,
        "case {case:?}: expected the limit stated as {message:?}, got {:?}",
        reply.message()
    );
    let answered = reply.error_type.clone().unwrap_or_default();
    assert!(
        !answered.contains(UNMODELLED_CODE),
        "case {case:?}: the tag surface must not answer {UNMODELLED_CODE}, got {answered:?}"
    );
    assert!(
        !reply.message().contains(KEY_PREFIX),
        "case {case:?}: the refusal must echo none of the selection, got {:?}",
        reply.message()
    );
}

/// The rule: a selection past the modelled maximum is refused with the tag surface's own
/// code and sentence — in every member one travels in, however far past the bound it
/// sits, whether the addressed resource exists — and changes nothing.
#[tokio::test]
async fn a_tag_selection_past_the_modelled_maximum_is_refused_by_the_tag_surface() {
    let h = harness();
    // One registered provider carrying a tag set the refusals must leave alone, and a
    // well-formed ARN this account holds no resource for: the size of the request is
    // settled before the ARN is resolved, so both must answer the size refusal rather
    // than the surface's 404.
    let live = seed(&h, "live", 3);
    let unregistered = provider_arn_for("77777777-7777-4777-8777-777777777777");
    let registrations = stored_provider_count(&h);

    for count in [MAX_TAG_SELECTION + 1, 60, 120] {
        for selection in [
            Selection::RemovalKeys,
            Selection::WriteMap,
            Selection::CreateMap,
        ] {
            // A create addresses no existing resource, so it is driven once per size; the
            // two operations that do address one are driven against both classes of ARN.
            let targets: Vec<(&str, &str)> = match selection {
                Selection::CreateMap => vec![("a new registration", "")],
                _ => vec![
                    ("the live provider", live.as_str()),
                    ("an unregistered provider", unregistered.as_str()),
                ],
            };
            for (label, arn) in targets {
                let case = format!(
                    "{} with {count} tags in {} on {label}",
                    selection.operation(),
                    selection.member()
                );
                let secret = secret_arn(&format!("refused-{count}"));
                let reply = selection.send(&h, arn, &secret, count).await;
                assert_tag_limit(&reply, &case);

                // Nothing the refused request named was written, removed or registered.
                let left = reported_tags_of(&h, &live, &case).await;
                assert_eq!(
                    left,
                    expected(3),
                    "case {case:?}: a refused request must leave the stored set as it was, \
                     got {left:?}"
                );
                assert_eq!(
                    stored_provider_count(&h),
                    registrations,
                    "case {case:?}: a refused request must register nothing"
                );
            }
        }
    }
}

/// The other side of the bound: a selection AT the modelled maximum and below it is
/// accepted and its effect is readable back, so the refusal above is about the SIZE of
/// the selection and not about the member it travels in.
#[tokio::test]
async fn a_tag_selection_within_the_modelled_maximum_is_accepted() {
    let h = harness();

    // A removal names between one key and the maximum, and empties a resource carrying
    // exactly those keys. (`tagKeys` is `@required`, so a request naming none of them is
    // a different refusal and not a row of this rule — `tag_api_refusal_code.rs` pins
    // what that refusal is.)
    for count in [1_usize, 2, MAX_TAG_SELECTION - 1, MAX_TAG_SELECTION] {
        let case = format!("UntagResource naming {count} key(s)");
        let label = format!("removal-{count}");
        let arn = seed(&h, &label, count);
        Selection::RemovalKeys
            .send(&h, &arn, "", count)
            .await
            .ok(&case);
        let left = reported_tags_of(&h, &arn, &case).await;
        assert_eq!(
            left,
            BTreeMap::new(),
            "case {case:?}: a removal of every stored key must leave the empty set, got {left:?}"
        );
    }

    // A write names between one pair and the maximum, and the resource reports exactly what
    // it stored. (A write naming NO pair is a different refusal and not a row of this rule —
    // `tag_plane_authorization.rs` pins what that refusal is.)
    for count in [1_usize, 2, MAX_TAG_SELECTION - 1, MAX_TAG_SELECTION] {
        let case = format!("TagResource with {count} pair(s)");
        let label = format!("write-{count}");
        let arn = seed(&h, &label, 0);
        Selection::WriteMap.send(&h, &arn, "", count).await.ok(&case);
        let got = reported_tags_of(&h, &arn, &case).await;
        assert_eq!(
            got,
            expected(count),
            "case {case:?}: expected the pairs the write stored, got {got:?}"
        );
    }

    // And the create: a registration at the maximum is taken, with the tag set it carried
    // readable through the tag surface. Its own emulator, because the account holds ONE
    // registration per source: the set a registration carries is the set the resource is
    // created with only when nothing is registered for that source yet.
    let h = harness();
    let case = format!("RegisterSubscriptionProvider with {MAX_TAG_SELECTION} tags");
    let secret = secret_arn("accepted-create");
    let reply = Selection::CreateMap
        .send(&h, "", &secret, MAX_TAG_SELECTION)
        .await;
    let arn = reply
        .ok(&case)
        .get("SubscriptionProviderArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no ARN in {:?}", reply.body))
        .to_string();
    let got = reported_tags_of(&h, &arn, &case).await;
    assert_eq!(
        got,
        expected(MAX_TAG_SELECTION),
        "case {case:?}: expected the tag set the registration carried, got {got:?}"
    );
}
