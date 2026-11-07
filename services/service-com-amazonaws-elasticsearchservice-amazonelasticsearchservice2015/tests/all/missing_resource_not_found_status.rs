// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for ONE rule of this API's error envelope: a refusal that the
//! resource a request names does not exist is answered with **HTTP 404** carrying the
//! `ResourceNotFoundException` error class.
//!
//! The model annotates that shape with `@httpError(409)`
//! (`smithy_query(query="shape",shape="ResourceNotFoundException")`, whose own doc
//! sentence says 400 instead), so the annotation is not what a client sees on any
//! reading of the model: the status the service answers with is the requirement, and it
//! is the same status for every operation of this API that can refuse a request for
//! this reason.
//!
//! The rule is asserted over a TABLE, not over one call: every operation of this crate
//! that resolves a named resource, both resource kinds it addresses (a domain and a
//! package), each HTTP method those routes use (GET, POST, DELETE), each place the
//! identifier is bound (a URI label, a query parameter, a body member), and identifier
//! forms at the published bounds of `DomainName` (length 3..28,
//! `^[a-z][a-z0-9\-]+$`) — including a name that exists only in ANOTHER Region, which
//! is absent here because a domain is held per Region.
//!
//! A second test is the control: the same routes answer 200 for a resource that IS
//! there, so the 404 is about the resource's absence and not about the route.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call_in, router, Answer};

/// The Region every call below is signed for unless the case says otherwise.
const REGION: &str = "us-east-1";

/// A second Region, used to create a resource that is therefore absent in [`REGION`].
const OTHER_REGION: &str = "eu-west-1";

/// The status this API answers "the resource you named does not exist" with, as the
/// recorded wire answer of such a refusal carries it.
const NOT_FOUND: u16 = 404;

/// The error class that status carries.
const NOT_FOUND_ERROR: &str = "ResourceNotFoundException";

/// One operation of the table: how it is called, in the order the route names things.
struct Call {
    operation: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
}

/// Create a domain in `region`, returning its name.
async fn create_domain(router: &MultiServiceRouter, region: &str, name: &str) -> String {
    let created = call_in(
        router,
        region,
        "POST",
        "/2015-01-01/es/domain",
        Some(&json!({"DomainName": name})),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {name:?} in {region:?}: the create must answer 200, got {} {}",
        created.status, created.body
    );
    name.to_string()
}

