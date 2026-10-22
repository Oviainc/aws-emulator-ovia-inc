// SPDX-License-Identifier: CC0-1.0

//! What a WAFv2 refusal SAYS: one constant sentence per exception type.
//!
//! Every WAFv2 exception shape here models its detail as a lone `Message` (the
//! limits error adds an optional `SourceType`), and the model documents ONE
//! sentence for the SHAPE itself rather than a template with a slot for the
//! resource that was named — so the message belongs to the exception TYPE, not to
//! the call. These suites therefore assert two things over a table of refusals
//! spanning the operations, the resource kinds, both scopes and both addressing
//! forms: the body carries that type's one sentence, and it discloses nothing the
//! request supplied (no name, id, ARN, scope or lock token).
//!
//! The sentences for the duplicate-item, optimistic-lock and nonexistent-item
//! exceptions are the wire wording recorded from WAF; the associated-item and
//! limits sentences are the ones the model documents for those shapes
//! (`smithy_query(query="shape",shape="WAFAssociatedItemException")`). They are
//! spelled out here as literals, independently of the service's own constants, so
//! this suite pins the text and not merely its reuse.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::MAX_IP_SET_ADDRESSES;

use crate::web_acl_lifecycle::{create_ip_set, create_web_acl, rule, visibility, Waf};

/// `WAFDuplicateItemException`'s sentence, as recorded from WAF.
const DUPLICATE_ITEM: &str = "AWS WAF couldn’t perform the operation because some \
     resource in your request is a duplicate of an existing one.";

/// `WAFOptimisticLockException`'s sentence, as recorded from WAF.
const OPTIMISTIC_LOCK: &str = "AWS WAF couldn’t save your changes because someone \
     changed the resource after you started to edit it. Reapply your changes.";

/// `WAFNonexistentItemException`'s sentence, as recorded from WAF.
const NONEXISTENT_ITEM: &str =
    "AWS WAF couldn’t perform the operation because your resource doesn’t exist.";

/// `WAFAssociatedItemException`'s sentence, as the model documents it.
const ASSOCIATED_ITEM: &str = "WAF couldn’t perform the operation because your \
     resource is being used by another resource or it’s associated with another \
     resource.";

/// `WAFLimitsExceededException`'s sentence, as the model documents it.
const LIMITS_EXCEEDED: &str = "WAF couldn’t perform the operation because you \
     exceeded your resource limit. For example, the maximum number of WebACL \
     objects that you can create for an Amazon Web Services account. For more \
     information, see WAF quotas in the WAF Developer Guide.";

/// An `EntityId`-shaped id (`^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$`) that
/// no create minted, so it names nothing stored.
const MISSING_ID: &str = "00000000-1111-2222-3333-444444444444";

/// A `LockToken`-shaped token that is not the one any stored resource holds.
const STALE_LOCK: &str = "99999999-8888-7777-6666-555555555555";

/// A well-formed wafv2 web ACL ARN naming nothing stored.
const MISSING_ACL_ARN: &str = "arn:aws:wafv2:us-west-2:000000000000:regional/webacl/\
     probe-absent-acl/00000000-1111-2222-3333-444444444444";

/// A well-formed wafv2 IP set ARN naming nothing stored, for a rule reference that
/// cannot be resolved.
const MISSING_IP_SET_ARN: &str = "arn:aws:wafv2:us-west-2:000000000000:regional/ipset/\
     probe-absent-ips/00000000-1111-2222-3333-444444444444";

/// One refusal to drive: the call, the exception type it must answer, and the
/// values the request carried that the answer must not disclose.
struct Refusal {
    case: &'static str,
    operation: &'static str,
    exception: &'static str,
    request: Value,
    request_values: Vec<String>,
}

