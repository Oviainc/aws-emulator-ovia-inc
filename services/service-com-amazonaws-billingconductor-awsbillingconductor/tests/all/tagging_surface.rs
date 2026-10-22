// SPDX-License-Identifier: CC0-1.0

//! The tagging surface — `TagResource`, `ListTagsForResource`, `UntagResource` — driven
//! end to end over the service's real `restJson1` wire through
//! [`MultiServiceRouter::dispatch`], the same path the production router serves.
//!
//! RULES under test, each asserted over a SET of inputs rather than one recorded call:
//!
//! 1. A tagging operation addresses the resource its `ResourceArn` names and acts on
//!    THAT resource's stored tags: `TagResource` MERGES the request's pairs over them
//!    (a key the request does not name keeps its value — "If existing tags on a resource
//!    are not specified in the request parameters, they are not changed"),
//!    `UntagResource` removes exactly the named keys, and `ListTagsForResource` reports
//!    what is stored without changing it.
//! 2. A `ResourceArn` that names no resource this service holds is answered HTTP 404
//!    with the error code `NotFoundException` — the code, status and sentence the
//!    behavioral corpus recorded for this surface (see
//!    `issues/.../20261022T200837Z-billingconductor-tagresource-stub.md`) — and nothing
//!    is created by the attempt.
//!
//! The rows differ along what those rules generalise over: the resource KIND
//! (`pricingplan`, `billinggroup`, and the two kinds nothing persists yet), the ARN
//! SPELLING of one resource (the `billinggroup` separator the modelled pattern makes
//! optional), how many pairs one request carries, whether a key is new or overwritten,
//! the tag value at each end of the modelled `TagValue` length bound, a key at the
//! modelled `TagKey` maximum, how many keys one `UntagResource` removes, whether a
//! removed key was ever set, and which single ARN field (account, partition, resource
//! kind, resource id) a not-found ARN differs from a stored one in.

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The recorded wire error code of the tagging surface's not-found: HTTP 404, code
/// `NotFoundException` (NOT the model's `ResourceNotFoundException` — see the issue
/// cited in the module header, whose check asserts exactly this code).
const NOT_FOUND_CODE: &str = "NotFoundException";

