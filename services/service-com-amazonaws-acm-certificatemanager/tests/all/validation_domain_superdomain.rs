// SPDX-License-Identifier: CC0-1.0

//! `RequestCertificate`'s `DomainValidationOptions` validation, over the wire
//! (`POST /`, `X-Amz-Target: CertificateManager.<Op>`, awsJson1_1).
//!
//! RULE: a `DomainValidationOption` entry is accepted only when its
//! `ValidationDomain` is the entry's own `DomainName` or a superdomain of it — equal
//! to it, or a DOT-BOUNDARY suffix of it with its wildcard label stripped — compared
//! case-insensitively as DNS names are; anything else is
//! `InvalidDomainValidationOptionsException` (HTTP 400) and leaves no certificate
//! behind. The rule is
//! `smithy_query(query="doc", shape="com.amazonaws.acm#DomainValidationOption",
//! member="ValidationDomain")`: "This must be the same as the `DomainName` value or a
//! superdomain of the `DomainName` value."
//!
//! The table spans the class the rule generalises over: label depth, wildcard and
//! plain names, casing, a suffix that is not on a label boundary, a sibling domain, a
//! subdomain (the wrong direction), an unrelated domain, and the public suffix above
//! the registrable name. Each accepted row is read back through
//! `DescribeCertificate`, and each refused row through `ListCertificates`, so nothing
//! is asserted about a response that the store does not back.

use serde_json::{json, Value};

use crate::certificate_lifecycle::{
    call, describe, error_code, hold_pending_validations, list_all_arns, router_with_engine,
};

/// One row: what it exercises, the certificate's `DomainName`, the entry's
/// `DomainName`, the entry's `ValidationDomain`, and whether ACM accepts it.
type SuperdomainCase<'a> = (&'a str, &'a str, &'a str, &'a str, bool);

/// The rows. `accepted` is the rule's verdict, not a recording: equal names and
/// dot-boundary suffixes are superdomains, everything else is not.
const CASES: &[SuperdomainCase<'_>] = &[
    // The same name as the domain: "the same as the DomainName value".
    (
        "identical name",
        "testing.example.com",
        "testing.example.com",
        "testing.example.com",
        true,
    ),
    // One label up, the example in the member's own doc.
    (
        "one label up",
        "testing.example.com",
        "testing.example.com",
        "example.com",
        true,
    ),
    // Two labels up, from a deeper name.
    (
        "two labels up",
        "site.subdomain.example.com",
        "site.subdomain.example.com",
        "example.com",
        true,
    ),
    (
        "one label up from a deep name",
        "site.subdomain.example.com",
        "site.subdomain.example.com",
        "subdomain.example.com",
        true,
    ),
    // DNS names are case-insensitive, so casing never decides the verdict.
    (
        "superdomain in another case",
        "testing.example.com",
        "TESTING.Example.COM",
        "ExAmPlE.CoM",
        true,
    ),
    // A wildcard certificate is validated through the domain it covers.
    (
        "wildcard name, its own base",
        "*.example.com",
        "*.example.com",
        "example.com",
        true,
    ),
    (
        "wildcard name, repeated verbatim",
        "*.example.com",
        "*.example.com",
        "*.example.com",
        true,
    ),
    // A string suffix that does not start at a label boundary is not a superdomain.
    (
        "suffix off the label boundary",
        "testing.example.com",
        "testing.example.com",
        "ample.com",
        false,
    ),
    // A sibling shares a parent but is not above the domain.
    (
        "sibling domain",
        "testing.example.com",
        "testing.example.com",
        "other.example.com",
        false,
    ),
    // A SUBdomain is below the domain, the wrong direction.
    (
        "subdomain of the domain",
        "example.com",
        "example.com",
        "deep.example.com",
        false,
    ),
    (
        "subdomain of a deep domain",
        "subdomain.example.com",
        "subdomain.example.com",
        "site.subdomain.example.com",
        false,
    ),
    // The recorded shape of the defect: an unrelated registrable domain.
    (
        "unrelated domain",
        "probe.example.com",
        "probe.example.com",
        "totally-unrelated.org",
        false,
    ),
    // The public suffix above the registrable name shares no label with it beyond
    // the TLD, so it is not the domain's superdomain either.
    (
        "another TLD's parent",
        "testing.example.com",
        "testing.example.com",
        "example.org",
        false,
    ),
    // A wildcard validation domain names no mail suffix: only the identical-name
    // case above makes one acceptable.
    (
        "wildcard validation domain of a plain name",
        "testing.example.com",
        "testing.example.com",
        "*.example.com",
        false,
    ),
];

