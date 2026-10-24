// SPDX-License-Identifier: CC0-1.0

//! The OpenSearch Serverless **write** path for the resources a client changes after it
//! created them: `UpdateAccessPolicy`, `UpdateSecurityPolicy`, `UpdateCollectionGroup` and
//! `DeleteAccessPolicy`, each proved through the read that follows it, end to end over the
//! wire.
//!
//! Every case drives the real request path (`MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body), through the
//! harness this crate's `collection_lifecycle` suite already owns, so the boundary
//! validator, the protocol codec and the handler all take part exactly as they do for an
//! SDK client.
//!
//! The rule under test: a mutating call WRITES the stored record and answers from what it
//! wrote, so the next read of that resource reports the written state — a policy write
//! replaces the members the request names, mints a fresh `policyVersion` and stamps
//! `lastModifiedDate`; a group write replaces the members the request names and stamps
//! `lastModifiedDate`; and a delete makes the record absent, so the read raises the
//! modelled not-found error. The tables span what the rule generalises over: all three
//! policy `type` values across both policy families, which members the request names (both,
//! each alone, neither, an empty description), the JSON form and whitespace of the document
//! text, capacity limits raised / lowered / dropped / at their published bounds, the
//! `PolicyName` length bounds, and the `ClientToken` length bounds — not the one request
//! the issue quoted.

use serde_json::{json, Value};

use crate::collection_lifecycle::{
    at, call, emulator, is_policy_version, text, ACCOUNT, CLIENT_ERROR,
};

/// One policy family this rule covers: the `type` value, the operations that create, write
/// and read one of its policies, the member its detail is reported under, and a document
/// of the shape that type carries.
///
/// The `type` values are the model's own value lists
/// (`smithy_query(query="shape", shape="AccessPolicyType")` -> `["data"]` and
/// `shape="SecurityPolicyType"` -> `["encryption", "network"]`), so the table below covers
/// every policy type this service models — including `encryption`, which the issue's
/// evidence did not quote for the document rows.
struct Family {
    kind: &'static str,
    create: &'static str,
    update: &'static str,
    get: &'static str,
    detail: &'static str,
    base_policy: &'static str,
}

const FAMILIES: [Family; 3] = [
    Family {
        kind: "data",
        create: "CreateAccessPolicy",
        update: "UpdateAccessPolicy",
        get: "GetAccessPolicy",
        detail: "accessPolicyDetail",
        base_policy: r#"{"Rules":[{"ResourceType":"collection","Resource":["collection/demo"],"Permission":["aoss:CreateCollectionItems"]}],"Principal":["arn:aws:iam::000000000000:role/Writer"]}"#,
    },
    Family {
        kind: "encryption",
        create: "CreateSecurityPolicy",
        update: "UpdateSecurityPolicy",
        get: "GetSecurityPolicy",
        detail: "securityPolicyDetail",
        base_policy: r#"{"Rules":[{"ResourceType":"collection","Resource":["collection/demo"]}],"AWSOwnedKey":true}"#,
    },
    Family {
        kind: "network",
        create: "CreateSecurityPolicy",
        update: "UpdateSecurityPolicy",
        get: "GetSecurityPolicy",
        detail: "securityPolicyDetail",
        base_policy: r#"{"Rules":[{"ResourceType":"collection","Resource":["collection/demo"]}],"AllowFromPublic":true}"#,
    },
];

/// The description every policy below is created with, so a row that names none proves the
/// stored value survived the write.
const CREATED_DESCRIPTION: &str = "the description the create stored";

/// A syntactically valid `PolicyVersion` that no write ever mints: 24 characters out of
/// `[0-9a-f]`, inside `PolicyVersion`'s `@length(min: 20, max: 36)` and its
/// `^([0-9a-zA-Z+/]{4})*...$` pattern, so the boundary validator accepts it and the handler
/// is what refuses it.
const STALE_VERSION: &str = "000000000000000000000000";

/// One write row: which members the request names, and the document text it carries when it
/// names one. A member this row leaves `None` is one the request does not name.
struct Row {
    what: &'static str,
    policy: Option<&'static str>,
    description: Option<&'static str>,
}

