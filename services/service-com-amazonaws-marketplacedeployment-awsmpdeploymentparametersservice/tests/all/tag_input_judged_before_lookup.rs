// SPDX-License-Identifier: CC0-1.0

//! The tag judgement a tagging request earns on its OWN content, independent of
//! what the store holds.
//!
//! The rule under test: every part of the tag judgement that reads only the request
//! — a key in the namespace AWS reserves, a key outside the characters a tag key may
//! hold, a value longer than a tag value may be, a map of more entries than one
//! resource may hold — is answered the SAME way whether or not the ARN the request
//! names resolves to a stored record: HTTP 400 with the same code, the same message
//! and the same `fieldName`. Only the part of
//! the per-resource cap that needs the record — whether the set would pass the cap on
//! top of the tags the record ALREADY holds — may wait for the record to be resolved;
//! nothing is stored or changed either way.
//!
//! WHICH code a row carries is the other half of the identity, and it is per row: a
//! bound on what a tag IS is this service's own modelled `ValidationException`, which
//! names the member it is about, while the COUNT of tags one resource may hold is the
//! tagging front end's limit, answered with its own unmodelled identity and no member
//! (`tag_count_front_end` owns that rule).
//!
//! The complement holds too: a tag set the request-level judgement ADMITS carries no
//! verdict of its own, so against a stored record it is the success the merge earns
//! and against an ARN that names no record it is still this service's
//! `NotFoundException` (HTTP 404) — the ordering may not turn a missing resource
//! into a bad request. The store's three states are covered: a live record, an ARN
//! that was never minted, and a record the clock has already expired.
//!
//! `TagResource$tags` targets the tag map the model leaves unconstrained
//! (`smithy_query(query="operation",operation="TagResource")`:
//! `tags` -> `Tags`, `map<String,String>`), so the bounds under test are AWS's own
//! tagging limits (`state::TAG_KEY_MAX_LEN` / `state::TAG_VALUE_MAX_LEN` /
//! `state::MAX_TAGS_PER_RESOURCE`) and not the character class the model's
//! constrained `TagsMap` adds.
//!
//! Every case drives the real wire path through the harness the
//! `deployment_parameters` suite owns.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::scaffolded::{
    BAD_REQUEST_CODE, NOT_FOUND_CODE,
};
use service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::state::{
    MAX_TAGS_PER_RESOURCE, RESERVED_TAG_KEY_PREFIX, TAG_KEY_MAX_LEN, TAG_VALUE_MAX_LEN,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::deployment_parameters::{
    emulator, error_code, field_name, list_tags, message, provision, put, rfc3339, tag, tag_map,
    tags_of, ACCOUNT, CATALOG, REGION, SIGNING_NAME,
};

/// The agreement and product the one stored record below is written against.
const AGREEMENT: &str = "agmt-input-first";
const PRODUCT: &str = "prod-input-first";
/// The tag map that record carries, which no refused call may change.
const STORED_KEY: &str = "env";
const STORED_VALUE: &str = "dev";
/// `2020-01-01T00:00:00Z` as epoch seconds: a date long past, so a record written
/// with it is already scheduled for deletion by the time the write returns.
const PAST_2020: i64 = 1_577_836_800;

/// One emulator holding exactly one deployment parameter, and the ARN minted for it.
async fn emulator_with_one_parameter() -> (MultiServiceRouter, String) {
    let (router, engine) = emulator();
    provision(&engine, CATALOG, AGREEMENT, &[PRODUCT]);
    let (status, body) = put(
        &router,
        CATALOG,
        PRODUCT,
        &json!({
            "agreementId": AGREEMENT,
            "deploymentParameter": { "name": "InputFirst", "secretString": "s" },
            "tags": { STORED_KEY: STORED_VALUE },
        }),
    )
    .await;
    assert_eq!(status, 200, "the setup Put must succeed, got {body:?}");
    let arn = body
        .get("resourceArn")
        .and_then(Value::as_str)
        .expect("a minted resourceArn")
        .to_string();
    (router, arn)
}

/// ARNs of THIS service's vendor that name no stored record, differing along the
/// part of the ARN that fails to resolve: the region, the account, the partition and
/// the resource id. Each one is well-formed enough to reach the store lookup, which
/// is what makes it the counterpart of the live ARN for the rule under test. A value
/// this API cannot address at all is a different refusal (`resource_arn_validation`).
fn unresolved_arns(live: &str) -> Vec<(&'static str, String)> {
    vec![
        (
            "an id this emulator never minted",
            format!(
                "arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:deployment-parameter/dp-ffffffffffffffff"
            ),
        ),
        (
            "the live ARN in another region",
            live.replace(REGION, "eu-central-1"),
        ),
        (
            "the live ARN in another account",
            live.replace(ACCOUNT, "000000000001"),
        ),
        (
            "the live ARN in another partition",
            live.replace("arn:aws:", "arn:aws-cn:"),
        ),
    ]
}

/// Tag maps the request-level judgement refuses, spanning what that judgement
/// generalises over: what is at fault (the number of entries, or one side of one
/// pair), which bound it crosses (the number of entries, the lower bound of a key,
/// the upper bound of a key, the upper bound of a value), by how much, whether the
/// offending pair arrives alone or beside legal ones, and whether it sorts first or
/// last among them. The non-ASCII row is there because the length bounds count
/// CHARACTERS, not the bytes UTF-8 took to carry them.
///
/// Each row carries the wire code its own refusal has: the count rows are the tagging
/// front end's, the rest this service's modelled input error.
///
/// `RESERVED_TAG_KEY_PREFIX`, `TAG_KEY_MAX_LEN`, `TAG_VALUE_MAX_LEN` and
/// `MAX_TAGS_PER_RESOURCE` are the service's own statements of AWS's tagging limits;
/// the `reserved_tag_keys` and `deployment_parameters` suites pin what each one is.
fn refused_by_the_input() -> Vec<(&'static str, BTreeMap<String, String>, &'static str)> {
    let long_key = "k".repeat(TAG_KEY_MAX_LEN + 1);
    vec![
        // The COUNT the request itself carries. Applying a tag set can only leave a
        // resource holding at least as many tags as the set, so a map past the
        // per-resource cap could not be stored on any resource in any state. The
        // merge's own cap — what the set would leave on top of the tags a record
        // ALREADY holds — is a different judgement and keeps needing the record
        // (the `deployment_parameters` suite pins that one).
        (
            "a map one entry past the per-resource cap",
            (0..=MAX_TAGS_PER_RESOURCE)
                .map(|index| (format!("over-{index:03}"), "v".to_string()))
                .collect(),
            BAD_REQUEST_CODE,
        ),
        (
            "a map far past the per-resource cap",
            (0..MAX_TAGS_PER_RESOURCE * 4)
                .map(|index| (format!("over-{index:03}"), "v".to_string()))
                .collect(),
            BAD_REQUEST_CODE,
        ),
        (
            "an empty key alone",
            tag_map(&[("", "v")]),
            "ValidationException",
        ),
        (
            "an empty key beside a legal pair",
            tag_map(&[("", "v"), ("team", "payments")]),
            "ValidationException",
        ),
        (
            "a key one character past its maximum",
            [(long_key.clone(), "v".to_string())].into_iter().collect(),
            "ValidationException",
        ),
        (
            "a key far past its maximum",
            [("k".repeat(TAG_KEY_MAX_LEN * 4), "v".to_string())]
                .into_iter()
                .collect(),
            "ValidationException",
        ),
        (
            // Sorts AFTER the legal pair, so the refusal cannot depend on the
            // offending entry being the first one walked.
            "an over-long key sorting last",
            [
                ("alpha".to_string(), "v".to_string()),
                (long_key.clone(), "v".to_string()),
            ]
            .into_iter()
            .collect(),
            "ValidationException",
        ),
        (
            "a key of non-ASCII characters one past its maximum",
            [("é".repeat(TAG_KEY_MAX_LEN + 1), "v".to_string())]
                .into_iter()
                .collect(),
            "ValidationException",
        ),
        (
            "a value one character past its maximum",
            [("k".to_string(), "v".repeat(TAG_VALUE_MAX_LEN + 1))]
                .into_iter()
                .collect(),
            "ValidationException",
        ),
        (
            "a value far past its maximum",
            [("k".to_string(), "v".repeat(TAG_VALUE_MAX_LEN * 4))]
                .into_iter()
                .collect(),
            "ValidationException",
        ),
        (
            "a value of non-ASCII characters one past its maximum",
            [("k".to_string(), "é".repeat(TAG_VALUE_MAX_LEN + 1))]
                .into_iter()
                .collect(),
            "ValidationException",
        ),
        (
            "a key in the namespace AWS reserves",
            [(
                format!("{RESERVED_TAG_KEY_PREFIX}cloudformation:stack-name"),
                "mystack".to_string(),
            )]
            .into_iter()
            .collect(),
            "ValidationException",
        ),
        (
            "a reserved key beside a legal pair",
            [
                ("team".to_string(), "payments".to_string()),
                (
                    format!("{RESERVED_TAG_KEY_PREFIX}region"),
                    "v".to_string(),
                ),
            ]
            .into_iter()
            .collect(),
            "ValidationException",
        ),
    ]
}

/// Tag maps the request-level judgement ADMITS, at and inside the bounds the rows
/// above cross: the shortest key, each bound exactly, the empty value (a tag's key
/// is what must carry text) and a non-ASCII key measured in characters.
fn admitted_by_the_input() -> Vec<(&'static str, BTreeMap<String, String>)> {
    vec![
        ("the shortest key", tag_map(&[("k", "v")])),
        (
            "a key exactly at its maximum",
            [("k".repeat(TAG_KEY_MAX_LEN), "v".to_string())]
                .into_iter()
                .collect(),
        ),
        (
            "a value exactly at its maximum",
            [("at-max".to_string(), "v".repeat(TAG_VALUE_MAX_LEN))]
                .into_iter()
                .collect(),
        ),
        ("an empty value", tag_map(&[("empty", "")])),
        (
            "a non-ASCII key exactly at its maximum",
            [("é".repeat(TAG_KEY_MAX_LEN), "v".to_string())]
                .into_iter()
                .collect(),
        ),
        (
            "several legal pairs at once",
            tag_map(&[("owner", "team"), ("punct", "a/b+c=d.e:f@g_h-i")]),
        ),
    ]
}

#[tokio::test]
async fn a_tag_set_the_input_refuses_is_refused_identically_whether_the_arn_resolves() {
    let (router, live) = emulator_with_one_parameter().await;
    let stored = tag_map(&[(STORED_KEY, STORED_VALUE)]);

    for (case, tags, expected_code) in refused_by_the_input() {
        // The answer the SAME tag set earns against the record that exists: the
        // reference every unresolved row below is compared with.
        let (status, resolved) = tag(&router, &live, &tags).await;
        assert_eq!(
            status, 400,
            "case {case:?}/resolved: expected HTTP 400, got {status} {resolved:?}"
        );
        assert_eq!(
            error_code(&resolved),
            expected_code,
            "case {case:?}/resolved: expected {expected_code}, got {resolved:?}"
        );
        if expected_code == "ValidationException" {
            assert_eq!(
                field_name(&resolved),
                "tags",
                "case {case:?}/resolved: expected fieldName \"tags\", got {resolved:?}"
            );
        }

        for (arn_case, arn) in unresolved_arns(&live) {
            assert_ne!(
                arn, live,
                "case {case:?}/{arn_case}: the row must not name the stored record"
            );
            let (status, answered) = tag(&router, &arn, &tags).await;
            assert_eq!(
                status, 400,
                "case {case:?}/{arn_case}: a tag set the input refuses must be 400 \
                 whatever the store holds, got {status} {answered:?}"
            );
            assert_eq!(
                error_code(&answered),
                expected_code,
                "case {case:?}/{arn_case}: expected {expected_code}, got {answered:?}"
            );
            assert_eq!(
                message(&answered),
                message(&resolved),
                "case {case:?}/{arn_case}: expected the message the resolved ARN earns \
                 ({:?}), got {answered:?}",
                message(&resolved)
            );
            assert_eq!(
                field_name(&answered),
                field_name(&resolved),
                "case {case:?}/{arn_case}: expected fieldName {:?}, got {answered:?}",
                field_name(&resolved)
            );
        }

        // Nothing was written by any of those calls.
        let (status, listed) = list_tags(&router, &live).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the stored record must still read back, got {status} {listed:?}"
        );
        assert_eq!(
            tags_of(&listed),
            stored,
            "case {case:?}: a refused tag write must store nothing, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn the_count_the_request_carries_is_judged_at_the_number_a_resource_may_hold() {
    // The bound itself, from below and from above, against ARNs naming no record: at
    // and under the cap the request decides nothing on its own, so the answer is the
    // store's not-found; past it the request alone is already refused. Every entry of
    // every row is a legal pair, so the only thing a row varies is how many there are.
    let (router, live) = emulator_with_one_parameter().await;
    let rows: Vec<(&str, usize, u16)> = vec![
        ("one tag", 1, 404),
        ("one short of the cap", MAX_TAGS_PER_RESOURCE - 1, 404),
        ("exactly the cap", MAX_TAGS_PER_RESOURCE, 404),
        ("one past the cap", MAX_TAGS_PER_RESOURCE + 1, 400),
        ("twice the cap", MAX_TAGS_PER_RESOURCE * 2, 400),
    ];

    for (case, count, expected_status) in rows {
        let tags: BTreeMap<String, String> = (0..count)
            .map(|index| (format!("count-{index:03}"), "v".to_string()))
            .collect();
        assert_eq!(
            tags.len(),
            count,
            "case {case:?}: the row must carry {count} distinct keys, got {}",
            tags.len()
        );
        for (arn_case, arn) in unresolved_arns(&live) {
            let (status, answered) = tag(&router, &arn, &tags).await;
            assert_eq!(
                status, expected_status,
                "case {case:?}/{arn_case}: expected HTTP {expected_status}, got {status} \
                 {answered:?}"
            );
            let expected_code = if expected_status == 400 {
                BAD_REQUEST_CODE
            } else {
                NOT_FOUND_CODE
            };
            assert_eq!(
                error_code(&answered),
                expected_code,
                "case {case:?}/{arn_case}: expected code {expected_code}, got {answered:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_record_the_clock_has_expired_answers_exactly_as_an_unknown_arn_does() {
    // A record written with a date already past is expired the moment it is stored, so
    // its ARN is a THIRD state of the store — one the handler resolves to nothing only
    // after materialising the expiry — and the rule may not tell it apart from an ARN
    // that was never minted: what the request decides on its own is still a 400, and
    // what it does not is still the not-found.
    let (router, engine) = emulator();
    provision(&engine, CATALOG, "agmt-expired", &["prod-expired"]);
    let (status, body) = put(
        &router,
        CATALOG,
        "prod-expired",
        &json!({
            "agreementId": "agmt-expired",
            "deploymentParameter": { "name": "Expired", "secretString": "s" },
            "tags": { STORED_KEY: STORED_VALUE },
            "expirationDate": rfc3339(PAST_2020),
        }),
    )
    .await;
    assert_eq!(status, 200, "the setup Put must succeed, got {body:?}");
    let expired = body
        .get("resourceArn")
        .and_then(Value::as_str)
        .expect("a minted resourceArn")
        .to_string();

    for (case, tags, expected_code) in refused_by_the_input() {
        let (status, answered) = tag(&router, &expired, &tags).await;
        assert_eq!(
            status, 400,
            "case {case:?}/expired: a tag set the input refuses must be 400 whatever the \
             store holds, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            expected_code,
            "case {case:?}/expired: expected {expected_code}, got {answered:?}"
        );
        if expected_code == "ValidationException" {
            assert_eq!(
                field_name(&answered),
                "tags",
                "case {case:?}/expired: expected fieldName tags, got {answered:?}"
            );
        }
    }

    for (case, tags) in admitted_by_the_input() {
        let (status, answered) = tag(&router, &expired, &tags).await;
        assert_eq!(
            status, 404,
            "case {case:?}/expired: a legal tag set against an expired record must stay \
             not-found, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            NOT_FOUND_CODE,
            "case {case:?}/expired: expected code {NOT_FOUND_CODE}, got {answered:?}"
        );
    }
}

#[tokio::test]
async fn a_tag_set_the_input_admits_still_reaches_the_store_lookup() {
    let (router, live) = emulator_with_one_parameter().await;
    let mut expected = tag_map(&[(STORED_KEY, STORED_VALUE)]);

    for (case, tags) in admitted_by_the_input() {
        // Against an ARN naming no record, a legal tag set carries no verdict of its
        // own: the answer is this service's not-found, never a bad request.
        for (arn_case, arn) in unresolved_arns(&live) {
            let (status, answered) = tag(&router, &arn, &tags).await;
            assert_eq!(
                status, 404,
                "case {case:?}/{arn_case}: a legal tag set against an unknown ARN must \
                 stay not-found, got {status} {answered:?}"
            );
            assert_eq!(
                error_code(&answered),
                NOT_FOUND_CODE,
                "case {case:?}/{arn_case}: expected code {NOT_FOUND_CODE}, got {answered:?}"
            );
        }

        // And against the record that exists it is the merge's success, so the rows
        // above are inside the bounds rather than merely unreached.
        let (status, answered) = tag(&router, &live, &tags).await;
        assert_eq!(
            status, 204,
            "case {case:?}/resolved: a legal tag set must be accepted, got {status} {answered:?}"
        );
        expected.extend(tags.clone());
        let (_, listed) = list_tags(&router, &live).await;
        assert_eq!(
            tags_of(&listed),
            expected,
            "case {case:?}: the merge must leave exactly the merged map, got {listed:?}"
        );
    }
}
