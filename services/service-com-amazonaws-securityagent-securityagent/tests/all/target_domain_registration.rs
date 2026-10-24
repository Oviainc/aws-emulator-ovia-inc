// SPDX-License-Identifier: CC0-1.0

//! Target domain registration in the SecurityAgent emulator, end to end over the wire:
//! `CreateTargetDomain` followed by `BatchGetTargetDomains`.
//!
//! Every call goes through the real `MultiServiceRouter` the emulator boots (see
//! `crate::support`), so nothing here can pass against a handler that answers a canned
//! body: a value must come back out of the shared store through
//! `BatchGetTargetDomains` before a case counts.
//!
//! Each case asserts a RULE over a TABLE of inputs rather than one recorded call: the
//! rows differ along the `DomainVerificationMethod` chosen, the form and the size of
//! the domain name, and the tag map — and the published DNS limits are exercised on
//! each bound and one character past it.

use std::collections::BTreeSet;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_securityagent_securityagent::state::{
    MAX_DOMAIN_LABEL_LEN, MAX_DOMAIN_NAME_LEN, MAX_TARGET_DOMAINS, TARGET_DOMAIN_STATUS_PENDING,
    VERIFICATION_METHOD_DNS_TXT, VERIFICATION_METHOD_HTTP_ROUTE, VERIFICATION_METHOD_PRIVATE_VPC,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{call, emulator, member, ok};

/// Every `DomainVerificationMethod` the model admits
/// (`smithy_query(query="shape",shape="DomainVerificationMethod")`: `DNS_TXT`,
/// `HTTP_ROUTE`, `PRIVATE_VPC`), so a row exists for each and a new value cannot be
/// added to the enum without this list going stale.
fn verification_methods() -> [&'static str; 3] {
    [
        VERIFICATION_METHOD_DNS_TXT,
        VERIFICATION_METHOD_HTTP_ROUTE,
        VERIFICATION_METHOD_PRIVATE_VPC,
    ]
}

/// A domain name of exactly `total` characters, built from maximum-length labels and a
/// final label that makes up the remainder.
fn domain_of_length(total: usize) -> String {
    let mut name = String::new();
    while name.len() + MAX_DOMAIN_LABEL_LEN < total {
        name.push_str(&"a".repeat(MAX_DOMAIN_LABEL_LEN));
        name.push('.');
    }
    name.push_str(&"b".repeat(total - name.len()));
    assert_eq!(
        name.len(),
        total,
        "the generated domain name must be exactly {total} characters"
    );
    name
}

/// The domain name forms the registration cases span: the shape of the name is what
/// `targetDomainName` generalises over (a bare `String` with no `@length` and no
/// `@pattern` in the model, per
/// `smithy_query(query="operation",operation="CreateTargetDomain")`), so the rows carry
/// an apex domain, deeper names, digits and hyphens, an IDN in its punycode form, mixed
/// case, and the two published DNS size limits exactly at their bound (RFC 1035 §2.3.4
/// with RFC 1123 §2.1: 253 characters overall, 63 per label).
fn domain_name_rows() -> Vec<(&'static str, String)> {
    vec![
        ("an apex domain", "example.com".to_string()),
        ("a subdomain", "shop.eu.example.co.uk".to_string()),
        ("a two-character label", "a.io".to_string()),
        (
            "a punycode IDN label",
            "xn--80ak6aa92e.example".to_string(),
        ),
        (
            "digits and hyphens in the labels",
            "123-api.internal-test.example".to_string(),
        ),
        ("mixed case", "ShOp.Example.COM".to_string()),
        (
            "a label at the 63-character limit",
            format!("{}.example.com", "a".repeat(MAX_DOMAIN_LABEL_LEN)),
        ),
        (
            "a name at the 253-character limit",
            domain_of_length(MAX_DOMAIN_NAME_LEN),
        ),
    ]
}

/// The tag maps the registration cases span: absent, empty, one, several.
fn tag_rows() -> Vec<(&'static str, Option<Value>)> {
    vec![
        ("no tags", None),
        ("an empty tag map", Some(json!({}))),
        ("one tag", Some(json!({"env": "test"}))),
        (
            "several tags",
            Some(json!({"env": "prod", "team": "appsec", "cost-center": "4711"})),
        ),
    ]
}

/// `CreateTargetDomain` for `domain` with `method`, requiring HTTP 200.
async fn create(
    router: &MultiServiceRouter,
    domain: &str,
    method: &str,
    tags: Option<&Value>,
    case: &str,
) -> Value {
    let mut request = json!({"targetDomainName": domain, "verificationMethod": method});
    if let Some(tags) = tags {
        request["tags"] = tags.clone();
    }
    ok(router, "CreateTargetDomain", &request, case).await
}

/// `BatchGetTargetDomains` for `ids`, requiring HTTP 200.
async fn batch_get(router: &MultiServiceRouter, ids: &[String], case: &str) -> Value {
    ok(
        router,
        "BatchGetTargetDomains",
        &json!({"targetDomainIds": ids}),
        case,
    )
    .await
}

/// The members every projection of a target domain carries, however it was read.
///
/// RULE, applied to both the create response and the read: a registered target domain
/// reports the domain name it was registered with, the `PENDING` status, a creation
/// time in the modelled `date-time` form, no verification time, and the challenge its
/// own verification method requires — and nothing else.
///
/// Returns the verification token the challenge carries, if that method has one.
fn assert_pending_projection(
    payload: &Value,
    expected_id: &str,
    domain: &str,
    method: &str,
    case: &str,
) -> Option<String> {
    assert_eq!(
        member(payload, "targetDomainId").as_deref(),
        Some(expected_id),
        "case {case:?}: the projection names another target domain: {payload}"
    );
    assert_eq!(
        member(payload, "domainName").as_deref(),
        Some(domain),
        "case {case:?}: expected domainName {domain:?}, got {payload}"
    );
    assert_eq!(
        member(payload, "verificationStatus").as_deref(),
        Some(TARGET_DOMAIN_STATUS_PENDING),
        "case {case:?}: expected verificationStatus {TARGET_DOMAIN_STATUS_PENDING:?}, got \
         {payload}"
    );
    // The recorded wire reports the one stored status under `status` as well; the two
    // read the same value (`crate::recorded_wire_form` pins the rule).
    assert_eq!(
        member(payload, "status"),
        member(payload, "verificationStatus"),
        "case {case:?}: status and verificationStatus report one stored status: {payload}"
    );
    assert!(
        payload.get("verifiedAt").is_none_or(Value::is_null),
        "case {case:?}: a domain whose ownership is not proven carries no verifiedAt: {payload}"
    );
    let reason = member(payload, "verificationStatusReason").unwrap_or_else(|| {
        panic!("case {case:?}: a PENDING domain says why it is pending: {payload}")
    });
    assert!(
        !reason.trim().is_empty(),
        "case {case:?}: verificationStatusReason must say something, got {reason:?}"
    );

    let created_at = payload
        .get("createdAt")
        .unwrap_or_else(|| panic!("case {case:?}: the projection carries no createdAt: {payload}"));
    let created_secs = timestamp_secs(created_at, case);
    let now = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .expect("the test clock is after the epoch")
        .as_secs_f64();
    // The creation time is the instant the record was registered, so it is in the past
    // and recent — never a placeholder such as the epoch.
    assert!(
        created_secs > now - 3600.0 && created_secs <= now + 1.0,
        "case {case:?}: createdAt {created_secs} is not the instant this record was \
         registered (now {now}): {payload}"
    );

    let details = payload
        .get("verificationDetails")
        .unwrap_or_else(|| panic!("case {case:?}: the projection carries no details: {payload}"));
    assert_eq!(
        member(details, "method").as_deref(),
        Some(method),
        "case {case:?}: the details report another verification method: {details}"
    );

    match method {
        VERIFICATION_METHOD_DNS_TXT => {
            assert!(
                details.get("httpRoute").is_none_or(Value::is_null),
                "case {case:?}: a DNS_TXT challenge carries no HTTP route: {details}"
            );
            let dns = details.get("dnsTxt").unwrap_or_else(|| {
                panic!("case {case:?}: a DNS_TXT challenge carries a dnsTxt record: {details}")
            });
            let token = assert_token(dns, case);
            // `DNSRecordType` admits exactly one value, `TXT`
            // (`smithy_query(query="shape",shape="DnsVerification")`).
            assert_eq!(
                member(dns, "dnsRecordType").as_deref(),
                Some("TXT"),
                "case {case:?}: the only DNS record type the model admits is TXT: {dns}"
            );
            let record = member(dns, "dnsRecordName").unwrap_or_else(|| {
                panic!("case {case:?}: the challenge names the DNS record to create: {dns}")
            });
            assert!(
                record.ends_with(&format!(".{domain}")),
                "case {case:?}: the record {record:?} must sit under {domain:?}"
            );
            assert!(
                record.starts_with('_'),
                "case {case:?}: the challenge record {record:?} must be an underscore label, so \
                 it cannot collide with a host the domain serves"
            );
            Some(token)
        }
        VERIFICATION_METHOD_HTTP_ROUTE => {
            assert!(
                details.get("dnsTxt").is_none_or(Value::is_null),
                "case {case:?}: an HTTP_ROUTE challenge carries no DNS record: {details}"
            );
            let http = details.get("httpRoute").unwrap_or_else(|| {
                panic!("case {case:?}: an HTTP_ROUTE challenge carries a httpRoute: {details}")
            });
            let token = assert_token(http, case);
            let path = member(http, "routePath").unwrap_or_else(|| {
                panic!("case {case:?}: the challenge names the route to serve: {http}")
            });
            // The route is ONE fixed, token-free path for every domain, with the token
            // carried beside it; `crate::recorded_wire_form` pins the path itself.
            assert!(
                path.contains(".well-known/"),
                "case {case:?}: the route path {path:?} must sit under the /.well-known/ \
                 prefix reserved for this kind of challenge (RFC 8615)"
            );
            assert!(
                !path.contains(&token),
                "case {case:?}: the token travels beside the route path, not inside it: \
                 {http}"
            );
            Some(token)
        }
        // `PRIVATE_VPC` verifies "ownership via IP for private VPC pentests"
        // (`smithy_query(query="shape",shape="DomainVerificationMethod")`), so there is
        // nothing to publish on the domain and neither challenge is filled in.
        _ => {
            assert!(
                details.get("dnsTxt").is_none_or(Value::is_null),
                "case {case:?}: a PRIVATE_VPC domain has no DNS challenge to publish: {details}"
            );
            assert!(
                details.get("httpRoute").is_none_or(Value::is_null),
                "case {case:?}: a PRIVATE_VPC domain has no HTTP challenge to serve: {details}"
            );
            None
        }
    }
}

/// The instant a timestamp member reports, in seconds since the epoch.
///
/// A target domain's `createdAt`/`verifiedAt` are `@timestampFormat("date-time")`
/// members, so the wire form is an RFC 3339 instant STRING;
/// `crate::recorded_wire_form` owns that rule. What this case needs is the VALUE, read
/// back out of the rendered string.
fn timestamp_secs(value: &Value, case: &str) -> f64 {
    match value {
        Value::String(text) => DateTime::from_str(text, Format::DateTime)
            .unwrap_or_else(|error| {
                panic!("case {case:?}: {text:?} is not a date-time: {error}")
            })
            .as_secs_f64(),
        other => panic!("case {case:?}: {other} is not a date-time timestamp string"),
    }
}

/// The verification token a challenge carries: 64 lowercase hex characters, the
/// hex-encoded SHA-256 the service mints per record.
fn assert_token(challenge: &Value, case: &str) -> String {
    let token = member(challenge, "token")
        .unwrap_or_else(|| panic!("case {case:?}: the challenge carries no token: {challenge}"));
    assert_eq!(
        token.len(),
        64,
        "case {case:?}: expected a 64-character token, got {token:?}"
    );
    assert!(
        token
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "case {case:?}: expected a lowercase hex token, got {token:?}"
    );
    token
}

/// RULE: `CreateTargetDomain` registers the domain as a stored record — it mints the
/// `targetDomainId` and the verification challenge, reports the domain as `PENDING`
/// with no verification time, and `BatchGetTargetDomains` answers that same record,
/// member for member, from the shared store.
#[tokio::test]
async fn create_then_batch_get_reads_back_the_registered_target_domain() {
    let router = emulator();
    let mut seen_ids: BTreeSet<String> = BTreeSet::new();
    let mut seen_tokens: BTreeSet<String> = BTreeSet::new();

    for (domain_case, domain) in domain_name_rows() {
        for method in verification_methods() {
            for (tag_case, tags) in tag_rows() {
                let case = format!("{domain_case}, {method}, {tag_case}");
                let created = create(&router, &domain, method, tags.as_ref(), &case).await;

                let id = member(&created, "targetDomainId").unwrap_or_else(|| {
                    panic!("case {case:?}: CreateTargetDomain minted no targetDomainId: {created}")
                });
                assert!(
                    seen_ids.insert(id.clone()),
                    "case {case:?}: CreateTargetDomain reused the targetDomainId {id:?}"
                );
                let token = assert_pending_projection(&created, &id, &domain, method, &case);
                if let Some(token) = token.clone() {
                    assert!(
                        seen_tokens.insert(token.clone()),
                        "case {case:?}: CreateTargetDomain reused the verification token {token:?}"
                    );
                }

                let page = batch_get(&router, std::slice::from_ref(&id), &case).await;
                let found = page
                    .get("targetDomains")
                    .and_then(Value::as_array)
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: BatchGetTargetDomains answered no list: {page}")
                    });
                assert_eq!(
                    found.len(),
                    1,
                    "case {case:?}: expected exactly the one registered domain, got {page}"
                );
                assert_eq!(
                    page.get("notFound").and_then(Value::as_array).map(Vec::len),
                    Some(0),
                    "case {case:?}: a known id is not missing: {page}"
                );
                let read_token =
                    assert_pending_projection(&found[0], &id, &domain, method, &case);
                assert_eq!(
                    read_token, token,
                    "case {case:?}: the read must report the token the registration minted"
                );
                assert_eq!(
                    found[0].get("createdAt"),
                    created.get("createdAt"),
                    "case {case:?}: the stored creation time must not move between reads: \
                     {found:?}"
                );
                assert_eq!(
                    member(&found[0], "verificationStatusReason"),
                    member(&created, "verificationStatusReason"),
                    "case {case:?}: the stored status reason must not move between reads: \
                     {found:?}"
                );
            }
        }
    }
}