/// RULE: `ValidationDomain` must be the entry's `DomainName` or a superdomain of it;
/// anything else is `InvalidDomainValidationOptionsException` (400) and no
/// certificate is created.
#[tokio::test]
async fn validation_domain_must_be_the_domain_or_a_superdomain() {
    for (case, domain_name, option_domain, validation_domain, accepted) in CASES {
        // A router per row, so the store a refusal must leave empty is this row's own.
        let (router, engine) = router_with_engine();
        let body = json!({
            "DomainName": domain_name,
            "ValidationMethod": "EMAIL",
            "DomainValidationOptions": [
                { "DomainName": option_domain, "ValidationDomain": validation_domain }
            ]
        });
        let (status, response) = call(&router, "RequestCertificate", &body).await;

        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: ValidationDomain {validation_domain} is a superdomain of \
                 {option_domain}, so the request must succeed, got {status} {response}"
            );
            let arn = response["CertificateArn"].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: RequestCertificate returned no CertificateArn: {response}")
            });
            // Read the stored record back: the accepted validation domain is the
            // suffix ACM mails, so it must be what DescribeCertificate reports —
            // without a wildcard label, which no mail suffix carries.
            let expected_suffix = validation_domain
                .strip_prefix("*.")
                .unwrap_or(validation_domain);
            // The row is about the record the request created, so the transitions the
            // clock owes it — including the initial validation mail — are held out.
            hold_pending_validations(&engine);
            let detail = describe(&router, arn).await;
            let validation = &detail["DomainValidationOptions"][0];
            assert!(
                validation["ValidationDomain"]
                    .as_str()
                    .is_some_and(|stored| stored.eq_ignore_ascii_case(expected_suffix)),
                "case {case:?}: expected the stored ValidationDomain to be \
                 {expected_suffix}, got {validation}"
            );
            // The mailbox list of an email-validated name holds the messages ACM has
            // SENT. The initial dispatch is still held out, so it is the empty list
            // rather than a set of addresses no message went to.
            assert_eq!(
                validation["ValidationEmails"],
                json!([]),
                "case {case:?}: no validation email has been sent yet, got {validation}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: ValidationDomain {validation_domain} is not a superdomain of \
                 {option_domain}, so the request must be refused, got {status} {response}"
            );
            assert_eq!(
                error_code(&response),
                "InvalidDomainValidationOptionsException",
                "case {case:?}: expected InvalidDomainValidationOptionsException, got {response}"
            );
            assert!(
                response["message"]
                    .as_str()
                    .is_some_and(|message| message.contains(validation_domain)),
                "case {case:?}: the refusal must name the rejected validation domain \
                 {validation_domain}, got {response}"
            );
            // A refused request leaks no certificate into the store.
            let stored = list_all_arns(&router, &json!({})).await;
            assert!(
                stored.is_empty(),
                "case {case:?}: a refused request must create no certificate, found {stored:?}"
            );
        }
    }
}