/// A live emulator holding only this service.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Send one request over the wire and return the status plus the parsed body
    /// (`Value::Null` for an empty body).
    async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> (u16, Value) {
        let builder = http::Request::builder()
            .method(method)
            .uri(format!(
                "https://billingconductor.us-east-1.amazonaws.com{path}"
            ))
            .header("host", "billingconductor.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/us-east-1/billingconductor/aws4_request, \
                 SignedHeaders=host, Signature=00",
            );
        let payload = body.map_or_else(Bytes::new, |value| {
            Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        });
        let request = builder.body(payload).expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// `POST /tags/{ResourceArn}` with `Tags` in the body — `TagResource`.
    async fn tag(&self, resource_arn: &str, tags: &Value) -> (u16, Value) {
        self.call(
            "POST",
            &format!("/tags/{}", encode_label(resource_arn)),
            Some(&json!({ "Tags": tags })),
        )
        .await
    }

    /// `DELETE /tags/{ResourceArn}?tagKeys=..&tagKeys=..` — `UntagResource`. `TagKeys` is
    /// bound to `@httpQuery("tagKeys")`, so every key is its own query pair.
    async fn untag(&self, resource_arn: &str, tag_keys: &[&str]) -> (u16, Value) {
        let query = tag_keys
            .iter()
            .map(|key| format!("tagKeys={}", encode_label(key)))
            .collect::<Vec<_>>()
            .join("&");
        self.call(
            "DELETE",
            &format!("/tags/{}?{query}", encode_label(resource_arn)),
            None,
        )
        .await
    }

    /// `GET /tags/{ResourceArn}` — `ListTagsForResource`.
    async fn list_tags(&self, resource_arn: &str) -> (u16, Value) {
        self.call("GET", &format!("/tags/{}", encode_label(resource_arn)), None)
            .await
    }

    /// Create a pricing plan and return the ARN it minted.
    async fn create_plan(&self, name: &str, tags: Option<Value>) -> String {
        let mut body = json!({ "Name": name });
        if let Some(tags) = tags {
            body["Tags"] = tags;
        }
        let (status, answered) = self.call("POST", "/create-pricing-plan", Some(&body)).await;
        assert_eq!(
            status, 200,
            "setup CreatePricingPlan {name:?}: expected 200, got {status} with {answered}"
        );
        answered["Arn"]
            .as_str()
            .expect("setup CreatePricingPlan: no Arn in the response")
            .to_string()
    }

    /// Create a billing group on `plan_arn` and return the ARN it minted.
    async fn create_group(&self, name: &str, plan_arn: &str, tags: Option<Value>) -> String {
        let mut body = json!({
            "Name": name,
            "AccountGrouping": { "LinkedAccountIds": ["111122223333"] },
            "ComputationPreference": { "PricingPlanArn": plan_arn },
            "PrimaryAccountId": "111122223333",
        });
        if let Some(tags) = tags {
            body["Tags"] = tags;
        }
        let (status, answered) = self.call("POST", "/create-billing-group", Some(&body)).await;
        assert_eq!(
            status, 200,
            "setup CreateBillingGroup {name:?}: expected 200, got {status} with {answered}"
        );
        answered["Arn"]
            .as_str()
            .expect("setup CreateBillingGroup: no Arn in the response")
            .to_string()
    }

    /// The tags `ListTagsForResource` reports, as sorted pairs, asserting the read
    /// itself was served.
    async fn stored_tags(&self, case: &str, resource_arn: &str) -> Vec<(String, String)> {
        let (status, body) = self.list_tags(resource_arn).await;
        assert_eq!(
            status, LIST_TAGS_STATUS,
            "case {case:?}: ListTagsForResource on {resource_arn:?} expected \
             {LIST_TAGS_STATUS}, got {status} with {body}"
        );
        let reported = body
            .get("Tags")
            .unwrap_or_else(|| panic!("case {case:?}: no Tags member in {body}"));
        let mut pairs: Vec<(String, String)> = reported
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: Tags is not a map in {body}"))
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    value.as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        pairs.sort();
        pairs
    }
}

/// The status the model binds to the tagging operations
/// (`smithy_query(query="operation",operation="TagResource")`: `http.code` 204 for all
/// three).
const TAGGING_STATUS: u16 = 204;

/// `ListTagsForResource` answers on the same modelled code, with the `Tags` body.
const LIST_TAGS_STATUS: u16 = TAGGING_STATUS;

/// `ResourceArn` is bound to a NON-greedy `@httpLabel` (`/tags/{ResourceArn}`), so an
/// ARN travels as ONE path segment with its `/` and `:` percent-encoded — exactly what
/// an SDK client sends.
fn encode_label(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '/' => "%2F".to_string(),
            ':' => "%3A".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// The wire error code of a failure response (`__type`), without any namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<none>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// A JSON tag map from pairs.
fn tag_map(pairs: &[(&str, &str)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), Value::String((*value).to_string())))
            .collect::<Map<String, Value>>(),
    )
}

/// The expected pairs, sorted the way [`Emulator::stored_tags`] reports them.
fn expected(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut owned: Vec<(String, String)> = pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    owned.sort();
    owned
}

/// A deterministic string of `length` ASCII letters, for exercising a declared length
/// bound rather than one hand-picked spelling.
fn filler(length: usize) -> String {
    (0..length)
        .map(|index| char::from(b'a' + u8::try_from(index % 26).unwrap_or(0)))
        .collect()
}

