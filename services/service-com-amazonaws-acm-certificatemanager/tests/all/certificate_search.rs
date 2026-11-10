// SPDX-License-Identifier: CC0-1.0

//! `SearchCertificates` end to end over the wire (`POST /`, `X-Amz-Target:
//! CertificateManager.SearchCertificates`, awsJson1_1).
//!
//! RULE under test: `SearchCertificates` answers from the PERSISTED certificate
//! records — it keeps the certificates whose own stored X.509 attributes and ACM
//! metadata satisfy the request's filter statement (a tree of `And`/`Or`/`Not` over
//! leaf filters), orders them by the requested `SortBy`/`SortOrder`, and reports at
//! most `MaxResults` of them per page with a `NextToken` that resumes in that same
//! order.
//!
//! Every call goes through [`MultiServiceRouter::dispatch`], the path the emulator
//! serves real traffic on, and every certificate is created with
//! `RequestCertificate` and cross-read with `DescribeCertificate`, so a response
//! that was not computed from the store cannot pass.
//!
//! The tables span what the rule generalises over: every leaf filter variant the
//! model declares, both comparison operators, each logical operator and a nesting of
//! them, every sort field and both directions, page sizes either side of the result
//! count, both forms of unsatisfiable `TimestampRange`, the statement-nesting bound
//! either side of its limit, and a revoked certificate beside one left alone.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_lifecycle::{
    call, describe, error_code, error_message, hold_pending_validations, request_certificate,
    router, router_with_engine,
};
use crate::certificate_revocation::mark_exported;
use crate::certificate_validation_state_machine::make_validation_due_now;

/// A private certificate authority ARN of the form `CertificateAuthorityArn`'s
/// pattern requires (`^arn:[\w+=/,.@-]+:acm-pca:[\w+=/,.@-]*:[0-9]+:[\w+=,.@-]+(/[\w+=,.@-]+)*$`).
/// A request naming one needs no proof of domain ownership, so the certificate it
/// creates is owed `ISSUED`.
const PRIVATE_CA_ARN: &str =
    "arn:aws:acm-pca:us-west-2:000000000000:certificate-authority/12345678-1234-1234-1234-123456789012";

/// Seconds in a day, for the timestamp ranges below.
const DAY: i64 = 24 * 60 * 60;

/// Every `SearchCertificatesSortBy` value the model declares
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#SearchCertificatesSortBy")`).
const SORT_FIELDS: &[&str] = &[
    "CREATED_AT",
    "NOT_AFTER",
    "STATUS",
    "RENEWAL_STATUS",
    "EXPORTED",
    "IN_USE",
    "NOT_BEFORE",
    "KEY_ALGORITHM",
    "TYPE",
    "CERTIFICATE_ARN",
    "COMMON_NAME",
    "REVOKED_AT",
    "RENEWAL_ELIGIBILITY",
    "ISSUED_AT",
    "MANAGED_BY",
    "EXPORT_OPTION",
    "VALIDATION_METHOD",
    "IMPORTED_AT",
    "ACME_ENDPOINT_ARN",
    "ACME_ACCOUNT_ID",
    "CERTIFICATE_KEY_PAIR_ORIGIN",
];

/// Both `SearchCertificatesSortOrder` values the model declares
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#SearchCertificatesSortOrder")`).
const SORT_ORDERS: &[&str] = &["ASCENDING", "DESCENDING"];

/// One stored certificate of the fixture: the label the expectations name it by and
/// the ARN the service minted for it.
#[derive(Debug, Clone)]
struct Stored {
    label: &'static str,
    arn: String,
}

/// The `RequestCertificate` bodies the filter tables run against.
///
/// The rows differ along everything the leaf filters select on: the status the clock
/// owes them (private authority -> `ISSUED`, a name the public DNS delegates to
/// nobody -> `FAILED`), the key algorithm,
/// the validation method, the export option, the manager, the name form (plain,
/// wildcard, deep label) and the number of subject alternative names.
fn fixture_requests() -> Vec<(&'static str, Value)> {
    vec![
        (
            "issued-rsa2048-export",
            json!({
                "DomainName": "alpha.example.com",
                "SubjectAlternativeNames": ["www.alpha.example.com"],
                "CertificateAuthorityArn": PRIVATE_CA_ARN,
                "KeyAlgorithm": "RSA_2048",
                "ValidationMethod": "DNS",
                "Options": { "Export": "ENABLED" },
            }),
        ),
        (
            "issued-ec-cloudfront",
            json!({
                "DomainName": "beta.example.net",
                "CertificateAuthorityArn": PRIVATE_CA_ARN,
                "KeyAlgorithm": "EC_prime256v1",
                "ValidationMethod": "EMAIL",
                "ManagedBy": "CLOUDFRONT",
            }),
        ),
        (
            "issued-wildcard-rsa1024",
            json!({
                "DomainName": "*.delta.example.io",
                "CertificateAuthorityArn": PRIVATE_CA_ARN,
                "KeyAlgorithm": "RSA_1024",
                "ValidationMethod": "DNS",
                "Options": { "Export": "DISABLED" },
            }),
        ),
        (
            "failed-rsa4096",
            json!({
                "DomainName": "gamma.service.example.test",
                "SubjectAlternativeNames": ["api.gamma.service.example.test", "edge.example.test"],
                "KeyAlgorithm": "RSA_4096",
                "ValidationMethod": "HTTP",
            }),
        ),
    ]
}

/// Create every fixture certificate on `router`, in table order.
async fn store_fixtures(router: &MultiServiceRouter) -> Vec<Stored> {
    let mut stored = Vec::new();
    for (label, body) in fixture_requests() {
        let arn = request_certificate(router, &body).await;
        stored.push(Stored { label, arn });
    }
    stored
}

/// The ARNs of the fixtures `labels` names, sorted — the expectations are about
/// membership, not page order.
fn expected(stored: &[Stored], labels: &[&str]) -> Vec<String> {
    let mut arns: Vec<String> = stored
        .iter()
        .filter(|entry| labels.contains(&entry.label))
        .map(|entry| entry.arn.clone())
        .collect();
    assert_eq!(
        arns.len(),
        labels.len(),
        "the expectation names a label the fixture does not hold: {labels:?}"
    );
    arns.sort();
    arns
}

