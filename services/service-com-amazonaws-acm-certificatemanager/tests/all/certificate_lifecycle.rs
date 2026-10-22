// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for ACM's public-certificate lifecycle over the wire
//! (`POST /`, `X-Amz-Target: CertificateManager.<Op>`, awsJson1_1).
//!
//! Every assertion goes through [`MultiServiceRouter::dispatch`], the same path the
//! emulator serves production traffic on, so a response that is not computed from the
//! persisted store cannot pass: each test writes with one operation and reads back
//! with another.
//!
//! The tables below span the classes the rules generalise over — every
//! `ValidationMethod`, every page size across a page boundary, several ARN shapes —
//! rather than the single case the bootstrap issue quoted.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_acm_certificatemanager::state;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for; ARNs the service mints must carry it.
pub(crate) const REGION: &str = "us-west-2";

/// The canonical emulator account id every ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// Every `ValidationMethod` the model declares
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#ValidationMethod")`).
const VALIDATION_METHODS: &[&str] = &["DNS", "EMAIL", "HTTP"];

/// Every `KeyAlgorithm` the model declares
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#KeyAlgorithm")`).
pub(crate) const KEY_ALGORITHMS: &[&str] = &[
    "EC_prime256v1",
    "EC_secp384r1",
    "EC_secp521r1",
    "RSA_1024",
    "RSA_2048",
    "RSA_3072",
    "RSA_4096",
];

/// One `RequestCertificate` case: the domain name, its subject alternative names,
/// the `ValidationMethod` to ask for (if any) and the `KeyAlgorithm` to ask for.
type RequestCase<'a> = (&'a str, &'a [&'a str], Option<&'a str>, Option<&'a str>);

/// One `AddTagsToCertificate` case: what it does, the tags it adds, and the full tag
/// set expected on the certificate afterwards.
type TagCase<'a> = (&'a str, Value, Vec<(&'a str, &'a str)>);

/// A router serving only this service, over its own engine.
pub(crate) fn router() -> MultiServiceRouter {
    router_with_engine().0
}

/// A router serving only this service, beside the engine it serves from — so a test
/// that must observe the transient `PENDING_VALIDATION` state can pin the instant the
/// validation outcome falls due instead of racing the wall clock.
pub(crate) fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_acm_certificatemanager::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// Hold every stored certificate in `PENDING_VALIDATION` for the rest of the test by
/// moving the instant its validation outcome falls due an hour out.
///
/// This writes only the due instant the emulator itself stores; the transition is
/// still materialised by `materialize_due` on the next request, exactly as in
/// production. A test about the pending state therefore cannot be made flaky by the
/// [`state::VALIDATION_SETTLE_DELAY`] span elapsing mid-test.
pub(crate) fn hold_pending_validations(engine: &Arc<engine::InMemoryEngine>) {
    let store = engine.state::<state::ComAmazonawsAcmCertificatemanagerState>();
    let mut store = store.lock();
    let held = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
    for certificate in store.certificates.values_mut() {
        if let Some(pending) = certificate.pending_validation.as_mut() {
            pending.due_at = held;
        }
    }
}

/// Call one operation, signed for [`REGION`], and return `(status, body)`.
pub(crate) async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    call_in_region(router, REGION, operation, body).await
}

/// Call one operation signed for `region` and return `(status, body)`.
pub(crate) async fn call_in_region(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("CertificateManager.{operation}"))
        // The routing key and the ARN region both come from this credential scope.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20240101/{region}/acm/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The wire error code of a failure body (`__type`, without its namespace).
pub(crate) fn error_code(body: &Value) -> String {
    let rendered = body["__type"].as_str().unwrap_or_default();
    rendered
        .split('#')
        .next_back()
        .unwrap_or(rendered)
        .to_string()
}

/// Request one certificate and return its ARN, failing the test if the call did not
/// succeed.
pub(crate) async fn request_certificate(router: &MultiServiceRouter, body: &Value) -> String {
    let (status, response) = call(router, "RequestCertificate", body).await;
    assert_eq!(
        status, 200,
        "case {body}: RequestCertificate must succeed, got {status} {response}"
    );
    response["CertificateArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {body}: RequestCertificate returned no CertificateArn"))
        .to_string()
}

