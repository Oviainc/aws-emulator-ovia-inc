// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage for this service's tagging slice: `TagResource`
//! (`PUT /tags/{resourceArn}`) and `ListTagsForResource` (`GET /tags/{resourceArn}`).
//!
//! Every case drives a real request through the router's `dispatch`, so what is asserted
//! is the wire behaviour an SDK client sees: deserialize → constraint check → handle →
//! serialize.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! * a tag write MERGES the pairs it names into the tag set the addressed resource
//!   already carries in state — a key the request names takes the request's value, a key
//!   it does not name keeps the value it had — and the read reports exactly that stored
//!   set, so two writes of different keys converge on the union of both and a repeat of
//!   one write changes nothing (`TagResource` is `@idempotent`);
//! * the set is per RESOURCE: a write addressed to one subscription provider leaves
//!   every other provider's set alone;
//! * `ListTagsForResource` is `@readonly` — reading tags changes nothing a later caller
//!   sees, including the rest of the provider record `GetRegisteredSubscriptionProvider`
//!   reports;
//! * a `resourceArn` addressing no resource of this account is refused by BOTH
//!   operations with the code the tag APIs answer (`NotFoundException`, HTTP 400), and
//!   the refused write stores nothing;
//! * a write whose RESULTANT set would be larger than a resource may carry
//!   ([`MAX_TAGS`], the modelled `Tags` maximum) is refused with `BadRequestException`
//!   and leaves the stored set as it was.
//!
//! The tables span the class each rule covers rather than the one recorded tag set: the
//! number of pairs written (none, one, three, the modelled maximum of 50), keys and
//! values that are non-ASCII or URI-significant, a write that overwrites a stored key, a
//! write disjoint from the stored keys, and — for the maximum — the published bound, one
//! below it and one past it.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_licensemanagerlinuxsubscriptions_licensemanagerlinuxsubscriptions::state::{
    SubscriptionProviderRecord, PROVIDER_STATUS_ACTIVE,
};
use service_com_amazonaws_licensemanagerlinuxsubscriptions_licensemanagerlinuxsubscriptions::{
    ComAmazonawsLicensemanagerlinuxsubscriptionsLicensemanagerlinuxsubscriptionsState as ServiceState,
    EmulatedService,
};

/// This service's sigv4 signing name (`OPERATIONS.md`, "Wire protocol") — how the router
/// picks the service out of the request's credential scope.
const SIGNING_NAME: &str = "license-manager-linux-subscriptions";
/// The region the test requests are signed for; every minted ARN carries it.
const REGION: &str = "us-west-2";
/// The one account id a generated emulator presents.
const ACCOUNT: &str = "000000000000";
/// The only `SubscriptionProviderSource` the model declares (`one of: RedHat`).
const RED_HAT: &str = "RedHat";

/// The largest tag set a resource may carry: the modelled `Tags` map is `len 0..50`,
/// both as `TagResource`'s input and as `ListTagsForResource`'s output
/// (`smithy_query(query="shape",shape="Tags")`).
const MAX_TAGS: usize = 50;

/// The wire error code and message the tag APIs answer for a `resourceArn` that
/// addresses no resource of this account.
const NOT_FOUND: (&str, &str) = ("NotFoundException", "Resource was not found");

/// The wire error code and message a write past [`MAX_TAGS`] is refused with.
const TAG_LIMIT: (&str, &str) = (
    "BadRequestException",
    "The resultant tag set must not have more than 50 user tags.",
);

