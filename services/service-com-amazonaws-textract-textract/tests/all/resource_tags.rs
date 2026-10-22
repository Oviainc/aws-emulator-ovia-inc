// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of Textract's resource TAGGING over the wire (`awsJson1_1`,
//! `POST /` with `X-Amz-Target: Textract.<Operation>`): `TagResource`,
//! `UntagResource` and `ListTagsForResource`, plus what a later `GetAdapter` reports.
//!
//! Each test asserts a RULE over a TABLE of inputs rather than one recorded request:
//!
//! * `TagResource` writes the request's pairs to the resource its `ResourceARN` names,
//!   so a later `ListTagsForResource` — and `GetAdapter`, the adapter's own reader —
//!   reports exactly what was written, whatever the pairs' count, length or character
//!   class;
//! * a second `TagResource` MERGES: the keys it names take its values, every other
//!   stored key keeps its own, and repeating one call changes nothing further;
//! * `UntagResource` removes exactly the keys its `TagKeys` name and leaves the rest,
//!   and a key the resource does not hold is nothing to remove rather than a failure;
//! * a resource is addressed by the type and the identifier of its ARN, so every
//!   spelling of one adapter's ARN (with or without the leading `/`, in any partition,
//!   region or account) reaches the same stored tags;
//! * a well-formed Amazon Textract ARN that names no resource this service holds is
//!   refused by all three operations with the modelled `ResourceNotFoundException`, and
//!   a value that is no Textract ARN at all with the modelled
//!   `InvalidParameterException`;
//! * `TagMap`'s modelled `0..200` length is the ceiling on the stored set too: a merge
//!   landing exactly on it is accepted, one past it is refused in full and changes
//!   nothing;
//! * tags belong to one resource: tagging one adapter leaves every other adapter's tags
//!   alone.
//!
//! The wire harness (`harness`, `call`, `error_code`) is the one `document_extraction`
//! exposes to this test target; only the tagging helpers live here.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use service_com_amazonaws_textract_textract::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::document_extraction::{call, error_code, harness};

/// The region the ARNs in these tests are written for. Any region reaches this store —
/// `every_spelling_of_one_adapter_arn_reaches_one_resource` is what pins that.
const REGION: &str = "us-east-1";

