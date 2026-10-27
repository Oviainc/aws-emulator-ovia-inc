// SPDX-License-Identifier: CC0-1.0

//! The ONE sequence every operation that addresses a deployment parameter by a
//! caller-supplied `resourceArn` runs before it can touch the record.
//!
//! The rule under test: the three operations that bind `resourceArn` as a required
//! `@httpLabel`
//! (`smithy_query(query="operation",operation="ListTagsForResource,TagResource,UntagResource")`:
//! `GET /tags/{resourceArn}`, `POST /tags/{resourceArn}`, `DELETE /tags/{resourceArn}`)
//! resolve their target through the SAME steps in the same order — the ARN is judged
//! as an input first, the rest of the request's own content next, then the store is
//! taken, the deletion the clock has already made due is materialised, and the record
//! is resolved or the service's not-found failure answered. Only what each operation
//! then does with the resolved record differs.
//!
//! So for one `resourceArn` the three must agree: the same status, the same wire error
//! code and the same message wherever the sequence refuses, and each one's own success
//! for a record the sequence resolves. That is what pins the sequence from three sides
//! — a step added, dropped or reordered for one operation would show up here as the
//! three disagreeing.
//!
//! The rows span what the sequence decides between: a value this API cannot address
//! at all (the input judgement), an addressable ARN naming no record and an ARN whose
//! record the clock has already taken (the store's not-found), and records that are
//! live both with and without a date in the future (the success). The individual
//! rules those answers follow have their own suites — `resource_arn_validation` for
//! the input refusal, `not_found_error_code` for the not-found identity,
//! `deployment_parameter_expiry` for the expiry, `tag_input_judged_before_lookup` for
//! the tag judgement — and this suite asserts that one sequence serves them all.
//!
//! The harness (router, signed request, per-operation callers) is the one the
//! `deployment_parameters` suite owns.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::deployment_parameters::{
    emulator, error_code, field_name, list_tags, message, provision, put, tag, tag_map, tags_of,
    untag, ACCOUNT, CATALOG, REGION, SIGNING_NAME,
};
use crate::support::rfc3339;

/// The status and wire code an ARN this API cannot address earns, written out rather
/// than read from the crate so the assertion is not the implementation compared with
/// itself (`resource_arn_validation` owns the rule).
const BAD_REQUEST: (u16, &str) = (400, "BadRequestException");
/// The status and wire code an addressable ARN naming no record earns
/// (`not_found_error_code` owns the rule).
const NOT_FOUND: (u16, &str) = (404, "NotFoundException");
/// The modelled client-input error, for the rows that must earn it instead.
const VALIDATION: (u16, &str) = (400, "ValidationException");
/// `2020-01-01T00:00:00Z` as epoch seconds — a date long past, so a record carrying
/// it is already expired when the sequence samples the clock.
const PAST_2020: i64 = 1_577_836_800;
/// `2100-01-01T00:00:00Z` as epoch seconds — a date long in the future.
const FUTURE_2100: i64 = 4_102_444_800;
/// The tag every stored record below is created with.
const STORED: (&str, &str) = ("env", "dev");

/// What the sequence must answer for one `resourceArn`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// Every operation refuses with this status and wire code, identically.
    Refused(u16, &'static str),
    /// Every operation reaches the record and answers its own success.
    Resolved,
}

/// Store one deployment parameter under its own identity and report the ARN the
/// service minted for it. `expiration_date` is an epoch second, or `None` to omit
/// the member.
async fn store_parameter(
    router: &smithy_server_runtime::MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    slug: &str,
    expiration_date: Option<i64>,
) -> String {
    let product_id = format!("prod-{slug}");
    let agreement_id = format!("agmt-{slug}");
    provision(engine, CATALOG, &agreement_id, &[&product_id]);
    let mut body = json!({
        "agreementId": agreement_id,
        "deploymentParameter": { "name": format!("Parameter-{slug}"), "secretString": "s3cr3t" },
        "tags": { STORED.0: STORED.1 },
    });
    if let Some(at) = expiration_date {
        body.as_object_mut()
            .expect("an object body")
            .insert("expirationDate".to_string(), json!(rfc3339(at)));
    }
    let (status, created) = put(router, CATALOG, &product_id, &body).await;
    assert_eq!(
        status, 200,
        "case {slug:?}: the setup Put must succeed, got {status} {created:?}"
    );
    created
        .get("resourceArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {slug:?}: expected a minted resourceArn, got {created:?}"))
        .to_string()
}