/// RULE 1: `TagResource` merges its pairs into the tags of the resource its
/// `ResourceArn` names, and `ListTagsForResource` reads back exactly what is stored —
/// including the pairs the resource's own create persisted.
///
/// The rows differ along the resource kind, the ARN spelling of one resource, how many
/// pairs the request carries, whether each key is new or already set, and the value's
/// length at both ends of the modelled `TagValue` bound
/// (`smithy_query(query="shape",shape="TagValue")`: len 0..256, with
/// `TagKey` len 1..128).
#[tokio::test]
async fn tag_resource_merges_its_pairs_into_the_named_resource() {
    let em = Emulator::new();
    // Created before the billing group so the group's minted id is still all digits:
    // the tagging `Arn` pattern spells a billing group `billinggroup/?[0-9]{12}`.
    let plan_with_tags = em
        .create_plan("PlanWithTags", Some(tag_map(&[("team", "finance")])))
        .await;
    let plan_bare = em.create_plan("PlanBare", None).await;
    let group = em
        .create_group("GroupTagged", &plan_bare, Some(tag_map(&[("stage", "test")])))
        .await;
    // The same group, spelled without the separator the modelled pattern makes optional
    // (`billinggroup/?[0-9]{12}`): one resource, two spellings.
    let group_unseparated = group.replace("billinggroup/", "billinggroup");

    let long_key = filler(128);
    let long_value = filler(256);

    // One row of the merge table: the ARN to tag, the pairs to send, the pairs stored
    // afterwards, and the ARN spelling to read them back through.
    struct MergeCase<'a> {
        label: &'a str,
        arn: &'a str,
        tags: Value,
        after: Vec<(&'a str, &'a str)>,
        read_through: &'a str,
    }

    let cases: Vec<MergeCase<'_>> = vec![
        MergeCase {
            label: "one new key on a plan created with tags",
            arn: &plan_with_tags,
            tags: tag_map(&[("env", "prod")]),
            after: vec![("team", "finance"), ("env", "prod")],
            read_through: &plan_with_tags,
        },
        MergeCase {
            label: "two pairs, one overwriting a stored key and one new",
            arn: &plan_with_tags,
            tags: tag_map(&[("env", "staging"), ("owner", "platform")]),
            after: vec![
                ("team", "finance"),
                ("env", "staging"),
                ("owner", "platform"),
            ],
            read_through: &plan_with_tags,
        },
        MergeCase {
            label: "an empty tag value (the modelled TagValue minimum length)",
            arn: &plan_with_tags,
            tags: tag_map(&[("note", "")]),
            after: vec![
                ("team", "finance"),
                ("env", "staging"),
                ("owner", "platform"),
                ("note", ""),
            ],
            read_through: &plan_with_tags,
        },
        MergeCase {
            label: "a key and value at the modelled maxima (128 and 256 characters)",
            arn: &plan_with_tags,
            tags: tag_map(&[(long_key.as_str(), long_value.as_str())]),
            after: vec![
                ("team", "finance"),
                ("env", "staging"),
                ("owner", "platform"),
                ("note", ""),
                (long_key.as_str(), long_value.as_str()),
            ],
            read_through: &plan_with_tags,
        },
        MergeCase {
            label: "three pairs on a plan created with no tags at all",
            arn: &plan_bare,
            tags: tag_map(&[("a", "1"), ("b", "2"), ("c", "3")]),
            after: vec![("a", "1"), ("b", "2"), ("c", "3")],
            read_through: &plan_bare,
        },
        MergeCase {
            label: "a billing group, addressed by its full ARN",
            arn: &group,
            tags: tag_map(&[("cost-center", "42")]),
            after: vec![("stage", "test"), ("cost-center", "42")],
            read_through: &group,
        },
        MergeCase {
            label: "the same billing group, addressed without the optional separator",
            arn: &group_unseparated,
            tags: tag_map(&[("owner", "ops")]),
            after: vec![
                ("stage", "test"),
                ("cost-center", "42"),
                ("owner", "ops"),
            ],
            // Read back through the SLASHED spelling: both name one resource.
            read_through: &group,
        },
    ];

    for case in &cases {
        let label = case.label;
        let (status, body) = em.tag(case.arn, &case.tags).await;
        assert_eq!(
            status, TAGGING_STATUS,
            "case {label:?}: TagResource expected {TAGGING_STATUS}, got {status} with {body}"
        );
        let stored = em.stored_tags(label, case.read_through).await;
        assert_eq!(
            stored,
            expected(&case.after),
            "case {label:?}: expected the stored tags {:?}, got {stored:?}",
            expected(&case.after)
        );
    }

    // A resource's tags are ITS OWN: tagging one never changes another's.
    let other = em.stored_tags("tags do not leak between resources", &plan_bare).await;
    assert_eq!(
        other,
        expected(&[("a", "1"), ("b", "2"), ("c", "3")]),
        "expected the other plan's tags to be untouched, got {other:?}"
    );

    // A read changes nothing: the same request twice answers the same map.
    let first = em.stored_tags("a read does not mutate", &group).await;
    let second = em.stored_tags("a read does not mutate", &group).await;
    assert_eq!(
        first, second,
        "expected two reads to answer the same tags, got {first:?} then {second:?}"
    );
}