/// Every result one `SearchCertificates` request reports, paging to the end in page
/// order, failing the test on any error.
async fn search_results(router: &MultiServiceRouter, body: &Value) -> Vec<Value> {
    let limit = body["MaxResults"].as_u64();
    let mut all: Vec<Value> = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut request = body.clone();
        if let Some(token) = next.as_ref() {
            request["NextToken"] = json!(token);
        }
        let (status, response) = call(router, "SearchCertificates", &request).await;
        assert_eq!(
            status, 200,
            "case {request}: SearchCertificates must succeed, got {status} {response}"
        );
        let page = response["Results"]
            .as_array()
            .unwrap_or_else(|| panic!("case {request}: no Results in {response}"))
            .clone();
        if let Some(limit) = limit {
            assert!(
                page.len() as u64 <= limit,
                "case {request}: a page carried {} results, more than MaxResults {limit}",
                page.len()
            );
        }
        all.extend(page);
        next = response["NextToken"].as_str().map(str::to_string);
        if next.is_none() {
            break;
        }
        assert!(
            all.len() <= 64,
            "case {request}: paging did not terminate after {} results",
            all.len()
        );
    }
    all
}

/// The ARNs of some search results, in the order the pages reported them.
fn result_arns(case: &str, results: &[Value]) -> Vec<String> {
    results
        .iter()
        .map(|result| {
            result["CertificateArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: a result carried no CertificateArn: {result}"))
                .to_string()
        })
        .collect()
}

/// The ARNs one filter statement selects, sorted.
async fn matching_arns(router: &MultiServiceRouter, case: &str, statement: &Value) -> Vec<String> {
    let body = if statement.is_null() {
        json!({})
    } else {
        json!({ "FilterStatement": statement })
    };
    let results = search_results(router, &body).await;
    let mut arns = result_arns(case, &results);
    arns.sort();
    arns
}

/// A nested object of a response, failing the test with the case name when it is
/// absent rather than silently comparing against `null`.
fn object(case: &str, value: &Value) -> Value {
    assert!(
        value.is_object(),
        "case {case}: expected an object, got {value}"
    );
    value.clone()
}

/// Epoch seconds now, the base of the timestamp ranges below.
fn epoch_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as i64
}