/// RULE: the identity is minted per call, not derived from the request — two
/// registrations of the same domain name with the same method are two distinct
/// records, each readable on its own and each carrying its own token.
#[tokio::test]
async fn identical_requests_register_distinct_target_domains() {
    let router = emulator();

    for method in verification_methods() {
        let case = format!("two identical registrations, {method}");
        let first = create(&router, "repeat.example.com", method, None, &case).await;
        let second = create(&router, "repeat.example.com", method, None, &case).await;

        let first_id = member(&first, "targetDomainId").expect("a minted targetDomainId");
        let second_id = member(&second, "targetDomainId").expect("a minted targetDomainId");
        assert_ne!(
            first_id, second_id,
            "case {case:?}: two registrations must be two records, got {first_id:?} twice"
        );

        let page = batch_get(&router, &[first_id.clone(), second_id.clone()], &case).await;
        let found = page
            .get("targetDomains")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no targetDomains list: {page}"));
        assert_eq!(
            found.len(),
            2,
            "case {case:?}: both records must be readable, got {page}"
        );
        let first_token = assert_pending_projection(
            &found[0],
            &first_id,
            "repeat.example.com",
            method,
            &case,
        );
        let second_token = assert_pending_projection(
            &found[1],
            &second_id,
            "repeat.example.com",
            method,
            &case,
        );
        if method != VERIFICATION_METHOD_PRIVATE_VPC {
            assert_ne!(
                first_token, second_token,
                "case {case:?}: each record carries its own verification token"
            );
        }
    }
}