/// Create a package in `region`, returning the id the service minted for it.
async fn create_package(router: &MultiServiceRouter, region: &str, name: &str) -> String {
    let created = call_in(
        router,
        region,
        "POST",
        "/2015-01-01/packages",
        Some(&json!({
            "PackageName": name,
            "PackageType": "TXT-DICTIONARY",
            "PackageSource": {"S3BucketName": "pkg-bucket", "S3Key": "v1.txt"}
        })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {name:?} in {region:?}: the create must answer 200, got {} {}",
        created.status, created.body
    );
    created.text(name, "/PackageDetails/PackageID")
}

/// Every operation that resolves a DOMAIN by name, called for `domain`.
///
/// `package` is a package that exists, so the association row's refusal is about the
/// domain rather than about the package.
fn domain_calls(domain: &str, package: &str) -> Vec<Call> {
    vec![
        Call {
            operation: "DescribeElasticsearchDomain",
            method: "GET",
            path: format!("/2015-01-01/es/domain/{domain}"),
            body: None,
        },
        Call {
            operation: "DescribeElasticsearchDomainConfig",
            method: "GET",
            path: format!("/2015-01-01/es/domain/{domain}/config"),
            body: None,
        },
        Call {
            operation: "UpdateElasticsearchDomainConfig",
            method: "POST",
            path: format!("/2015-01-01/es/domain/{domain}/config"),
            body: Some(json!({"SnapshotOptions": {"AutomatedSnapshotStartHour": 4}})),
        },
        Call {
            operation: "DeleteElasticsearchDomain",
            method: "DELETE",
            path: format!("/2015-01-01/es/domain/{domain}"),
            body: None,
        },
        Call {
            operation: "ListPackagesForDomain",
            method: "GET",
            path: format!("/2015-01-01/domain/{domain}/packages"),
            body: None,
        },
        Call {
            // The one route that binds the name to a QUERY parameter rather than a
            // URI label.
            operation: "GetCompatibleElasticsearchVersions",
            method: "GET",
            path: format!("/2015-01-01/es/compatibleVersions?domainName={domain}"),
            body: None,
        },
        Call {
            operation: "AssociatePackage",
            method: "POST",
            path: format!("/2015-01-01/packages/associate/{package}/{domain}"),
            body: None,
        },
    ]
}

/// Every operation that resolves a PACKAGE by id, called for `package`.
///
/// `domain` is a domain that exists, so the association row's refusal is about the
/// package rather than about the domain.
fn package_calls(package: &str, domain: &str) -> Vec<Call> {
    vec![
        Call {
            operation: "GetPackageVersionHistory",
            method: "GET",
            path: format!("/2015-01-01/packages/{package}/history"),
            body: None,
        },
        Call {
            operation: "ListDomainsForPackage",
            method: "GET",
            path: format!("/2015-01-01/packages/{package}/domains"),
            body: None,
        },
        Call {
            // The one route that binds the id in the BODY rather than in the URI.
            operation: "UpdatePackage",
            method: "POST",
            path: "/2015-01-01/packages/update".to_string(),
            body: Some(json!({
                "PackageID": package,
                "PackageSource": {"S3BucketName": "pkg-bucket", "S3Key": "v2.txt"}
            })),
        },
        Call {
            operation: "AssociatePackage",
            method: "POST",
            path: format!("/2015-01-01/packages/associate/{package}/{domain}"),
            body: None,
        },
    ]
}

/// Assert one row of the table: HTTP 404, the `ResourceNotFoundException` class, and a
/// message that names the identifier that resolved to nothing.
fn assert_not_found(label: &str, identifier: &str, answer: &Answer) {
    assert_eq!(
        answer.status, NOT_FOUND,
        "case {label:?}: an absent resource must answer HTTP {NOT_FOUND}, got {} {}",
        answer.status, answer.body
    );
    assert!(
        answer.error_type.contains(NOT_FOUND_ERROR),
        "case {label:?}: the error must be {NOT_FOUND_ERROR}, got {:?} / {}",
        answer.error_type,
        answer.body
    );
    let message = answer.text(label, "/message");
    assert!(
        message.contains(identifier),
        "case {label:?}: the refusal must name {identifier:?}, got {message:?}"
    );
}

/// RULE: on every operation of this API, a resource the Region does not hold is
/// refused with HTTP 404 `ResourceNotFoundException` — not with the `@httpError(409)`
/// the model annotates on the shape.
#[tokio::test]
async fn an_absent_resource_is_refused_with_http_404_on_every_operation() {
    let router = router();

    // Resources that DO exist, so each refusal below is about the identifier the row
    // varies and not about an empty store.
    let present_domain = create_domain(&router, REGION, "present-domain").await;
    let present_package = create_package(&router, REGION, "present-package").await;
    // Held by another Region's control plane, so absent in `REGION`.
    let elsewhere_domain = create_domain(&router, OTHER_REGION, "elsewhere-domain").await;
    let elsewhere_package = create_package(&router, OTHER_REGION, "elsewhere-package").await;

    // Domain names spanning what the rule generalises over: the shortest and the
    // longest `DomainName` the model allows (length 3..28), a hyphenated middle case,
    // one carrying digits, and a name that exists only in another Region.
    let absent_domains = [
        ("shortest allowed name", "abc".to_string()),
        ("a hyphenated name", "never-created-domain".to_string()),
        ("a name carrying digits", "domain-404-probe-7".to_string()),
        (
            "longest allowed name",
            "a-very-long-absent-domain-nm".to_string(),
        ),
        ("a name held by another Region", elsewhere_domain.clone()),
    ];
    for (form, domain) in &absent_domains {
        assert!(
            domain.len() >= 3 && domain.len() <= 28,
            "case {form:?}: {domain:?} is outside the modelled DomainName length 3..28"
        );
        for call in domain_calls(domain, &present_package) {
            let label = format!("{} on an absent domain ({form})", call.operation);
            let answer = call_in(
                &router,
                REGION,
                call.method,
                &call.path,
                call.body.as_ref(),
            )
            .await;
            assert_not_found(&label, domain, &answer);
        }
    }

    // Package ids spanning the same dimension: an id of the minted shape that was
    // never minted, one of an unrelated shape, and one minted in another Region.
    let absent_packages = [
        ("an id nothing minted", "pkg-0000000000000000".to_string()),
        ("an id of another shape", "F12345678".to_string()),
        ("an id minted in another Region", elsewhere_package.clone()),
    ];
    for (form, package) in &absent_packages {
        for call in package_calls(package, &present_domain) {
            let label = format!("{} on an absent package ({form})", call.operation);
            let answer = call_in(
                &router,
                REGION,
                call.method,
                &call.path,
                call.body.as_ref(),
            )
            .await;
            assert_not_found(&label, package, &answer);
        }
    }

    // None of those refusals changed what a later caller sees: the resources that were
    // there are still there, and nothing the table named was created.
    let listed = call_in(&router, REGION, "GET", "/2015-01-01/domain", None).await;
    assert_eq!(
        listed.at("after the table", "/DomainNames"),
        &json!([{"DomainName": present_domain, "EngineType": "Elasticsearch"}]),
        "after the table: the Region must still hold exactly the domain it held, got {}",
        listed.body
    );
}

/// The control: the same routes answer 200 for a resource that IS there, so the 404
/// above is the resource's absence and not the route.
#[tokio::test]
async fn the_same_routes_answer_200_for_a_resource_that_exists() {
    let router = router();
    let domain = create_domain(&router, REGION, "control-domain").await;
    let package = create_package(&router, REGION, "control-package").await;

    for (operation, path) in [
        (
            "DescribeElasticsearchDomain",
            format!("/2015-01-01/es/domain/{domain}"),
        ),
        (
            "DescribeElasticsearchDomainConfig",
            format!("/2015-01-01/es/domain/{domain}/config"),
        ),
        (
            "ListPackagesForDomain",
            format!("/2015-01-01/domain/{domain}/packages"),
        ),
        (
            "GetCompatibleElasticsearchVersions",
            format!("/2015-01-01/es/compatibleVersions?domainName={domain}"),
        ),
        (
            "GetPackageVersionHistory",
            format!("/2015-01-01/packages/{package}/history"),
        ),
        (
            "ListDomainsForPackage",
            format!("/2015-01-01/packages/{package}/domains"),
        ),
    ] {
        let answer = call_in(&router, REGION, "GET", &path, None).await;
        assert_eq!(
            answer.status, 200,
            "case {operation:?}: a resource that exists must answer 200, got {} {}",
            answer.status, answer.body
        );
        assert!(
            answer.error_type.is_empty(),
            "case {operation:?}: a success carries no error type, got {:?}",
            answer.error_type
        );
    }
}