/// RULE: a leaf filter keeps exactly the certificates whose own stored value carries
/// what the filter names — for every leaf filter variant the model declares.
#[tokio::test]
async fn each_leaf_filter_selects_on_the_stored_record() {
    let (router, engine) = router_with_engine();
    let stored = store_fixtures(&router).await;
    // The clock owes each fixture its terminal status; moving the stored due instant
    // into the past is the state the wall clock would have left, and the next request
    // materialises it exactly as in production.
    make_validation_due_now(&engine);

    let now = epoch_now();
    let issued = &[
        "issued-rsa2048-export",
        "issued-ec-cloudfront",
        "issued-wildcard-rsa1024",
    ];
    let all = &[
        "issued-rsa2048-export",
        "issued-ec-cloudfront",
        "issued-wildcard-rsa1024",
        "failed-rsa4096",
    ];
    let beta = stored
        .iter()
        .find(|entry| entry.label == "issued-ec-cloudfront")
        .expect("the fixture holds beta")
        .arn
        .clone();

    // Each row: what it is, the filter statement, and the labels it must select.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        ("no statement at all", Value::Null, all.to_vec()),
        (
            "CertificateArn names one record",
            json!({ "Filter": { "CertificateArn": beta } }),
            vec!["issued-ec-cloudfront"],
        ),
        (
            "Status ISSUED",
            json!({ "Filter": { "AcmCertificateMetadataFilter": { "Status": "ISSUED" } } }),
            issued.to_vec(),
        ),
        (
            "Status FAILED",
            json!({ "Filter": { "AcmCertificateMetadataFilter": { "Status": "FAILED" } } }),
            vec!["failed-rsa4096"],
        ),
        (
            "Status no record is in",
            json!({ "Filter": { "AcmCertificateMetadataFilter": { "Status": "REVOKED" } } }),
            vec![],
        ),
        (
            "Type AMAZON_ISSUED",
            json!({ "Filter": { "AcmCertificateMetadataFilter": { "Type": "AMAZON_ISSUED" } } }),
            all.to_vec(),
        ),
        (
            "Type IMPORTED, which nothing imported",
            json!({ "Filter": { "AcmCertificateMetadataFilter": { "Type": "IMPORTED" } } }),
            vec![],
        ),
        (
            "ValidationMethod DNS",
            json!({ "Filter": { "AcmCertificateMetadataFilter": { "ValidationMethod": "DNS" } } }),
            vec!["issued-rsa2048-export", "issued-wildcard-rsa1024"],
        ),
        (
            "ValidationMethod HTTP",
            json!({ "Filter": { "AcmCertificateMetadataFilter": { "ValidationMethod": "HTTP" } } }),
            vec!["failed-rsa4096"],
        ),
        (
            "ExportOption ENABLED",
            json!({ "Filter": { "AcmCertificateMetadataFilter": { "ExportOption": "ENABLED" } } }),
            vec!["issued-rsa2048-export"],
        ),
        (
            "ExportOption DISABLED, the value a request naming none gets",
            json!({ "Filter": { "AcmCertificateMetadataFilter": { "ExportOption": "DISABLED" } } }),
            vec![
                "issued-ec-cloudfront",
                "issued-wildcard-rsa1024",
                "failed-rsa4096",
            ],
        ),
        (
            "ManagedBy CLOUDFRONT",
            json!({ "Filter": { "AcmCertificateMetadataFilter": { "ManagedBy": "CLOUDFRONT" } } }),
            vec!["issued-ec-cloudfront"],
        ),
        (
            "CertificateKeyPairOrigin AWS_MANAGED",
            json!({ "Filter": { "AcmCertificateMetadataFilter": { "CertificateKeyPairOrigin": "AWS_MANAGED" } } }),
            all.to_vec(),
        ),
        (
            "CertificateKeyPairOrigin ACME, which nothing issued",
            json!({ "Filter": { "AcmCertificateMetadataFilter": { "CertificateKeyPairOrigin": "ACME" } } }),
            vec![],
        ),
        (
            "InUse false: nothing has associated a certificate",
            json!({ "Filter": { "AcmCertificateMetadataFilter": { "InUse": false } } }),
            all.to_vec(),
        ),
        (
            "InUse true",
            json!({ "Filter": { "AcmCertificateMetadataFilter": { "InUse": true } } }),
            vec![],
        ),
        (
            "Exported false: nothing has been exported",
            json!({ "Filter": { "AcmCertificateMetadataFilter": { "Exported": false } } }),
            all.to_vec(),
        ),
        (
            "Exported true",
            json!({ "Filter": { "AcmCertificateMetadataFilter": { "Exported": true } } }),
            vec![],
        ),
        (
            "RenewalStatus, which no record carries",
            json!({ "Filter": { "AcmCertificateMetadataFilter": { "RenewalStatus": "SUCCESS" } } }),
            vec![],
        ),
        (
            "AcmeAccountId, which no record carries",
            json!({ "Filter": { "AcmCertificateMetadataFilter": { "AcmeAccountId": "12345678-1234-1234-1234-123456789012" } } }),
            vec![],
        ),
        (
            "AcmeEndpointArn, which no record carries",
            json!({ "Filter": { "AcmCertificateMetadataFilter": { "AcmeEndpointArn": "arn:aws:acm:us-west-2:000000000000:acme-endpoint/abc" } } }),
            vec![],
        ),
        (
            "KeyAlgorithm RSA_2048",
            json!({ "Filter": { "X509AttributeFilter": { "KeyAlgorithm": "RSA_2048" } } }),
            vec!["issued-rsa2048-export"],
        ),
        (
            "KeyAlgorithm RSA_1024, outside ListCertificates' default set",
            json!({ "Filter": { "X509AttributeFilter": { "KeyAlgorithm": "RSA_1024" } } }),
            vec!["issued-wildcard-rsa1024"],
        ),
        (
            "KeyAlgorithm EC_secp521r1, which no record carries",
            json!({ "Filter": { "X509AttributeFilter": { "KeyAlgorithm": "EC_secp521r1" } } }),
            vec![],
        ),
        (
            "KeyUsage of an issued certificate",
            json!({ "Filter": { "X509AttributeFilter": { "KeyUsage": "DIGITAL_SIGNATURE" } } }),
            issued.to_vec(),
        ),
        (
            "KeyUsage no certificate carries",
            json!({ "Filter": { "X509AttributeFilter": { "KeyUsage": "CRL_SIGNING" } } }),
            vec![],
        ),
        (
            "ExtendedKeyUsage of an issued certificate",
            json!({ "Filter": { "X509AttributeFilter": { "ExtendedKeyUsage": "TLS_WEB_SERVER_AUTHENTICATION" } } }),
            issued.to_vec(),
        ),
        (
            "ExtendedKeyUsage no certificate carries",
            json!({ "Filter": { "X509AttributeFilter": { "ExtendedKeyUsage": "CODE_SIGNING" } } }),
            vec![],
        ),
        (
            "SerialNumber, which no record carries",
            json!({ "Filter": { "X509AttributeFilter": { "SerialNumber": "0a:1b:2c" } } }),
            vec![],
        ),
        (
            "Subject CommonName EQUALS",
            json!({ "Filter": { "X509AttributeFilter": { "Subject": { "CommonName": { "Value": "beta.example.net", "ComparisonOperator": "EQUALS" } } } } }),
            vec!["issued-ec-cloudfront"],
        ),
        (
            "Subject CommonName EQUALS, in another case: DNS labels are case-insensitive",
            json!({ "Filter": { "X509AttributeFilter": { "Subject": { "CommonName": { "Value": "BETA.Example.NET", "ComparisonOperator": "EQUALS" } } } } }),
            vec!["issued-ec-cloudfront"],
        ),
        (
            "Subject CommonName EQUALS a name only a SAN carries",
            json!({ "Filter": { "X509AttributeFilter": { "Subject": { "CommonName": { "Value": "www.alpha.example.com", "ComparisonOperator": "EQUALS" } } } } }),
            vec![],
        ),
        (
            "Subject CommonName CONTAINS a label",
            json!({ "Filter": { "X509AttributeFilter": { "Subject": { "CommonName": { "Value": "service.example", "ComparisonOperator": "CONTAINS" } } } } }),
            vec!["failed-rsa4096"],
        ),
        (
            "Subject CommonName CONTAINS a suffix every fixture shares",
            json!({ "Filter": { "X509AttributeFilter": { "Subject": { "CommonName": { "Value": "example.", "ComparisonOperator": "CONTAINS" } } } } }),
            all.to_vec(),
        ),
        (
            "SubjectAlternativeName DnsName EQUALS a SAN",
            json!({ "Filter": { "X509AttributeFilter": { "SubjectAlternativeName": { "DnsName": { "Value": "edge.example.test", "ComparisonOperator": "EQUALS" } } } } }),
            vec!["failed-rsa4096"],
        ),
        (
            "SubjectAlternativeName DnsName EQUALS the domain name, which is also a SAN",
            json!({ "Filter": { "X509AttributeFilter": { "SubjectAlternativeName": { "DnsName": { "Value": "*.delta.example.io", "ComparisonOperator": "EQUALS" } } } } }),
            vec!["issued-wildcard-rsa1024"],
        ),
        (
            "SubjectAlternativeName DnsName CONTAINS",
            json!({ "Filter": { "X509AttributeFilter": { "SubjectAlternativeName": { "DnsName": { "Value": "api.", "ComparisonOperator": "CONTAINS" } } } } }),
            vec!["failed-rsa4096"],
        ),
        (
            "NotAfter inside the validity window of every issued certificate",
            json!({ "Filter": { "X509AttributeFilter": { "NotAfter": { "Start": now, "End": now + 400 * DAY } } } }),
            issued.to_vec(),
        ),
        (
            "NotAfter after every validity window",
            json!({ "Filter": { "X509AttributeFilter": { "NotAfter": { "Start": now + 800 * DAY } } } }),
            vec![],
        ),
        (
            "NotAfter with an open start",
            json!({ "Filter": { "X509AttributeFilter": { "NotAfter": { "End": now + 400 * DAY } } } }),
            issued.to_vec(),
        ),
        (
            "NotBefore up to now: only an issued certificate has one",
            json!({ "Filter": { "X509AttributeFilter": { "NotBefore": { "End": now + DAY } } } }),
            issued.to_vec(),
        ),
        (
            "NotBefore in the past, before any fixture existed",
            json!({ "Filter": { "X509AttributeFilter": { "NotBefore": { "End": now - 400 * DAY } } } }),
            vec![],
        ),
    ];

    for (case, statement, labels) in cases {
        let actual = matching_arns(&router, case, &statement).await;
        assert_eq!(
            actual,
            expected(&stored, &labels),
            "case {case:?}: expected the certificates {labels:?}, got {actual:?}"
        );
    }
}