/// RULE: `BatchGetTargetDomains` partitions the ids it was asked for — every id it
/// holds comes back as a projected record, every id it does not comes back in
/// `notFound`, in the order the caller asked, and the read changes nothing a later
/// caller sees.
#[tokio::test]
async fn batch_get_target_domains_partitions_known_and_unknown_ids() {
    let router = emulator();
    let mut ids = Vec::new();
    for (index, method) in verification_methods().into_iter().enumerate() {
        let case = format!("seed {index}, {method}");
        let created = create(
            &router,
            &format!("seed{index}.example.com"),
            method,
            None,
            &case,
        )
        .await;
        ids.push(member(&created, "targetDomainId").expect("a minted targetDomainId"));
    }
    // An id of the right shape that the store never minted.
    let unknown = "f".repeat(32);

    // The rows differ along the count of ids, which of them the store holds, their
    // order, and whether one repeats.
    let rows: Vec<(&str, Vec<String>, usize, usize)> = vec![
        ("no ids at all", vec![], 0, 0),
        ("one known id", vec![ids[0].clone()], 1, 0),
        ("one unknown id", vec![unknown.clone()], 0, 1),
        (
            "known and unknown interleaved",
            vec![
                ids[1].clone(),
                unknown.clone(),
                ids[0].clone(),
                "0".repeat(32),
            ],
            2,
            2,
        ),
        (
            "every known id, reversed",
            ids.iter().rev().cloned().collect(),
            ids.len(),
            0,
        ),
        (
            "a known id twice",
            vec![ids[2].clone(), ids[2].clone()],
            2,
            0,
        ),
    ];

    for (case, request_ids, expected_found, expected_missing) in rows {
        let page = batch_get(&router, &request_ids, case).await;
        let found = page
            .get("targetDomains")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no targetDomains list: {page}"));
        let missing = page
            .get("notFound")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no notFound list: {page}"));
        assert_eq!(
            found.len(),
            expected_found,
            "case {case:?}: expected {expected_found} found, got {page}"
        );
        assert_eq!(
            missing.len(),
            expected_missing,
            "case {case:?}: expected {expected_missing} missing, got {page}"
        );
        let found_ids: Vec<String> = found
            .iter()
            .map(|domain| member(domain, "targetDomainId").unwrap_or_default())
            .collect();
        let expected_found_ids: Vec<String> = request_ids
            .iter()
            .filter(|id| ids.contains(id))
            .cloned()
            .collect();
        assert_eq!(
            found_ids, expected_found_ids,
            "case {case:?}: the answers must keep the order the caller asked in: {page}"
        );
        let missing_ids: Vec<String> = missing
            .iter()
            .map(|id| id.as_str().unwrap_or_default().to_string())
            .collect();
        let expected_missing_ids: Vec<String> = request_ids
            .iter()
            .filter(|id| !ids.contains(id))
            .cloned()
            .collect();
        assert_eq!(
            missing_ids, expected_missing_ids,
            "case {case:?}: every id the store does not hold is reported missing: {page}"
        );
    }

    // The reads above wrote nothing: every seeded record still reads back.
    let page = batch_get(&router, &ids, "after the reads").await;
    assert_eq!(
        page.get("targetDomains")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(ids.len()),
        "a read must not change what a later caller sees: {page}"
    );
}