/// The one sentence `exception` always carries.
fn constant_message(exception: &str) -> &'static str {
    match exception {
        "WAFDuplicateItemException" => DUPLICATE_ITEM,
        "WAFOptimisticLockException" => OPTIMISTIC_LOCK,
        "WAFNonexistentItemException" => NONEXISTENT_ITEM,
        "WAFAssociatedItemException" => ASSOCIATED_ITEM,
        "WAFLimitsExceededException" => LIMITS_EXCEEDED,
        other => panic!("no constant sentence is pinned for {other:?}"),
    }
}

/// The awsJson error body's message member, whichever casing the shape models.
fn error_message(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Every modelled WAFv2 refusal this slice can produce answers the one sentence
/// its exception type carries, and nothing from the request.
///
/// One row per (operation, exception type) pair the implemented operations can
/// reach, with the rows of a type differing in resource kind (IP set / web ACL),
/// scope (`REGIONAL` / `CLOUDFRONT`), how the resource is addressed (ARN or
/// Name+Scope+Id) and which member is at fault, so a message built from any of
/// them would show up as a difference between rows.
#[tokio::test]
async fn every_waf_exception_carries_its_one_constant_message() {
    let waf = Waf::new();

    let (regional_ips_id, _regional_ips_arn, _regional_ips_lock) =
        create_ip_set(&waf, "REGIONAL", "probe-ips-regional").await;
    let (cloudfront_ips_id, _cloudfront_ips_arn, _cloudfront_ips_lock) =
        create_ip_set(&waf, "CLOUDFRONT", "probe-ips-cloudfront").await;
    let (regional_acl_id, _regional_acl_arn, regional_acl_lock) =
        create_web_acl(&waf, "REGIONAL", "probe-acl-regional", json!([])).await;
    let (cloudfront_acl_id, _cloudfront_acl_arn, _cloudfront_acl_lock) =
        create_web_acl(&waf, "CLOUDFRONT", "probe-acl-cloudfront", json!([])).await;
    let (referenced_ips_id, referenced_ips_arn, referenced_ips_lock) =
        create_ip_set(&waf, "REGIONAL", "probe-ips-referenced").await;
    let (_referrer_id, _referrer_arn, _referrer_lock) = create_web_acl(
        &waf,
        "REGIONAL",
        "probe-acl-referrer",
        json!([rule(
            "block-listed-ips",
            json!({ "IPSetReferenceStatement": { "ARN": referenced_ips_arn } })
        )]),
    )
    .await;

    // One address past the published per-set quota, for the limits row.
    let oversized: Vec<String> = (0..=MAX_IP_SET_ADDRESSES)
        .map(|index| format!("10.{}.{}.0/24", index / 256, index % 256))
        .collect();

    let refusals = vec![
        Refusal {
            case: "CreateIPSet, a name already taken in REGIONAL",
            operation: "CreateIPSet",
            exception: "WAFDuplicateItemException",
            request: json!({
                "Name": "probe-ips-regional",
                "Scope": "REGIONAL",
                "IPAddressVersion": "IPV4",
                "Addresses": ["10.1.0.0/16"],
            }),
            request_values: vec!["probe-ips-regional".to_string(), "REGIONAL".to_string()],
        },
        Refusal {
            case: "CreateIPSet, a name already taken in CLOUDFRONT",
            operation: "CreateIPSet",
            exception: "WAFDuplicateItemException",
            request: json!({
                "Name": "probe-ips-cloudfront",
                "Scope": "CLOUDFRONT",
                "IPAddressVersion": "IPV6",
                "Addresses": ["2001:db8::/32"],
            }),
            request_values: vec![
                "probe-ips-cloudfront".to_string(),
                "CLOUDFRONT".to_string(),
            ],
        },
        Refusal {
            case: "CreateWebACL, a name already taken in REGIONAL",
            operation: "CreateWebACL",
            exception: "WAFDuplicateItemException",
            request: json!({
                "Name": "probe-acl-regional",
                "Scope": "REGIONAL",
                "DefaultAction": { "Allow": {} },
                "VisibilityConfig": visibility("probe-acl-regional"),
            }),
            request_values: vec!["probe-acl-regional".to_string(), "REGIONAL".to_string()],
        },
        Refusal {
            case: "CreateWebACL, a name already taken in CLOUDFRONT",
            operation: "CreateWebACL",
            exception: "WAFDuplicateItemException",
            request: json!({
                "Name": "probe-acl-cloudfront",
                "Scope": "CLOUDFRONT",
                "DefaultAction": { "Block": {} },
                "VisibilityConfig": visibility("probe-acl-cloudfront"),
            }),
            request_values: vec![
                "probe-acl-cloudfront".to_string(),
                "CLOUDFRONT".to_string(),
            ],
        },
        Refusal {
            case: "GetIPSet, an id that was never minted",
            operation: "GetIPSet",
            exception: "WAFNonexistentItemException",
            request: json!({
                "Name": "probe-ips-regional",
                "Scope": "REGIONAL",
                "Id": MISSING_ID,
            }),
            request_values: vec![
                "probe-ips-regional".to_string(),
                MISSING_ID.to_string(),
                "REGIONAL".to_string(),
            ],
        },
        Refusal {
            case: "GetIPSet, a stored id under the wrong name",
            operation: "GetIPSet",
            exception: "WAFNonexistentItemException",
            request: json!({
                "Name": "probe-ips-absent",
                "Scope": "CLOUDFRONT",
                "Id": cloudfront_ips_id,
            }),
            request_values: vec![
                "probe-ips-absent".to_string(),
                cloudfront_ips_id.clone(),
                "CLOUDFRONT".to_string(),
            ],
        },
        Refusal {
            case: "GetWebACL, a triple that names nothing",
            operation: "GetWebACL",
            exception: "WAFNonexistentItemException",
            request: json!({
                "Name": "probe-acl-absent",
                "Scope": "REGIONAL",
                "Id": MISSING_ID,
            }),
            request_values: vec!["probe-acl-absent".to_string(), MISSING_ID.to_string()],
        },
        Refusal {
            case: "GetWebACL, a well-formed ARN that names nothing",
            operation: "GetWebACL",
            exception: "WAFNonexistentItemException",
            request: json!({ "ARN": MISSING_ACL_ARN }),
            request_values: vec![MISSING_ACL_ARN.to_string(), "probe-absent-acl".to_string()],
        },
        Refusal {
            case: "DeleteIPSet, a triple that names nothing",
            operation: "DeleteIPSet",
            exception: "WAFNonexistentItemException",
            request: json!({
                "Name": "probe-ips-absent",
                "Scope": "REGIONAL",
                "Id": MISSING_ID,
                "LockToken": STALE_LOCK,
            }),
            request_values: vec![
                "probe-ips-absent".to_string(),
                MISSING_ID.to_string(),
                STALE_LOCK.to_string(),
            ],
        },
        Refusal {
            case: "DeleteWebACL, a triple that names nothing",
            operation: "DeleteWebACL",
            exception: "WAFNonexistentItemException",
            request: json!({
                "Name": "probe-acl-absent",
                "Scope": "CLOUDFRONT",
                "Id": MISSING_ID,
                "LockToken": STALE_LOCK,
            }),
            request_values: vec!["probe-acl-absent".to_string(), MISSING_ID.to_string()],
        },
        Refusal {
            case: "UpdateWebACL, a triple that names nothing",
            operation: "UpdateWebACL",
            exception: "WAFNonexistentItemException",
            request: json!({
                "Name": "probe-acl-absent",
                "Scope": "REGIONAL",
                "Id": MISSING_ID,
                "DefaultAction": { "Allow": {} },
                "VisibilityConfig": visibility("probe-acl-absent"),
                "LockToken": STALE_LOCK,
            }),
            request_values: vec!["probe-acl-absent".to_string(), MISSING_ID.to_string()],
        },
        Refusal {
            case: "CreateWebACL, a rule reference that resolves to nothing",
            operation: "CreateWebACL",
            exception: "WAFNonexistentItemException",
            request: json!({
                "Name": "probe-acl-dangling",
                "Scope": "REGIONAL",
                "DefaultAction": { "Allow": {} },
                "VisibilityConfig": visibility("probe-acl-dangling"),
                "Rules": [rule(
                    "block-listed-ips",
                    json!({ "IPSetReferenceStatement": { "ARN": MISSING_IP_SET_ARN } })
                )],
            }),
            request_values: vec![
                MISSING_IP_SET_ARN.to_string(),
                "probe-acl-dangling".to_string(),
            ],
        },
        Refusal {
            case: "UpdateWebACL, a rule reference that resolves to nothing",
            operation: "UpdateWebACL",
            exception: "WAFNonexistentItemException",
            request: json!({
                "Name": "probe-acl-regional",
                "Scope": "REGIONAL",
                "Id": regional_acl_id,
                "DefaultAction": { "Allow": {} },
                "VisibilityConfig": visibility("probe-acl-regional"),
                "LockToken": regional_acl_lock,
                "Rules": [rule(
                    "block-listed-ips",
                    json!({ "IPSetReferenceStatement": { "ARN": MISSING_IP_SET_ARN } })
                )],
            }),
            request_values: vec![MISSING_IP_SET_ARN.to_string(), regional_acl_id.clone()],
        },
        Refusal {
            case: "DeleteIPSet, a LockToken that is not the current one",
            operation: "DeleteIPSet",
            exception: "WAFOptimisticLockException",
            request: json!({
                "Name": "probe-ips-regional",
                "Scope": "REGIONAL",
                "Id": regional_ips_id,
                "LockToken": STALE_LOCK,
            }),
            request_values: vec![
                "probe-ips-regional".to_string(),
                regional_ips_id.clone(),
                STALE_LOCK.to_string(),
            ],
        },
        Refusal {
            case: "DeleteWebACL, a LockToken that is not the current one",
            operation: "DeleteWebACL",
            exception: "WAFOptimisticLockException",
            request: json!({
                "Name": "probe-acl-cloudfront",
                "Scope": "CLOUDFRONT",
                "Id": cloudfront_acl_id,
                "LockToken": STALE_LOCK,
            }),
            request_values: vec![
                "probe-acl-cloudfront".to_string(),
                cloudfront_acl_id.clone(),
                STALE_LOCK.to_string(),
            ],
        },
        Refusal {
            case: "UpdateWebACL, a LockToken that is not the current one",
            operation: "UpdateWebACL",
            exception: "WAFOptimisticLockException",
            request: json!({
                "Name": "probe-acl-regional",
                "Scope": "REGIONAL",
                "Id": regional_acl_id,
                "DefaultAction": { "Block": {} },
                "VisibilityConfig": visibility("probe-acl-regional"),
                "LockToken": STALE_LOCK,
            }),
            request_values: vec![
                "probe-acl-regional".to_string(),
                regional_acl_id.clone(),
                STALE_LOCK.to_string(),
            ],
        },
        Refusal {
            case: "DeleteIPSet, a set a live rule tree still references",
            operation: "DeleteIPSet",
            exception: "WAFAssociatedItemException",
            request: json!({
                "Name": "probe-ips-referenced",
                "Scope": "REGIONAL",
                "Id": referenced_ips_id,
                "LockToken": referenced_ips_lock,
            }),
            request_values: vec![
                "probe-ips-referenced".to_string(),
                "probe-acl-referrer".to_string(),
                referenced_ips_id.clone(),
                referenced_ips_arn.clone(),
            ],
        },
        Refusal {
            case: "CreateIPSet, one address past the per-set quota",
            operation: "CreateIPSet",
            exception: "WAFLimitsExceededException",
            request: json!({
                "Name": "probe-ips-oversized",
                "Scope": "REGIONAL",
                "IPAddressVersion": "IPV4",
                "Addresses": oversized,
            }),
            request_values: vec!["probe-ips-oversized".to_string()],
        },
    ];

    let mut by_exception: BTreeMap<&str, Vec<(&str, String)>> = BTreeMap::new();
    for refusal in &refusals {
        let body = waf
            .refused(
                refusal.operation,
                refusal.request.clone(),
                refusal.exception,
                refusal.case,
            )
            .await;
        let message = error_message(&body);
        let expected = constant_message(refusal.exception);
        assert_eq!(
            message,
            expected,
            "case {:?}: {} must carry its constant sentence {expected:?}, got {message:?}",
            refusal.case,
            refusal.exception
        );
        let rendered = body.to_string();
        for value in &refusal.request_values {
            assert!(
                !rendered.contains(value.as_str()),
                "case {:?}: the refusal must disclose nothing the request supplied, \
                 but its body names {value:?}: {rendered}",
                refusal.case
            );
        }
        by_exception
            .entry(refusal.exception)
            .or_default()
            .push((refusal.case, message));
    }

    // The table must exercise every exception type a constant sentence is pinned
    // for, and each type must have answered the SAME bytes on every one of its
    // rows — which is the property, independent of what the sentence says.
    let covered: Vec<&str> = by_exception.keys().copied().collect();
    assert_eq!(
        covered,
        vec![
            "WAFAssociatedItemException",
            "WAFDuplicateItemException",
            "WAFLimitsExceededException",
            "WAFNonexistentItemException",
            "WAFOptimisticLockException",
        ],
        "the table must cover every exception type with a constant sentence, covered {covered:?}"
    );
    for (exception, rows) in &by_exception {
        let (first_case, first_message) = &rows[0];
        for (case, message) in rows {
            assert_eq!(
                message, first_message,
                "case {case:?}: {exception} must answer exactly what case \
                 {first_case:?} answered ({first_message:?}), got {message:?}"
            );
        }
    }
}

/// The nonexistent-item sentence does not vary with the request that trips it:
/// the same bytes for every name, id, scope and operation.
///
/// The rows span what an `EntityName` can be —
/// `smithy_query(query="shape",shape="EntityName")` is `^[\w\-]+$`, length 1 to
/// 128 — at both published length bounds and over each character class the pattern
/// allows, crossed with both `Scope` values, several `EntityId`s and the three
/// operations that look a resource up by triple.
#[tokio::test]
async fn the_nonexistent_item_message_is_identical_whatever_was_looked_up() {
    let waf = Waf::new();

    // Both `EntityName` length bounds and one name per character class the
    // pattern allows (letters, digits, `_`, `-`).
    let names: Vec<String> = vec![
        "a".to_string(),
        "9".to_string(),
        "_".to_string(),
        "probe-absent-resource".to_string(),
        "A".repeat(128),
        "x_9-".repeat(32),
    ];
    // `EntityId` is a lowercase hyphenated UUID; none of these was minted here.
    let ids = [
        MISSING_ID,
        "ffffffff-ffff-ffff-ffff-ffffffffffff",
        "0a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d",
    ];
    let operations = ["GetIPSet", "GetWebACL", "DeleteWebACL"];

    let mut answers: Vec<(String, String)> = Vec::new();
    for (index, name) in names.iter().enumerate() {
        for scope in ["REGIONAL", "CLOUDFRONT"] {
            let id = ids[index % ids.len()];
            for operation in operations {
                let case = format!(
                    "{operation} {scope} name of {} chars, id {id}",
                    name.chars().count()
                );
                let mut request = json!({ "Name": name, "Scope": scope, "Id": id });
                if operation == "DeleteWebACL" {
                    request["LockToken"] = json!(STALE_LOCK);
                }
                let body = waf
                    .refused(
                        operation,
                        request,
                        "WAFNonexistentItemException",
                        case.as_str(),
                    )
                    .await;
                answers.push((case, error_message(&body)));
            }
        }
    }

    for (case, message) in &answers {
        assert_eq!(
            message, NONEXISTENT_ITEM,
            "case {case:?}: expected the constant sentence {NONEXISTENT_ITEM:?}, \
             got {message:?}"
        );
    }
}