/// RULE: the logical operators combine leaf filters — `And` keeps a certificate every
/// nested statement keeps, `Or` one any of them keeps, `Not` one its nested statement
/// does not — to any depth.
#[tokio::test]
async fn logical_operators_combine_the_leaf_filters() {
    let (router, engine) = router_with_engine();
    let stored = store_fixtures(&router).await;
    make_validation_due_now(&engine);

    let issued_status = json!({ "Filter": { "AcmCertificateMetadataFilter": { "Status": "ISSUED" } } });
    let rsa2048 = json!({ "Filter": { "X509AttributeFilter": { "KeyAlgorithm": "RSA_2048" } } });
    let rsa4096 = json!({ "Filter": { "X509AttributeFilter": { "KeyAlgorithm": "RSA_4096" } } });
    let dns = json!({ "Filter": { "AcmCertificateMetadataFilter": { "ValidationMethod": "DNS" } } });

    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "And of one statement",
            json!({ "And": [issued_status.clone()] }),
            vec![
                "issued-rsa2048-export",
                "issued-ec-cloudfront",
                "issued-wildcard-rsa1024",
            ],
        ),
        (
            "And of two, one certificate satisfying both",
            json!({ "And": [issued_status.clone(), rsa2048.clone()] }),
            vec!["issued-rsa2048-export"],
        ),
        (
            "And of two nothing satisfies",
            json!({ "And": [issued_status.clone(), rsa4096.clone()] }),
            vec![],
        ),
        (
            "Or of two disjoint leaves",
            json!({ "Or": [rsa2048.clone(), rsa4096.clone()] }),
            vec!["issued-rsa2048-export", "failed-rsa4096"],
        ),
        (
            "Not of a leaf",
            json!({ "Not": issued_status.clone() }),
            vec!["failed-rsa4096"],
        ),
        (
            "Not of a Not is the leaf again",
            json!({ "Not": { "Not": rsa2048.clone() } }),
            vec!["issued-rsa2048-export"],
        ),
        (
            "And over an Or and a Not, three levels deep",
            json!({
                "And": [
                    { "Or": [rsa2048.clone(), { "Filter": { "X509AttributeFilter": { "KeyAlgorithm": "RSA_1024" } } }] },
                    { "Not": { "Not": dns.clone() } },
                ]
            }),
            vec!["issued-rsa2048-export", "issued-wildcard-rsa1024"],
        ),
        (
            "Or over an And and a leaf",
            json!({
                "Or": [
                    { "And": [issued_status.clone(), rsa2048.clone()] },
                    rsa4096.clone(),
                ]
            }),
            vec!["issued-rsa2048-export", "failed-rsa4096"],
        ),
    ];

    for (case, statement, labels) in cases {
        let actual = matching_arns(&router, case, &statement).await;
        assert_eq!(
            actual,
            expected(&stored, &labels),
            "case {case:?}: expected the certificates {labels:?}, got {actual:?}"
        );
    }
}

