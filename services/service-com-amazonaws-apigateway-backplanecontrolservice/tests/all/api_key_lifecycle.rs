// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the API key lifecycle: `CreateApiKey`, `GetApiKey` and
//! `DeleteApiKey`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi` helper are this crate's existing harness, reused from
//! `rest_api_lifecycle`.
//!
//! The tests assert RULES over sets of inputs, not one recorded call: the create/read
//! round trip over a table of request shapes (members present and absent, tag counts, a
//! supplied value and a minted one), the caller-supplied `value` over its published
//! length bounds and one past each, `stageKeys` over a table of entry counts and
//! malformed entries, and deletion over the records it must and must not remove.

use serde_json::{json, Value};

use crate::rest_api_lifecycle::{create_api, string_member, Answer, Gateway};
use crate::stage_lifecycle::create_deployed_stage;

/// AWS's published length range for an API key value a caller supplies
/// (`CreateApiKey$value`, "Specifies a value of the API key"): 20 to 128 characters.
/// The model declares no `@length`, so the service enforces it.
const MIN_VALUE_CHARS: usize = 20;

/// The upper end of that range; see [`MIN_VALUE_CHARS`].
const MAX_VALUE_CHARS: usize = 128;

/// The length of an API key value the service mints when the request supplies none,
/// as observed from AWS: 40 characters.
const MINTED_VALUE_CHARS: usize = 40;