/// RULE: the rule holds per entry and over the whole list — one bad entry refuses the
/// request however many good ones accompany it, and an entry must name a domain the
/// certificate covers (`DomainName` or one of its `SubjectAlternativeNames`).
#[tokio::test]
async fn every_entry_in_the_list_is_checked() {
    // (what it exercises, the request, the expected error code or None for success)
    let cases: &[(&str, Value, Option<&str>)] = &[
        (
            "one entry per covered name, each with its own superdomain",
            json!({
                "DomainName": "a.example.com",
                "SubjectAlternativeNames": ["b.sub.example.com", "*.wild.example.com"],
                "ValidationMethod": "EMAIL",
                "DomainValidationOptions": [
                    { "DomainName": "a.example.com", "ValidationDomain": "example.com" },
                    { "DomainName": "b.sub.example.com", "ValidationDomain": "sub.example.com" },
                    { "DomainName": "*.wild.example.com", "ValidationDomain": "wild.example.com" }
                ]
            }),
            None,
        ),
        (
            "the last entry of three is not a superdomain",
            json!({
                "DomainName": "a.example.com",
                "SubjectAlternativeNames": ["b.sub.example.com", "*.wild.example.com"],
                "ValidationMethod": "EMAIL",
                "DomainValidationOptions": [
                    { "DomainName": "a.example.com", "ValidationDomain": "example.com" },
                    { "DomainName": "b.sub.example.com", "ValidationDomain": "sub.example.com" },
                    { "DomainName": "*.wild.example.com", "ValidationDomain": "elsewhere.test" }
                ]
            }),
            Some("InvalidDomainValidationOptionsException"),
        ),
        (
            "the first entry of three is not a superdomain",
            json!({
                "DomainName": "a.example.com",
                "SubjectAlternativeNames": ["b.sub.example.com"],
                "ValidationMethod": "EMAIL",
                "DomainValidationOptions": [
                    { "DomainName": "a.example.com", "ValidationDomain": "example.net" },
                    { "DomainName": "b.sub.example.com", "ValidationDomain": "sub.example.com" }
                ]
            }),
            Some("InvalidDomainValidationOptionsException"),
        ),
        (
            "an entry naming a domain the certificate does not cover",
            json!({
                "DomainName": "a.example.com",
                "ValidationMethod": "EMAIL",
                "DomainValidationOptions": [
                    { "DomainName": "z.example.com", "ValidationDomain": "example.com" }
                ]
            }),
            Some("InvalidDomainValidationOptionsException"),
        ),
        (
            "no DomainValidationOptions at all: nothing to check",
            json!({ "DomainName": "a.example.com", "ValidationMethod": "EMAIL" }),
            None,
        ),
        (
            // The rule is about the structure's values, so it holds whichever
            // validation method the request asked for.
            "a bad entry under DNS validation",
            json!({
                "DomainName": "a.example.com",
                "ValidationMethod": "DNS",
                "DomainValidationOptions": [
                    { "DomainName": "a.example.com", "ValidationDomain": "unrelated.test" }
                ]
            }),
            Some("InvalidDomainValidationOptionsException"),
        ),
    ];

    for (case, body, expected_error) in cases {
        let (router, _engine) = router_with_engine();
        let (status, response) = call(&router, "RequestCertificate", body).await;
        match expected_error {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: the request must succeed, got {status} {response}"
                );
                let arn = response["CertificateArn"].as_str().unwrap_or_else(|| {
                    panic!("case {case:?}: no CertificateArn in {response}")
                });
                let detail = describe(&router, arn).await;
                assert_eq!(
                    detail["CertificateArn"].as_str(),
                    Some(arn),
                    "case {case:?}: DescribeCertificate must report the stored certificate, \
                     got {detail}"
                );
            }
            Some(code) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: the request must be refused, got {status} {response}"
                );
                assert_eq!(
                    &error_code(&response),
                    code,
                    "case {case:?}: expected {code}, got {response}"
                );
                let stored = list_all_arns(&router, &json!({})).await;
                assert!(
                    stored.is_empty(),
                    "case {case:?}: a refused request must create no certificate, \
                     found {stored:?}"
                );
            }
        }
    }
}