/// RULE: every result is projected from the stored record, so what a search reports
/// about a certificate is what `DescribeCertificate` reports about the same one.
#[tokio::test]
async fn a_search_result_reports_the_stored_record() {
    let (router, engine) = router_with_engine();
    let stored = store_fixtures(&router).await;
    make_validation_due_now(&engine);

    let results = search_results(&router, &json!({})).await;
    assert_eq!(
        results.len(),
        stored.len(),
        "expected one result per stored certificate, got {results:?}"
    );

    for result in &results {
        let arn = result["CertificateArn"]
            .as_str()
            .unwrap_or_else(|| panic!("a result carried no CertificateArn: {result}"))
            .to_string();
        let case = stored
            .iter()
            .find(|entry| entry.arn == arn)
            .map(|entry| entry.label)
            .unwrap_or_else(|| panic!("the search reported an ARN nothing created: {arn}"));
        let detail = describe(&router, &arn).await;
        let metadata = object(
            case,
            &result["CertificateMetadata"]["AcmCertificateMetadata"],
        );
        let attributes = object(case, &result["X509Attributes"]);

        // Each member a search result shares with the described record must carry the
        // same value: one record, two readers.
        for member in [
            "Status",
            "Type",
            "ManagedBy",
            "CertificateKeyPairOrigin",
            "RenewalEligibility",
            "CreatedAt",
            "IssuedAt",
        ] {
            assert_eq!(
                metadata[member], detail[member],
                "case {case}: {member} must agree with DescribeCertificate, got {metadata} vs {detail}"
            );
        }
        for member in ["NotBefore", "NotAfter"] {
            assert_eq!(
                attributes[member], detail[member],
                "case {case}: {member} must agree with DescribeCertificate, got {attributes} vs {detail}"
            );
        }
        // `KeyAlgorithm` is the one member the two readers state in different
        // spellings: a search result reports the value its own
        // `X509AttributeFilter.KeyAlgorithm` selects on — the model enum's value
        // (`EC_prime256v1`) — while the certificate's detail reports the X.509
        // descriptor (`EC-prime256v1`). They are the same algorithm, so the
        // detail's spelling is the reported one with its first separator restored.
        let described_algorithm = detail["KeyAlgorithm"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: no KeyAlgorithm in {detail}"))
            .replacen('-', "_", 1);
        assert_eq!(
            attributes["KeyAlgorithm"].as_str(),
            Some(described_algorithm.as_str()),
            "case {case}: KeyAlgorithm must be the described algorithm in the enum's \
             spelling, got {attributes} vs {detail}"
        );
        // The subject common name of an ACM certificate is its domain name.
        assert_eq!(
            attributes["Subject"]["CommonName"], detail["DomainName"],
            "case {case}: the subject common name is the certificate's domain name, got {attributes} vs {detail}"
        );
        // The subject alternative names are the DNS names the record covers.
        let reported: Vec<&str> = attributes["SubjectAlternativeNames"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: no SubjectAlternativeNames in {attributes}"))
            .iter()
            .map(|name| {
                name["DnsName"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case}: a general name is no DNS name: {name}"))
            })
            .collect();
        let described: Vec<&str> = detail["SubjectAlternativeNames"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: no SubjectAlternativeNames in {detail}"))
            .iter()
            .map(|name| name.as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            reported, described,
            "case {case}: the reported SANs must be the stored ones, got {reported:?} vs {described:?}"
        );
        // An attribute no stored record carries stays absent rather than being
        // invented: this emulator signs no X.509 certificate, so there is no issuer
        // and no serial number.
        for member in ["Issuer", "SerialNumber"] {
            assert!(
                attributes[member].is_null(),
                "case {case}: {member} is not stored, so it must be absent, got {attributes}"
            );
        }
        for member in ["ImportedAt", "RevokedAt", "RenewalStatus", "AcmeAccountId"] {
            assert!(
                metadata[member].is_null(),
                "case {case}: {member} is not stored, so it must be absent, got {metadata}"
            );
        }
    }
}

/// RULE: a search reports the status the certificates are actually in — the transient
/// `PENDING_VALIDATION` while the clock still owes the outcome, and the terminal
/// status once it has fallen due.
#[tokio::test]
async fn a_status_filter_follows_the_clock_owed_status() {
    let (router, engine) = router_with_engine();
    let stored = store_fixtures(&router).await;
    // Hold every validation outcome in the future: the fixtures are all still pending.
    hold_pending_validations(&engine);

    let pending =
        json!({ "Filter": { "AcmCertificateMetadataFilter": { "Status": "PENDING_VALIDATION" } } });
    let issued = json!({ "Filter": { "AcmCertificateMetadataFilter": { "Status": "ISSUED" } } });

    let all: Vec<&str> = stored.iter().map(|entry| entry.label).collect();
    let held = matching_arns(&router, "held pending", &pending).await;
    assert_eq!(
        held,
        expected(&stored, &all),
        "case held pending: every fixture is still PENDING_VALIDATION, got {held:?}"
    );
    let issued_while_held = matching_arns(&router, "issued while held", &issued).await;
    assert!(
        issued_while_held.is_empty(),
        "case issued while held: nothing is issued yet, got {issued_while_held:?}"
    );

    // The clock makes the outcomes due; the next request materialises them.
    make_validation_due_now(&engine);
    let still_pending = matching_arns(&router, "after the outcome fell due", &pending).await;
    assert!(
        still_pending.is_empty(),
        "case after the outcome fell due: nothing is pending any more, got {still_pending:?}"
    );
    let now_issued = matching_arns(&router, "after the outcome fell due", &issued).await;
    assert_eq!(
        now_issued,
        expected(
            &stored,
            &[
                "issued-rsa2048-export",
                "issued-ec-cloudfront",
                "issued-wildcard-rsa1024",
            ]
        ),
        "case after the outcome fell due: the private requests are issued, got {now_issued:?}"
    );
}

/// RULE: results are ordered by the requested field and direction, and paging through
/// them with any `MaxResults` reports every match exactly once in that same order.
#[tokio::test]
async fn results_are_ordered_and_paged_in_that_order() {
    let (router, engine) = router_with_engine();
    let stored = store_fixtures(&router).await;
    make_validation_due_now(&engine);

    let all_arns = expected(
        &stored,
        &stored.iter().map(|entry| entry.label).collect::<Vec<_>>(),
    );

    for field in SORT_FIELDS {
        for order in SORT_ORDERS {
            let case = format!("SortBy {field} {order}");
            let unpaged = search_results(
                &router,
                &json!({ "SortBy": field, "SortOrder": order }),
            )
            .await;
            let unpaged = result_arns(&case, &unpaged);
            let mut seen = unpaged.clone();
            seen.sort();
            assert_eq!(
                seen, all_arns,
                "case {case}: one page of 100 reports every certificate exactly once, got {unpaged:?}"
            );

            // Every page size either side of the result count walks the same order.
            for max_results in 1..=(stored.len() + 1) {
                let paged = search_results(
                    &router,
                    &json!({ "SortBy": field, "SortOrder": order, "MaxResults": max_results }),
                )
                .await;
                let paged = result_arns(&case, &paged);
                assert_eq!(
                    paged, unpaged,
                    "case {case} with MaxResults {max_results}: paging must walk the unpaged order, got {paged:?}"
                );
            }
        }
    }

    // Two fields whose expected order the test can state outright: the ARN order, and
    // the common-name order of the domains the fixtures were requested for.
    let by_arn = result_arns(
        "SortBy CERTIFICATE_ARN ASCENDING",
        &search_results(
            &router,
            &json!({ "SortBy": "CERTIFICATE_ARN", "SortOrder": "ASCENDING" }),
        )
        .await,
    );
    assert_eq!(
        by_arn, all_arns,
        "case SortBy CERTIFICATE_ARN ASCENDING: expected the ARN order, got {by_arn:?}"
    );
    let mut descending = all_arns.clone();
    descending.reverse();
    let by_arn_descending = result_arns(
        "SortBy CERTIFICATE_ARN DESCENDING",
        &search_results(
            &router,
            &json!({ "SortBy": "CERTIFICATE_ARN", "SortOrder": "DESCENDING" }),
        )
        .await,
    );
    assert_eq!(
        by_arn_descending, descending,
        "case SortBy CERTIFICATE_ARN DESCENDING: expected the reversed ARN order, got {by_arn_descending:?}"
    );

    let by_name = result_arns(
        "SortBy COMMON_NAME ASCENDING",
        &search_results(
            &router,
            &json!({ "SortBy": "COMMON_NAME", "SortOrder": "ASCENDING" }),
        )
        .await,
    );
    let mut names: Vec<(String, String)> = Vec::new();
    for entry in &stored {
        let detail = describe(&router, &entry.arn).await;
        names.push((
            detail["DomainName"]
                .as_str()
                .expect("a described certificate carries its domain name")
                .to_string(),
            entry.arn.clone(),
        ));
    }
    names.sort();
    let expected_by_name: Vec<String> = names.into_iter().map(|(_, arn)| arn).collect();
    assert_eq!(
        by_name, expected_by_name,
        "case SortBy COMMON_NAME ASCENDING: expected the stored domain-name order, got {by_name:?}"
    );

    // The default order is the model's: CREATED_AT, ascending — the order the
    // fixtures were created in.
    let defaulted = result_arns("no SortBy", &search_results(&router, &json!({})).await);
    let created_order: Vec<String> = stored.iter().map(|entry| entry.arn.clone()).collect();
    assert_eq!(
        defaulted, created_order,
        "case no SortBy: the model's default is CREATED_AT ascending, got {defaulted:?}"
    );
}

/// RULE: a request the service cannot satisfy as written is refused with a 400
/// `ValidationException` — a timestamp range that bounds nothing or can contain no
/// instant, a `NextToken` this service did not issue, and every value outside the
/// model's own constraints.
#[tokio::test]
async fn an_unsatisfiable_request_is_refused() {
    let router = router();
    // One certificate, so a refusal cannot pass merely because the store is empty.
    request_certificate(
        &router,
        &json!({ "DomainName": "refusals.example.com", "ValidationMethod": "DNS" }),
    )
    .await;

    let now = epoch_now();
    // Each row: what it is, and the request body that must be refused.
    let refused: Vec<(&str, Value)> = vec![
        (
            "NotAfter with neither bound",
            json!({ "FilterStatement": { "Filter": { "X509AttributeFilter": { "NotAfter": {} } } } }),
        ),
        (
            "NotBefore with neither bound",
            json!({ "FilterStatement": { "Filter": { "X509AttributeFilter": { "NotBefore": {} } } } }),
        ),
        (
            "NotAfter starting after it ends",
            json!({ "FilterStatement": { "Filter": { "X509AttributeFilter": { "NotAfter": { "Start": now + DAY, "End": now } } } } }),
        ),
        (
            "NotBefore starting after it ends",
            json!({ "FilterStatement": { "Filter": { "X509AttributeFilter": { "NotBefore": { "Start": now + DAY, "End": now } } } } }),
        ),
        (
            "an unsatisfiable range nested inside And",
            json!({ "FilterStatement": { "And": [
                { "Filter": { "AcmCertificateMetadataFilter": { "Status": "ISSUED" } } },
                { "Filter": { "X509AttributeFilter": { "NotAfter": {} } } },
            ] } }),
        ),
        (
            "an unsatisfiable range nested inside Or",
            json!({ "FilterStatement": { "Or": [
                { "Filter": { "X509AttributeFilter": { "NotBefore": { "Start": now + DAY, "End": now } } } },
            ] } }),
        ),
        (
            "an unsatisfiable range nested inside Not",
            json!({ "FilterStatement": { "Not": { "Filter": { "X509AttributeFilter": { "NotAfter": {} } } } } }),
        ),
        (
            "a NextToken that is not hex",
            json!({ "NextToken": "not-a-token" }),
        ),
        (
            "a NextToken that decodes to nothing this service issued",
            json!({ "NextToken": hex::encode("arn:aws:acm:us-west-2:000000000000:certificate/abc") }),
        ),
        (
            "a NextToken naming something that is not a certificate ARN",
            json!({ "NextToken": hex::encode("\u{1f}arn:aws:acm:us-west-2:000000000000:other/abc") }),
        ),
        // The model's own constraints, refused by the boundary with the same status
        // and code: `SearchMaxResults` is `range 1..500`, and a filter value outside
        // its enum is no value at all.
        ("MaxResults below the modelled range", json!({ "MaxResults": 0 })),
        ("MaxResults above the modelled range", json!({ "MaxResults": 501 })),
        (
            "a Status outside the modelled enum",
            json!({ "FilterStatement": { "Filter": { "AcmCertificateMetadataFilter": { "Status": "NOT_A_STATUS" } } } }),
        ),
        (
            "a RenewalStatus outside the modelled enum",
            json!({ "FilterStatement": { "Filter": { "AcmCertificateMetadataFilter": { "RenewalStatus": "RENEWED" } } } }),
        ),
        (
            "a Type outside the modelled enum",
            json!({ "FilterStatement": { "Filter": { "AcmCertificateMetadataFilter": { "Type": "AMAZON" } } } }),
        ),
        (
            "an ExportOption outside the modelled enum",
            json!({ "FilterStatement": { "Filter": { "AcmCertificateMetadataFilter": { "ExportOption": "MAYBE" } } } }),
        ),
        (
            "a ManagedBy outside the modelled enum",
            json!({ "FilterStatement": { "Filter": { "AcmCertificateMetadataFilter": { "ManagedBy": "CLOUDWATCH" } } } }),
        ),
        (
            "a ValidationMethod outside the modelled enum",
            json!({ "FilterStatement": { "Filter": { "AcmCertificateMetadataFilter": { "ValidationMethod": "CARRIER_PIGEON" } } } }),
        ),
        (
            "a CertificateKeyPairOrigin outside the modelled enum",
            json!({ "FilterStatement": { "Filter": { "AcmCertificateMetadataFilter": { "CertificateKeyPairOrigin": "CUSTOMER" } } } }),
        ),
        (
            "a KeyAlgorithm outside the modelled enum",
            json!({ "FilterStatement": { "Filter": { "X509AttributeFilter": { "KeyAlgorithm": "RSA_512" } } } }),
        ),
        (
            "a KeyUsage outside the modelled enum",
            json!({ "FilterStatement": { "Filter": { "X509AttributeFilter": { "KeyUsage": "SIGNING" } } } }),
        ),
        (
            "an ExtendedKeyUsage outside the modelled enum",
            json!({ "FilterStatement": { "Filter": { "X509AttributeFilter": { "ExtendedKeyUsage": "TLS" } } } }),
        ),
        (
            "a CommonName ComparisonOperator outside the modelled enum",
            json!({ "FilterStatement": { "Filter": { "X509AttributeFilter": { "Subject": { "CommonName": { "Value": "alpha.example.com", "ComparisonOperator": "STARTS_WITH" } } } } } }),
        ),
        (
            "a DnsName ComparisonOperator outside the modelled enum",
            json!({ "FilterStatement": { "Filter": { "X509AttributeFilter": { "SubjectAlternativeName": { "DnsName": { "Value": "alpha.example.com", "ComparisonOperator": "MATCHES" } } } } } }),
        ),
        (
            "a value outside its enum nested inside And",
            json!({ "FilterStatement": { "And": [
                { "Filter": { "AcmCertificateMetadataFilter": { "Status": "ISSUED" } } },
                { "Filter": { "X509AttributeFilter": { "KeyUsage": "SIGNING" } } },
            ] } }),
        ),
        (
            "a SortBy outside the modelled enum",
            json!({ "SortBy": "NOT_A_FIELD" }),
        ),
    ];

    for (case, body) in refused {
        let (status, response) = call(&router, "SearchCertificates", &body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case}: expected a ValidationException, got {response}"
        );
    }

    // The bounds the rule accepts: either end open, and a one-instant range.
    let accepted: Vec<(&str, Value)> = vec![
        (
            "NotAfter with an open end",
            json!({ "FilterStatement": { "Filter": { "X509AttributeFilter": { "NotAfter": { "Start": now } } } } }),
        ),
        (
            "NotAfter with an open start",
            json!({ "FilterStatement": { "Filter": { "X509AttributeFilter": { "NotAfter": { "End": now } } } } }),
        ),
        (
            "NotBefore over a single instant, both bounds being inclusive",
            json!({ "FilterStatement": { "Filter": { "X509AttributeFilter": { "NotBefore": { "Start": now, "End": now } } } } }),
        ),
        ("MaxResults at the modelled minimum", json!({ "MaxResults": 1 })),
        ("MaxResults at the modelled maximum", json!({ "MaxResults": 500 })),
    ];
    for (case, body) in accepted {
        let (status, response) = call(&router, "SearchCertificates", &body).await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} {response}"
        );
    }
}