/// RULE 1, the removal half: `UntagResource` deletes exactly the keys it names, leaves
/// every other key in place, and is not refused for a key the resource never carried.
///
/// The rows differ along how many keys one request removes, whether each was ever set,
/// and whether the resource is left with some tags or with none.
#[tokio::test]
async fn untag_resource_removes_only_the_keys_it_names() {
    let em = Emulator::new();
    let plan = em
        .create_plan(
            "PlanToUntag",
            Some(tag_map(&[("a", "1"), ("b", "2"), ("c", "3"), ("d", "4")])),
        )
        .await;

    // One row of the removal table: the keys to remove and the pairs left afterwards.
    struct UntagCase<'a> {
        label: &'a str,
        keys: Vec<&'a str>,
        after: Vec<(&'a str, &'a str)>,
    }

    let cases: Vec<UntagCase<'_>> = vec![
        UntagCase {
            label: "one stored key",
            keys: vec!["a"],
            after: vec![("b", "2"), ("c", "3"), ("d", "4")],
        },
        UntagCase {
            label: "two keys, one of them never set",
            keys: vec!["b", "never-set"],
            after: vec![("c", "3"), ("d", "4")],
        },
        UntagCase {
            label: "every remaining key",
            keys: vec!["c", "d"],
            after: vec![],
        },
        UntagCase {
            label: "a key already removed, again",
            keys: vec!["c"],
            after: vec![],
        },
    ];

    for case in &cases {
        let label = case.label;
        let (status, body) = em.untag(&plan, &case.keys).await;
        assert_eq!(
            status, TAGGING_STATUS,
            "case {label:?}: UntagResource expected {TAGGING_STATUS}, got {status} with {body}"
        );
        let stored = em.stored_tags(label, &plan).await;
        assert_eq!(
            stored,
            expected(&case.after),
            "case {label:?}: expected the stored tags {:?}, got {stored:?}",
            expected(&case.after)
        );
    }

    // And the pair is a round trip: what `TagResource` writes, `UntagResource` takes
    // back off, leaving the resource as it was.
    let (status, body) = em.tag(&plan, &tag_map(&[("round", "trip")])).await;
    assert_eq!(
        status, TAGGING_STATUS,
        "the round-trip tag expected {TAGGING_STATUS}, got {status} with {body}"
    );
    assert_eq!(
        em.stored_tags("round trip", &plan).await,
        expected(&[("round", "trip")]),
        "expected the round-trip key to be stored"
    );
    let (status, body) = em.untag(&plan, &["round"]).await;
    assert_eq!(
        status, TAGGING_STATUS,
        "the round-trip untag expected {TAGGING_STATUS}, got {status} with {body}"
    );
    assert_eq!(
        em.stored_tags("round trip", &plan).await,
        expected(&[]),
        "expected the round-trip key to be gone again"
    );
}