/// `DescribeCertificate`'s `Certificate` member, failing the test on any error.
pub(crate) async fn describe(router: &MultiServiceRouter, arn: &str) -> Value {
    let (status, response) = call(
        router,
        "DescribeCertificate",
        &json!({ "CertificateArn": arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {arn}: DescribeCertificate must succeed, got {status} {response}"
    );
    response["Certificate"].clone()
}

/// Every certificate ARN `ListCertificates` reports, paging to the end.
pub(crate) async fn list_all_arns(router: &MultiServiceRouter, filters: &Value) -> Vec<String> {
    let mut arns = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut body = filters.clone();
        if let Some(token) = next.as_ref() {
            body["NextToken"] = json!(token);
        }
        let (status, response) = call(router, "ListCertificates", &body).await;
        assert_eq!(
            status, 200,
            "case {body}: ListCertificates must succeed, got {status} {response}"
        );
        for summary in response["CertificateSummaryList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {body}: no CertificateSummaryList in {response}"))
        {
            arns.push(
                summary["CertificateArn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {body}: a summary carried no ARN"))
                    .to_string(),
            );
        }
        next = response["NextToken"].as_str().map(str::to_string);
        if next.is_none() {
            break;
        }
        assert!(
            arns.len() <= 64,
            "case {body}: paging did not terminate after {} ARNs",
            arns.len()
        );
    }
    arns
}