/// The write table: rows differ in which members the request names, in the JSON form of the
/// document (object, bare array, nested), in the whitespace of the request text and in
/// whether the description is emptied.
fn rows() -> Vec<Row> {
    vec![
        Row {
            what: "a document with an added rule, and a new description",
            policy: Some(
                r#"{"Rules":[{"ResourceType":"collection","Resource":["collection/demo"]},{"ResourceType":"index","Resource":["index/demo/*"]}]}"#,
            ),
            description: Some("updated to include index rules"),
        },
        Row {
            what: "a document only, leaving the stored description",
            policy: Some(r#"{"Rules":[]}"#),
            description: None,
        },
        Row {
            what: "a description only, leaving the stored document",
            policy: None,
            description: Some("updated description"),
        },
        Row {
            what: "an empty description",
            policy: None,
            // `PolicyDescription` is @length(min: 0, max: 1000): the lower bound.
            description: Some(""),
        },
        Row {
            what: "a bare array document, from request text carrying tabs and newlines",
            policy: Some("[\n\t{ \"Rules\" : [] }\n]"),
            // `é` and `—` are inside `PolicyDescription`'s own character range.
            description: Some("naïve café — non-ASCII text"),
        },
        Row {
            what: "a nested document of mixed scalar types",
            policy: Some(r#"{"a":{"b":{"c":{"d":[1,2,{"e":true,"f":null,"g":1.5}]}}}}"#),
            description: None,
        },
        Row {
            what: "neither member named, so only the version moves",
            policy: None,
            description: None,
        },
    ]
}

/// Call `operation` with `body`, asserting it was accepted, and answer the member of the
/// response named by `detail`.
async fn accepted(
    router: &smithy_server_runtime::MultiServiceRouter,
    operation: &str,
    detail: &str,
    body: Value,
) -> Value {
    let case = body.to_string();
    let (status, code, answer) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation}({case}): expected 200, got {status} {code} with {answer}"
    );
    at(&answer, detail).clone()
}