/// RULE: a `targetDomainName` that is not a domain name is refused with HTTP 400
/// `ValidationException` naming that member — the model constrains the string not at
/// all, so the DNS form is enforced here — while a name at each published bound is
/// accepted.
#[tokio::test]
async fn create_target_domain_refuses_a_name_that_is_not_a_domain_name() {
    let router = emulator();

    // One row per way the DNS form can be missed, including one character past each
    // published bound (RFC 1035 §2.3.4 with RFC 1123 §2.1).
    let refused: Vec<(&str, String)> = vec![
        ("an empty name", String::new()),
        ("only whitespace", "   ".to_string()),
        (
            "one character past the 253-character limit",
            domain_of_length(MAX_DOMAIN_NAME_LEN + 1),
        ),
        (
            "one character past the 63-character label limit",
            format!("{}.example.com", "a".repeat(MAX_DOMAIN_LABEL_LEN + 1)),
        ),
        ("an empty label", "shop..example.com".to_string()),
        ("two trailing dots", "example.com..".to_string()),
        ("a leading dot", ".example.com".to_string()),
        ("a label starting with a hyphen", "-shop.example.com".to_string()),
        ("a label ending with a hyphen", "shop-.example.com".to_string()),
        ("a space inside a label", "sh op.example.com".to_string()),
        ("an underscore inside a label", "sh_op.example.com".to_string()),
        ("a URL rather than a domain", "https://example.com/".to_string()),
        ("a domain with a port", "example.com:8443".to_string()),
    ];

    for (case, domain) in refused {
        let answer = call(
            &router,
            "CreateTargetDomain",
            &json!({
                "targetDomainName": domain,
                "verificationMethod": VERIFICATION_METHOD_DNS_TXT,
            }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400 for {domain:?}, got {} ({}) with {}",
            answer.status, answer.error_code, answer.body
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected a ValidationException for {domain:?}, got {}",
            answer.body
        );
        let fields = answer
            .body
            .get("fieldList")
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!("case {case:?}: the refusal names the field that failed: {}", answer.body)
            });
        assert!(
            fields
                .iter()
                .any(|field| member(field, "path").as_deref() == Some("targetDomainName")),
            "case {case:?}: the refusal must name targetDomainName, got {fields:?}"
        );
    }

    // And the bounds themselves are inside the rule, so they are accepted, as is the
    // rooted (trailing-dot) form of a name, which is a legal DNS presentation name.
    for (case, domain) in [
        (
            "a name at the 253-character limit",
            domain_of_length(MAX_DOMAIN_NAME_LEN),
        ),
        (
            "a label at the 63-character limit",
            format!("{}.example.com", "a".repeat(MAX_DOMAIN_LABEL_LEN)),
        ),
        ("a rooted name, with one trailing dot", "example.com.".to_string()),
    ] {
        let created = create(
            &router,
            &domain,
            VERIFICATION_METHOD_HTTP_ROUTE,
            None,
            case,
        )
        .await;
        let id = member(&created, "targetDomainId")
            .unwrap_or_else(|| panic!("case {case:?}: no targetDomainId: {created}"));
        assert_pending_projection(
            &created,
            &id,
            &domain,
            VERIFICATION_METHOD_HTTP_ROUTE,
            case,
        );
    }
}