/// `CreateAdapter`, asserting it succeeded, and the `AdapterId` it minted.
async fn create_adapter(router: &MultiServiceRouter, case: &str, request: &Value) -> String {
    let (status, body) = call(router, "CreateAdapter", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateAdapter, got {status} with {body}"
    );
    body.get("AdapterId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A `CreateAdapter` request for `name`, carrying `tags` when they are given.
fn create_request(name: &str, tags: Option<&Value>) -> Value {
    let mut request = json!({ "AdapterName": name, "FeatureTypes": ["QUERIES"] });
    if let Some(tags) = tags {
        request["Tags"] = tags.clone();
    }
    request
}

/// The tags a `ListTagsForResource` / `GetAdapter` body reports, as a comparable map.
fn reported_tags(body: &Value) -> BTreeMap<String, String> {
    body.get("Tags")
        .and_then(Value::as_object)
        .map(|tags| {
            tags.iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The pairs of a JSON tag map, as a comparable map.
fn expected_tags(tags: &Value) -> BTreeMap<String, String> {
    tags.as_object()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|(key, value)| (key, value.as_str().unwrap_or_default().to_string()))
        .collect()
}

/// `ListTagsForResource`, asserting it succeeded, and the tags it reported.
async fn list_tags(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
) -> BTreeMap<String, String> {
    let (status, body) = call(
        router,
        "ListTagsForResource",
        &json!({ "ResourceARN": arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource of {arn:?} must succeed, got {status} with {body}"
    );
    reported_tags(&body)
}

/// The tags `GetAdapter` reports for `adapter_id`, the adapter's own reader.
async fn adapter_tags(
    router: &MultiServiceRouter,
    case: &str,
    adapter_id: &str,
) -> BTreeMap<String, String> {
    let (status, body) = call(router, "GetAdapter", &json!({ "AdapterId": adapter_id })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetAdapter of {adapter_id:?} must succeed, got {status} with {body}"
    );
    reported_tags(&body)
}

/// A one-pair tag map, built without a macro key so any computed key is legal here.
fn pair(key: &str, value: &str) -> Value {
    let mut map = Map::new();
    map.insert(key.to_string(), json!(value));
    Value::Object(map)
}

/// A tag map of `count` pairs, with keys and values inside `TagKey`'s and `TagValue`'s
/// modelled patterns (`smithy_query(query="shape", shape="TagMap")`).
fn tag_map_of(count: usize) -> Value {
    let mut map = Map::new();
    for index in 0..count {
        map.insert(format!("key-{index}"), json!(format!("value-{index}")));
    }
    Value::Object(map)
}

#[tokio::test]
async fn tag_resource_writes_what_every_reader_reports() {
    let router = harness();

    // The rows differ along what the rule generalises over: how many pairs the request
    // carries, and the length and character class of its keys and values. `TagValue`'s
    // modelled length starts at 0, so an empty value is a legal pair rather than an
    // absent one; `TagKey`/`TagValue` admit letters, marks, digits and `_.:/=+-@`
    // (`smithy_query(query="shape", shape="TagMap")`).
    let cases = vec![
        ("one pair", json!({ "env": "test" })),
        (
            "several pairs",
            json!({ "env": "prod", "team": "claims", "owner": "platform" }),
        ),
        ("an empty value", json!({ "env": "" })),
        (
            "every punctuation character the pattern admits",
            json!({ "a_b.c:d/e=f+g-h@i": "j_k.l:m/n=o+p-q@r" }),
        ),
        (
            "a key and a value at their modelled maximum length",
            pair(&"k".repeat(128), &"v".repeat(256)),
        ),
        ("non-ASCII letters and digits", json!({ "équipe-1": "été-2" })),
        ("the largest map the ceiling admits", tag_map_of(200)),
        ("no pairs at all", json!({})),
    ];

    for (case, tags) in cases {
        let adapter_id = create_adapter(&router, case, &create_request("tag-target", None)).await;
        let arn = state::adapter_arn(REGION, &adapter_id);

        let (status, body) = call(
            &router,
            "TagResource",
            &json!({ "ResourceARN": arn, "Tags": tags }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from TagResource, got {status} with {body}"
        );

        let expected = expected_tags(&tags);
        let listed = list_tags(&router, case, &arn).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: ListTagsForResource must report the written tags, got {listed:?}"
        );
        // The same tags through the adapter's own reader: one stored set, two readers.
        let from_get_adapter = adapter_tags(&router, case, &adapter_id).await;
        assert_eq!(
            from_get_adapter, expected,
            "case {case:?}: GetAdapter must report the written tags, got {from_get_adapter:?}"
        );
        // A read changes nothing a later caller sees.
        let again = list_tags(&router, case, &arn).await;
        assert_eq!(
            again, expected,
            "case {case:?}: a second ListTagsForResource must report the same tags, got {again:?}"
        );
    }
}

/// One row of the merge table: the tags a second `TagResource` carries, and what the
/// resource must hold afterwards.
struct MergeCase {
    name: &'static str,
    second: Value,
    expect: Value,
}

#[tokio::test]
async fn tag_resource_merges_into_the_stored_tags() {
    let router = harness();

    // The adapter starts from the tags its CREATE carried, so the merge is asserted over
    // tags this operation did not write either.
    let initial = json!({ "env": "test", "team": "claims" });

    // The rows differ along how the second request's keys OVERLAP the stored ones: not
    // at all, partly, wholly, and with no keys at all.
    let cases = vec![
        MergeCase {
            name: "a disjoint key is added",
            second: json!({ "owner": "platform" }),
            expect: json!({ "env": "test", "team": "claims", "owner": "platform" }),
        },
        MergeCase {
            name: "an overlapping key takes the new value",
            second: json!({ "env": "prod" }),
            expect: json!({ "env": "prod", "team": "claims" }),
        },
        MergeCase {
            name: "overlapping and new keys together",
            second: json!({ "team": "fraud", "owner": "platform" }),
            expect: json!({ "env": "test", "team": "fraud", "owner": "platform" }),
        },
        MergeCase {
            name: "every stored key is overwritten",
            second: json!({ "env": "prod", "team": "fraud" }),
            expect: json!({ "env": "prod", "team": "fraud" }),
        },
        MergeCase {
            name: "an empty map leaves the stored tags alone",
            second: json!({}),
            expect: initial.clone(),
        },
        MergeCase {
            name: "the same pairs again leave the stored tags alone",
            second: initial.clone(),
            expect: initial.clone(),
        },
    ];

    for case in cases {
        let name = case.name;
        let adapter_id =
            create_adapter(&router, name, &create_request("merge-target", Some(&initial))).await;
        let arn = state::adapter_arn(REGION, &adapter_id);

        for attempt in 1..=2 {
            let (status, body) = call(
                &router,
                "TagResource",
                &json!({ "ResourceARN": arn, "Tags": case.second }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {name:?}: attempt {attempt} of TagResource must succeed, \
                 got {status} with {body}"
            );
            // Idempotent: the second attempt is asserted against the SAME expectation,
            // so a merge that accumulated would be caught here.
            let listed = list_tags(&router, name, &arn).await;
            assert_eq!(
                listed,
                expected_tags(&case.expect),
                "case {name:?}: after attempt {attempt} the resource must hold the merged \
                 tags, got {listed:?}"
            );
        }
    }
}

/// One row of the untag table: which keys the request names, and what survives.
struct UntagCase {
    name: &'static str,
    keys: Value,
    expect: Value,
}

#[tokio::test]
async fn untag_resource_removes_exactly_the_named_keys() {
    let router = harness();

    let initial = json!({ "env": "test", "team": "claims", "owner": "platform" });

    // The rows differ along which keys the request names: one, several, all, none, and
    // keys the resource does not hold.
    let cases = vec![
        UntagCase {
            name: "one held key",
            keys: json!(["env"]),
            expect: json!({ "team": "claims", "owner": "platform" }),
        },
        UntagCase {
            name: "several held keys",
            keys: json!(["env", "owner"]),
            expect: json!({ "team": "claims" }),
        },
        UntagCase {
            name: "every held key",
            keys: json!(["env", "team", "owner"]),
            expect: json!({}),
        },
        UntagCase {
            name: "a key the resource does not hold",
            keys: json!(["absent"]),
            expect: initial.clone(),
        },
        UntagCase {
            name: "a held key beside one the resource does not hold",
            keys: json!(["team", "absent"]),
            expect: json!({ "env": "test", "owner": "platform" }),
        },
        UntagCase {
            name: "no keys at all",
            keys: json!([]),
            expect: initial.clone(),
        },
        UntagCase {
            name: "the same key twice",
            keys: json!(["env", "env"]),
            expect: json!({ "team": "claims", "owner": "platform" }),
        },
    ];

    for case in cases {
        let name = case.name;
        let adapter_id =
            create_adapter(&router, name, &create_request("untag-target", Some(&initial))).await;
        let arn = state::adapter_arn(REGION, &adapter_id);

        for attempt in 1..=2 {
            let (status, body) = call(
                &router,
                "UntagResource",
                &json!({ "ResourceARN": arn, "TagKeys": case.keys }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {name:?}: attempt {attempt} of UntagResource must succeed, \
                 got {status} with {body}"
            );
            // Removal is idempotent: the second attempt must leave the same set.
            let listed = list_tags(&router, name, &arn).await;
            assert_eq!(
                listed,
                expected_tags(&case.expect),
                "case {name:?}: after attempt {attempt} the resource must hold the surviving \
                 tags, got {listed:?}"
            );
        }
    }
}

#[tokio::test]
async fn every_spelling_of_one_adapter_arn_reaches_one_resource() {
    let router = harness();

    let adapter_id = create_adapter(
        &router,
        "arn spellings",
        &create_request("arn-target", None),
    )
    .await;

    // The resource is resolved from the ARN's resource TYPE and IDENTIFIER: this
    // emulator serves one account, so the partition, region and account fields take no
    // part in it, and the leading `/` Amazon Textract writes an adapter's resource field
    // with (`/adapters/{AdapterId}`) is a separator rather than part of the type.
    let spellings = [
        (
            "the canonical form",
            state::adapter_arn(REGION, &adapter_id),
        ),
        (
            "without the leading slash",
            format!("arn:aws:textract:{REGION}:000000000000:adapters/{adapter_id}"),
        ),
        (
            "another partition",
            format!("arn:aws-us-gov:textract:{REGION}:000000000000:/adapters/{adapter_id}"),
        ),
        (
            "another region",
            format!("arn:aws:textract:eu-west-1:000000000000:/adapters/{adapter_id}"),
        ),
        (
            "another account",
            format!("arn:aws:textract:{REGION}:123456789012:/adapters/{adapter_id}"),
        ),
        (
            "no region at all",
            format!("arn:aws:textract::000000000000:/adapters/{adapter_id}"),
        ),
    ];

    for (index, (case, arn)) in spellings.iter().enumerate() {
        // Each spelling writes its own key, and every spelling must then see every key
        // written so far: one stored set behind all of them.
        let key = format!("spelling-{index}");
        let (status, body) = call(
            &router,
            "TagResource",
            &json!({ "ResourceARN": arn, "Tags": pair(&key, case) }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: TagResource through {arn:?} must succeed, got {status} with {body}"
        );

        let through_canonical = list_tags(
            &router,
            case,
            &state::adapter_arn(REGION, &adapter_id),
        )
        .await;
        assert_eq!(
            through_canonical.len(),
            index + 1,
            "case {case:?}: the canonical ARN must see every key written so far, \
             got {through_canonical:?}"
        );
        let through_spelling = list_tags(&router, case, arn).await;
        assert_eq!(
            through_spelling, through_canonical,
            "case {case:?}: {arn:?} must report the same tags as the canonical ARN, \
             got {through_spelling:?}"
        );
    }
}

#[tokio::test]
async fn an_arn_naming_no_resource_is_refused_by_every_tagging_operation() {
    let router = harness();

    let live = create_adapter(
        &router,
        "a live adapter",
        &create_request("still-here", None),
    )
    .await;
    let deleted = create_adapter(&router, "a deleted adapter", &create_request("gone", None)).await;
    let (status, body) = call(&router, "DeleteAdapter", &json!({ "AdapterId": deleted })).await;
    assert_eq!(
        status, 200,
        "DeleteAdapter of {deleted:?} must succeed, got {status} with {body}"
    );

    // The rows differ along WHY the ARN names nothing: an id no create minted, an id a
    // delete removed, a nested resource of a live adapter (a resource this service mints
    // none of while `CreateAdapterVersion` is a stub), and another Textract resource
    // type.
    let cases = vec![
        (
            "an adapter id no create minted",
            state::adapter_arn(REGION, "0123456789abcdef0123456789abcdef"),
        ),
        (
            "an adapter a delete removed",
            state::adapter_arn(REGION, &deleted),
        ),
        (
            "a nested resource of a live adapter",
            format!("arn:aws:textract:{REGION}:000000000000:/adapters/{live}/versions/1"),
        ),
        (
            "another Textract resource type",
            format!("arn:aws:textract:{REGION}:000000000000:/jobs/{live}"),
        ),
    ];

    for (case, arn) in cases {
        for (operation, request) in tagging_requests(&arn) {
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: {operation} of {arn:?} must answer HTTP 400, \
                 got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: {operation} of {arn:?} must answer the modelled \
                 ResourceNotFoundException, got {body}"
            );
        }
    }

    // The live adapter is untouched by every refusal above.
    let listed = list_tags(&router, "a live adapter", &state::adapter_arn(REGION, &live)).await;
    assert!(
        listed.is_empty(),
        "case {:?}: the live adapter must still hold no tags, got {listed:?}",
        "a live adapter"
    );
}

#[tokio::test]
async fn a_value_that_is_no_textract_arn_is_refused_by_every_tagging_operation() {
    let router = harness();

    let adapter_id = create_adapter(
        &router,
        "the adapter that must not move",
        &create_request("untouched", Some(&json!({ "env": "test" }))),
    )
    .await;
    let arn = state::adapter_arn(REGION, &adapter_id);

    // The rows differ along WHICH part of the ARN syntax the value fails: the `arn`
    // prefix, the field count, the partition, the service, and the resource field's type
    // or identifier.
    let cases = vec![
        ("not an ARN at all", "untouched".to_string()),
        (
            "the adapter id alone",
            adapter_id.clone(),
        ),
        (
            "too few fields",
            format!("arn:aws:textract:{REGION}:000000000000"),
        ),
        (
            "an empty partition",
            format!("arn::textract:{REGION}:000000000000:/adapters/{adapter_id}"),
        ),
        (
            "another service's resource",
            format!("arn:aws:comprehend:{REGION}:000000000000:/adapters/{adapter_id}"),
        ),
        (
            "an empty resource field",
            format!("arn:aws:textract:{REGION}:000000000000:"),
        ),
        (
            "a resource field with no identifier",
            format!("arn:aws:textract:{REGION}:000000000000:/adapters"),
        ),
        (
            "a resource field with an empty identifier",
            format!("arn:aws:textract:{REGION}:000000000000:/adapters/"),
        ),
    ];

    for (case, candidate) in cases {
        for (operation, request) in tagging_requests(&candidate) {
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: {operation} of {candidate:?} must answer HTTP 400, \
                 got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidParameterException",
                "case {case:?}: {operation} of {candidate:?} must answer the modelled \
                 InvalidParameterException, got {body}"
            );
        }
    }

    let listed = list_tags(&router, "the adapter that must not move", &arn).await;
    assert_eq!(
        listed,
        expected_tags(&json!({ "env": "test" })),
        "no refused request may change the adapter's tags, got {listed:?}"
    );
}

/// One request per tagging operation against `arn`, so a rule about which ARNs name
/// something is asserted on all three rather than on the write alone.
fn tagging_requests(arn: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "TagResource",
            json!({ "ResourceARN": arn, "Tags": { "env": "test" } }),
        ),
        (
            "UntagResource",
            json!({ "ResourceARN": arn, "TagKeys": ["env"] }),
        ),
        ("ListTagsForResource", json!({ "ResourceARN": arn })),
    ]
}

#[tokio::test]
async fn the_stored_tag_set_is_bounded_by_the_modelled_tagmap_length() {
    let router = harness();

    let adapter_id = create_adapter(
        &router,
        "the tag ceiling",
        &create_request("ceiling-target", None),
    )
    .await;
    let arn = state::adapter_arn(REGION, &adapter_id);

    // Exactly at the published bound: `TagMap`'s modelled maximum length, which
    // `state::MAX_TAGS_PER_RESOURCE` carries.
    let at_bound = tag_map_of(state::MAX_TAGS_PER_RESOURCE);
    let (status, body) = call(
        &router,
        "TagResource",
        &json!({ "ResourceARN": arn, "Tags": at_bound }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: a merge landing exactly on the bound must succeed, got {status} with {body}",
        "at the bound"
    );
    let listed = list_tags(&router, "at the bound", &arn).await;
    assert_eq!(
        listed.len(),
        state::MAX_TAGS_PER_RESOURCE,
        "case {:?}: the resource must hold every pair written, got {} pairs",
        "at the bound",
        listed.len()
    );

    // One past it: a key the resource does not hold would make the stored set larger
    // than a `TagMap` can express, so the whole request is refused and nothing changes.
    let (status, body) = call(
        &router,
        "TagResource",
        &json!({ "ResourceARN": arn, "Tags": { "one-too-many": "x" } }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {:?}: a merge one past the bound must answer HTTP 400, got {status} with {body}",
        "one past the bound"
    );
    assert_eq!(
        error_code(&body),
        "ServiceQuotaExceededException",
        "case {:?}: a merge one past the bound must answer the modelled \
         ServiceQuotaExceededException, got {body}",
        "one past the bound"
    );
    let after = list_tags(&router, "one past the bound", &arn).await;
    assert_eq!(
        after, listed,
        "case {:?}: the refused merge must change nothing, got {after:?}",
        "one past the bound"
    );

    // A request that only OVERWRITES keys the resource already holds does not grow it,
    // so it is accepted even with the set at the bound.
    let (status, body) = call(
        &router,
        "TagResource",
        &json!({ "ResourceARN": arn, "Tags": { "key-0": "rewritten" } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: overwriting a held key at the bound must succeed, got {status} with {body}",
        "an overwrite at the bound"
    );
    let after = list_tags(&router, "an overwrite at the bound", &arn).await;
    assert_eq!(
        after.len(),
        state::MAX_TAGS_PER_RESOURCE,
        "case {:?}: an overwrite must not change how many tags the resource holds, got {} pairs",
        "an overwrite at the bound",
        after.len()
    );
    assert_eq!(
        after.get("key-0").map(String::as_str),
        Some("rewritten"),
        "case {:?}: the overwritten key must carry the new value, got {after:?}",
        "an overwrite at the bound"
    );
}

#[tokio::test]
async fn tags_belong_to_one_resource() {
    let router = harness();

    // Three adapters, so a write through one ARN is asserted not to reach the others.
    let mut adapters = Vec::new();
    for index in 0..3 {
        let case = format!("adapter {index}");
        let adapter_id =
            create_adapter(&router, &case, &create_request(&format!("tenant-{index}"), None)).await;
        adapters.push(adapter_id);
    }

    for (index, adapter_id) in adapters.iter().enumerate() {
        let case = format!("adapter {index}");
        let arn = state::adapter_arn(REGION, adapter_id);
        let (status, body) = call(
            &router,
            "TagResource",
            &json!({ "ResourceARN": arn, "Tags": { "owner": format!("tenant-{index}") } }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: TagResource must succeed, got {status} with {body}"
        );
    }

    for (index, adapter_id) in adapters.iter().enumerate() {
        let case = format!("adapter {index}");
        let listed = list_tags(&router, &case, &state::adapter_arn(REGION, adapter_id)).await;
        assert_eq!(
            listed,
            expected_tags(&json!({ "owner": format!("tenant-{index}") })),
            "case {case:?}: each adapter must hold only its own tags, got {listed:?}"
        );
    }

    // Untagging one adapter leaves the others as they were.
    let arn = state::adapter_arn(REGION, &adapters[0]);
    let (status, body) = call(
        &router,
        "UntagResource",
        &json!({ "ResourceARN": arn, "TagKeys": ["owner"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "UntagResource must succeed, got {status} with {body}"
    );
    let untagged = list_tags(&router, "the untagged adapter", &arn).await;
    assert!(
        untagged.is_empty(),
        "the untagged adapter must hold no tags, got {untagged:?}"
    );
    for (index, adapter_id) in adapters.iter().enumerate().skip(1) {
        let case = format!("adapter {index}");
        let listed = list_tags(&router, &case, &state::adapter_arn(REGION, adapter_id)).await;
        assert_eq!(
            listed,
            expected_tags(&json!({ "owner": format!("tenant-{index}") })),
            "case {case:?}: an untag of another adapter must change nothing here, got {listed:?}"
        );
    }
}