/// `CreateApiKey` with `request`, asserting the modelled 201 and returning the body.
/// Shared with the sibling `patch_updates` suite, which patches the keys it creates.
pub(crate) async fn create_key(gateway: &Gateway, case: &str, request: Value) -> Value {
    let answer = gateway.call("POST", "/apikeys", Some(request)).await;
    assert_eq!(
        answer.status, 201,
        "case {case}: expected 201, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

/// `GetApiKey` for `id`, with `includeValue` set when `include_value`.
pub(crate) async fn get_key(gateway: &Gateway, id: &str, include_value: bool) -> Answer {
    let uri = if include_value {
        format!("/apikeys/{id}?includeValue=true")
    } else {
        format!("/apikeys/{id}")
    };
    gateway.call("GET", &uri, None).await
}

/// This crate's one modelled-failure assertion, re-exported here because the sibling
/// suites that grew up around this one import it from this module.
pub(crate) use crate::rest_api_lifecycle::assert_failure;

/// A tag map of `count` entries.
fn tag_map(count: usize) -> Value {
    Value::Object(
        (0..count)
            .map(|n| (format!("k{n}"), Value::String(format!("v{n}"))))
            .collect(),
    )
}

#[tokio::test]
async fn a_created_api_key_is_persisted_and_read_back_member_for_member() {
    // Rows differ along what the rule generalises over: which optional members the
    // request carries, how many tags it sets, whether the caller supplies the key value
    // or lets the service mint one, and how `enabled` is set (including left absent, so
    // the member's declared default applies).
    let supplied = "x".repeat(MIN_VALUE_CHARS);
    let cases: Vec<(&str, Value)> = vec![
        ("every member absent", json!({})),
        (
            "named and described",
            json!({ "name": "integration", "description": "the integration key" }),
        ),
        (
            "enabled with a marketplace customer",
            json!({ "enabled": true, "customerId": "marketplace-customer-1" }),
        ),
        (
            "explicitly disabled",
            json!({ "name": "disabled-key", "enabled": false }),
        ),
        ("one tag", json!({ "name": "tagged-1", "tags": tag_map(1) })),
        ("three tags", json!({ "name": "tagged-3", "tags": tag_map(3) })),
        (
            "a caller-supplied value",
            json!({ "name": "imported", "value": supplied.clone(), "enabled": true }),
        ),
        (
            "the deprecated generateDistinctId",
            json!({ "name": "distinct", "generateDistinctId": true }),
        ),
    ];

    let gateway = Gateway::new();
    let mut ids: Vec<String> = Vec::new();
    let mut values: Vec<String> = Vec::new();

    for (case, request) in &cases {
        let created = create_key(&gateway, case, request.clone()).await;
        let id = string_member(&created, "id", case);
        assert!(
            id.len() == 10 && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
            "case {case}: expected a ten-character lowercase alphanumeric id, got {id:?}"
        );

        // The value is the caller's when the request carries one, and otherwise a
        // minted 40-character alphanumeric secret.
        let value = string_member(&created, "value", case);
        match request.get("value").and_then(Value::as_str) {
            Some(expected) => assert_eq!(
                value, expected,
                "case {case}: expected the supplied value back, got {value:?}"
            ),
            None => assert!(
                value.chars().count() == MINTED_VALUE_CHARS
                    && value.chars().all(|c| c.is_ascii_alphanumeric()),
                "case {case}: expected {MINTED_VALUE_CHARS} alphanumeric characters, got {value:?}"
            ),
        }

        // Every echoed member reports what the request set, and nothing it did not.
        for member in ["name", "description", "customerId"] {
            assert_eq!(
                created[member],
                request.get(member).cloned().unwrap_or(Value::Null),
                "case {case}: {member} must report the request's value, got {:?}",
                created[member]
            );
        }
        assert_eq!(
            created["enabled"],
            request.get("enabled").cloned().unwrap_or(json!(false)),
            "case {case}: enabled must report the request's value or its default, got {:?}",
            created["enabled"]
        );
        // The two collection members are always reported, empty when the key holds
        // nothing: an `ApiKey` carries a list of stage associations and a map of tags,
        // which are empty rather than absent on a key that has neither.
        assert_eq!(
            created["tags"],
            request.get("tags").cloned().unwrap_or(json!({})),
            "case {case}: tags must report the request's map, got {:?}",
            created["tags"]
        );
        assert!(
            created["createdDate"].is_number() && created["lastUpdatedDate"].is_number(),
            "case {case}: expected both timestamps, got {created:?}"
        );
        // A timestamp on this surface is a WHOLE number of epoch seconds.
        for member in ["createdDate", "lastUpdatedDate"] {
            let stamp = created[member]
                .as_f64()
                .unwrap_or_else(|| panic!("case {case}: expected a numeric {member}"));
            assert!(
                stamp.fract() == 0.0,
                "case {case}: {member} must be whole epoch seconds, got {stamp}"
            );
        }
        assert_eq!(
            created["stageKeys"],
            json!([]),
            "case {case}: a key created with no stageKeys reports an empty list, got {:?}",
            created["stageKeys"]
        );

        // Read back through GetApiKey: the stored record, and the value only when the
        // request asks for it.
        let read = get_key(&gateway, &id, false).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetApiKey, got {} with {:?}",
            read.status, read.body
        );
        assert!(
            read.body["value"].is_null(),
            "case {case}: GetApiKey without includeValue must withhold the value, got {:?}",
            read.body["value"]
        );
        for member in [
            "id",
            "name",
            "description",
            "customerId",
            "enabled",
            "createdDate",
            "lastUpdatedDate",
            "tags",
        ] {
            assert_eq!(
                read.body[member], created[member],
                "case {case}: GetApiKey must report the stored {member}, got {:?} after {:?}",
                read.body[member], created[member]
            );
        }

        let with_value = get_key(&gateway, &id, true).await;
        assert_eq!(
            with_value.body["value"],
            created["value"],
            "case {case}: includeValue must report the stored value, got {:?}",
            with_value.body["value"]
        );

        ids.push(id);
        values.push(value);
    }

    let mut distinct_ids = ids.clone();
    distinct_ids.sort();
    distinct_ids.dedup();
    assert_eq!(
        distinct_ids.len(),
        ids.len(),
        "every created key takes its own id, got {ids:?}"
    );

    let mut minted: Vec<String> = values
        .into_iter()
        .filter(|value| *value != supplied)
        .collect();
    let minted_count = minted.len();
    minted.sort();
    minted.dedup();
    assert_eq!(
        minted.len(), minted_count,
        "every minted value is distinct, got {minted:?}"
    );
}

#[tokio::test]
async fn a_caller_supplied_value_is_accepted_only_within_its_published_bounds() {
    // Source: AWS's published API key value length, 20 to 128 characters. Both bounds
    // are tested, and one character past each.
    let cases: &[(usize, bool)] = &[
        (1, false),
        (MIN_VALUE_CHARS - 1, false),
        (MIN_VALUE_CHARS, true),
        (MINTED_VALUE_CHARS, true),
        (MAX_VALUE_CHARS, true),
        (MAX_VALUE_CHARS + 1, false),
    ];

    let gateway = Gateway::new();
    for (length, accepted) in cases.iter().copied() {
        let case = format!("CreateApiKey(value of {length} characters)");
        // Each row's value is distinct, so nothing is refused for being a duplicate.
        let value = format!("{length:0>width$}", width = length);
        let answer = gateway
            .call("POST", "/apikeys", Some(json!({ "value": value })))
            .await;
        if accepted {
            assert_eq!(
                answer.status, 201,
                "case {case}: expected 201 inside the bound, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.body["value"].as_str(),
                Some(value.as_str()),
                "case {case}: expected the supplied value back, got {:?}",
                answer.body["value"]
            );
        } else {
            assert_failure(&answer, 400, "BadRequestException", &case);
        }
    }
}

#[tokio::test]
async fn two_api_keys_cannot_share_one_value() {
    let gateway = Gateway::new();
    // Rows differ in the value's length and character set; each is created once and
    // then offered again.
    for value in [
        "a".repeat(MIN_VALUE_CHARS),
        "0123456789abcdefghijKLMNOPQRST".to_string(),
        "z".repeat(MAX_VALUE_CHARS),
    ] {
        let case = format!("CreateApiKey(duplicate value of {} chars)", value.len());
        create_key(&gateway, &case, json!({ "value": value })).await;
        let again = gateway
            .call("POST", "/apikeys", Some(json!({ "value": value })))
            .await;
        assert_failure(&again, 409, "ConflictException", &case);

        // A different value is not a conflict.
        let other = format!("{value}-other");
        if other.chars().count() <= MAX_VALUE_CHARS {
            create_key(&gateway, &format!("{case} / distinct value"), json!({ "value": other }))
                .await;
        }
    }
}

#[tokio::test]
async fn stage_keys_must_name_a_stored_stage_and_are_stored_as_api_and_stage_pairs() {
    let gateway = Gateway::new();
    let (first, _) = create_api(&gateway, "stage-keys-one").await;
    let (second, _) = create_api(&gateway, "stage-keys-two").await;

    // A stage key must name a stage, so each one this case associates is deployed
    // first — over the wire, through `CreateDeployment`.
    create_deployed_stage(&gateway, &first, "prod").await;
    create_deployed_stage(&gateway, &first, "beta").await;
    create_deployed_stage(&gateway, &second, "canary").await;

    // Accepted rows differ in how many stages one key is associated with, and whether
    // they belong to one API or several.
    let accepted: Vec<(&str, Vec<(String, &str)>)> = vec![
        ("one stage", vec![(first.clone(), "prod")]),
        (
            "two stages of one api",
            vec![(first.clone(), "prod"), (first.clone(), "beta")],
        ),
        (
            "stages of two apis",
            vec![(first.clone(), "prod"), (second.clone(), "canary")],
        ),
    ];

    for (case, stages) in &accepted {
        let request = json!({
            "name": format!("key-{case}"),
            "stageKeys": stages
                .iter()
                .map(|(api, stage)| json!({ "restApiId": api, "stageName": stage }))
                .collect::<Vec<Value>>(),
        });
        let created = create_key(&gateway, case, request).await;
        let expected: Vec<Value> = stages
            .iter()
            .map(|(api, stage)| Value::String(format!("{api}/{stage}")))
            .collect();
        assert_eq!(
            created["stageKeys"],
            Value::Array(expected.clone()),
            "case {case}: expected {expected:?}, got {:?}",
            created["stageKeys"]
        );

        // The association is persisted, not just echoed.
        let id = string_member(&created, "id", case);
        let read = get_key(&gateway, &id, false).await;
        assert_eq!(
            read.body["stageKeys"],
            Value::Array(expected.clone()),
            "case {case}: GetApiKey must report the stored stageKeys, got {:?}",
            read.body["stageKeys"]
        );
    }

    // A stage key that names no stored stage — because the API is unknown, or because
    // that API has no such stage — is the modelled NotFoundException; one that does not
    // name a stage at all is a bad request. Rows put the offending entry alone, beside a
    // valid one, and in either order.
    let refused: Vec<(&str, Value, u16, &str)> = vec![
        (
            "unknown restApiId",
            json!([{ "restApiId": "zzzzzzzzzz", "stageName": "prod" }]),
            404,
            "NotFoundException",
        ),
        (
            "an unknown api beside a known stage",
            json!([
                { "restApiId": first, "stageName": "prod" },
                { "restApiId": "zzzzzzzzzz", "stageName": "prod" },
            ]),
            404,
            "NotFoundException",
        ),
        (
            "a stage no api has",
            json!([{ "restApiId": first, "stageName": "nosuchstage" }]),
            404,
            "NotFoundException",
        ),
        (
            "a stage of the other api",
            json!([{ "restApiId": first, "stageName": "canary" }]),
            404,
            "NotFoundException",
        ),
        (
            "an unknown stage before a known one",
            json!([
                { "restApiId": second, "stageName": "prod" },
                { "restApiId": second, "stageName": "canary" },
            ]),
            404,
            "NotFoundException",
        ),
        (
            "no stageName",
            json!([{ "restApiId": first }]),
            400,
            "BadRequestException",
        ),
        (
            "no restApiId",
            json!([{ "stageName": "prod" }]),
            400,
            "BadRequestException",
        ),
    ];

    for (case, stage_keys, status, code) in &refused {
        let answer = gateway
            .call("POST", "/apikeys", Some(json!({ "stageKeys": stage_keys })))
            .await;
        assert_failure(&answer, *status, code, case);
    }
}

#[tokio::test]
async fn deleting_an_api_key_removes_that_key_and_nothing_else() {
    let gateway = Gateway::new();
    let kept = create_key(&gateway, "kept", json!({ "name": "kept" })).await;
    let doomed = create_key(&gateway, "doomed", json!({ "name": "doomed" })).await;
    let kept_id = string_member(&kept, "id", "kept");
    let doomed_id = string_member(&doomed, "id", "doomed");

    let deleted = gateway
        .call("DELETE", &format!("/apikeys/{doomed_id}"), None)
        .await;
    assert_eq!(
        deleted.status, 202,
        "case DeleteApiKey: expected the modelled 202, got {} with {:?}",
        deleted.status, deleted.body
    );

    let gone = get_key(&gateway, &doomed_id, false).await;
    assert_failure(&gone, 404, "NotFoundException", "GetApiKey(deleted)");

    let again = gateway
        .call("DELETE", &format!("/apikeys/{doomed_id}"), None)
        .await;
    assert_failure(&again, 404, "NotFoundException", "DeleteApiKey(deleted)");

    let survivor = get_key(&gateway, &kept_id, true).await;
    assert_eq!(
        survivor.status, 200,
        "case GetApiKey(kept): the other key must survive, got {} with {:?}",
        survivor.status, survivor.body
    );
    assert_eq!(
        survivor.body["value"], kept["value"],
        "case GetApiKey(kept): expected the stored value, got {:?}",
        survivor.body["value"]
    );

    // A value is reusable once the key holding it is gone.
    let value = string_member(&doomed, "value", "doomed");
    create_key(&gateway, "recreate with the freed value", json!({ "value": value })).await;
}

#[tokio::test]
async fn an_unknown_api_key_identifier_is_a_not_found_on_every_operation_that_resolves_one() {
    let gateway = Gateway::new();
    let created = create_key(&gateway, "existing", json!({ "name": "existing" })).await;
    let value = string_member(&created, "value", "existing");

    // Rows differ in the form of the identifier: one no key ever had, and the key's
    // VALUE — which is not its id, the member `apiKey` names.
    for identifier in ["zzzzzzzzzz", value.as_str()] {
        for (method, include_value) in [("GET", false), ("GET", true), ("DELETE", false)] {
            let case = format!("{method} /apikeys/{identifier} (includeValue={include_value})");
            let uri = if include_value {
                format!("/apikeys/{identifier}?includeValue=true")
            } else {
                format!("/apikeys/{identifier}")
            };
            let answer = gateway.call(method, &uri, None).await;
            assert_failure(&answer, 404, "NotFoundException", &case);
        }
    }
}

#[tokio::test]
async fn reading_an_api_key_does_not_change_what_a_later_caller_sees() {
    let gateway = Gateway::new();
    let created = create_key(
        &gateway,
        "stable",
        json!({ "name": "stable", "enabled": true, "tags": tag_map(2) }),
    )
    .await;
    let id = string_member(&created, "id", "stable");

    let mut seen: Vec<Value> = Vec::new();
    for round in 0..3 {
        let read = get_key(&gateway, &id, round % 2 == 0).await;
        assert_eq!(
            read.status, 200,
            "case GetApiKey(round {round}): expected 200, got {} with {:?}",
            read.status, read.body
        );
        seen.push(read.body);
    }
    assert_eq!(
        seen[0], seen[2],
        "case GetApiKey repeated: a read must not change the record, got {:?} then {:?}",
        seen[0], seen[2]
    );
    assert_eq!(
        seen[1]["id"], seen[0]["id"],
        "case GetApiKey(withheld value): the record is the same one, got {:?}",
        seen[1]
    );
}