/// RULE: a `verificationMethod` outside the modelled `DomainVerificationMethod` enum
/// is refused with HTTP 400 `ValidationException` — never stored and read back as a
/// method with no challenge to publish — while every member of the enum is accepted.
#[tokio::test]
async fn create_target_domain_refuses_a_method_outside_the_enum() {
    let router = emulator();

    // The rows differ along how the value misses the enum: case, separator, a
    // plausible successor, and text that is not a method at all.
    for (case, method) in [
        ("the lowercase form", "dns_txt"),
        ("a hyphen instead of an underscore", "DNS-TXT"),
        ("a plausible successor value", "DNS_TXT_V2"),
        ("a method that does not exist", "CARRIER_PIGEON"),
        ("the empty string", ""),
    ] {
        let answer = call(
            &router,
            "CreateTargetDomain",
            &json!({"targetDomainName": "method.example.com", "verificationMethod": method}),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400 for {method:?}, got {} ({}) with {}",
            answer.status, answer.error_code, answer.body
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected a ValidationException for {method:?}, got {}",
            answer.body
        );
    }

    for method in verification_methods() {
        let case = format!("the enum member {method}");
        let created = create(&router, "method.example.com", method, None, &case).await;
        let id = member(&created, "targetDomainId")
            .unwrap_or_else(|| panic!("case {case:?}: no targetDomainId: {created}"));
        assert_pending_projection(&created, &id, "method.example.com", method, &case);
    }
}