#[tokio::test]
async fn the_three_operations_answer_one_resolution_sequence_for_one_resource_arn() {
    let (router, engine) = emulator();
    let live = store_parameter(&router, &engine, "live", None).await;
    let dated = store_parameter(&router, &engine, "dated", Some(FUTURE_2100)).await;
    let expired = store_parameter(&router, &engine, "expired", Some(PAST_2020)).await;

    // The rows span what the sequence decides between, in the order its steps decide
    // it: values the input judgement refuses (differing along which part of the ARN
    // form is wrong), ARNs it admits that the store does not hold (differing along
    // which field fails to resolve, including one whose record the clock has taken),
    // and records it resolves (with and without a date still ahead).
    let rows: Vec<(&str, String, Verdict)> = vec![
        (
            "an ARN of another vendor",
            format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret/dp-0000000000000001"),
            Verdict::Refused(BAD_REQUEST.0, BAD_REQUEST.1),
        ),
        (
            "a value that is no ARN at all",
            "dp-0000000000000001".to_string(),
            Verdict::Refused(BAD_REQUEST.0, BAD_REQUEST.1),
        ),
        (
            // Outside the modelled `ResourceArn` character class: a space is not in
            // `[a-zA-Z0-9:*/-]`.
            "an ARN of this vendor carrying a space",
            format!("arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:deployment parameter/dp-1"),
            Verdict::Refused(BAD_REQUEST.0, BAD_REQUEST.1),
        ),
        (
            "an id of this vendor that was never minted",
            format!("arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:deployment-parameter/dp-ffffffff"),
            Verdict::Refused(NOT_FOUND.0, NOT_FOUND.1),
        ),
        (
            "the live ARN in another region",
            live.replace(REGION, "eu-central-1"),
            Verdict::Refused(NOT_FOUND.0, NOT_FOUND.1),
        ),
        (
            "the live ARN in another account",
            live.replace(ACCOUNT, "000000000001"),
            Verdict::Refused(NOT_FOUND.0, NOT_FOUND.1),
        ),
        (
            "a record whose expirationDate has passed",
            expired.clone(),
            Verdict::Refused(NOT_FOUND.0, NOT_FOUND.1),
        ),
        (
            "a stored record with no expirationDate",
            live.clone(),
            Verdict::Resolved,
        ),
        (
            "a stored record whose expirationDate is still ahead",
            dated.clone(),
            Verdict::Resolved,
        ),
    ];

    for (case, resource_arn, verdict) in rows {
        // The reporting operation first, then the two writers: whatever the sequence
        // decides, it decides the same way for each.
        let (list_status, listed) = list_tags(&router, &resource_arn).await;
        let (tag_status, tagged) = tag(&router, &resource_arn, &tag_map(&[("owner", "team")])).await;
        let (untag_status, untagged) =
            untag(&router, &resource_arn, &[STORED.0.to_string()]).await;

        match verdict {
            Verdict::Refused(status, code) => {
                for (operation, answered_status, answered) in [
                    ("ListTagsForResource", list_status, &listed),
                    ("TagResource", tag_status, &tagged),
                    ("UntagResource", untag_status, &untagged),
                ] {
                    assert_eq!(
                        answered_status, status,
                        "case {case:?}/{operation}: expected HTTP {status}, got \
                         {answered_status} {answered:?}"
                    );
                    assert_eq!(
                        error_code(answered),
                        code,
                        "case {case:?}/{operation}: expected code {code}, got {answered:?}"
                    );
                    assert_eq!(
                        message(answered),
                        message(&listed),
                        "case {case:?}/{operation}: expected the message every operation \
                         answers ({:?}), got {answered:?}",
                        message(&listed)
                    );
                    assert!(
                        answered.get("tags").is_none(),
                        "case {case:?}/{operation}: a refusal carries no tags, got {answered:?}"
                    );
                }
            }
            Verdict::Resolved => {
                // Each operation's own success, over the record the sequence resolved:
                // the reporter answers the stored map, the two writers answer 204, and
                // what they wrote reads back through the reporter.
                assert_eq!(
                    (list_status, tags_of(&listed)),
                    (200, tag_map(&[STORED])),
                    "case {case:?}/ListTagsForResource: expected the stored tag map, \
                     got {list_status} {listed:?}"
                );
                assert_eq!(
                    tag_status, 204,
                    "case {case:?}/TagResource: expected HTTP 204, got {tag_status} {tagged:?}"
                );
                assert_eq!(
                    untag_status, 204,
                    "case {case:?}/UntagResource: expected HTTP 204, got \
                     {untag_status} {untagged:?}"
                );
                let (status, reread) = list_tags(&router, &resource_arn).await;
                assert_eq!(
                    (status, tags_of(&reread)),
                    (200, tag_map(&[("owner", "team")])),
                    "case {case:?}: the writes must leave exactly the merged map minus the \
                     removed key, got {status} {reread:?}"
                );
            }
        }
    }

    // The refused rows reached no record: the two records the sequence never resolved
    // still hold what their create left, and the expired one is still gone.
    let (status, listed) = list_tags(&router, &expired).await;
    assert_eq!(
        (status, error_code(&listed)),
        (NOT_FOUND.0, NOT_FOUND.1.to_string()),
        "the expired record must stay gone, got {status} {listed:?}"
    );
}