// ---------------------------------------------------------------------------
// A policy write replaces the members the request names, mints a fresh version, and is
// exactly what the following read reports — for every policy type, in both policy
// families.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_policy_update_is_what_the_following_read_reports() {
    let router = emulator();

    for family in &FAMILIES {
        for (index, row) in rows().into_iter().enumerate() {
            // `PolicyName` is @length(min: 3, max: 32) with `^[a-z][a-z0-9-]+$`.
            let name = format!("upd-{}-{index}", family.kind);
            let created = accepted(
                &router,
                family.create,
                family.detail,
                json!({
                    "type": family.kind,
                    "name": name,
                    "policy": family.base_policy,
                    "description": CREATED_DESCRIPTION,
                }),
            )
            .await;

            let case = format!("{}({}/{name}, {})", family.update, family.kind, row.what);
            let created_version = text(&created, "policyVersion");
            let created_date = at(&created, "createdDate").as_i64().unwrap_or_default();

            let mut request = json!({
                "type": family.kind,
                "name": name,
                "policyVersion": created_version,
            });
            if let Some(policy) = row.policy {
                request["policy"] = json!(policy);
            }
            if let Some(description) = row.description {
                request["description"] = json!(description);
            }
            let written = accepted(&router, family.update, family.detail, request).await;

            // A member the request named is the stored value now; one it did not name kept
            // what the create stored.
            let expected_policy: Value = serde_json::from_str(row.policy.unwrap_or(family.base_policy))
                .expect("every row's document text is JSON");
            assert_eq!(
                at(&written, "policy"),
                &expected_policy,
                "case {case}: the stored document must be the written one, parsed, got {written}"
            );
            assert_eq!(
                at(&written, "description"),
                &json!(row.description.unwrap_or(CREATED_DESCRIPTION)),
                "case {case}: the stored description must be the written one, got {written}"
            );

            // Every write mints a fresh `policyVersion` and stamps `lastModifiedDate`,
            // while the identity and the creation stamp of the record stay put.
            let written_version = text(&written, "policyVersion");
            assert!(
                is_policy_version(&written_version),
                "case {case}: policyVersion must match PolicyVersion's length and pattern, got {written_version:?}"
            );
            assert_ne!(
                written_version, created_version,
                "case {case}: a write mints a NEW policyVersion, got the creation version {written_version:?}"
            );
            assert_eq!(
                (text(&written, "type"), text(&written, "name")),
                (family.kind.to_string(), name.clone()),
                "case {case}: the write answers the policy it was addressed to, got {written}"
            );
            assert_eq!(
                at(&written, "createdDate").as_i64(),
                Some(created_date),
                "case {case}: a write leaves createdDate where the create put it, got {written}"
            );
            let modified = at(&written, "lastModifiedDate").as_i64().unwrap_or_default();
            assert!(
                modified >= created_date,
                "case {case}: lastModifiedDate must be the write's own instant, expected >= {created_date}, got {modified}"
            );

            // The round trip: the read answers the record the write stored, member for
            // member — the version and both stamps included.
            let read = accepted(
                &router,
                family.get,
                family.detail,
                json!({ "type": family.kind, "name": name }),
            )
            .await;
            assert_eq!(
                read, written,
                "case {case}: the read must answer the written record, got {read}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// `policyVersion` is the write's precondition: the version chains forward, and a version
// that is not the stored one is refused and writes nothing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_policy_update_chains_the_version_and_refuses_a_stale_one() {
    let router = emulator();

    for family in &FAMILIES {
        let name = format!("chain-{}", family.kind);
        let created = accepted(
            &router,
            family.create,
            family.detail,
            json!({
                "type": family.kind,
                "name": name,
                "policy": family.base_policy,
                "description": CREATED_DESCRIPTION,
            }),
        )
        .await;

        // Each write is addressed with the version the write before it returned, and every
        // one of them mints a version no earlier write used.
        let mut versions = vec![text(&created, "policyVersion")];
        for step in 0..3 {
            let current = versions.last().cloned().expect("a version to write against");
            let written = accepted(
                &router,
                family.update,
                family.detail,
                json!({
                    "type": family.kind,
                    "name": name,
                    "policyVersion": current,
                    "description": format!("write {step}"),
                }),
            )
            .await;
            let version = text(&written, "policyVersion");
            assert!(
                !versions.contains(&version),
                "case {}({}/{name}) write {step}: every write mints a fresh version, got a repeat of {version:?}",
                family.update,
                family.kind
            );
            versions.push(version);
        }

        // (case, the version the request carries, expected status, expected error shape)
        let refusals: Vec<(&str, String, u16, &str)> = vec![
            (
                "the creation version, long since superseded",
                versions[0].clone(),
                CLIENT_ERROR,
                "ConflictException",
            ),
            (
                "the version before the last write",
                versions[versions.len() - 2].clone(),
                CLIENT_ERROR,
                "ConflictException",
            ),
            (
                "a well-formed version nothing ever minted",
                STALE_VERSION.to_string(),
                CLIENT_ERROR,
                "ConflictException",
            ),
        ];
        for (what, version, expected_status, expected_code) in refusals {
            let (status, code, answer) = call(
                &router,
                family.update,
                json!({
                    "type": family.kind,
                    "name": name,
                    "policyVersion": version,
                    "description": "this write must not land",
                }),
            )
            .await;
            assert_eq!(
                (status, code.as_str()),
                (expected_status, expected_code),
                "case {}({}/{name}, {what}): expected {expected_status} {expected_code}, got {status} {code} with {answer}",
                family.update,
                family.kind
            );
        }

        // A refused write stored nothing: the policy still carries the last write that was
        // accepted, at the version that write minted.
        let read = accepted(
            &router,
            family.get,
            family.detail,
            json!({ "type": family.kind, "name": name }),
        )
        .await;
        assert_eq!(
            (
                text(&read, "policyVersion"),
                text(&read, "description")
            ),
            (
                versions.last().cloned().unwrap_or_default(),
                "write 2".to_string()
            ),
            "case {}({}/{name}): a refused write must leave the record as the last accepted write left it, got {read}",
            family.update,
            family.kind
        );

        // A `(type, name)` pair that names no stored policy is the modelled not-found
        // refusal, whatever version the request carries.
        let (status, code, answer) = call(
            &router,
            family.update,
            json!({
                "type": family.kind,
                "name": format!("absent-{}", family.kind),
                "policyVersion": STALE_VERSION,
                "description": "there is nothing to write",
            }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ResourceNotFoundException"),
            "case {}({}/absent): expected {CLIENT_ERROR} ResourceNotFoundException, got {status} {code} with {answer}",
            family.update,
            family.kind
        );
    }
}

// ---------------------------------------------------------------------------
// A replayed `clientToken` on a write is a RETRY of that write: the same record, the same
// version, no second write.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_replayed_client_token_on_a_policy_update_answers_the_write_it_already_made() {
    let router = emulator();

    for (index, family) in FAMILIES.iter().enumerate() {
        // `ClientToken` is @length(min: 1, max: 512): both published bounds, and one token
        // in between. Each family spends its OWN tokens, because a token names the request
        // it was first spent on — one shared across families would make the second
        // family's call no retry of anything.
        let tokens = [
            ["a", "b", "c"][index].to_string(),
            format!("retry-token-{}", family.kind),
            format!("{}{}", family.kind, "t".repeat(512 - family.kind.len())),
        ];
        for (row, token) in tokens.iter().enumerate() {
            let name = format!("retry-{}-{row}", family.kind);
            let created = accepted(
                &router,
                family.create,
                family.detail,
                json!({
                    "type": family.kind,
                    "name": name,
                    "policy": family.base_policy,
                    "description": CREATED_DESCRIPTION,
                }),
            )
            .await;
            let request = json!({
                "type": family.kind,
                "name": name,
                "policyVersion": text(&created, "policyVersion"),
                "description": "written once",
                "clientToken": token,
            });

            let first = accepted(&router, family.update, family.detail, request.clone()).await;
            // The retry carries the version the FIRST call carried, which that call has
            // since superseded: without the token it would be the stale-version conflict.
            let replay = accepted(&router, family.update, family.detail, request).await;
            assert_eq!(
                replay, first,
                "case {}({}/{name}, a replayed clientToken of {} characters): the retry must answer the write it already made, got {replay}",
                family.update,
                family.kind,
                token.len()
            );

            let read = accepted(
                &router,
                family.get,
                family.detail,
                json!({ "type": family.kind, "name": name }),
            )
            .await;
            assert_eq!(
                read, first,
                "case {}({}/{name}): the retry wrote nothing, so the read still answers the first write, got {read}",
                family.get,
                family.kind
            );
        }
    }
}

// ---------------------------------------------------------------------------
// `DeleteAccessPolicy`: the record is gone, so every later read of that pair raises the
// modelled not-found error — and the pair is free to be created again.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_deleted_access_policy_is_absent_from_every_later_read() {
    let router = emulator();
    let document = r#"{"Rules":[{"ResourceType":"index","Resource":["index/demo/*"]}]}"#;

    // Names spanning `PolicyName`'s published length bounds (@length(min: 3, max: 32)) and
    // one in between.
    let names = vec![
        "abc".to_string(),
        "deleted-policy".to_string(),
        format!("d{}", "e".repeat(31)),
    ];
    // One policy nothing below deletes: a delete must remove the record it names and no
    // other.
    let kept = "kept-policy";
    for name in names.iter().map(String::as_str).chain([kept]) {
        accepted(
            &router,
            "CreateAccessPolicy",
            "accessPolicyDetail",
            json!({ "type": "data", "name": name, "policy": document }),
        )
        .await;
    }

    for name in &names {
        let (status, code, answer) = call(
            &router,
            "DeleteAccessPolicy",
            json!({ "type": "data", "name": name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case DeleteAccessPolicy(data/{name}): expected 200, got {status} {code} with {answer}"
        );
        assert_eq!(
            answer,
            json!({}),
            "case DeleteAccessPolicy(data/{name}): the model declares no output members, got {answer}"
        );

        // The read of a deleted pair is the modelled not-found refusal, and it stays that
        // way however often it is polled.
        for attempt in 0..3 {
            let (status, code, answer) = call(
                &router,
                "GetAccessPolicy",
                json!({ "type": "data", "name": name }),
            )
            .await;
            assert_eq!(
                (status, code.as_str()),
                (CLIENT_ERROR, "ResourceNotFoundException"),
                "case GetAccessPolicy(data/{name}) poll {attempt}: expected {CLIENT_ERROR} ResourceNotFoundException, got {status} {code} with {answer}"
            );
            assert_eq!(
                at(&answer, "accessPolicyDetail"),
                &Value::Null,
                "case GetAccessPolicy(data/{name}) poll {attempt}: a deleted policy has no detail, got {answer}"
            );
        }

        // Deleting it again names no record either.
        let (status, code, answer) = call(
            &router,
            "DeleteAccessPolicy",
            json!({ "type": "data", "name": name }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ResourceNotFoundException"),
            "case DeleteAccessPolicy(data/{name}, twice): expected {CLIENT_ERROR} ResourceNotFoundException, got {status} {code} with {answer}"
        );

        // A write to a deleted policy names no record either — the same refusal the read
        // gives.
        let (status, code, answer) = call(
            &router,
            "UpdateAccessPolicy",
            json!({
                "type": "data",
                "name": name,
                "policyVersion": STALE_VERSION,
                "description": "there is nothing to write",
            }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ResourceNotFoundException"),
            "case UpdateAccessPolicy(data/{name}, deleted): expected {CLIENT_ERROR} ResourceNotFoundException, got {status} {code} with {answer}"
        );

        // And the teardown/recreate cycle: the freed pair takes a new policy, whose own
        // document is what the read reports.
        let recreated_document = r#"{"Rules":[{"ResourceType":"collection","Resource":["collection/demo"]}]}"#;
        let recreated = accepted(
            &router,
            "CreateAccessPolicy",
            "accessPolicyDetail",
            json!({ "type": "data", "name": name, "policy": recreated_document }),
        )
        .await;
        let read = accepted(
            &router,
            "GetAccessPolicy",
            "accessPolicyDetail",
            json!({ "type": "data", "name": name }),
        )
        .await;
        assert_eq!(
            read, recreated,
            "case CreateAccessPolicy(data/{name}, after the delete): the read must answer the new record, got {read}"
        );
    }

    // The policy no delete named is untouched.
    let read = accepted(
        &router,
        "GetAccessPolicy",
        "accessPolicyDetail",
        json!({ "type": "data", "name": kept }),
    )
    .await;
    assert_eq!(
        text(&read, "name"),
        kept,
        "case GetAccessPolicy(data/{kept}): a delete must remove only the record it named, got {read}"
    );

    // A delete drops the `clientToken` replay entries that named the policy with it, so the
    // same token spent again creates a FRESH policy rather than replaying the deleted one.
    let token = "a-token-whose-policy-is-deleted";
    let request = json!({
        "type": "data",
        "name": "token-then-deleted",
        "policy": document,
        "clientToken": token,
    });
    let first = accepted(&router, "CreateAccessPolicy", "accessPolicyDetail", request.clone()).await;
    let (status, code, answer) = call(
        &router,
        "DeleteAccessPolicy",
        json!({ "type": "data", "name": "token-then-deleted" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case DeleteAccessPolicy(data/token-then-deleted): expected 200, got {status} {code} with {answer}"
    );
    let again = accepted(&router, "CreateAccessPolicy", "accessPolicyDetail", request).await;
    assert_ne!(
        text(&again, "policyVersion"),
        text(&first, "policyVersion"),
        "case CreateAccessPolicy(a token whose policy was deleted): expected a freshly created policy, got the deleted one's record {again}"
    );
}

// ---------------------------------------------------------------------------
// A create carrying an `@idempotencyToken` stores the policy, so the read that follows
// reports it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_access_policy_created_with_a_client_token_is_readable() {
    let router = emulator();
    let document = r#"{"Rules":[{"ResourceType":"index","Resource":["index/demo/*"]}]}"#;

    // (case, the `clientToken` the create carries) — `ClientToken` is
    // @length(min: 1, max: 512), so both published bounds are here beside the absent case
    // and an SDK-shaped token.
    let tokens: Vec<(&str, Option<String>)> = vec![
        ("no clientToken at all", None),
        ("a clientToken at the 1-character minimum", Some("x".to_string())),
        (
            "an SDK-shaped clientToken",
            Some("3f1b9c2e-7a84-4d1f-9b0e-2c6d5a8f4e31".to_string()),
        ),
        (
            "a clientToken at the 512-character maximum",
            Some("t".repeat(512)),
        ),
    ];

    for (index, (what, token)) in tokens.into_iter().enumerate() {
        let name = format!("idem-probe-{index}");
        let mut request = json!({ "type": "data", "name": name, "policy": document });
        if let Some(token) = &token {
            request["clientToken"] = json!(token);
        }
        let created = accepted(&router, "CreateAccessPolicy", "accessPolicyDetail", request).await;
        let read = accepted(
            &router,
            "GetAccessPolicy",
            "accessPolicyDetail",
            json!({ "type": "data", "name": name }),
        )
        .await;
        assert_eq!(
            read, created,
            "case CreateAccessPolicy({what}): the create must store the policy, so the read answers it, got {read}"
        );
    }
}

// ---------------------------------------------------------------------------
// A `clientToken` replays the request it was first spent on, and only that one: a token
// spent again on a DIFFERENT resource is a request for that resource, which is still
// created.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_client_token_spent_on_another_resource_still_creates_it() {
    let router = emulator();

    for family in &FAMILIES {
        // One token per family: a token names the request it was first spent on, so a
        // token shared across the families would not be a second spend within one family.
        let token = format!("one-token-two-resources-{}", family.kind);
        // Two different policies of the same type, created with the SAME token.
        let first_name = format!("tok-one-{}", family.kind);
        let second_name = format!("tok-two-{}", family.kind);
        let mut created = Vec::new();
        for name in [&first_name, &second_name] {
            created.push(
                accepted(
                    &router,
                    family.create,
                    family.detail,
                    json!({
                        "type": family.kind,
                        "name": name,
                        "policy": family.base_policy,
                        "clientToken": token,
                    }),
                )
                .await,
            );
        }

        // Each create answered the policy its own request named, and each one is readable.
        for (name, detail) in [&first_name, &second_name].into_iter().zip(&created) {
            assert_eq!(
                text(detail, "name"),
                *name,
                "case {}({}/{name}, a token already spent): the create must answer the policy it was asked for, got {detail}",
                family.create,
                family.kind
            );
            let read = accepted(
                &router,
                family.get,
                family.detail,
                json!({ "type": family.kind, "name": name }),
            )
            .await;
            assert_eq!(
                &read, detail,
                "case {}({}/{name}): the create must store the policy it answered, got {read}",
                family.get,
                family.kind
            );
        }

        // And the token still replays the request it WAS first spent on, rather than
        // creating a second policy under that name.
        let replay = accepted(
            &router,
            family.create,
            family.detail,
            json!({
                "type": family.kind,
                "name": first_name,
                "policy": family.base_policy,
                "clientToken": token,
            }),
        )
        .await;
        assert_eq!(
            replay, created[0],
            "case {}({}/{first_name}, the request the token was first spent on): expected the replay of that call, got {replay}",
            family.create,
            family.kind
        );
    }
}

// ---------------------------------------------------------------------------
// `UpdateCollectionGroup`: the written description and capacity limits are what the
// following `BatchGetCollectionGroup` reports.
// ---------------------------------------------------------------------------

/// The limits every group below is created with, so a row that names none proves the
/// stored structure survived the write. Every member is named, so these ARE the group's
/// effective limits: nothing is left for the service to default in.
fn created_limits() -> Value {
    json!({
        "minIndexingCapacityInOCU": 2.0,
        "maxIndexingCapacityInOCU": 2.0,
        "minSearchCapacityInOCU": 2.0,
        "maxSearchCapacityInOCU": 2.0,
    })
}

/// The description every group below is created with.
const CREATED_GROUP_DESCRIPTION: &str = "initial group description";

/// The first `collectionGroupDetails` entry of a `BatchGetCollectionGroup` answer, or
/// `Value::Null` when it reported none.
fn only_detail(details: &Value) -> Value {
    details
        .as_array()
        .and_then(|details| details.first())
        .cloned()
        .unwrap_or(Value::Null)
}

#[tokio::test]
async fn a_collection_group_update_is_what_the_following_batch_get_reports() {
    let router = emulator();

    // (case, the members the update request names)
    let rows: Vec<(&str, Value)> = vec![
        (
            "raised ceilings and a new description",
            json!({
                "description": "updated group description",
                "capacityLimits": {
                    "maxIndexingCapacityInOCU": 4.0,
                    "maxSearchCapacityInOCU": 4.0,
                },
            }),
        ),
        (
            "lowered limits, floors and ceilings together",
            json!({
                "capacityLimits": {
                    "minIndexingCapacityInOCU": 1.0,
                    "maxIndexingCapacityInOCU": 2.0,
                    "minSearchCapacityInOCU": 0.0,
                    "maxSearchCapacityInOCU": 1.0,
                },
            }),
        ),
        (
            "a description only, leaving the stored limits",
            json!({ "description": "only the description moved" }),
        ),
        (
            // `UpdateCollectionGroupRequest$description` is @length(max: 1000) with no
            // minimum, so the empty string is a value it may be set to.
            "an empty description",
            json!({ "description": "" }),
        ),
        (
            "neither member named",
            json!({}),
        ),
        (
            // `CollectionGroupMaxIndexingCapacityValue` is @range(min: 1) and
            // `CollectionGroupMinSearchCapacityValue` @range(min: 0): the published bounds.
            "limits at their published bounds, each floor equal to its ceiling",
            json!({
                "description": "at the bounds",
                "capacityLimits": {
                    "minIndexingCapacityInOCU": 1.0,
                    "maxIndexingCapacityInOCU": 1.0,
                    "minSearchCapacityInOCU": 0.0,
                    "maxSearchCapacityInOCU": 16.0,
                },
            }),
        ),
    ];

    for (index, (what, update)) in rows.into_iter().enumerate() {
        let name = format!("wrt-group-{index}");
        let created = accepted(
            &router,
            "CreateCollectionGroup",
            "createCollectionGroupDetail",
            json!({
                "name": name,
                "standbyReplicas": "ENABLED",
                "description": CREATED_GROUP_DESCRIPTION,
                "capacityLimits": created_limits(),
            }),
        )
        .await;
        let id = text(&created, "id");
        let created_date = at(&created, "createdDate").as_i64().unwrap_or_default();
        let case = format!("UpdateCollectionGroup({name}, {what})");

        let mut request = update.clone();
        request["id"] = json!(id);
        let written = accepted(
            &router,
            "UpdateCollectionGroup",
            "updateCollectionGroupDetail",
            request,
        )
        .await;

        // A member the request named is the stored value now; one it did not name kept what
        // the create stored. `capacityLimits` is one structure, so the request's structure
        // is the group's limits.
        let expected_description = match at(&update, "description") {
            Value::Null => json!(CREATED_GROUP_DESCRIPTION),
            description => description.clone(),
        };
        let expected_limits = match at(&update, "capacityLimits") {
            Value::Null => created_limits(),
            // One structure, so the limits the request states are the group's limits —
            // through the same ceiling defaulting a create applies, because a group always
            // has effective limits (the rule the `collection_group_capacity` suite owns).
            limits => crate::collection_group_capacity::expected_capacity_limits(limits),
        };
        assert_eq!(
            at(&written, "description"),
            &expected_description,
            "case {case}: the stored description must be the written one, got {written}"
        );
        assert_eq!(
            at(&written, "capacityLimits"),
            &expected_limits,
            "case {case}: the stored limits must be the written ones, got {written}"
        );

        // The group's identity and its creation stamp do not move; `lastModifiedDate` is
        // the write's own instant.
        assert_eq!(
            (
                text(&written, "id"),
                text(&written, "name"),
                text(&written, "arn"),
                text(&written, "generation"),
            ),
            (
                id.clone(),
                name.clone(),
                format!("arn:aws:aoss:us-east-1:{ACCOUNT}:collection-group/{id}"),
                "CLASSIC".to_string(),
            ),
            "case {case}: a write leaves the group's identity where the create put it, got {written}"
        );
        assert_eq!(
            at(&written, "createdDate").as_i64(),
            Some(created_date),
            "case {case}: a write leaves createdDate where the create put it, got {written}"
        );
        let modified = at(&written, "lastModifiedDate").as_i64().unwrap_or_default();
        assert!(
            modified >= created_date,
            "case {case}: lastModifiedDate must be the write's own instant, expected >= {created_date}, got {modified}"
        );

        // The round trip, addressed by id and by name: the read reports what the write
        // stored, and the derived current capacity follows the limits that are now stored.
        for selector in [json!({ "ids": [&id] }), json!({ "names": [&name] })] {
            let label = format!("{case} read back with {selector}");
            let answer = accepted(
                &router,
                "BatchGetCollectionGroup",
                "collectionGroupDetails",
                selector.clone(),
            )
            .await;
            let read = only_detail(&answer);
            for member in ["id", "arn", "name", "generation", "createdDate"] {
                assert_eq!(
                    at(&read, member),
                    at(&written, member),
                    "case {label}: {member} must be what the write reported, got {read}"
                );
            }
            assert_eq!(
                at(&read, "description"),
                &expected_description,
                "case {label}: the read must report the written description, got {read}"
            );
            assert_eq!(
                at(&read, "capacityLimits"),
                &expected_limits,
                "case {label}: the read must report the written limits, got {read}"
            );
            for (half, member) in [("Search", "search"), ("Indexing", "indexing")] {
                let expected = at(&expected_limits, &format!("min{half}CapacityInOCU"))
                    .as_f64()
                    .map(|floor| json!({ "capacityInOcu": floor, "autoscalingStatus": "NO_ACTION" }))
                    .unwrap_or(Value::Null);
                assert_eq!(
                    at(&read, &format!("currentCapacity.{member}")),
                    &expected,
                    "case {label}: currentCapacity.{member} must follow the stored floor, got {read}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_collection_group_update_refuses_what_it_cannot_apply_and_writes_nothing() {
    let router = emulator();
    let created = accepted(
        &router,
        "CreateCollectionGroup",
        "createCollectionGroupDetail",
        json!({
            "name": "refusal-group",
            "standbyReplicas": "ENABLED",
            "description": CREATED_GROUP_DESCRIPTION,
            "capacityLimits": created_limits(),
        }),
    )
    .await;
    let id = text(&created, "id");

    // (case, request, expected status, expected error shape)
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            // `UpdateCollectionGroup` models no `ResourceNotFoundException`, so an id that
            // addresses no group is refused with the only client error it does model.
            "an id that addresses no group",
            json!({ "id": "zzz999zzz", "description": "nothing to write" }),
            400,
            "ValidationException",
        ),
        (
            "an indexing floor above its own ceiling",
            json!({
                "id": id,
                "capacityLimits": {
                    "minIndexingCapacityInOCU": 8.0,
                    "maxIndexingCapacityInOCU": 4.0,
                },
            }),
            400,
            "ValidationException",
        ),
        (
            "a search floor above its own ceiling",
            json!({
                "id": id,
                "capacityLimits": {
                    "minSearchCapacityInOCU": 8.0,
                    "maxSearchCapacityInOCU": 4.0,
                },
            }),
            400,
            "ValidationException",
        ),
        (
            // One past `CollectionGroupMaxSearchCapacityValue`'s published @range(min: 1).
            "a ceiling below the modelled minimum",
            json!({ "id": id, "capacityLimits": { "maxSearchCapacityInOCU": 0.5 } }),
            400,
            "ValidationException",
        ),
        (
            // One past `CollectionGroupMinIndexingCapacityValue`'s published @range(min: 0).
            "a floor below the modelled minimum",
            json!({ "id": id, "capacityLimits": { "minIndexingCapacityInOCU": -1.0 } }),
            400,
            "ValidationException",
        ),
        (
            // `CollectionGroupId` is @length(min: 3, max: 40) with `^[a-z0-9]{3,40}$`.
            "an id outside the modelled pattern",
            json!({ "id": "NOT-AN-ID", "description": "nothing to write" }),
            400,
            "ValidationException",
        ),
    ];

    for (what, request, expected_status, expected_code) in cases {
        let (status, code, answer) = call(&router, "UpdateCollectionGroup", request).await;
        assert_eq!(
            (status, code.as_str()),
            (expected_status, expected_code),
            "case UpdateCollectionGroup({what}): expected {expected_status} {expected_code}, got {status} {code} with {answer}"
        );
    }

    // Every refusal above stored nothing: the group still carries what the create stored.
    let answer = accepted(
        &router,
        "BatchGetCollectionGroup",
        "collectionGroupDetails",
        json!({ "ids": [&id] }),
    )
    .await;
    let read = only_detail(&answer);
    assert_eq!(
        (at(&read, "description").clone(), at(&read, "capacityLimits").clone()),
        (json!(CREATED_GROUP_DESCRIPTION), created_limits()),
        "case UpdateCollectionGroup(after the refusals): a refused write must store nothing, got {read}"
    );
}

#[tokio::test]
async fn a_replayed_client_token_on_a_group_update_answers_the_write_it_already_made() {
    let router = emulator();

    // `ClientToken` is @length(min: 1, max: 512): both published bounds.
    for (index, token) in ["x".to_string(), "t".repeat(512)].into_iter().enumerate() {
        let name = format!("retry-group-{index}");
        let created = accepted(
            &router,
            "CreateCollectionGroup",
            "createCollectionGroupDetail",
            json!({
                "name": name,
                "standbyReplicas": "ENABLED",
                "description": CREATED_GROUP_DESCRIPTION,
                "capacityLimits": created_limits(),
            }),
        )
        .await;
        let id = text(&created, "id");
        let request = json!({
            "id": id,
            "description": "written once",
            "capacityLimits": { "maxIndexingCapacityInOCU": 4.0, "maxSearchCapacityInOCU": 4.0 },
            "clientToken": token,
        });

        let first = accepted(
            &router,
            "UpdateCollectionGroup",
            "updateCollectionGroupDetail",
            request.clone(),
        )
        .await;
        let replay = accepted(
            &router,
            "UpdateCollectionGroup",
            "updateCollectionGroupDetail",
            request,
        )
        .await;
        assert_eq!(
            replay, first,
            "case UpdateCollectionGroup({name}, a replayed clientToken of {} characters): the retry must answer the write it already made, got {replay}",
            token.len()
        );

        let answer = accepted(
            &router,
            "BatchGetCollectionGroup",
            "collectionGroupDetails",
            json!({ "ids": [&id] }),
        )
        .await;
        let read = only_detail(&answer);
        assert_eq!(
            at(&read, "description"),
            at(&first, "description"),
            "case UpdateCollectionGroup({name}): the retry wrote nothing, so the read still answers the first write, got {read}"
        );
        assert_eq!(
            at(&read, "capacityLimits"),
            at(&first, "capacityLimits"),
            "case UpdateCollectionGroup({name}): the retry wrote nothing, so the read still answers the first write, got {read}"
        );
    }
}