/// One emulator: the router that dispatches, plus the shared engine its service was
/// built from (so a test can read the store the handlers write).
struct Harness {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

fn harness() -> Harness {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The tests sign nothing; authentication is the framework's concern, not this
    // service's.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    Harness { router, engine }
}

/// What a call answered: the HTTP status, the wire error code (if any) and the body.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    fn ok(&self, case: &str) -> &Value {
        assert_eq!(
            self.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {:?}",
            self.status, self.body
        );
        &self.body
    }

    /// Assert this reply is HTTP 400 with the wire error code and message of `refusal`.
    fn refused(&self, case: &str, refusal: (&str, &str)) {
        let (code, message) = refusal;
        assert_eq!(
            self.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {:?}",
            self.status, self.body
        );
        let actual = self.error_type.as_deref().unwrap_or("<none>");
        assert!(
            actual.ends_with(code),
            "case {case:?}: expected error code {code}, got {actual:?} with body {:?}",
            self.body
        );
        assert_eq!(
            self.body.get("message").and_then(Value::as_str),
            Some(message),
            "case {case:?}: expected message {message:?}, got body {:?}",
            self.body
        );
    }
}

async fn call(h: &Harness, method: &str, path: &str, body: Option<&Value>) -> Reply {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{SIGNING_NAME}.{REGION}.amazonaws.com{path}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTEXAMPLE/20260101/{REGION}/{SIGNING_NAME}\
                 /aws4_request, SignedHeaders=host, Signature=0"
            ),
        );
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let payload = body.map_or_else(bytes::Bytes::new, |value| {
        bytes::Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
    });
    let request = builder.body(payload).expect("a well-formed request");
    let response = h.router.dispatch(request).await;
    let status = response.status().as_u16();
    let header_error = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    let error_type = header_error.or_else(|| {
        body.get("__type")
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    Reply {
        status,
        error_type,
        body,
    }
}

/// `resourceArn` is bound to a NON-greedy `@httpLabel` (`/tags/{resourceArn}`), so an
/// ARN travels as ONE path segment with its `/` and `:` percent-encoded — exactly what
/// an SDK client sends.
fn encode_label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '/' => "%2F".to_string(),
            ':' => "%3A".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// A JSON tag map from `(key, value)` pairs.
fn tag_map(pairs: &[(String, String)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect::<Map<String, Value>>(),
    )
}

/// Owned `(key, value)` rows, so a table can hold computed keys and values beside
/// written-out ones.
fn pairs(rows: &[(&str, &str)]) -> Vec<(String, String)> {
    rows.iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// `count` distinct generated pairs, for the rows that exercise a tag COUNT.
fn generated_pairs(count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
        .collect()
}

/// Put a subscription provider carrying `tags` in the store and return its ARN.
///
/// The provider is seeded DIRECTLY, through the same `engine.state::<…>()` seam the
/// handlers use, because `RegisterSubscriptionProvider` registers nothing in this
/// emulator: the real service registers a provider only once it has validated the access
/// token held in the referenced secret, which cannot be done here, so every registration
/// is refused (see `src/ops/register_subscription_provider.rs`). The tag operations under
/// test still have to answer from the store, so the suite puts the resource there. The
/// `secret` keeps each seeded record distinct, exactly as it kept each registration
/// distinct.
async fn register(h: &Harness, case: &str, secret: &str, tags: &[(String, String)]) -> String {
    let store = h.engine.state::<ServiceState>();
    let mut guard = store.lock();
    let arn = format!(
        "arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:subscription-provider/\
         00000000-0000-4000-8000-{:012x}",
        guard.providers.len() + 1
    );
    assert!(
        !guard.providers.contains_key(&arn),
        "case {case:?}: the seeded ARN {arn} is already in the store"
    );
    guard.providers.insert(
        arn.clone(),
        SubscriptionProviderRecord {
            arn: arn.clone(),
            source: RED_HAT.to_string(),
            secret_arn: secret.to_string(),
            status: PROVIDER_STATUS_ACTIVE.to_string(),
            status_message: None,
            last_successful_data_retrieval_time: None,
            tags: tags.iter().cloned().collect(),
        },
    );
    guard.provider_order.push(arn.clone());
    arn
}

/// A Secrets Manager ARN satisfying the modelled `SecretArn` pattern, distinct per index.
fn secret_arn(index: usize) -> String {
    format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:rhel-token-{index}")
}

async fn tag_resource(h: &Harness, arn: &str, tags: &[(String, String)]) -> Reply {
    call(
        h,
        "PUT",
        &format!("/tags/{}", encode_label(arn)),
        Some(&json!({ "tags": tag_map(tags) })),
    )
    .await
}

async fn list_tags(h: &Harness, arn: &str) -> Reply {
    call(h, "GET", &format!("/tags/{}", encode_label(arn)), None).await
}

/// The tag set a `ListTagsForResource` body reports, as a sorted map so two sets compare
/// independently of iteration order.
fn reported_tags(reply: &Reply, case: &str) -> BTreeMap<String, String> {
    let body = reply.ok(case);
    let tags = body
        .get("tags")
        .unwrap_or_else(|| panic!("case {case:?}: no tags member in {body:?}"));
    tags.as_object()
        .unwrap_or_else(|| panic!("case {case:?}: tags is not a map in {body:?}"))
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

/// The expected set, as the same sorted map.
fn expected_tags(rows: &[(String, String)]) -> BTreeMap<String, String> {
    rows.iter().cloned().collect()
}

/// One row of the merge table: what the resource already carries, what the write names,
/// and the set the resource is left with.
struct MergeCase {
    case: String,
    stored: Vec<(String, String)>,
    written: Vec<(String, String)>,
    want: Vec<(String, String)>,
}

fn merge_cases() -> Vec<MergeCase> {
    let mut cases = vec![
        MergeCase {
            case: "no pairs leaves the stored set alone".to_string(),
            stored: pairs(&[("owner", "platform")]),
            written: Vec::new(),
            want: pairs(&[("owner", "platform")]),
        },
        MergeCase {
            case: "one pair onto an empty set".to_string(),
            stored: Vec::new(),
            written: pairs(&[("env", "prod")]),
            want: pairs(&[("env", "prod")]),
        },
        MergeCase {
            case: "three pairs onto an empty set".to_string(),
            stored: Vec::new(),
            written: pairs(&[("env", "prod"), ("owner", "platform"), ("cost-centre", "42")]),
            want: pairs(&[("env", "prod"), ("owner", "platform"), ("cost-centre", "42")]),
        },
        MergeCase {
            case: "a stored key takes the written value".to_string(),
            stored: pairs(&[("env", "dev"), ("owner", "platform")]),
            written: pairs(&[("env", "prod")]),
            want: pairs(&[("env", "prod"), ("owner", "platform")]),
        },
        MergeCase {
            case: "disjoint keys converge on the union".to_string(),
            stored: pairs(&[("a", "1")]),
            written: pairs(&[("b", "2"), ("c", "3")]),
            want: pairs(&[("a", "1"), ("b", "2"), ("c", "3")]),
        },
        MergeCase {
            case: "an empty value overwrites a non-empty one".to_string(),
            stored: pairs(&[("env", "prod")]),
            written: pairs(&[("env", "")]),
            want: pairs(&[("env", "")]),
        },
        MergeCase {
            case: "non-ASCII keys and values".to_string(),
            stored: Vec::new(),
            written: pairs(&[("propriétaire", "équipe-plate-forme"), ("团队", "平台")]),
            want: pairs(&[("propriétaire", "équipe-plate-forme"), ("团队", "平台")]),
        },
        MergeCase {
            case: "URI-significant keys and values".to_string(),
            stored: pairs(&[("path", "a/b")]),
            written: pairs(&[
                ("path", "a/b:c?d=e&f"),
                ("arn", "arn:aws:iam::000000000000:role/x"),
            ]),
            want: pairs(&[
                ("path", "a/b:c?d=e&f"),
                ("arn", "arn:aws:iam::000000000000:role/x"),
            ]),
        },
    ];
    // The modelled maximum, written in one call onto an empty set.
    cases.push(MergeCase {
        case: format!("the modelled maximum of {MAX_TAGS} pairs onto an empty set"),
        stored: Vec::new(),
        written: generated_pairs(MAX_TAGS),
        want: generated_pairs(MAX_TAGS),
    });
    // A resource already AT the maximum: a write that only overwrites stored keys keeps
    // the resultant set at the bound, so it is accepted.
    let mut at_max = generated_pairs(MAX_TAGS);
    let mut overwritten = at_max.clone();
    at_max[0].1 = "replaced".to_string();
    overwritten.truncate(1);
    overwritten[0].1 = "replaced".to_string();
    cases.push(MergeCase {
        case: format!("a set already at {MAX_TAGS} accepts an overwrite of a stored key"),
        stored: generated_pairs(MAX_TAGS),
        written: overwritten,
        want: at_max,
    });
    cases
}

#[tokio::test]
async fn a_tag_write_merges_into_the_stored_set_the_read_reports() {
    let h = harness();
    let mut resources = Vec::new();

    for (index, row) in merge_cases().into_iter().enumerate() {
        let case = row.case.clone();
        let arn = register(&h, &case, &secret_arn(index), &row.stored).await;

        tag_resource(&h, &arn, &row.written).await.ok(&case);
        let got = reported_tags(&list_tags(&h, &arn).await, &case);
        assert_eq!(
            got,
            expected_tags(&row.want),
            "case {case:?}: expected the merged set {:?}, got {got:?}",
            expected_tags(&row.want)
        );

        // `TagResource` is `@idempotent`: the same write again leaves the same set.
        tag_resource(&h, &arn, &row.written).await.ok(&case);
        let again = reported_tags(&list_tags(&h, &arn).await, &case);
        assert_eq!(
            again, got,
            "case {case:?}: a repeated write changed the set to {again:?}"
        );

        resources.push((case, arn, row.want));
    }

    // Per resource: every earlier row still reports its OWN set after every later write.
    for (case, arn, want) in &resources {
        let got = reported_tags(&list_tags(&h, arn).await, case);
        assert_eq!(
            got,
            expected_tags(want),
            "case {case:?}: after the other rows' writes, {arn} reports {got:?}"
        );
    }
}

#[tokio::test]
async fn reading_tags_changes_nothing_a_later_caller_sees() {
    let h = harness();
    let case = "read a provider's tags twice";
    let stored = pairs(&[("env", "prod"), ("owner", "platform")]);
    let arn = register(&h, case, &secret_arn(0), &stored).await;

    let first = reported_tags(&list_tags(&h, &arn).await, case);
    let second = reported_tags(&list_tags(&h, &arn).await, case);
    assert_eq!(
        first,
        expected_tags(&stored),
        "case {case:?}: expected the registration's tags {:?}, got {first:?}",
        expected_tags(&stored)
    );
    assert_eq!(
        second, first,
        "case {case:?}: a second read reported {second:?} after {first:?}"
    );

    // The rest of the record is untouched too: the reader is `@readonly`.
    let provider = call(
        &h,
        "POST",
        "/subscription/GetRegisteredSubscriptionProvider",
        Some(&json!({ "SubscriptionProviderArn": arn })),
    )
    .await;
    let body = provider.ok(case);
    for (member, want) in [
        ("SubscriptionProviderArn", arn.as_str()),
        ("SubscriptionProviderSource", RED_HAT),
        ("SecretArn", secret_arn(0).as_str()),
    ] {
        assert_eq!(
            body.get(member).and_then(Value::as_str),
            Some(want),
            "case {case:?}: expected {member} {want:?}, got {body:?}"
        );
    }
}

#[tokio::test]
async fn an_arn_addressing_no_resource_is_refused_by_both_tag_operations() {
    let h = harness();
    // One registered provider, so a refusal cannot be confused with an empty store, and
    // so the test can show a refused write stored nothing.
    let stored = pairs(&[("env", "prod")]);
    let live = register(&h, "the registered provider", &secret_arn(0), &stored).await;
    // A provider that WAS registered and is not any more: its ARN is well-formed and
    // addresses nothing.
    let deregistered = register(&h, "a provider to deregister", &secret_arn(1), &[]).await;
    call(
        &h,
        "POST",
        "/subscription/DeregisterSubscriptionProvider",
        Some(&json!({ "SubscriptionProviderArn": deregistered })),
    )
    .await
    .ok("deregister the second provider");

    let unknown: Vec<(String, String)> = vec![
        (
            "an id nothing was ever registered under".to_string(),
            format!(
                "arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:subscription-provider/\
                 55555555-5555-4555-8555-555555555555"
            ),
        ),
        ("a deregistered provider".to_string(), deregistered),
        (
            "the live provider's id in another region".to_string(),
            live.replace(REGION, "eu-west-1"),
        ),
        (
            "the live provider's id in another account".to_string(),
            live.replace(ACCOUNT, "123456789012"),
        ),
        (
            "another resource kind".to_string(),
            live.replace("subscription-provider/", "linux-subscription/"),
        ),
        (
            "another service's namespace".to_string(),
            live.replace(&format!(":{SIGNING_NAME}:"), ":license-manager:"),
        ),
    ];

    for (label, arn) in &unknown {
        let case = format!("TagResource on {label}");
        tag_resource(&h, arn, &pairs(&[("env", "prod")]))
            .await
            .refused(&case, NOT_FOUND);

        let case = format!("ListTagsForResource on {label}");
        list_tags(&h, arn).await.refused(&case, NOT_FOUND);
    }

    // Nothing the refused writes carried was stored: the store still holds exactly the
    // one live provider, with exactly the tags its registration carried.
    let store = h.engine.state::<ServiceState>();
    let guard = store.lock();
    assert_eq!(
        guard.providers.len(),
        1,
        "expected only the live provider in the store, got {:?}",
        guard.providers.keys().collect::<Vec<_>>()
    );
    let got: BTreeMap<String, String> = guard
        .resource_tags(&live)
        .unwrap_or_else(|| panic!("the live provider {live} is gone from the store"))
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    assert_eq!(
        got,
        expected_tags(&stored),
        "expected the live provider's tags {:?}, got {got:?}",
        expected_tags(&stored)
    );
}

#[tokio::test]
async fn a_write_past_the_tag_maximum_is_refused_and_stores_nothing() {
    let h = harness();

    // The bound, one below it, and one past it: a resource holding `stored` keys taking
    // `added` new keys is accepted exactly while the resultant set fits in MAX_TAGS.
    let rows = [
        (MAX_TAGS - 1, 1, true),
        (MAX_TAGS, 1, false),
        (MAX_TAGS - 1, 2, false),
        (MAX_TAGS / 2, MAX_TAGS / 2, true),
    ];

    for (index, (stored_count, added, accepted)) in rows.into_iter().enumerate() {
        let case = format!("{stored_count} stored tags plus {added} new keys");
        let stored = generated_pairs(stored_count);
        let arn = register(&h, &case, &secret_arn(index), &stored).await;
        let added_pairs: Vec<(String, String)> = (0..added)
            .map(|offset| {
                (
                    format!("added-{offset:02}"),
                    format!("value-{offset:02}"),
                )
            })
            .collect();

        let reply = tag_resource(&h, &arn, &added_pairs).await;
        let mut want = stored.clone();
        if accepted {
            reply.ok(&case);
            want.extend(added_pairs.iter().cloned());
        } else {
            reply.refused(&case, TAG_LIMIT);
        }

        let got = reported_tags(&list_tags(&h, &arn).await, &case);
        assert_eq!(
            got,
            expected_tags(&want),
            "case {case:?}: expected the stored set {:?}, got {got:?}",
            expected_tags(&want)
        );
        assert!(
            got.len() <= MAX_TAGS,
            "case {case:?}: the resource carries {} tags, past the maximum of {MAX_TAGS}",
            got.len()
        );
    }
}