/// RULE: a refusal says what was wrong with the request — the member the offending
/// value sat on and the requirement it failed — so the caller can fix the request from
/// the message alone, and it states the member in the WIRE spelling of its path while
/// withholding the text submitted there (see `crate::validation_message_rendering`).
#[tokio::test]
async fn a_refusal_names_the_member_and_its_requirement() {
    let router = router();
    let now = epoch_now();

    // Each row: the request, every fragment its message must carry, and every
    // caller-submitted string the message must NOT carry.
    let cases: Vec<(&str, Value, Vec<&str>, Vec<&str>)> = vec![
        (
            "a Status outside its enum, refused by this handler",
            json!({ "FilterStatement": { "Filter": { "AcmCertificateMetadataFilter": { "Status": "NOT_A_STATUS" } } } }),
            vec![
                "filterStatement.filter.acmCertificateMetadataFilter.status",
                "PENDING_VALIDATION",
                // The position alone: ACM names what failed, not what was sent.
                "Value of the input at",
            ],
            vec!["NOT_A_STATUS"],
        ),
        (
            "a ComparisonOperator outside its enum, refused by the boundary validator",
            json!({ "FilterStatement": { "Filter": { "X509AttributeFilter": { "Subject": { "CommonName": { "Value": "alpha.example.com", "ComparisonOperator": "STARTS_WITH" } } } } } }),
            vec![
                "filterStatement.filter.x509AttributeFilter.subject.commonName.comparisonOperator",
                "CONTAINS, EQUALS",
                "Value of the input at",
            ],
            vec!["STARTS_WITH"],
        ),
        (
            "a range bounding nothing",
            json!({ "FilterStatement": { "Filter": { "X509AttributeFilter": { "NotAfter": {} } } } }),
            vec!["filterStatement.filter.x509AttributeFilter.notAfter", "Start"],
            vec![],
        ),
        (
            "a range that can contain no instant",
            json!({ "FilterStatement": { "Filter": { "X509AttributeFilter": { "NotBefore": { "Start": now + DAY, "End": now } } } } }),
            vec![
                "filterStatement.filter.x509AttributeFilter.notBefore",
                "must not be after End",
            ],
            vec![],
        ),
        (
            "a NextToken this service did not issue",
            json!({ "NextToken": "not-a-token" }),
            vec!["NextToken"],
            vec![],
        ),
    ];

    for (case, body, fragments, withheld) in cases {
        let (status, response) = call(&router, "SearchCertificates", &body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {response}"
        );
        let message = error_message(&response);
        for fragment in fragments {
            assert!(
                message.contains(fragment),
                "case {case}: expected the message to name {fragment:?}, got {message:?}"
            );
        }
        for value in withheld {
            assert!(
                !message.contains(value),
                "case {case}: the submitted value {value:?} must not be echoed, got {message:?}"
            );
        }
    }
}