#[tokio::test]
async fn the_resource_arn_is_judged_before_the_rest_of_the_request() {
    let (router, engine) = emulator();
    let live = store_parameter(&router, &engine, "order", None).await;
    let unknown =
        format!("arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:deployment-parameter/dp-ffffffff");
    let unaddressable = format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret/dp-1");
    // A tag key in the namespace AWS reserves for its own generated tags: refused by
    // the request-only judgement, which the sequence runs AFTER the ARN and BEFORE the
    // store. `reserved_tag_keys` owns what that refusal says.
    let reserved: BTreeMap<String, String> =
        [("aws:cloudformation:stack-name".to_string(), "s".to_string())]
            .into_iter()
            .collect();
    let reserved_key = vec!["aws:cloudformation:stack-name".to_string()];

    // Each row pairs an ARN with the verdict the SEQUENCE owes it for a request whose
    // own content is refusable: the ARN is judged first, so a value this API cannot
    // address is the input refusal however bad the rest of the request is; an
    // addressable ARN lets the request-only judgement answer, which it does before the
    // store is consulted, so it does not matter that the ARN names no record.
    let rows: Vec<(&str, &str, (u16, &str))> = vec![
        (
            "a value this API cannot address, beside a reserved tag key",
            unaddressable.as_str(),
            BAD_REQUEST,
        ),
        (
            "an addressable ARN naming no record, beside a reserved tag key",
            unknown.as_str(),
            VALIDATION,
        ),
        (
            "the stored record's own ARN, beside a reserved tag key",
            live.as_str(),
            VALIDATION,
        ),
    ];

    for (case, resource_arn, (status, code)) in rows {
        let (tag_status, tagged) = tag(&router, resource_arn, &reserved).await;
        let (untag_status, untagged) = untag(&router, resource_arn, &reserved_key).await;
        for (operation, answered_status, answered, member) in [
            ("TagResource", tag_status, &tagged, "tags"),
            ("UntagResource", untag_status, &untagged, "tagKeys"),
        ] {
            assert_eq!(
                answered_status, status,
                "case {case:?}/{operation}: expected HTTP {status}, got \
                 {answered_status} {answered:?}"
            );
            assert_eq!(
                error_code(answered),
                code,
                "case {case:?}/{operation}: expected code {code}, got {answered:?}"
            );
            if code == VALIDATION.1 {
                // The modelled shape carries the member the refusal is about; the
                // front-end's ARN refusal is not that shape and carries none.
                assert_eq!(
                    field_name(answered),
                    member,
                    "case {case:?}/{operation}: expected fieldName {member:?}, got {answered:?}"
                );
            } else {
                assert_eq!(
                    field_name(answered),
                    "<no fieldName>",
                    "case {case:?}/{operation}: the input refusal is not the modelled shape, \
                     so it carries no fieldName, got {answered:?}"
                );
            }
        }
    }

    // None of those calls changed the record: the tag its create left is still all it
    // holds.
    let (status, listed) = list_tags(&router, &live).await;
    assert_eq!(
        (status, tags_of(&listed)),
        (200, tag_map(&[STORED])),
        "a refused call must change nothing, got {status} {listed:?}"
    );
}