/// RULE: the store a caller can grow is bounded — at the target domain quota
/// `CreateTargetDomain` answers the service's `ServiceQuotaExceededException` (HTTP
/// 402) and the domains already registered are untouched.
#[tokio::test]
async fn create_target_domain_stops_at_the_target_domain_quota() {
    let router = emulator();
    let mut first_id = String::new();

    for index in 0..MAX_TARGET_DOMAINS {
        let created = create(
            &router,
            &format!("d{index}.example.com"),
            VERIFICATION_METHOD_DNS_TXT,
            None,
            "filling the quota",
        )
        .await;
        if index == 0 {
            first_id = member(&created, "targetDomainId").expect("a minted targetDomainId");
        }
    }

    let answer = call(
        &router,
        "CreateTargetDomain",
        &json!({
            "targetDomainName": "one-too-many.example.com",
            "verificationMethod": VERIFICATION_METHOD_DNS_TXT,
        }),
    )
    .await;
    assert_eq!(
        answer.status, 402,
        "case \"past the quota\": expected HTTP 402, got {} ({}) with {}",
        answer.status, answer.error_code, answer.body
    );
    assert_eq!(
        answer.error_code, "ServiceQuotaExceededException",
        "case \"past the quota\": expected the service's quota error, got {}",
        answer.body
    );

    let page = batch_get(&router, std::slice::from_ref(&first_id), "past the quota").await;
    assert_eq!(
        page.get("targetDomains")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1),
        "case \"past the quota\": a refused registration must not disturb the stored ones: \
         {page}"
    );
}