/// A filter statement `levels` statement nodes deep: `levels - 1` `Not` operators
/// around one leaf filter.
fn nested_statement(levels: usize) -> Value {
    let mut statement = json!({ "Filter": { "AcmCertificateMetadataFilter": { "Status": "ISSUED" } } });
    for _ in 1..levels {
        statement = json!({ "Not": statement });
    }
    statement
}

/// RULE: the service, not the request, decides how deep and how wide a filter
/// statement it will walk — a statement inside the bounds is answered, one past them
/// is refused with the modelled 400 `ValidationException`.
#[tokio::test]
async fn an_over_nested_statement_is_refused() {
    let (router, engine) = router_with_engine();
    store_fixtures(&router).await;
    make_validation_due_now(&engine);

    // The depth bound the service enforces
    // (`search_certificates::MAX_STATEMENT_DEPTH`), tested at the bound, inside it and
    // one past it. An odd number of `Not`s inverts the leaf, so each accepted row is
    // checked only for being answered at all.
    let accepted: &[usize] = &[1, 2, 19, 20];
    for levels in accepted {
        let body = json!({ "FilterStatement": nested_statement(*levels) });
        let (status, response) = call(&router, "SearchCertificates", &body).await;
        assert_eq!(
            status, 200,
            "case {levels} levels: a statement inside the depth bound must be answered, got {status} {response}"
        );
    }

    // Past the bound: refused with the same status and code, and with the one sentence
    // the service states for that bound — by the READER, which recurses on the native
    // stack as it builds the statement and so enforces the bound before the statement
    // becomes a value at all, however much deeper the request nests.
    let refused: &[usize] = &[21, 50, 120, 500];
    for levels in refused {
        let body = json!({ "FilterStatement": nested_statement(*levels) });
        let (status, response) = call(&router, "SearchCertificates", &body).await;
        assert_eq!(
            status, 400,
            "case {levels} levels: a statement past the depth bound must be refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {levels} levels: expected a ValidationException, got {response}"
        );
        assert!(
            error_message(&response).contains("nest at most"),
            "case {levels} levels: the message must state the bound, got {response}"
        );
    }
}