/// RULE 1, the bound: a resource holds at most as many tags as the modelled `TagMap`
/// allows (`smithy_query(query="shape",shape="TagMap")`: len 1..200), counted over what
/// the resource ACCUMULATES rather than over one request. At the cap a tagging call that
/// adds no key is still served; one that would add a key past it is refused with the
/// modelled `ValidationException` and stores nothing.
///
/// The rows walk the bound: one key short of it, exactly at it, a re-tag of a stored key
/// at it, and one key past it.
#[tokio::test]
async fn a_resource_holds_at_most_the_modelled_number_of_tags() {
    // The modelled `TagMap` maximum, which is also the per-resource cap.
    const MAX_TAGS: usize = 200;

    let em = Emulator::new();
    let plan = em.create_plan("PlanAtTheTagCap", None).await;

    // One request carries at most `MAX_TAGS` pairs (the same modelled bound), so the cap
    // is reached in two calls: 199 pairs, then the 200th.
    let first: Vec<(String, String)> = (0..MAX_TAGS - 1)
        .map(|index| (format!("key-{index:03}"), index.to_string()))
        .collect();
    let borrowed: Vec<(&str, &str)> = first
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let (status, body) = em.tag(&plan, &tag_map(&borrowed)).await;
    assert_eq!(
        status, TAGGING_STATUS,
        "case \"one key short of the cap\": expected {TAGGING_STATUS}, got {status} with {body}"
    );
    assert_eq!(
        em.stored_tags("one key short of the cap", &plan).await.len(),
        MAX_TAGS - 1,
        "case \"one key short of the cap\": expected {} stored tags",
        MAX_TAGS - 1
    );

    let (status, body) = em.tag(&plan, &tag_map(&[("key-199", "199")])).await;
    assert_eq!(
        status, TAGGING_STATUS,
        "case \"the key that reaches the cap\": expected {TAGGING_STATUS}, got {status} with \
         {body}"
    );
    assert_eq!(
        em.stored_tags("the key that reaches the cap", &plan).await.len(),
        MAX_TAGS,
        "case \"the key that reaches the cap\": expected {MAX_TAGS} stored tags"
    );

    // AT the cap, a request that only overwrites a stored key adds nothing, so it is
    // served.
    let (status, body) = em.tag(&plan, &tag_map(&[("key-000", "rewritten")])).await;
    assert_eq!(
        status, TAGGING_STATUS,
        "case \"overwriting a stored key at the cap\": expected {TAGGING_STATUS}, got {status} \
         with {body}"
    );
    let at_cap = em.stored_tags("overwriting a stored key at the cap", &plan).await;
    assert_eq!(
        at_cap.len(),
        MAX_TAGS,
        "case \"overwriting a stored key at the cap\": expected {MAX_TAGS} stored tags"
    );
    assert!(
        at_cap.contains(&("key-000".to_string(), "rewritten".to_string())),
        "case \"overwriting a stored key at the cap\": expected the overwritten value, got \
         {at_cap:?}"
    );

    // One key PAST the cap is refused, in the service's own modelled shape, and nothing
    // is stored.
    let (status, body) = em.tag(&plan, &tag_map(&[("one-too-many", "x")])).await;
    assert_eq!(
        status, 400,
        "case \"one key past the cap\": expected HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case \"one key past the cap\": expected the modelled ValidationException, got {body}"
    );
    assert_eq!(
        body["Reason"].as_str(),
        Some("FIELD_VALIDATION_FAILED"),
        "case \"one key past the cap\": expected the field-validation reason, got {body}"
    );
    let after = em.stored_tags("one key past the cap", &plan).await;
    assert_eq!(
        after.len(),
        MAX_TAGS,
        "case \"one key past the cap\": a refused merge must store nothing, got {} tags",
        after.len()
    );
    assert!(
        !after.iter().any(|(key, _)| key == "one-too-many"),
        "case \"one key past the cap\": the refused key must not be stored, got {after:?}"
    );
}

