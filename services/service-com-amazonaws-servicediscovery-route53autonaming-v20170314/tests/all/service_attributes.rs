// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the service-level attribute triple:
//! `UpdateServiceAttributes` upserts into the stored map, `GetServiceAttributes`
//! reports what is stored, `DeleteServiceAttributes` removes the keys it names.
//!
//! The rule under test: a service carries ONE attribute map; an update stores every
//! key the request names (replacing any value stored for it) and leaves every key it
//! does not name alone, a delete removes every key it names and ignores the rest, and
//! a read answers exactly the stored map — for any key or value the modelled
//! `ServiceAttributeKey`/`ServiceAttributeValue` length bounds admit, and bounded by
//! the documented quota of 30 attributes per service.
//!
//! Every request goes through `MultiServiceRouter::dispatch`, i.e. the same path the
//! emulator serves in production (`awsJson1_1`: `POST /` with `X-Amz-Target`).

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloud_map_core::{emulator, failure, namespace, ok, service};

/// The `length` bounds the model puts on `ServiceAttributeKey` (0..255) and
/// `ServiceAttributeValue` (0..1024), from
/// `smithy_query(query="shape",shape="ServiceAttributeKey,ServiceAttributeValue")`.
const MAX_KEY_LEN: usize = 255;
const MAX_VALUE_LEN: usize = 1024;

/// The per-service attribute quota, from the documentation of
/// `ServiceAttributes$Attributes`: "You can specify a total of 30 attributes."
const MAX_ATTRIBUTES: usize = 30;

/// A fresh service to hang attributes off, returning `(service id, service ARN)`.
async fn attributed_service(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let (_, namespace_id) = namespace(router, &format!("{name}-ns")).await;
    let service_id = service(router, &namespace_id, name).await;
    let read = ok(router, "GetService", json!({ "Id": service_id })).await;
    let arn = read["Service"]["Arn"]
        .as_str()
        .expect("a stored service carries its ARN")
        .to_string();
    (service_id, arn)
}

