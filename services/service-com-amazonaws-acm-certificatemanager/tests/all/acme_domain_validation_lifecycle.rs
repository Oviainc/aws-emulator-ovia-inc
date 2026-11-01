// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for ACM's ACME domain validations over the wire (`POST /`,
//! `X-Amz-Target: CertificateManager.<Op>`, awsJson1_1).
//!
//! RULE: `CreateAcmeDomainValidation` persists ONE domain validation per
//! (ACME endpoint, domain name) pair — minting its ARN as a path under the endpoint's
//! own ARN, recording the prevalidation scope and hosted zone the request configured
//! and the resource tags it carries, minting the DNS record the caller publishes from
//! the validation's own identity, and starting it in `VALIDATING` — and
//! `DescribeAcmeDomainValidation` reports that stored record rather than echoing any
//! request of its own. A second create for a domain the endpoint already validates, and
//! a re-sent idempotency token carrying different parameters, are refused as conflicts;
//! an endpoint ARN with no record behind it is refused as not found.
//!
//! Every assertion goes through [`MultiServiceRouter::dispatch`], the same path the
//! emulator serves production traffic on, and every read goes through a DIFFERENT
//! operation than the write, so a response that is not computed from the persisted
//! store cannot pass. The wire harness is shared with [`crate::certificate_lifecycle`].
//!
//! The table below spans the classes the rule generalises over: domain names of several
//! forms (apex, deep subdomain, punycode, single label, and one at `DomainName`'s
//! modelled maximum length), prevalidation options with and without a `DomainScope`,
//! with and without a `HostedZoneId`, across both `DomainScopeOption` values, and tag
//! sets of several sizes and scripts — not the single configuration the stub issue
//! quoted.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_acm_certificatemanager::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::acme_endpoint_lifecycle::{create_endpoint, AUTHORIZATION_BEHAVIORS};
use crate::certificate_lifecycle::{call, error_code, error_message, router_with_engine, ACCOUNT};

/// Every `DomainScopeOption` the model declares
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#DomainScopeOption")`).
pub(crate) const DOMAIN_SCOPE_OPTIONS: &[&str] = &["ENABLED", "DISABLED"];

/// A `HostedZoneId` satisfying the member's modelled pattern (`^Z[A-Z0-9]+$`).
pub(crate) const HOSTED_ZONE_ID: &str = "Z2FDTNDATAQYW2";

/// One `CreateAcmeDomainValidation` case: what it exercises, the domain name to
/// validate, and the `PrevalidationOptions` to configure.
pub(crate) type ValidationCase<'a> = (&'a str, String, Value);

/// A request body creating one ACME endpoint to hang domain validations off.
pub(crate) fn endpoint_body() -> Value {
    json!({
        "AuthorizationBehavior": AUTHORIZATION_BEHAVIORS[0],
        "Contact": "REQUIRED",
        "CertificateAuthority": { "PublicCertificateAuthority": {} },
    })
}

/// `CreateAcmeDomainValidation`, asserted to succeed, returning the ARN it minted.
pub(crate) async fn create_validation(
    router: &MultiServiceRouter,
    case: &str,
    body: &Value,
) -> String {
    let (status, response) = call(router, "CreateAcmeDomainValidation", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateAcmeDomainValidation must succeed, got {status} {response}"
    );
    response["AcmeDomainValidationArn"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: CreateAcmeDomainValidation returned no AcmeDomainValidationArn")
        })
        .to_string()
}