/// True when `label` is a 32-character lowercase-hex DNS validation label prefixed
/// with `_`, the shape the `ResourceRecord` doc shows
/// (`_a79865eb4cd1a6ab990a45779b4e0b96`).
fn is_validation_label(label: &str) -> bool {
    let Some(hex) = label.strip_prefix('_') else {
        return false;
    };
    hex.len() == 32 && hex.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// RULE: `RequestCertificate` persists a certificate and `DescribeCertificate`
/// reports that stored record — for every validation method and key algorithm, and
/// for a wildcard name as for a plain one.
#[tokio::test]
async fn describe_reports_what_request_certificate_stored() {
    // One row per validation method, each with a different key algorithm, SAN count
    // and domain form (plain, wildcard, deep label), plus a row that names neither a
    // validation method nor a key algorithm so the defaults are exercised.
    let cases: &[RequestCase<'_>] = &[
        (
            "www.example.com",
            &["example.com"],
            Some("DNS"),
            Some("RSA_2048"),
        ),
        (
            "*.wildcard.example.net",
            &[],
            Some("DNS"),
            Some("EC_prime256v1"),
        ),
        (
            "mail.example.org",
            &["smtp.example.org", "imap.example.org"],
            Some("EMAIL"),
            Some("RSA_4096"),
        ),
        (
            "edge.app.example.io",
            &["app.example.io"],
            Some("HTTP"),
            Some("EC_secp521r1"),
        ),
        ("defaults.example.co", &[], None, None),
    ];

    let (router, engine) = router_with_engine();
    for (domain, sans, method, algorithm) in cases {
        let case = format!("{domain} {method:?} {algorithm:?}");
        let mut body = json!({ "DomainName": domain });
        if !sans.is_empty() {
            body["SubjectAlternativeNames"] = json!(sans);
        }
        if let Some(method) = method {
            body["ValidationMethod"] = json!(method);
        }
        if let Some(algorithm) = algorithm {
            body["KeyAlgorithm"] = json!(algorithm);
        }

        let arn = request_certificate(&router, &body).await;
        // This case is about the record a request creates, so the certificate is held
        // in the status a request puts it in rather than racing the clock-owed
        // validation outcome.
        hold_pending_validations(&engine);
        let prefix = format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/");
        let id = arn.strip_prefix(&prefix).unwrap_or_else(|| {
            panic!("case {case}: expected an ARN starting {prefix}, got {arn:?}")
        });
        assert_eq!(
            (id.len(), id.matches('-').count()),
            (36, 4),
            "case {case}: expected a v4 UUID certificate id, got {id:?}"
        );

        let detail = describe(&router, &arn).await;
        assert_eq!(
            detail["CertificateArn"].as_str(),
            Some(arn.as_str()),
            "case {case}: expected the described ARN to be the minted one, got {detail}"
        );
        assert_eq!(
            detail["DomainName"].as_str(),
            Some(*domain),
            "case {case}: expected DomainName {domain}, got {detail}"
        );

        // SubjectAlternativeNames is the domain name followed by the requested SANs.
        let mut expected_names = vec![(*domain).to_string()];
        expected_names.extend(sans.iter().map(|san| (*san).to_string()));
        let actual_names: Vec<String> = detail["SubjectAlternativeNames"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: no SubjectAlternativeNames in {detail}"))
            .iter()
            .map(|name| name.as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            actual_names, expected_names,
            "case {case}: expected SubjectAlternativeNames {expected_names:?}, got {actual_names:?}"
        );

        assert_eq!(
            detail["Status"].as_str(),
            Some("PENDING_VALIDATION"),
            "case {case}: a requested certificate enters PENDING_VALIDATION, got {detail}"
        );
        assert_eq!(
            detail["Type"].as_str(),
            Some("AMAZON_ISSUED"),
            "case {case}: an ACM-provided certificate is AMAZON_ISSUED, got {detail}"
        );
        assert_eq!(
            detail["RenewalEligibility"].as_str(),
            Some("INELIGIBLE"),
            "case {case}: only exported private certificates are renewable, got {detail}"
        );
        assert!(
            detail["CreatedAt"].is_number(),
            "case {case}: expected a numeric CreatedAt timestamp, got {detail}"
        );
        // Members that only exist once a certificate has been issued stay absent.
        for absent in ["IssuedAt", "Serial", "Subject", "Issuer", "NotAfter"] {
            assert!(
                detail[absent].is_null(),
                "case {case}: {absent} must be absent before issuance, got {detail}"
            );
        }

        let expected_algorithm = algorithm.unwrap_or("RSA_2048");
        assert_eq!(
            detail["KeyAlgorithm"].as_str(),
            Some(expected_algorithm),
            "case {case}: expected KeyAlgorithm {expected_algorithm}, got {detail}"
        );
        assert!(
            KEY_ALGORITHMS.contains(&detail["KeyAlgorithm"].as_str().unwrap_or_default()),
            "case {case}: KeyAlgorithm must be a modelled value, got {detail}"
        );

        // One DomainValidationOptions entry per name, carrying the method's own
        // evidence: a CNAME for DNS, mailboxes for EMAIL, URLs for HTTP.
        let expected_method = method.unwrap_or("EMAIL");
        assert!(
            VALIDATION_METHODS.contains(&expected_method),
            "case {case}: the test's own method must be a modelled value"
        );
        let validations = detail["DomainValidationOptions"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: no DomainValidationOptions in {detail}"));
        assert_eq!(
            validations.len(),
            expected_names.len(),
            "case {case}: expected one validation per domain name, got {detail}"
        );
        for (validation, name) in validations.iter().zip(expected_names.iter()) {
            assert_eq!(
                validation["DomainName"].as_str(),
                Some(name.as_str()),
                "case {case}: expected validation for {name}, got {validation}"
            );
            assert_eq!(
                validation["ValidationMethod"].as_str(),
                Some(expected_method),
                "case {case}: expected ValidationMethod {expected_method}, got {validation}"
            );
            assert_eq!(
                validation["ValidationStatus"].as_str(),
                Some("PENDING_VALIDATION"),
                "case {case}: an unproven domain is PENDING_VALIDATION, got {validation}"
            );
            // ACM proves control of `*.example.com` through `example.com`.
            let target = name.strip_prefix("*.").unwrap_or(name);
            match expected_method {
                "DNS" => {
                    let record = &validation["ResourceRecord"];
                    assert_eq!(
                        record["Type"].as_str(),
                        Some("CNAME"),
                        "case {case}: the DNS validation record is a CNAME, got {record}"
                    );
                    let record_name = record["Name"].as_str().unwrap_or_default();
                    let label = record_name
                        .strip_suffix(&format!(".{target}."))
                        .unwrap_or_default();
                    assert!(
                        is_validation_label(label),
                        "case {case}: expected a _<32 hex>.{target}. record name, got {record_name:?}"
                    );
                    let value = record["Value"].as_str().unwrap_or_default();
                    assert!(
                        value.ends_with(".acm-validations.aws."),
                        "case {case}: expected a CNAME target in acm-validations.aws, got {value:?}"
                    );
                    assert!(
                        validation["ValidationEmails"].is_null()
                            && validation["HttpRedirect"].is_null(),
                        "case {case}: DNS validation carries no email or HTTP evidence, got {validation}"
                    );
                }
                "EMAIL" => {
                    assert_eq!(
                        validation["ValidationDomain"].as_str(),
                        Some(target),
                        "case {case}: expected ACM to mail {target}, got {validation}"
                    );
                    let mailboxes: Vec<&str> = validation["ValidationEmails"]
                        .as_array()
                        .unwrap_or_else(|| {
                            panic!("case {case}: email validation lists no mailboxes: {validation}")
                        })
                        .iter()
                        .map(|m| m.as_str().unwrap_or_default())
                        .collect();
                    assert!(
                        mailboxes.contains(&format!("admin@{target}").as_str()),
                        "case {case}: expected admin@{target} among {mailboxes:?}"
                    );
                    assert!(
                        mailboxes
                            .iter()
                            .all(|m| m.ends_with(&format!("@{target}"))),
                        "case {case}: every mailbox must be at {target}, got {mailboxes:?}"
                    );
                    assert!(
                        validation["ResourceRecord"].is_null(),
                        "case {case}: email validation publishes no DNS record, got {validation}"
                    );
                }
                _ => {
                    let redirect = &validation["HttpRedirect"];
                    let from = redirect["RedirectFrom"].as_str().unwrap_or_default();
                    let to = redirect["RedirectTo"].as_str().unwrap_or_default();
                    assert!(
                        from.starts_with(&format!("http://{target}/.well-known/pki-validation/")),
                        "case {case}: expected the CA to fetch {target}, got {from:?}"
                    );
                    assert!(
                        to.contains("/.well-known/pki-validation/") && to.ends_with(".txt"),
                        "case {case}: expected a validation-file URL, got {to:?}"
                    );
                    assert!(
                        validation["ResourceRecord"].is_null(),
                        "case {case}: HTTP validation publishes no DNS record, got {validation}"
                    );
                }
            }
        }

        // Reading twice must report the same record: a describe reports, it does not
        // re-roll the validation evidence or move the status on.
        let again = describe(&router, &arn).await;
        assert_eq!(
            again, detail,
            "case {case}: two describes of one certificate must agree"
        );
    }
}

/// RULE: a certificate covers at most 100 domain names, counting `DomainName`
/// itself; one more is an ACM quota refusal.
#[tokio::test]
async fn the_domain_name_quota_is_enforced_at_its_bound() {
    // 99 SANs is the last accepted request (100 names in all) and 100 is one past the
    // published bound. Past 100 SANs the model's own `DomainList` length bound
    // (`len 1..100`) refuses the request at the boundary instead.
    let quota_cases: &[(usize, Option<&str>)] = &[
        (0, None),
        (1, None),
        (98, None),
        (99, None),
        (100, Some("LimitExceededException")),
        (150, Some("ValidationException")),
    ];
    let router = router();
    for (san_count, expected_error) in quota_cases {
        let case = format!("{san_count} subject alternative names");
        let sans: Vec<String> = (0..*san_count)
            .map(|index| format!("san{index}.quota.example.com"))
            .collect();
        let mut body = json!({ "DomainName": "quota.example.com", "ValidationMethod": "DNS" });
        if !sans.is_empty() {
            body["SubjectAlternativeNames"] = json!(sans);
        }
        let (status, response) = call(&router, "RequestCertificate", &body).await;
        match expected_error {
            None => assert_eq!(
                status, 200,
                "case {case}: a certificate of {} names is within quota, got {status} {response}",
                san_count + 1
            ),
            Some(expected) => assert_eq!(
                (status, error_code(&response).as_str()),
                (400, *expected),
                "case {case}: expected {expected} past the bound, got {status} {response}"
            ),
        }
    }
}

/// RULE: an `IdempotencyToken` identifies one certificate request, so re-sending the
/// same request with it returns the certificate already created, while a different
/// token or different parameters make a new one.
#[tokio::test]
async fn an_idempotency_token_identifies_one_certificate_request() {
    let router = router();
    let base = json!({
        "DomainName": "idem.example.com",
        "ValidationMethod": "DNS",
        "Options": { "Export": "DISABLED" },
        "Tags": [
            { "Key": "env", "Value": "dev" },
            { "Key": "team", "Value": "platform" }
        ],
        "IdempotencyToken": "tok1"
    });

    let first = request_certificate(&router, &base).await;

    // Each row: what changed about the request, and whether it must reuse `first`.
    // The fingerprint covers every member the certificate is built from, so a row
    // that changes any of them gets its own certificate, while a row that only
    // reorders a set is recognised as the same request.
    let cases: Vec<(&str, Value, bool)> = vec![
        ("the identical request", base.clone(), true),
        (
            "the identical request a third time",
            base.clone(),
            true,
        ),
        (
            "the same tags listed in the other order",
            json!({
                "DomainName": "idem.example.com",
                "ValidationMethod": "DNS",
                "Options": { "Export": "DISABLED" },
                "Tags": [
                    { "Key": "team", "Value": "platform" },
                    { "Key": "env", "Value": "dev" }
                ],
                "IdempotencyToken": "tok1"
            }),
            true,
        ),
        (
            "a changed tag value under the same token",
            json!({
                "DomainName": "idem.example.com",
                "ValidationMethod": "DNS",
                "Options": { "Export": "DISABLED" },
                "Tags": [
                    { "Key": "env", "Value": "prod" },
                    { "Key": "team", "Value": "platform" }
                ],
                "IdempotencyToken": "tok1"
            }),
            false,
        ),
        (
            "changed Options under the same token",
            json!({
                "DomainName": "idem.example.com",
                "ValidationMethod": "DNS",
                "Options": { "Export": "ENABLED" },
                "Tags": [
                    { "Key": "env", "Value": "dev" },
                    { "Key": "team", "Value": "platform" }
                ],
                "IdempotencyToken": "tok1"
            }),
            false,
        ),
        (
            "added DomainValidationOptions under the same token",
            json!({
                "DomainName": "idem.example.com",
                "ValidationMethod": "DNS",
                "Options": { "Export": "DISABLED" },
                "Tags": [
                    { "Key": "env", "Value": "dev" },
                    { "Key": "team", "Value": "platform" }
                ],
                "DomainValidationOptions": [
                    { "DomainName": "idem.example.com", "ValidationDomain": "example.com" }
                ],
                "IdempotencyToken": "tok1"
            }),
            false,
        ),
        (
            "a different domain under the same token",
            json!({
                "DomainName": "other.example.com",
                "ValidationMethod": "DNS",
                "IdempotencyToken": "tok1"
            }),
            false,
        ),
        (
            "added subject alternative names under the same token",
            json!({
                "DomainName": "idem.example.com",
                "SubjectAlternativeNames": ["alt.example.com"],
                "ValidationMethod": "DNS",
                "IdempotencyToken": "tok1"
            }),
            false,
        ),
        (
            "a different validation method under the same token",
            json!({
                "DomainName": "idem.example.com",
                "ValidationMethod": "EMAIL",
                "IdempotencyToken": "tok1"
            }),
            false,
        ),
        (
            "a different token for the same request",
            json!({
                "DomainName": "idem.example.com",
                "ValidationMethod": "DNS",
                "IdempotencyToken": "tok2"
            }),
            false,
        ),
        (
            "no token at all",
            json!({ "DomainName": "idem.example.com", "ValidationMethod": "DNS" }),
            false,
        ),
    ];

    let mut minted = vec![first.clone()];
    for (case, body, reuses) in cases {
        let arn = request_certificate(&router, &body).await;
        if reuses {
            assert_eq!(
                arn, first,
                "case {case}: expected the ARN already created, got {arn}"
            );
        } else {
            assert!(
                !minted.contains(&arn),
                "case {case}: expected a fresh certificate, got the existing {arn}"
            );
            minted.push(arn);
        }
    }

    // And the store holds exactly the certificates that were actually created.
    let listed = list_all_arns(&router, &json!({})).await;
    minted.sort();
    let mut listed_sorted = listed.clone();
    listed_sorted.sort();
    assert_eq!(
        listed_sorted, minted,
        "expected ListCertificates to report exactly the created certificates"
    );

    // A re-send that changed a member must not have discarded it: the certificate the
    // changed-tag row created carries the changed tag, and the original still carries
    // the original one.
    let (_, original_tags) = call(
        &router,
        "ListTagsForCertificate",
        &json!({ "CertificateArn": first }),
    )
    .await;
    assert!(
        original_tags["Tags"]
            .as_array()
            .is_some_and(|tags| tags.iter().any(|tag| tag["Key"] == "env"
                && tag["Value"] == "dev")),
        "expected the first certificate to keep env=dev, got {original_tags}"
    );
    let changed = minted
        .iter()
        .find(|arn| *arn != &first)
        .expect("the table created more than one certificate");
    let (status, response) = call(
        &router,
        "ListTagsForCertificate",
        &json!({ "CertificateArn": changed }),
    )
    .await;
    assert_eq!(
        status, 200,
        "every minted certificate must be readable, got {status} {response}"
    );
}

/// RULE: tags live on the certificate record, merged by key with the later value
/// winning, and `ListTagsForCertificate` reports exactly what is stored.
#[tokio::test]
async fn tags_round_trip_through_the_certificate_record() {
    let router = router();
    let arn = request_certificate(
        &router,
        &json!({ "DomainName": "tagged.example.com", "ValidationMethod": "DNS" }),
    )
    .await;

    // Each row: the tags added, and the full set expected afterwards.
    let cases: Vec<TagCase<'_>> = vec![
        (
            "a first single tag",
            json!([{ "Key": "env", "Value": "dev" }]),
            vec![("env", "dev")],
        ),
        (
            "several new keys at once",
            json!([
                { "Key": "team", "Value": "platform" },
                { "Key": "cost-centre", "Value": "4711" }
            ]),
            vec![("cost-centre", "4711"), ("env", "dev"), ("team", "platform")],
        ),
        (
            "an existing key, whose later value wins",
            json!([{ "Key": "env", "Value": "prod" }]),
            vec![
                ("cost-centre", "4711"),
                ("env", "prod"),
                ("team", "platform"),
            ],
        ),
        (
            "a key with no value",
            json!([{ "Key": "transient" }]),
            vec![
                ("cost-centre", "4711"),
                ("env", "prod"),
                ("team", "platform"),
                ("transient", ""),
            ],
        ),
    ];

    for (case, tags, expected) in cases {
        let (status, response) = call(
            &router,
            "AddTagsToCertificate",
            &json!({ "CertificateArn": arn, "Tags": tags }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: AddTagsToCertificate must succeed, got {status} {response}"
        );

        let (status, response) = call(
            &router,
            "ListTagsForCertificate",
            &json!({ "CertificateArn": arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: ListTagsForCertificate must succeed, got {status} {response}"
        );
        let mut actual: Vec<(String, String)> = response["Tags"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: no Tags in {response}"))
            .iter()
            .map(|tag| {
                (
                    tag["Key"].as_str().unwrap_or_default().to_string(),
                    tag["Value"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        actual.sort();
        let expected: Vec<(String, String)> = expected
            .into_iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        assert_eq!(
            actual, expected,
            "case {case}: expected tags {expected:?}, got {actual:?}"
        );
    }

    // An untagged certificate reports no Tags member: `TagList` has a minimum length
    // of one, so an empty list is not a legal value.
    let fresh = request_certificate(
        &router,
        &json!({ "DomainName": "untagged.example.com", "ValidationMethod": "DNS" }),
    )
    .await;
    let (status, response) = call(
        &router,
        "ListTagsForCertificate",
        &json!({ "CertificateArn": fresh }),
    )
    .await;
    assert_eq!(status, 200, "an untagged certificate still lists: {response}");
    assert!(
        response["Tags"].is_null(),
        "expected no Tags member for an untagged certificate, got {response}"
    );

    // A reserved key is refused, and refused without touching the stored tags.
    let (status, response) = call(
        &router,
        "AddTagsToCertificate",
        &json!({
            "CertificateArn": arn,
            "Tags": [{ "Key": "aws:reserved", "Value": "x" }]
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "InvalidTagException"),
        "expected a reserved tag key to be refused, got {status} {response}"
    );
    let (_, after) = call(
        &router,
        "ListTagsForCertificate",
        &json!({ "CertificateArn": arn }),
    )
    .await;
    assert_eq!(
        after["Tags"].as_array().map(Vec::len),
        Some(4),
        "a refused AddTagsToCertificate must leave the stored tags alone, got {after}"
    );
}

/// RULE: `ListCertificates` enumerates the live store — filtered by the requested
/// statuses, ordered by the requested sort, and split into pages of at most
/// `MaxItems` whose `NextToken` resumes exactly where the previous page stopped.
#[tokio::test]
async fn list_certificates_filters_sorts_and_pages_the_store() {
    let (router, engine) = router_with_engine();
    let domains = [
        "a.example.com",
        "b.example.com",
        "c.example.com",
        "d.example.com",
        "e.example.com",
    ];
    let mut created = Vec::new();
    for domain in domains {
        created.push(
            request_certificate(
                &router,
                &json!({ "DomainName": domain, "ValidationMethod": "DNS" }),
            )
            .await,
        );
    }
    // Filtering, ordering and paging are asserted against one fixed set of statuses,
    // so the certificates are held in the status the requests put them in.
    hold_pending_validations(&engine);

    let unfiltered = list_all_arns(&router, &json!({})).await;
    let mut sorted_created = created.clone();
    sorted_created.sort();
    let mut sorted_listed = unfiltered.clone();
    sorted_listed.sort();
    assert_eq!(
        sorted_listed, sorted_created,
        "expected ListCertificates to report every stored certificate"
    );

    // A status filter keeps only certificates in one of the named statuses. Rows
    // cover a status every certificate has, one none has, and both together.
    let status_cases: &[(&str, &[&str], usize)] = &[
        ("the status every certificate is in", &["PENDING_VALIDATION"], 5),
        ("a status no certificate is in", &["ISSUED"], 0),
        ("two statuses, one of them matching", &["ISSUED", "PENDING_VALIDATION"], 5),
        ("three non-matching statuses", &["EXPIRED", "FAILED", "REVOKED"], 0),
    ];
    for (case, statuses, expected) in status_cases {
        let arns = list_all_arns(&router, &json!({ "CertificateStatuses": statuses })).await;
        assert_eq!(
            arns.len(),
            *expected,
            "case {case}: expected {expected} certificates, got {arns:?}"
        );
    }

    // `SortBy=CREATED_AT` orders by creation time; DESCENDING is the exact reverse.
    let ascending = list_all_arns(
        &router,
        &json!({ "SortBy": "CREATED_AT", "SortOrder": "ASCENDING" }),
    )
    .await;
    assert_eq!(
        ascending, created,
        "expected CREATED_AT ASCENDING to report the creation order, got {ascending:?}"
    );
    let descending = list_all_arns(
        &router,
        &json!({ "SortBy": "CREATED_AT", "SortOrder": "DESCENDING" }),
    )
    .await;
    let mut reversed = created.clone();
    reversed.reverse();
    assert_eq!(
        descending, reversed,
        "expected CREATED_AT DESCENDING to be the reverse order, got {descending:?}"
    );

    // Paging: every page size from 1 to one past the number of certificates, so both
    // sides of each page boundary are covered. Whatever the size, the pages
    // concatenate to the whole ordered list with no repeat and no gap.
    for max_items in 1..=domains.len() + 1 {
        let case = format!("MaxItems={max_items}");
        let paged = list_all_arns(
            &router,
            &json!({
                "MaxItems": max_items,
                "SortBy": "CREATED_AT",
                "SortOrder": "ASCENDING"
            }),
        )
        .await;
        assert_eq!(
            paged, created,
            "case {case}: paging must reproduce the full ordered list, got {paged:?}"
        );

        // The first page itself is capped, and carries a token only when more remain.
        let (status, response) = call(
            &router,
            "ListCertificates",
            &json!({
                "MaxItems": max_items,
                "SortBy": "CREATED_AT",
                "SortOrder": "ASCENDING"
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: ListCertificates must succeed, got {response}"
        );
        let page_len = response["CertificateSummaryList"]
            .as_array()
            .map_or(0, Vec::len);
        assert_eq!(
            page_len,
            max_items.min(domains.len()),
            "case {case}: expected a page of {} summaries, got {response}",
            max_items.min(domains.len())
        );
        assert_eq!(
            response["NextToken"].is_string(),
            max_items < domains.len(),
            "case {case}: a NextToken must appear exactly when certificates remain, got {response}"
        );
    }

    // A summary is projected from the stored record, not from the request.
    let (_, response) = call(&router, "ListCertificates", &json!({ "MaxItems": 1 })).await;
    let summary = &response["CertificateSummaryList"][0];
    for (member, expected) in [
        ("Status", json!("PENDING_VALIDATION")),
        ("Type", json!("AMAZON_ISSUED")),
        ("KeyAlgorithm", json!("RSA_2048")),
        ("RenewalEligibility", json!("INELIGIBLE")),
        ("InUse", json!(false)),
        ("Exported", json!(false)),
        ("HasAdditionalSubjectAlternativeNames", json!(false)),
    ] {
        assert_eq!(
            summary[member], expected,
            "expected summary {member} to be {expected}, got {summary}"
        );
    }
    assert!(
        summary["CreatedAt"].is_number()
            && summary["SubjectAlternativeNameSummaries"].is_array(),
        "expected a stateful summary, got {summary}"
    );

    // `SortBy` and `SortOrder` are documented as a pair, so either on its own is
    // refused: a direction with no key, and a key with no direction. A request naming
    // both (asserted above) is the only accepted sorted form.
    let lone_sort_members: &[(&str, Value)] = &[
        ("SortOrder=ASCENDING alone", json!({ "SortOrder": "ASCENDING" })),
        ("SortOrder=DESCENDING alone", json!({ "SortOrder": "DESCENDING" })),
        ("SortBy=CREATED_AT alone", json!({ "SortBy": "CREATED_AT" })),
        (
            "SortBy alone beside other members",
            json!({ "SortBy": "CREATED_AT", "MaxItems": 2 }),
        ),
    ];
    for (case, body) in lone_sort_members {
        let (status, response) = call(&router, "ListCertificates", body).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "InvalidArgsException"),
            "case {case}: expected a refusal, got {status} {response}"
        );
    }

    // A request that names no MaxItems still gets a server-side page size, capped at
    // the model's own `MaxItems` maximum, so one call cannot serialise the store
    // however large it has grown.
    let (status, response) = call(&router, "ListCertificates", &json!({})).await;
    assert_eq!(
        status, 200,
        "an unfiltered ListCertificates must succeed, got {status} {response}"
    );
    let page_len = response["CertificateSummaryList"]
        .as_array()
        .map_or(0, Vec::len);
    assert!(
        page_len <= 1000,
        "expected the default page to be capped at 1000 summaries, got {page_len}"
    );

    // A token this service never issued is refused rather than silently ignored or
    // allowed to panic the handler, whatever shape it takes: not hex, hex that is not
    // UTF-8, hex of something that is not a certificate ARN, and hex of a well-formed
    // ARN carrying a position no instant can hold (out-of-range nanoseconds, seconds
    // past the end of time, a missing or unparsable position).
    let certificate_arn = "arn:aws:acm:us-west-2:000000000000:certificate/abc";
    let forged_tokens: &[(&str, String)] = &[
        ("not hex at all", "not-a-token".to_string()),
        ("hex that is not UTF-8", "ff".to_string()),
        (
            "hex of a string that is not a certificate ARN",
            hex::encode("arn:aws:acm:us-west-2:000000000000:acme-endpoint/abc"),
        ),
        ("hex of a one-character string", hex::encode("x")),
        (
            "hex of a certificate ARN with no position",
            hex::encode(certificate_arn),
        ),
        (
            "a position with out-of-range nanoseconds",
            hex::encode(format!("{certificate_arn}\u{1f}0.1000000000")),
        ),
        (
            "a position whose seconds overflow every instant",
            hex::encode(format!("{certificate_arn}\u{1f}18446744073709551615.000000000")),
        ),
        (
            "a position that is not a number",
            hex::encode(format!("{certificate_arn}\u{1f}later.000000000")),
        ),
    ];
    for (case, token) in forged_tokens {
        let (status, response) =
            call(&router, "ListCertificates", &json!({ "NextToken": token })).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "InvalidArgsException"),
            "case {case}: expected a foreign NextToken to be refused, got {status} {response}"
        );
    }
}

/// RULE: `DeleteCertificate` removes the record, so the certificate is afterwards
/// invisible to every reader and a second delete finds nothing.
#[tokio::test]
async fn delete_certificate_removes_the_record() {
    let router = router();
    let keep = request_certificate(
        &router,
        &json!({ "DomainName": "keep.example.com", "ValidationMethod": "DNS" }),
    )
    .await;
    let doomed = request_certificate(
        &router,
        &json!({
            "DomainName": "doomed.example.com",
            "ValidationMethod": "DNS",
            "IdempotencyToken": "doomedtoken",
            "Tags": [{ "Key": "env", "Value": "dev" }]
        }),
    )
    .await;

    let (status, response) = call(
        &router,
        "DeleteCertificate",
        &json!({ "CertificateArn": doomed }),
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteCertificate must succeed, got {status} {response}"
    );

    // Gone from every reader.
    for operation in [
        "DescribeCertificate",
        "ListTagsForCertificate",
        "AddTagsToCertificate",
        "DeleteCertificate",
    ] {
        let mut body = json!({ "CertificateArn": doomed });
        if operation == "AddTagsToCertificate" {
            body["Tags"] = json!([{ "Key": "env", "Value": "dev" }]);
        }
        let (status, response) = call(&router, operation, &body).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ResourceNotFoundException"),
            "case {operation}: a deleted certificate must be gone, got {status} {response}"
        );
    }
    let remaining = list_all_arns(&router, &json!({})).await;
    assert_eq!(
        remaining,
        vec![keep.clone()],
        "expected only the surviving certificate to be listed"
    );

    // The idempotency token of a deleted certificate cannot resurrect its ARN: a
    // minted ARN always has a record behind it.
    let reissued = request_certificate(
        &router,
        &json!({
            "DomainName": "doomed.example.com",
            "ValidationMethod": "DNS",
            "IdempotencyToken": "doomedtoken"
        }),
    )
    .await;
    assert_ne!(
        reissued, doomed,
        "a deleted certificate's token must not return its ARN"
    );
    let detail = describe(&router, &reissued).await;
    assert_eq!(
        detail["CertificateArn"].as_str(),
        Some(reissued.as_str()),
        "expected the re-issued certificate to be readable, got {detail}"
    );
}

/// RULE: an ARN that is not an ACM certificate ARN is `InvalidArnException`, while a
/// well-formed one with no certificate behind it is `ResourceNotFoundException`.
#[tokio::test]
async fn arns_are_classified_before_they_are_looked_up() {
    let router = router();
    let live = request_certificate(
        &router,
        &json!({ "DomainName": "live.example.com", "ValidationMethod": "DNS" }),
    )
    .await;

    // Each row: an ARN that satisfies the member's modelled `@pattern` and length,
    // and the error it must draw. The `certificate/...` rows name no stored record;
    // the others name a resource that is not a certificate at all.
    let cases: &[(&str, String, &str)] = &[
        (
            "a certificate id with nothing behind it",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/does-not-exist"),
            "ResourceNotFoundException",
        ),
        (
            "a certificate in another region",
            format!("arn:aws:acm:eu-central-1:{ACCOUNT}:certificate/11111111-2222-3333-4444-555555555555"),
            "ResourceNotFoundException",
        ),
        (
            "a resource type that is not a certificate",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:acme-endpoint/abc"),
            "InvalidArnException",
        ),
        (
            "an ACM ARN with no resource type",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate"),
            "InvalidArnException",
        ),
        (
            "a nested resource below a certificate",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/abc/extra"),
            "InvalidArnException",
        ),
        (
            "a global ACM ARN with an empty region",
            "arn:aws:acm::0:aaaaaaaaaaaaaaaaaaaa".to_string(),
            "InvalidArnException",
        ),
    ];

    for (case, arn, expected) in cases {
        let (status, response) =
            call(&router, "DescribeCertificate", &json!({ "CertificateArn": arn })).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, *expected),
            "case {case}: expected {expected} for {arn}, got {status} {response}"
        );
    }

    // An ARN the model's own constraints refuse never reaches the handler, so it is
    // answered at the boundary — still a 400, with the request named as invalid.
    let (status, response) = call(
        &router,
        "DescribeCertificate",
        &json!({ "CertificateArn": "not-an-arn" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "expected a malformed ARN to be refused, got {status} {response}"
    );

    // And none of the refusals disturbed the live certificate.
    let detail = describe(&router, &live).await;
    assert_eq!(
        detail["DomainName"].as_str(),
        Some("live.example.com"),
        "expected the live certificate to be untouched, got {detail}"
    );
}