/// The stored attributes of `service_ref`, read back through `GetServiceAttributes`.
async fn stored(router: &MultiServiceRouter, service_ref: &str) -> BTreeMap<String, String> {
    let read = ok(
        router,
        "GetServiceAttributes",
        json!({ "ServiceId": service_ref }),
    )
    .await;
    read["ServiceAttributes"]["Attributes"]
        .as_object()
        .map(|map| {
            map.iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value
                            .as_str()
                            .expect("a ServiceAttributeValue is a string")
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A JSON object from key/value pairs.
fn object<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Value {
    Value::Object(
        pairs
            .into_iter()
            .map(|(key, value)| (key.to_string(), Value::String(value.to_string())))
            .collect(),
    )
}

/// An expected map from key/value pairs.
fn expected<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> BTreeMap<String, String> {
    pairs
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// Whatever a request stores, the read answers byte for byte — across value classes
/// (ASCII, unicode, a base64-encoded binary blob, whitespace, an empty value) and at
/// the declared length bounds of key and value.
#[tokio::test]
async fn every_admissible_key_and_value_round_trips_unchanged() {
    let router = emulator();
    let long_key = "k".repeat(MAX_KEY_LEN);
    let long_value = "v".repeat(MAX_VALUE_LEN);

    for (case, key, value) in [
        ("plain ascii", "Port", "80"),
        // The blob the SDK hands over is base64 text, padding included.
        ("base64 binary blob", "payload", "AAECA/3+/zo9PT0="),
        ("unicode key and value", "ключ-ünïcode", "日本語 — ✅ 🚀"),
        ("whitespace and punctuation", "note", "a b\tc  d,;:'\"{}[]"),
        ("an empty value", "empty", ""),
        ("a value that is only whitespace", "blank", "   "),
        ("key at the declared 255 maximum", long_key.as_str(), "edge"),
        ("value at the declared 1024 maximum", "big", long_value.as_str()),
    ] {
        // A fresh service per row, so each row asserts the whole stored map.
        let name = format!(
            "roundtrip-{}",
            case.chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                .collect::<String>()
        );
        let (service_id, service_arn) = attributed_service(&router, &name).await;

        let read = ok(
            &router,
            "GetServiceAttributes",
            json!({ "ServiceId": service_id }),
        )
        .await;
        assert!(
            read["ServiceAttributes"]["Attributes"].is_null(),
            "case {case:?}: a service with no attributes reports none, got {read}"
        );

        ok(
            &router,
            "UpdateServiceAttributes",
            json!({ "ServiceId": service_id, "Attributes": object([(key, value)]) }),
        )
        .await;

        let actual = stored(&router, &service_id).await;
        assert_eq!(
            actual,
            expected([(key, value)]),
            "case {case:?}: expected the stored pair back, got {actual:?}"
        );

        let read = ok(
            &router,
            "GetServiceAttributes",
            json!({ "ServiceId": service_id }),
        )
        .await;
        assert_eq!(
            read["ServiceAttributes"]["ServiceArn"],
            json!(service_arn),
            "case {case:?}: the attributes are reported against the stored service ARN, got {read}"
        );
    }
}

/// One past either declared length bound is refused and stores nothing, as is a
/// collection below its declared minimum.
///
/// Each row expects `InvalidInput`, whichever side refuses it: the handler answers it
/// for the one bound the centralized request walk does not see — a map KEY's length
/// ("Map *keys* are not covered", `vendor/smithy-schema-server/src/constraints.rs`) —
/// and the walk's own refusals (a collection's entry count, a map VALUE's own length)
/// are worded with the same code by this service's `override_refusal` seam, because
/// `InvalidInput` is the only client-input error this model declares ("a string value
/// might exceed length constraints").
#[tokio::test]
async fn a_key_or_value_past_its_declared_bound_is_refused() {
    let router = emulator();
    let (service_id, _) = attributed_service(&router, "bounds").await;

    for (case, key, value, want) in [
        (
            "one past the key maximum",
            "k".repeat(MAX_KEY_LEN + 1),
            "v".to_string(),
            "InvalidInput",
        ),
        (
            "one past the value maximum",
            "k".to_string(),
            "v".repeat(MAX_VALUE_LEN + 1),
            "InvalidInput",
        ),
        (
            "a multi-byte value past the maximum",
            "k".to_string(),
            "é".repeat(MAX_VALUE_LEN + 1),
            "InvalidInput",
        ),
    ] {
        let (status, code) = failure(
            &router,
            "UpdateServiceAttributes",
            json!({
                "ServiceId": service_id,
                "Attributes": object([(key.as_str(), value.as_str())]),
            }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, want),
            "case {case:?}: a value outside the declared length is refused, got {status} {code}"
        );
    }

    // At the bounds the same request is accepted, so the refusals above are the bound
    // and not the member.
    let at_bounds = [
        ("k".repeat(MAX_KEY_LEN), "v".to_string()),
        ("at-value-bound".to_string(), "v".repeat(MAX_VALUE_LEN)),
        ("multi-byte".to_string(), "é".repeat(MAX_VALUE_LEN / 2)),
    ];
    ok(
        &router,
        "UpdateServiceAttributes",
        json!({
            "ServiceId": service_id,
            "Attributes": object(at_bounds.iter().map(|(k, v)| (k.as_str(), v.as_str()))),
        }),
    )
    .await;

    for (case, operation, body, want) in [
        (
            "an empty attribute map",
            "UpdateServiceAttributes",
            json!({ "ServiceId": service_id, "Attributes": {} }),
            "InvalidInput",
        ),
        (
            "an empty key list",
            "DeleteServiceAttributes",
            json!({ "ServiceId": service_id, "Attributes": [] }),
            "InvalidInput",
        ),
        (
            "a key list entry past the key maximum",
            "DeleteServiceAttributes",
            json!({ "ServiceId": service_id, "Attributes": ["k".repeat(MAX_KEY_LEN + 1)] }),
            "InvalidInput",
        ),
    ] {
        let (status, code) = failure(&router, operation, body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, want),
            "case {case:?} on {operation}: expected the declared bound to be refused, got \
             {status} {code}"
        );
    }

    let actual = stored(&router, &service_id).await;
    assert_eq!(
        actual,
        expected(at_bounds.iter().map(|(k, v)| (k.as_str(), v.as_str()))),
        "a refused request stores and removes nothing, got {} entries",
        actual.len()
    );
}

/// An update is an upsert: it stores the keys it names, replaces the value of a key it
/// names again, and leaves every other stored key alone.
#[tokio::test]
async fn an_update_stores_the_named_keys_and_leaves_the_rest_alone() {
    let router = emulator();

    for (index, (case, first, second, want)) in [
        (
            "disjoint keys merge",
            vec![("a", "1")],
            vec![("b", "2")],
            vec![("a", "1"), ("b", "2")],
        ),
        (
            "a named key is replaced",
            vec![("a", "1"), ("b", "2")],
            vec![("a", "9")],
            vec![("a", "9"), ("b", "2")],
        ),
        (
            "replacing with the empty value keeps the key",
            vec![("a", "1")],
            vec![("a", "")],
            vec![("a", "")],
        ),
        (
            "a mixed request replaces and adds in one call",
            vec![("a", "1"), ("b", "2")],
            vec![("b", "22"), ("c", "3")],
            vec![("a", "1"), ("b", "22"), ("c", "3")],
        ),
        (
            "the same request twice is the same stored map",
            vec![("a", "1"), ("b", "2")],
            vec![("a", "1"), ("b", "2")],
            vec![("a", "1"), ("b", "2")],
        ),
        (
            "a many-key request stores every pair",
            (0..10)
                .map(|index| (KEYS[index], VALUES[index]))
                .collect::<Vec<_>>(),
            vec![("k0", "replaced")],
            std::iter::once(("k0", "replaced"))
                .chain((1..10).map(|index| (KEYS[index], VALUES[index])))
                .collect::<Vec<_>>(),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let name = format!("upsert-{index}");
        let (service_id, _) = attributed_service(&router, &name).await;

        for (step, pairs) in [("first", &first), ("second", &second)] {
            ok(
                &router,
                "UpdateServiceAttributes",
                json!({
                    "ServiceId": service_id,
                    "Attributes": object(pairs.iter().copied()),
                }),
            )
            .await;
            let actual = stored(&router, &service_id).await;
            assert!(
                pairs
                    .iter()
                    .all(|(key, value)| actual.get(*key).map(String::as_str) == Some(*value)),
                "case {case:?} after the {step} update: every named pair must be stored, \
                 got {actual:?}"
            );
        }

        let actual = stored(&router, &service_id).await;
        assert_eq!(
            actual,
            expected(want.iter().copied()),
            "case {case:?}: expected the merged map, got {actual:?}"
        );
    }
}

/// Keys and values for the many-key rows, named so a row can index them.
const KEYS: [&str; 10] = ["k0", "k1", "k2", "k3", "k4", "k5", "k6", "k7", "k8", "k9"];
const VALUES: [&str; 10] = ["v0", "v1", "v2", "v3", "v4", "v5", "v6", "v7", "v8", "v9"];

/// A delete removes exactly the keys it names; a key the service does not carry is not
/// an error, so the same delete repeated leaves the same stored map.
#[tokio::test]
async fn a_delete_removes_the_named_keys_and_ignores_the_unknown_ones() {
    let router = emulator();

    for (index, (case, stored_pairs, delete, want)) in [
        (
            "one known key",
            vec![("a", "1"), ("b", "2")],
            vec!["a"],
            vec![("b", "2")],
        ),
        (
            "an unknown key alone",
            vec![("a", "1")],
            vec!["absent"],
            vec![("a", "1")],
        ),
        (
            "known and unknown keys together",
            vec![("a", "1"), ("b", "2")],
            vec!["a", "absent"],
            vec![("b", "2")],
        ),
        (
            "every key",
            vec![("a", "1"), ("b", "2"), ("c", "3")],
            vec!["a", "b", "c"],
            vec![],
        ),
        (
            "a unicode key",
            vec![("ключ", "1"), ("b", "2")],
            vec!["ключ"],
            vec![("b", "2")],
        ),
        (
            "a key the request spells with different case",
            vec![("Key", "1")],
            vec!["key"],
            vec![("Key", "1")],
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let name = format!("delete-{index}");
        let (service_id, _) = attributed_service(&router, &name).await;
        ok(
            &router,
            "UpdateServiceAttributes",
            json!({
                "ServiceId": service_id,
                "Attributes": object(stored_pairs.iter().copied()),
            }),
        )
        .await;

        // Twice: the second call names keys that are already gone, which is not an
        // error and must not change the stored map.
        for attempt in 1..=2 {
            ok(
                &router,
                "DeleteServiceAttributes",
                json!({ "ServiceId": service_id, "Attributes": delete }),
            )
            .await;
            let actual = stored(&router, &service_id).await;
            assert_eq!(
                actual,
                expected(want.iter().copied()),
                "case {case:?} on attempt {attempt}: expected the remaining keys, got {actual:?}"
            );
        }

        if want.is_empty() {
            let read = ok(
                &router,
                "GetServiceAttributes",
                json!({ "ServiceId": service_id }),
            )
            .await;
            assert!(
                read["ServiceAttributes"]["Attributes"].is_null(),
                "case {case:?}: a service left with no attributes reports none, got {read}"
            );
        }
    }
}

/// A service holds up to the documented 30 attributes; the request that would push the
/// total past it is refused with `ServiceAttributesLimitExceededException` and stores
/// nothing, while a request that only replaces values at the quota is accepted.
#[tokio::test]
async fn the_total_attributes_a_service_holds_is_capped_at_the_documented_quota() {
    let router = emulator();
    let (service_id, _) = attributed_service(&router, "quota").await;

    // Up to the quota, in two requests, so the cap is on the TOTAL rather than on one
    // request.
    for chunk in [0..MAX_ATTRIBUTES / 2, MAX_ATTRIBUTES / 2..MAX_ATTRIBUTES] {
        let pairs: Vec<(String, String)> = chunk
            .map(|index| (format!("key-{index}"), format!("value-{index}")))
            .collect();
        ok(
            &router,
            "UpdateServiceAttributes",
            json!({
                "ServiceId": service_id,
                "Attributes": object(pairs.iter().map(|(k, v)| (k.as_str(), v.as_str()))),
            }),
        )
        .await;
    }
    let at_quota = stored(&router, &service_id).await;
    assert_eq!(
        at_quota.len(),
        MAX_ATTRIBUTES,
        "the quota itself is accepted, got {at_quota:?}"
    );

    // Replacing values at the quota keeps the total where it is, so it is accepted.
    ok(
        &router,
        "UpdateServiceAttributes",
        json!({ "ServiceId": service_id, "Attributes": { "key-0": "replaced" } }),
    )
    .await;
    let actual = stored(&router, &service_id).await;
    assert_eq!(
        actual.get("key-0").map(String::as_str),
        Some("replaced"),
        "a replacement at the quota is stored, got {actual:?}"
    );
    assert_eq!(
        actual.len(),
        MAX_ATTRIBUTES,
        "a replacement does not grow the map, got {actual:?}"
    );

    // One new key past the quota is refused, and the stored map is untouched.
    let (status, code) = failure(
        &router,
        "UpdateServiceAttributes",
        json!({ "ServiceId": service_id, "Attributes": { "one-too-many": "x" } }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ServiceAttributesLimitExceededException"),
        "one attribute past the quota is refused with the modelled error, got {status} {code}"
    );
    let after = stored(&router, &service_id).await;
    assert_eq!(
        after, actual,
        "a refused update stores nothing, got {after:?}"
    );

    // Freeing a slot lets the same request through, so the refusal was the quota and
    // not the key.
    ok(
        &router,
        "DeleteServiceAttributes",
        json!({ "ServiceId": service_id, "Attributes": ["key-1"] }),
    )
    .await;
    ok(
        &router,
        "UpdateServiceAttributes",
        json!({ "ServiceId": service_id, "Attributes": { "one-too-many": "x" } }),
    )
    .await;
    let after = stored(&router, &service_id).await;
    assert_eq!(
        after.get("one-too-many").map(String::as_str),
        Some("x"),
        "with a slot free the same key is stored, got {after:?}"
    );
    assert_eq!(
        after.len(),
        MAX_ATTRIBUTES,
        "the map is back at the quota, got {after:?}"
    );

    // A single request past the quota is refused too — by the walk, which refuses the
    // map's declared entry maximum before the handler sees it, worded with this
    // service's own `InvalidInput` by the `override_refusal` seam.
    let pairs: Vec<(String, String)> = (0..=MAX_ATTRIBUTES)
        .map(|index| (format!("bulk-{index}"), index.to_string()))
        .collect();
    let (status, code) = failure(
        &router,
        "UpdateServiceAttributes",
        json!({
            "ServiceId": service_id,
            "Attributes": object(pairs.iter().map(|(k, v)| (k.as_str(), v.as_str()))),
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidInput"),
        "a single request of {} attributes is refused, got {status} {code}",
        MAX_ATTRIBUTES + 1
    );
}

/// Two updates of the same key: the later one wins, whether they are sequential or
/// concurrent, and keys only one of them names survive either way.
#[tokio::test]
async fn the_last_update_of_a_key_is_the_one_a_read_sees() {
    let router = emulator();
    let (service_id, _) = attributed_service(&router, "last-write").await;

    // Sequential: each write replaces the previous value of the shared key.
    for value in ["first", "second", "third", ""] {
        ok(
            &router,
            "UpdateServiceAttributes",
            json!({ "ServiceId": service_id, "Attributes": { "shared": value } }),
        )
        .await;
        let actual = stored(&router, &service_id).await;
        assert_eq!(
            actual.get("shared").map(String::as_str),
            Some(value),
            "the last write of a key is what a read sees, got {actual:?}"
        );
    }

    // Concurrent: every call is accepted, exactly one value survives for the shared
    // key, and each caller's own key survives alongside it.
    let writers = ["alpha", "beta", "gamma", "delta"];
    let updates = writers.map(|writer| {
        ok(
            &router,
            "UpdateServiceAttributes",
            json!({
                "ServiceId": service_id,
                "Attributes": { "shared": writer, writer: "own" },
            }),
        )
    });
    let [a, b, c, d] = updates;
    let (_, _, _, _) = tokio::join!(a, b, c, d);

    let actual = stored(&router, &service_id).await;
    let winner = actual
        .get("shared")
        .map(String::as_str)
        .expect("the shared key survives every concurrent write");
    assert!(
        writers.contains(&winner),
        "the surviving value must be one a writer sent, got {winner:?} in {actual:?}"
    );
    for writer in writers {
        assert_eq!(
            actual.get(writer).map(String::as_str),
            Some("own"),
            "case {writer:?}: a key only one writer named still survives, got {actual:?}"
        );
    }
}

/// A service can be named by its id or by its ARN, on each of the three operations.
#[tokio::test]
async fn attributes_are_reachable_by_service_id_and_by_service_arn() {
    let router = emulator();
    let (service_id, service_arn) = attributed_service(&router, "by-reference").await;

    for (case, reference) in [("the id", service_id.clone()), ("the ARN", service_arn)] {
        ok(
            &router,
            "UpdateServiceAttributes",
            json!({ "ServiceId": reference, "Attributes": { case: "set" } }),
        )
        .await;
        let actual = stored(&router, &reference).await;
        assert_eq!(
            actual.get(case).map(String::as_str),
            Some("set"),
            "case {case:?}: a write addressed by this reference is readable by it, got {actual:?}"
        );
        // The two references name ONE service, so the read by id sees it too.
        let by_id = stored(&router, &service_id).await;
        assert_eq!(
            by_id, actual,
            "case {case:?}: both references reach the same stored map, got {by_id:?}"
        );

        ok(
            &router,
            "DeleteServiceAttributes",
            json!({ "ServiceId": reference, "Attributes": [case] }),
        )
        .await;
        let actual = stored(&router, &service_id).await;
        assert!(
            !actual.contains_key(case),
            "case {case:?}: a delete addressed by this reference is applied, got {actual:?}"
        );
    }
}

/// A service reference nothing stored answers `ServiceNotFound` on all three
/// operations, and a service that exists in another namespace is not substituted.
#[tokio::test]
async fn an_unresolvable_service_answers_service_not_found() {
    let router = emulator();
    let (service_id, _) = attributed_service(&router, "present").await;

    for (case, reference) in [
        ("an id no service carries", "srv-000000000000000"),
        (
            "an ARN no service carries",
            "arn:aws:servicediscovery:us-west-2:000000000000:service/srv-0",
        ),
        ("an empty reference", ""),
        ("a namespace id, not a service id", "ns-0000000000000000"),
    ] {
        for (operation, body) in [
            (
                "GetServiceAttributes",
                json!({ "ServiceId": reference }),
            ),
            (
                "UpdateServiceAttributes",
                json!({ "ServiceId": reference, "Attributes": { "a": "1" } }),
            ),
            (
                "DeleteServiceAttributes",
                json!({ "ServiceId": reference, "Attributes": ["a"] }),
            ),
        ] {
            let (status, code) = failure(&router, operation, body).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ServiceNotFound"),
                "case {case:?} on {operation}: expected 400 ServiceNotFound, got {status} {code}"
            );
        }
    }

    // The service that does exist still carries nothing, so no refused call wrote.
    let actual = stored(&router, &service_id).await;
    assert!(
        actual.is_empty(),
        "a refused call must not write to another service, got {actual:?}"
    );
}

/// Attributes belong to ONE service: writing to one leaves every other service's map
/// alone, and reading is free of side effects.
#[tokio::test]
async fn attributes_are_per_service_and_reads_change_nothing() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "per-service-ns").await;
    let first = service(&router, &namespace_id, "first-svc").await;
    let second = service(&router, &namespace_id, "second-svc").await;

    ok(
        &router,
        "UpdateServiceAttributes",
        json!({ "ServiceId": first, "Attributes": { "owner": "first" } }),
    )
    .await;
    ok(
        &router,
        "UpdateServiceAttributes",
        json!({ "ServiceId": second, "Attributes": { "owner": "second", "extra": "yes" } }),
    )
    .await;

    for (case, reference, want) in [
        ("the first service", &first, expected([("owner", "first")])),
        (
            "the second service",
            &second,
            expected([("owner", "second"), ("extra", "yes")]),
        ),
    ] {
        // Read repeatedly: a reporting call must answer the same map every time.
        for attempt in 1..=3 {
            let actual = stored(&router, reference).await;
            assert_eq!(
                actual, want,
                "case {case:?} on read {attempt}: expected its own map, got {actual:?}"
            );
        }
    }
}