/// RULE: the revocation a certificate's record carries is what a search reports and
/// filters on — the `REVOKED` status, the `RevokedAt` instant beside it, and nothing
/// for a certificate that was never revoked.
#[tokio::test]
async fn a_revoked_certificate_is_reported_and_filtered_as_revoked() {
    let (router, engine) = router_with_engine();
    // Two issued, exportable certificates; only the first is revoked, so the other is
    // the control the filter must leave out.
    let mut arns = Vec::new();
    for domain in ["revoked.example.com", "live.example.com"] {
        let arn = request_certificate(
            &router,
            &json!({
                "DomainName": domain,
                "CertificateAuthorityArn": PRIVATE_CA_ARN,
                "ValidationMethod": "DNS",
                "Options": { "Export": "ENABLED" },
            }),
        )
        .await;
        arns.push(arn);
    }
    make_validation_due_now(&engine);
    // The one fact `RevokeCertificate` requires of a certificate, which
    // `ExportCertificate` sets in production (see `mark_exported`).
    mark_exported(&engine, &arns[0]);

    let (status, response) = call(
        &router,
        "RevokeCertificate",
        &json!({ "CertificateArn": arns[0], "RevocationReason": "KEY_COMPROMISE" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "RevokeCertificate must succeed, got {status} {response}"
    );

    // The status filter follows the record: the revoked certificate and nothing else.
    let revoked = matching_arns(
        &router,
        "Status REVOKED",
        &json!({ "Filter": { "AcmCertificateMetadataFilter": { "Status": "REVOKED" } } }),
    )
    .await;
    assert_eq!(
        revoked,
        vec![arns[0].clone()],
        "case Status REVOKED: expected the revoked certificate alone, got {revoked:?}"
    );
    let issued = matching_arns(
        &router,
        "Status ISSUED",
        &json!({ "Filter": { "AcmCertificateMetadataFilter": { "Status": "ISSUED" } } }),
    )
    .await;
    assert_eq!(
        issued,
        vec![arns[1].clone()],
        "case Status ISSUED: a revoked certificate is no longer issued, got {issued:?}"
    );

    // And the reported revocation instant is the stored one `DescribeCertificate`
    // reports, while the certificate that was never revoked carries none.
    for (case, arn, revoked) in [
        ("the revoked certificate", arns[0].clone(), true),
        ("the certificate left alone", arns[1].clone(), false),
    ] {
        let results = search_results(
            &router,
            &json!({ "FilterStatement": { "Filter": { "CertificateArn": arn } } }),
        )
        .await;
        assert_eq!(
            results.len(),
            1,
            "case {case}: expected exactly the named certificate, got {results:?}"
        );
        let metadata = object(
            case,
            &results[0]["CertificateMetadata"]["AcmCertificateMetadata"],
        );
        let detail = describe(&router, &arn).await;
        assert_eq!(
            metadata["RevokedAt"], detail["RevokedAt"],
            "case {case}: RevokedAt must agree with DescribeCertificate, got {metadata} vs {detail}"
        );
        assert_eq!(
            metadata["RevokedAt"].is_null(),
            !revoked,
            "case {case}: RevokedAt exists only beside a recorded revocation, got {metadata}"
        );
    }
}

/// RULE: a `NextToken` resumes the page order it was cut in. A token replayed against
/// a different `SortBy` or `SortOrder` would be compared against values rendered from
/// another field, so it is refused rather than answered with a page that silently
/// skips or repeats certificates.
#[tokio::test]
async fn a_next_token_only_resumes_the_order_it_was_cut_in() {
    let (router, engine) = router_with_engine();
    let stored = store_fixtures(&router).await;
    make_validation_due_now(&engine);

    // One result per page, so every order issues a token.
    let first = json!({ "SortBy": "COMMON_NAME", "SortOrder": "ASCENDING", "MaxResults": 1 });
    let (status, response) = call(&router, "SearchCertificates", &first).await;
    assert_eq!(
        status, 200,
        "the first page must succeed, got {status} {response}"
    );
    let token = response["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a truncated page must carry a NextToken, got {response}"))
        .to_string();

    // Resumed in the order it was cut in: the next certificate in that order.
    let mut resumed = first.clone();
    resumed["NextToken"] = json!(token);
    let (status, response) = call(&router, "SearchCertificates", &resumed).await;
    assert_eq!(
        status, 200,
        "case same order: the token must resume, got {status} {response}"
    );
    assert_eq!(
        response["Results"].as_array().map(Vec::len),
        Some(1),
        "case same order: expected the next single result, got {response}"
    );

    // Each row changes the order the token is presented against.
    let mismatched: Vec<(&str, Value)> = vec![
        (
            "another SortOrder",
            json!({ "SortBy": "COMMON_NAME", "SortOrder": "DESCENDING", "MaxResults": 1, "NextToken": token.clone() }),
        ),
        (
            "another SortBy",
            json!({ "SortBy": "CERTIFICATE_ARN", "SortOrder": "ASCENDING", "MaxResults": 1, "NextToken": token.clone() }),
        ),
        (
            "the defaulted order",
            json!({ "MaxResults": 1, "NextToken": token.clone() }),
        ),
    ];
    for (case, body) in mismatched {
        let (status, response) = call(&router, "SearchCertificates", &body).await;
        assert_eq!(
            status, 400,
            "case {case}: a token from another order must be refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case}: expected a ValidationException, got {response}"
        );
    }

    // And the whole order still pages to every certificate, one result at a time.
    let paged = result_arns(
        "COMMON_NAME ascending, one at a time",
        &search_results(&router, &first).await,
    );
    let mut seen = paged.clone();
    seen.sort();
    assert_eq!(
        seen,
        expected(
            &stored,
            &stored.iter().map(|entry| entry.label).collect::<Vec<_>>()
        ),
        "paging one result at a time must report every certificate exactly once, got {paged:?}"
    );
}