/// RULE 2: every tagging operation answers HTTP 404 `NotFoundException` for a
/// `ResourceArn` that names no resource this service holds, and the attempt creates
/// nothing.
///
/// The rows are ARNs that differ from a STORED one in exactly one field — the resource
/// id, the resource kind, the account, the partition — plus the two taggable kinds
/// nothing persists yet (`pricingrule`, `customlineitem`, whose creates are still
/// scaffolds) and the bare-id spelling the modelled pattern admits.
#[tokio::test]
async fn every_tagging_operation_answers_404_not_found_for_an_unknown_resource() {
    let em = Emulator::new();
    let plan = em.create_plan("PlanThatExists", None).await;
    let plan_id = plan
        .rsplit('/')
        .next()
        .expect("the minted plan ARN carries a resource id")
        .to_string();

    let account = engine::emulator_account_id();
    let unknown: Vec<(&str, String)> = vec![
        (
            "a pricing plan ARN that was never created",
            format!("arn:aws:billingconductor::{account}:pricingplan/ZZZZZZZZZZ"),
        ),
        (
            "a stored plan's id under another ACCOUNT",
            format!("arn:aws:billingconductor::123456789012:pricingplan/{plan_id}"),
        ),
        (
            "a stored plan's id in another PARTITION",
            format!("arn:aws-cn:billingconductor::{account}:pricingplan/{plan_id}"),
        ),
        (
            "a stored plan's id under another resource KIND",
            format!("arn:aws:billingconductor::{account}:pricingrule/{plan_id}"),
        ),
        (
            "a billing group ARN that was never created",
            format!("arn:aws:billingconductor::{account}:billinggroup/999999999999"),
        ),
        (
            "the same unknown group without the optional separator",
            format!("arn:aws:billingconductor::{account}:billinggroup999999999999"),
        ),
        (
            "a custom line item ARN (nothing persists one yet)",
            format!("arn:aws:billingconductor::{account}:customlineitem/ABCDEFGHIJ"),
        ),
        (
            "a bare custom line item id, which is a plan id here",
            plan_id.clone(),
        ),
    ];

    for (label, arn) in &unknown {
        let attempts = [
            ("TagResource", em.tag(arn, &tag_map(&[("env", "x")])).await),
            ("UntagResource", em.untag(arn, &["env"]).await),
            ("ListTagsForResource", em.list_tags(arn).await),
        ];
        for (operation, (status, body)) in &attempts {
            assert_eq!(
                *status, 404,
                "case {label:?} / {operation}: expected HTTP 404 for {arn:?}, got {status} \
                 with {body}"
            );
            assert_eq!(
                error_code(body),
                NOT_FOUND_CODE,
                "case {label:?} / {operation}: expected the error code {NOT_FOUND_CODE}, got \
                 {body}"
            );
            assert!(
                body.get("message")
                    .or_else(|| body.get("Message"))
                    .and_then(Value::as_str)
                    .is_some_and(|message| !message.is_empty()),
                "case {label:?} / {operation}: expected a message saying what was not found, \
                 got {body}"
            );
        }
        // The refused calls created nothing: the ARN still names no resource.
        let (status, body) = em.list_tags(arn).await;
        assert_eq!(
            status, 404,
            "case {label:?}: a refused tagging call must not create the resource, got {status} \
             with {body}"
        );
    }

    // The stored resource is still readable and still untagged, so none of the refusals
    // touched it.
    assert_eq!(
        em.stored_tags("the stored plan is untouched", &plan).await,
        expected(&[]),
        "expected the stored plan to carry no tags"
    );
}