/// `DescribeAcmeDomainValidation`'s `AcmeDomainValidation` member, asserted to succeed.
pub(crate) async fn describe_validation(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
) -> Value {
    let (status, response) = call(
        router,
        "DescribeAcmeDomainValidation",
        &json!({ "AcmeDomainValidationArn": arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeAcmeDomainValidation({arn}) must succeed, got {status} {response}"
    );
    response["AcmeDomainValidation"].clone()
}

/// Whether `arn` satisfies `AcmeDomainValidationArn`'s modelled `@pattern`
/// (`^arn:aws[a-z-]*:acm:[a-z0-9-]+:[0-9]{12}:acme-endpoint/[a-zA-Z0-9-]+/acme-domain-validation/[a-zA-Z0-9-]+$`)
/// and its modelled length (1..200), as a path under `acme_endpoint_arn`.
fn is_well_formed_validation_arn(arn: &str, acme_endpoint_arn: &str) -> bool {
    let prefix = format!("{acme_endpoint_arn}/acme-domain-validation/");
    let Some(id) = arn.strip_prefix(&prefix) else {
        return false;
    };
    arn.len() <= 200
        && arn.contains(&format!(":{ACCOUNT}:acme-endpoint/"))
        && !id.is_empty()
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Hold every stored domain validation in `VALIDATING` for the rest of the test by
/// moving the instant its prevalidation falls due an hour out.
///
/// This writes only the due instant the emulator itself stores; the transition is still
/// materialised by `materialize_due` on the next request, exactly as in production. A
/// test about the transient state therefore cannot be made flaky by the
/// [`state::ACME_DOMAIN_VALIDATION_SETTLE_DELAY`] span elapsing mid-test.
pub(crate) fn hold_prevalidations(engine: &Arc<engine::InMemoryEngine>) {
    let store = engine.state::<state::ComAmazonawsAcmCertificatemanagerState>();
    let mut store = store.lock();
    let held = SystemTime::now() + Duration::from_secs(3600);
    for validation in store.acme_domain_validations.values_mut() {
        if let Some(pending) = validation.pending_status.as_mut() {
            pending.due_at = held;
        }
    }
}

/// Move the prevalidation every stored domain validation is still owed into the past, so
/// the next request materialises it. Answers the instant the transition is now dated at.
///
/// Like [`hold_prevalidations`] this writes only the due instant the emulator stores: the
/// transition is still applied by `materialize_due`, so a test that needs the settled
/// state does not have to wait [`state::ACME_DOMAIN_VALIDATION_SETTLE_DELAY`] out on the
/// wall clock.
pub(crate) fn settle_prevalidations_now(engine: &Arc<engine::InMemoryEngine>) -> SystemTime {
    let store = engine.state::<state::ComAmazonawsAcmCertificatemanagerState>();
    let mut store = store.lock();
    let past = SystemTime::now() - Duration::from_millis(1);
    for validation in store.acme_domain_validations.values_mut() {
        if let Some(pending) = validation.pending_status.as_mut() {
            pending.due_at = past;
        }
    }
    past
}

/// A `TagList` on the wire as `(key, value)` pairs in key order.
fn tag_pairs(tags: &Value) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = tags
        .as_array()
        .unwrap_or_else(|| panic!("{tags} is not a TagList"))
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or_default().to_string(),
                tag["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    pairs.sort();
    pairs
}

/// A domain name of exactly `DomainName`'s modelled maximum length (253), built from
/// labels the member's pattern admits (`^([a-z0-9]([a-z0-9-]*[a-z0-9])?\.)*...$`).
fn longest_domain_name() -> String {
    // 12 labels of "abcdefghijk." (12 bytes each) = 144, plus a 108-byte label and the
    // apex below bring the whole name to exactly 253 bytes.
    let mut name = "abcdefghijk.".repeat(12);
    name.push_str(&"c".repeat(253 - name.len() - "example.com".len() - 1));
    name.push('.');
    name.push_str("example.com");
    assert_eq!(name.len(), 253, "the longest name must be 253 bytes");
    name
}

/// One request body for each row of the table.
pub(crate) fn validation_cases() -> Vec<ValidationCase<'static>> {
    vec![
        (
            "apex domain, no prevalidation configuration at all",
            "example.com".to_string(),
            json!({ "DnsPrevalidation": {} }),
        ),
        (
            "deep subdomain, hosted zone only",
            "a.b.c.deep.example.com".to_string(),
            json!({ "DnsPrevalidation": { "HostedZoneId": HOSTED_ZONE_ID } }),
        ),
        (
            "punycode (IDN) domain, every scope member ENABLED",
            "xn--80ak6aa92e.xn--p1ai".to_string(),
            json!({
                "DnsPrevalidation": {
                    "DomainScope": {
                        "ExactDomain": DOMAIN_SCOPE_OPTIONS[0],
                        "Subdomains": DOMAIN_SCOPE_OPTIONS[0],
                        "Wildcards": DOMAIN_SCOPE_OPTIONS[0],
                    },
                },
            }),
        ),
        (
            "single-label domain, mixed scope and a hosted zone",
            "localhost".to_string(),
            json!({
                "DnsPrevalidation": {
                    "DomainScope": {
                        "ExactDomain": DOMAIN_SCOPE_OPTIONS[0],
                        "Subdomains": DOMAIN_SCOPE_OPTIONS[1],
                    },
                    "HostedZoneId": HOSTED_ZONE_ID,
                },
            }),
        ),
        (
            "domain at DomainName's maximum modelled length",
            longest_domain_name(),
            json!({
                "DnsPrevalidation": {
                    "DomainScope": { "Wildcards": DOMAIN_SCOPE_OPTIONS[1] },
                },
            }),
        ),
    ]
}

/// The `(key, value)` tag set each row of the table creates its validation with: no
/// tags, one, a non-ASCII pair, and `TagList`'s maximum length.
pub(crate) fn tag_case(index: usize) -> Option<Value> {
    match index % 4 {
        0 => None,
        1 => Some(json!([{ "Key": "owner", "Value": "platform" }])),
        2 => Some(json!([
            { "Key": "環境", "Value": "本番" },
            { "Key": "team", "Value": "" },
        ])),
        // 50 is `TagList`'s modelled maximum length, so this row sits on the bound.
        _ => Some(Value::Array(
            (0..50)
                .map(|n| json!({ "Key": format!("key-{n}"), "Value": format!("value-{n}") }))
                .collect(),
        )),
    }
}

/// RULE: `DescribeAcmeDomainValidation` reports the validation
/// `CreateAcmeDomainValidation` persisted — its minted ARN under the endpoint, the
/// domain exactly as named, the prevalidation configuration, the server-managed DNS
/// record and the `VALIDATING` status it starts in — for every row of the table above.
#[tokio::test]
async fn describe_reports_what_create_acme_domain_validation_stored() {
    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "validation host", &endpoint_body()).await;

    let mut seen_arns: BTreeSet<String> = BTreeSet::new();
    let mut seen_records: BTreeSet<String> = BTreeSet::new();

    for (index, (case, domain, options)) in validation_cases().into_iter().enumerate() {
        let tags = tag_case(index);
        let mut body = json!({
            "AcmeEndpointArn": endpoint_arn,
            "DomainName": domain,
            "PrevalidationOptions": options,
        });
        if let Some(tags) = tags.clone() {
            body["Tags"] = tags;
        }

        let arn = create_validation(&router, case, &body).await;
        // Pin the transient status: the prevalidation the create owes this validation is
        // pushed out of the test's way, so `VALIDATING` cannot race the wall clock.
        hold_prevalidations(&engine);

        assert!(
            is_well_formed_validation_arn(&arn, &endpoint_arn),
            "case {case:?}: the minted ARN must be a well-formed path under {endpoint_arn}, got {arn:?}"
        );
        assert!(
            seen_arns.insert(arn.clone()),
            "case {case:?}: every create must mint its own ARN, but {arn} was minted twice"
        );

        let validation = describe_validation(&router, case, &arn).await;
        assert_eq!(
            validation["AcmeDomainValidationArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected the described ARN to be {arn}, got {validation}"
        );
        assert_eq!(
            validation["AcmeEndpointArn"].as_str(),
            Some(endpoint_arn.as_str()),
            "case {case:?}: expected the validation to report endpoint {endpoint_arn}, got {validation}"
        );
        assert_eq!(
            validation["DomainName"].as_str(),
            Some(domain.as_str()),
            "case {case:?}: expected the stored DomainName {domain}, got {validation}"
        );
        assert_eq!(
            validation["PrevalidationType"].as_str(),
            Some(state::PREVALIDATION_TYPE_DNS),
            "case {case:?}: expected PrevalidationType {}, got {validation}",
            state::PREVALIDATION_TYPE_DNS
        );
        assert_eq!(
            validation["Status"].as_str(),
            Some(state::ACME_DOMAIN_VALIDATION_STATUS_VALIDATING),
            "case {case:?}: expected a freshly created validation to be {}, got {validation}",
            state::ACME_DOMAIN_VALIDATION_STATUS_VALIDATING
        );
        assert!(
            validation["FailureDetails"].is_null(),
            "case {case:?}: nothing failed, so FailureDetails must be absent, got {validation}"
        );
        assert!(
            validation["CreatedAt"].is_number() && validation["UpdatedAt"].is_number(),
            "case {case:?}: expected both timestamps to be reported, got {validation}"
        );
        assert_eq!(
            validation["CreatedAt"], validation["UpdatedAt"],
            "case {case:?}: nothing has changed the fresh validation, so UpdatedAt must be CreatedAt, got {validation}"
        );

        // The prevalidation details report what the request configured, member for
        // member, beside the record the SERVICE minted.
        let details = &validation["PrevalidationDetails"]["DnsPrevalidation"];
        let requested = &body["PrevalidationOptions"]["DnsPrevalidation"];
        assert_eq!(
            details["HostedZoneId"], requested["HostedZoneId"],
            "case {case:?}: expected the stored HostedZoneId to be reported, got {validation}"
        );
        assert_eq!(
            details["DomainScope"], requested["DomainScope"],
            "case {case:?}: expected the stored DomainScope to be reported, got {validation}"
        );

        // The DNS record the caller publishes is server-managed: a CNAME under the
        // domain being validated pointing into ACM's validation zone, derived from this
        // validation's own identity and unique to it.
        let record = &details["ResourceRecord"];
        let name = record["Name"].as_str().unwrap_or_default().to_string();
        let value = record["Value"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            record["Type"].as_str(),
            Some(state::RECORD_TYPE_CNAME),
            "case {case:?}: expected a {} record, got {validation}",
            state::RECORD_TYPE_CNAME
        );
        assert!(
            name.starts_with('_') && name.ends_with(&format!(".{domain}.")),
            "case {case:?}: expected a validation label under {domain}, got {name:?}"
        );
        assert!(
            value.starts_with('_')
                && value.ends_with(&format!(".{}.", state::ACM_VALIDATIONS_ZONE)),
            "case {case:?}: expected a CNAME target in {}, got {value:?}",
            state::ACM_VALIDATIONS_ZONE
        );
        assert!(
            seen_records.insert(value.clone()),
            "case {case:?}: every validation must get its own record, but {value} repeated"
        );

        // The tags the create carried are readable back through the generic tagging
        // read, which addresses the validation by the same ARN.
        let (status, listed) = call(
            &router,
            "ListTagsForResource",
            &json!({ "ResourceArn": arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource({arn}) must succeed, got {status} {listed}"
        );
        match tags {
            Some(requested) => assert_eq!(
                tag_pairs(&listed["Tags"]),
                tag_pairs(&requested),
                "case {case:?}: expected the stored Tags to be reported, got {listed}"
            ),
            None => assert_eq!(
                listed["Tags"],
                json!([]),
                "case {case:?}: a create that named no Tags must report the empty list, \
                 got {listed}"
            ),
        }
    }
}

/// RULE: the `VALID` transition a create owes its validation is due a non-zero span
/// later and is materialised by whichever call next touches the store — so a caller that
/// polls sees `VALIDATING` first and `VALID` once the span has passed, dated at the
/// instant it fell due rather than at the instant of the read.
#[tokio::test]
async fn prevalidation_settles_on_the_clock_the_create_fixed() {
    assert!(
        state::ACME_DOMAIN_VALIDATION_SETTLE_DELAY >= Duration::from_millis(250)
            && state::ACME_DOMAIN_VALIDATION_SETTLE_DELAY <= Duration::from_millis(750),
        "the prevalidation span must be a non-zero span a caller can observe, got {:?}",
        state::ACME_DOMAIN_VALIDATION_SETTLE_DELAY
    );

    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "settling host", &endpoint_body()).await;

    for (case, domain, options) in validation_cases() {
        let arn = create_validation(
            &router,
            case,
            &json!({
                "AcmeEndpointArn": endpoint_arn,
                "DomainName": domain,
                "PrevalidationOptions": options,
            }),
        )
        .await;
        hold_prevalidations(&engine);

        let pending = describe_validation(&router, case, &arn).await;
        assert_eq!(
            pending["Status"].as_str(),
            Some(state::ACME_DOMAIN_VALIDATION_STATUS_VALIDATING),
            "case {case:?}: expected the transient status before the span passes, got {pending}"
        );

        let due = settle_prevalidations_now(&engine);
        let settled = describe_validation(&router, case, &arn).await;
        assert_eq!(
            settled["Status"].as_str(),
            Some(state::ACME_DOMAIN_VALIDATION_STATUS_VALID),
            "case {case:?}: expected {} once the prevalidation fell due, got {settled}",
            state::ACME_DOMAIN_VALIDATION_STATUS_VALID
        );
        let updated_at = settled["UpdatedAt"].as_f64().unwrap_or_default();
        let due_at = due
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        assert!(
            (updated_at - due_at).abs() < 0.01,
            "case {case:?}: expected UpdatedAt to be the instant the transition fell due ({due_at}), got {updated_at}"
        );
    }
}

/// RULE: one endpoint validates one domain through ONE validation, so a second create
/// for a domain it already validates is refused with `ConflictException` — whatever the
/// case of the name, since DNS labels are case-insensitive — while the SAME domain on
/// ANOTHER endpoint is a different resource and is created.
#[tokio::test]
async fn a_domain_is_validated_once_per_endpoint() {
    let (router, engine) = router_with_engine();
    let first = create_endpoint(&router, "first host", &endpoint_body()).await;
    let second = create_endpoint(&router, "second host", &endpoint_body()).await;

    for (case, domain, options) in validation_cases() {
        let body = json!({
            "AcmeEndpointArn": first,
            "DomainName": domain,
            "PrevalidationOptions": options,
        });
        let arn = create_validation(&router, case, &body).await;
        hold_prevalidations(&engine);

        let (status, refusal) = call(&router, "CreateAcmeDomainValidation", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a duplicate domain must be refused, got {status} {refusal}"
        );
        assert_eq!(
            error_code(&refusal),
            "ConflictException",
            "case {case:?}: expected ConflictException for a duplicate domain, got {refusal}"
        );
        assert_eq!(
            error_message(&refusal),
            state::acme_domain_validation_exists_message(&domain),
            "case {case:?}: expected the duplicate-domain sentence naming {domain}, got {refusal}"
        );
        assert!(
            !error_message(&refusal).contains(&arn),
            "case {case:?}: the refusal must not name the existing validation {arn}, got {refusal}"
        );

        // The same name is still one domain when its labels are upper-cased, so the
        // second spelling conflicts too. (`DomainName`'s pattern admits lowercase only,
        // so the varied spelling goes through the state the handler compares.)
        let store = engine.state::<state::ComAmazonawsAcmCertificatemanagerState>();
        let found = {
            let held = store.lock();
            held.acme_domain_validation_for(&first, &domain.to_ascii_uppercase())
                .map(|validation| validation.arn.clone())
        };
        assert_eq!(
            found.as_deref(),
            Some(arn.as_str()),
            "case {case:?}: expected {domain} upper-cased to resolve to {arn}, got {found:?}"
        );

        // Another endpoint is another resource: the same domain is validated there, and
        // gets its own ARN and its own DNS record.
        let other = create_validation(
            &router,
            case,
            &json!({
                "AcmeEndpointArn": second,
                "DomainName": domain,
                "PrevalidationOptions": { "DnsPrevalidation": {} },
            }),
        )
        .await;
        hold_prevalidations(&engine);
        assert_ne!(
            other, arn,
            "case {case:?}: the same domain on another endpoint must be its own validation"
        );
        let mine = describe_validation(&router, case, &arn).await;
        let theirs = describe_validation(&router, case, &other).await;
        assert_ne!(
            mine["PrevalidationDetails"]["DnsPrevalidation"]["ResourceRecord"]["Value"],
            theirs["PrevalidationDetails"]["DnsPrevalidation"]["ResourceRecord"]["Value"],
            "case {case:?}: two validations of {domain} must publish different records, got {mine} and {theirs}"
        );
    }
}

/// RULE: an `IdempotencyToken` identifies ONE create against one endpoint — re-sent with
/// the same parameters it answers with the validation it already created and makes no
/// second one, and re-sent with different parameters it is refused as a conflict.
#[tokio::test]
async fn an_idempotency_token_names_one_create() {
    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "token host", &endpoint_body()).await;

    for (index, (case, domain, options)) in validation_cases().into_iter().enumerate() {
        let token = format!("token-{index}");
        let body = json!({
            "AcmeEndpointArn": endpoint_arn,
            "DomainName": domain,
            "PrevalidationOptions": options,
            "IdempotencyToken": token,
        });

        let first = create_validation(&router, case, &body).await;
        let replayed = create_validation(&router, case, &body).await;
        assert_eq!(
            first, replayed,
            "case {case:?}: re-sending {token} must answer with the validation it created"
        );
        hold_prevalidations(&engine);

        // One record, not two: the replay created nothing.
        let store = engine.state::<state::ComAmazonawsAcmCertificatemanagerState>();
        let stored = {
            let held = store.lock();
            held.acme_domain_validations_of(&endpoint_arn).count()
        };
        assert_eq!(
            stored,
            index + 1,
            "case {case:?}: expected the replay to create nothing, got {stored} validations"
        );

        // The same token on a DIFFERENT request is a conflict: its change may not be
        // discarded behind a 200.
        let mut changed = body.clone();
        changed["PrevalidationOptions"] =
            json!({ "DnsPrevalidation": { "HostedZoneId": "ZCHANGEDZONE" } });
        let (status, refusal) = call(&router, "CreateAcmeDomainValidation", &changed).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a reused token with changed parameters must be refused, got {status} {refusal}"
        );
        assert_eq!(
            error_code(&refusal),
            "ConflictException",
            "case {case:?}: expected ConflictException for a reused token, got {refusal}"
        );
    }

    // Presence is part of the request too: a scope the caller sent EMPTY is stored and
    // reported, so it is not the same request as one that named no scope at all, and the
    // token may not resolve the second to the first.
    let empty_scope_cases = [
        (
            "an empty DomainScope beside no DomainScope",
            json!({ "DnsPrevalidation": { "DomainScope": {} } }),
        ),
        (
            "an empty DnsPrevalidation beside no DomainScope",
            json!({ "DnsPrevalidation": {} }),
        ),
    ];
    let token = "presence-token";
    let created = create_validation(
        &router,
        "no DomainScope at all",
        &json!({
            "AcmeEndpointArn": endpoint_arn,
            "DomainName": "presence.example.com",
            "PrevalidationOptions": empty_scope_cases[1].1,
            "IdempotencyToken": token,
        }),
    )
    .await;
    hold_prevalidations(&engine);
    let (status, refusal) = call(
        &router,
        "CreateAcmeDomainValidation",
        &json!({
            "AcmeEndpointArn": endpoint_arn,
            "DomainName": "presence.example.com",
            "PrevalidationOptions": empty_scope_cases[0].1,
            "IdempotencyToken": token,
        }),
    )
    .await;
    let case = empty_scope_cases[0].0;
    assert_eq!(
        error_code(&refusal),
        "ConflictException",
        "case {case:?}: expected the token of {created} to conflict with a changed scope, got {status} {refusal}"
    );
}

/// RULE: a validation can only be created under an endpoint of the caller's OWN account —
/// an endpoint ARN naming another account is refused as invalid input
/// (`ValidationException`, HTTP 400) before the store is read — and under an endpoint of
/// this account it can only be created while that endpoint's record exists, and only
/// described while its own record does; both of those are `ResourceNotFoundException`
/// (HTTP 400), and none of the three is a 501 or a 200.
#[tokio::test]
async fn unknown_parents_and_unknown_validations_are_refused() {
    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "known host", &endpoint_body()).await;

    // Endpoint ARNs no record of THIS account answers to: one in this account's Region,
    // and one in another Region.
    let unknown_endpoints = [
        (
            "an endpoint id this account never created",
            format!("arn:aws:acm:us-west-2:{ACCOUNT}:acme-endpoint/00000000-0000-4000-8000-000000000000"),
        ),
        (
            "an endpoint in another Region",
            format!("arn:aws:acm:eu-central-1:{ACCOUNT}:acme-endpoint/11111111-1111-4111-8111-111111111111"),
        ),
    ];
    for (case, arn) in unknown_endpoints {
        let (status, refusal) = call(
            &router,
            "CreateAcmeDomainValidation",
            &json!({
                "AcmeEndpointArn": arn,
                "DomainName": "example.com",
                "PrevalidationOptions": { "DnsPrevalidation": {} },
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: creating under {arn} must be refused, got {status} {refusal}"
        );
        assert_eq!(
            error_code(&refusal),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException for {arn}, got {refusal}"
        );
        assert!(
            error_message(&refusal).contains(&arn),
            "case {case:?}: expected the refusal to name {arn}, got {refusal}"
        );
    }

    // An endpoint ARN of ANOTHER account is a verdict on the request: the ARN is not
    // addressable by this caller, whatever the store holds, so it is refused as invalid
    // input rather than reported as a resource that is missing.
    let case = "an endpoint in another account";
    let foreign = "arn:aws:acm:us-west-2:210987654321:acme-endpoint/22222222-2222-4222-8222-222222222222";
    let (status, refusal) = call(
        &router,
        "CreateAcmeDomainValidation",
        &json!({
            "AcmeEndpointArn": foreign,
            "DomainName": "example.com",
            "PrevalidationOptions": { "DnsPrevalidation": {} },
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: creating under {foreign} must be refused, got {status} {refusal}"
    );
    assert_eq!(
        error_code(&refusal),
        "ValidationException",
        "case {case:?}: expected ValidationException for {foreign}, got {refusal}"
    );
    assert!(
        !error_message(&refusal).contains(foreign),
        "case {case:?}: a cross-account refusal must withhold the submitted ARN, got {refusal}"
    );

    // A validation ARN of the right shape under a real endpoint, but no record behind
    // it, and one under an endpoint that does not exist either.
    let created = create_validation(
        &router,
        "known validation",
        &json!({
            "AcmeEndpointArn": endpoint_arn,
            "DomainName": "example.com",
            "PrevalidationOptions": { "DnsPrevalidation": {} },
        }),
    )
    .await;
    hold_prevalidations(&engine);

    let unknown_validations = [
        (
            "a validation id under a real endpoint",
            format!("{endpoint_arn}/acme-domain-validation/33333333-3333-4333-8333-333333333333"),
        ),
        (
            "a validation under an endpoint that does not exist",
            format!(
                "arn:aws:acm:us-west-2:{ACCOUNT}:acme-endpoint/44444444-4444-4444-8444-444444444444\
                 /acme-domain-validation/55555555-5555-4555-8555-555555555555"
            ),
        ),
    ];
    for (case, arn) in unknown_validations {
        let (status, refusal) = call(
            &router,
            "DescribeAcmeDomainValidation",
            &json!({ "AcmeDomainValidationArn": arn }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: describing {arn} must be refused, got {status} {refusal}"
        );
        assert_eq!(
            error_code(&refusal),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException for {arn}, got {refusal}"
        );
        assert!(
            error_message(&refusal).contains(&arn),
            "case {case:?}: expected the refusal to name {arn}, got {refusal}"
        );
    }

    // The validation that DOES exist is still readable, so the refusals above are about
    // the records they named and not about the operation.
    let described = describe_validation(&router, "known validation", &created).await;
    assert_eq!(
        described["AcmeDomainValidationArn"].as_str(),
        Some(created.as_str()),
        "expected the created validation to remain readable, got {described}"
    );
}

/// RULE: a domain validation is a path under its endpoint and cannot outlive it — the
/// state's endpoint removal drops the endpoint's validations with it (and with them the
/// idempotency tokens that resolve to them), so no minted ARN is left without a record.
#[tokio::test]
async fn removing_an_endpoint_drops_its_domain_validations() {
    let (router, engine) = router_with_engine();
    let doomed = create_endpoint(&router, "doomed host", &endpoint_body()).await;
    let survivor = create_endpoint(&router, "surviving host", &endpoint_body()).await;

    let mut doomed_arns: Vec<String> = Vec::new();
    for (index, (case, domain, options)) in validation_cases().into_iter().enumerate() {
        doomed_arns.push(
            create_validation(
                &router,
                case,
                &json!({
                    "AcmeEndpointArn": doomed,
                    "DomainName": domain,
                    "PrevalidationOptions": options,
                    "IdempotencyToken": format!("doomed-{index}"),
                }),
            )
            .await,
        );
    }
    let kept = create_validation(
        &router,
        "surviving validation",
        &json!({
            "AcmeEndpointArn": survivor,
            "DomainName": "example.org",
            "PrevalidationOptions": { "DnsPrevalidation": {} },
        }),
    )
    .await;
    hold_prevalidations(&engine);

    let store = engine.state::<state::ComAmazonawsAcmCertificatemanagerState>();
    let leftover_tokens = {
        let mut held = store.lock();
        held.remove_acme_endpoint(&doomed);
        held.acme_domain_validation_idempotency.len()
    };

    assert_eq!(
        leftover_tokens, 0,
        "expected the removed endpoint's idempotency tokens to go with it, {leftover_tokens} left"
    );
    for arn in &doomed_arns {
        let (status, refusal) = call(
            &router,
            "DescribeAcmeDomainValidation",
            &json!({ "AcmeDomainValidationArn": arn }),
        )
        .await;
        assert_eq!(
            error_code(&refusal),
            "ResourceNotFoundException",
            "case {arn:?}: expected the cascaded validation to be gone, got {status} {refusal}"
        );
    }

    let described = describe_validation(&router, "surviving validation", &kept).await;
    assert_eq!(
        described["AcmeEndpointArn"].as_str(),
        Some(survivor.as_str()),
        "expected another endpoint's validation to survive, got {described}"
    );
}
