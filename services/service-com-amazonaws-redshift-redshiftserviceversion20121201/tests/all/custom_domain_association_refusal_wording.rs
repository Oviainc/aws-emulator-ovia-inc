// SPDX-License-Identifier: CC0-1.0

//! The MESSAGE a custom-domain-association refusal carries, end to end over the
//! awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds, reusing the harness the cluster-lifecycle suite
//! owns rather than keeping a second copy of it.
//!
//! The RULE the suite asserts: an operation that resolves a custom domain
//! association through the `CustomDomainName` a request names, and finds none,
//! states that nothing is linked to THAT name — the fixed prose
//! `There aren't any associations linked to the custom domain name `, the name
//! the request supplied, then a period. The sentence is fixed and only the name
//! varies, so one condition reads the same way through every operation that
//! resolves an association by its domain name, and no refusal names a shape of
//! the emulator's own instead of the value the caller sent. A request that names
//! its association through the CERTIFICATE member instead is refused in the same
//! sentence about that member's value, so no certificate ARN is interpolated
//! into a sentence about a domain name.
//!
//! The table spans what the rule generalises over: the operation that makes the
//! lookup, and the shape of the name (two labels, a deeper name, hyphenated
//! labels, and a label at the longest the member's own pattern admits — 63
//! characters, from `smithy_query(query="operation",
//! operation="DescribeCustomDomainAssociations")`'s
//! `CustomDomainName` pattern `(?!-)[A-Za-z0-9-]{0,62}[A-Za-z0-9]` per label).
//!
//! The expected sentences are spelled out here rather than read off the crate
//! under test, so a row cannot pass by restating whatever the handler happened
//! to build.

use crate::cluster_lifecycle::{Answer, Emulator};

/// The cluster that serves the one association these cases seed.
const CLUSTER: &str = "domain-wording-cluster";

/// The domain name that cluster serves, so a refusal below cannot be an
/// "everything is refused" artefact.
const SERVED_DOMAIN: &str = "served.example.com";

/// The database user a `GetClusterCredentials` call issues a password for.
const MASTER_USERNAME: &str = "admin";

/// An ACM certificate ARN of the form `CustomDomainCertificateArn`'s own pattern
/// admits (`^arn:[\w+=/,.@-]+:acm:[\w+=/,.@-]*:[0-9]+:[\w+=,.@-]+(/[\w+=,.@-]+)*$`).
fn certificate_arn(token: &str) -> String {
    format!("arn:aws:acm:us-west-2:000000000000:certificate/{token}")
}

/// The sentence an operation answers when the custom domain NAME it resolved
/// names no association.
fn absent_name_sentence(custom_domain_name: &str) -> String {
    format!("There aren't any associations linked to the custom domain name {custom_domain_name}.")
}

/// The same sentence about a request that named its association through the
/// CERTIFICATE member instead.
fn absent_certificate_sentence(custom_domain_certificate_arn: &str) -> String {
    format!(
        "There aren't any associations linked to the custom domain certificate \
         {custom_domain_certificate_arn}."
    )
}

/// POST one action with owned parameters.
async fn call(emulator: &Emulator, action: &str, params: &[(String, String)]) -> Answer {
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    emulator.call(action, &borrowed).await
}

/// One form parameter, owned so a row can build it from a borrowed value.
fn pair(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// An emulator holding one cluster that serves [`SERVED_DOMAIN`].
async fn seeded() -> Emulator {
    let emulator = Emulator::new();
    let created = emulator.create_minimal(CLUSTER).await;
    assert_eq!(
        created.status, 200,
        "case seed the cluster: expected 200, got {} with body {}",
        created.status, created.body
    );
    let associated = call(
        &emulator,
        "CreateCustomDomainAssociation",
        &[
            pair("ClusterIdentifier", CLUSTER),
            pair("CustomDomainName", SERVED_DOMAIN),
            (
                "CustomDomainCertificateArn".to_string(),
                certificate_arn("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"),
            ),
        ],
    )
    .await;
    assert_eq!(
        associated.status, 200,
        "case seed the association: expected 200, got {} with body {}",
        associated.status, associated.body
    );
    emulator
}

/// Every domain name a row asks about, none of them served by any cluster.
fn absent_domain_names() -> Vec<(&'static str, String)> {
    // The longest label the member's pattern admits is 63 characters: 62 from
    // `[A-Za-z0-9-]{0,62}` plus the trailing `[A-Za-z0-9]`.
    let longest_label: String = std::iter::repeat_n('a', 62).chain(std::iter::once('z')).collect();
    vec![
        ("a two-label name", "absent.example.com".to_string()),
        (
            "a deeper name",
            "reports.eu.west.absent.example.org".to_string(),
        ),
        (
            "hyphenated labels",
            "no-such-warehouse.absent-domain.example.net".to_string(),
        ),
        (
            "a label at the longest the pattern admits",
            format!("{longest_label}.example.com"),
        ),
    ]
}

#[tokio::test]
async fn an_absent_custom_domain_association_is_refused_in_one_sentence() {
    let emulator = seeded().await;
    let rotated = certificate_arn("12121212-3434-5656-7878-909090909090");

    // Each operation that resolves an association through the name the request
    // carries, with the fault its model declares for the condition.
    type Request = fn(&str) -> Vec<(String, String)>;
    fn describe(name: &str) -> Vec<(String, String)> {
        vec![pair("CustomDomainName", name)]
    }
    fn modify(name: &str) -> Vec<(String, String)> {
        vec![
            pair("ClusterIdentifier", CLUSTER),
            pair("CustomDomainName", name),
            (
                "CustomDomainCertificateArn".to_string(),
                "arn:aws:acm:us-west-2:000000000000:certificate/12121212-3434-5656-7878-909090909090"
                    .to_string(),
            ),
        ]
    }
    fn delete(name: &str) -> Vec<(String, String)> {
        vec![
            pair("ClusterIdentifier", CLUSTER),
            pair("CustomDomainName", name),
        ]
    }
    fn credentials(name: &str) -> Vec<(String, String)> {
        vec![
            pair("CustomDomainName", name),
            pair("DbUser", MASTER_USERNAME),
        ]
    }

    // `GetClusterCredentials` resolves a cluster THROUGH the association, so the
    // same condition reaches it as the fault its own model declares
    // (`ClusterNotFound`) — the sentence is the condition's, not the fault's.
    let operations: Vec<(&str, Request, &str)> = vec![
        (
            "DescribeCustomDomainAssociations",
            describe as Request,
            "CustomDomainAssociationNotFoundFault",
        ),
        (
            "ModifyCustomDomainAssociation",
            modify as Request,
            "CustomDomainAssociationNotFoundFault",
        ),
        (
            "DeleteCustomDomainAssociation",
            delete as Request,
            "CustomDomainAssociationNotFoundFault",
        ),
        (
            "GetClusterCredentials",
            credentials as Request,
            "ClusterNotFound",
        ),
    ];

    for (name_case, name) in absent_domain_names() {
        for (action, request, code) in &operations {
            let case = format!("{action} on {name_case}");
            let refused = call(&emulator, action, &request(&name)).await;
            assert_eq!(
                (refused.status, refused.error_code().as_deref()),
                (404, Some(*code)),
                "case {case}: expected 404 {code}, got {} / {:?} with body {}",
                refused.status,
                refused.error_code(),
                refused.body
            );
            let expected = absent_name_sentence(&name);
            assert_eq!(
                refused.text("Message").as_deref(),
                Some(expected.as_str()),
                "case {case}: expected the message {expected:?}, got {:?} with body {}",
                refused.text("Message"),
                refused.body
            );
        }
    }

    // A request that names its association through the CERTIFICATE is refused
    // about that member's value, never about a domain name it never sent.
    for (arn_case, arn) in [
        ("a certificate nothing is associated with", rotated.clone()),
        (
            "a certificate in another Region",
            certificate_arn("55555555-6666-7777-8888-999999999999"),
        ),
    ] {
        let refused = call(
            &emulator,
            "DescribeCustomDomainAssociations",
            &[("CustomDomainCertificateArn".to_string(), arn.clone())],
        )
        .await;
        let expected = absent_certificate_sentence(&arn);
        assert_eq!(
            (refused.status, refused.text("Message").as_deref()),
            (404, Some(expected.as_str())),
            "case {arn_case}: expected 404 with {expected:?}, got {} / {:?} with body {}",
            refused.status,
            refused.text("Message"),
            refused.body
        );
    }

    // The association this account DOES hold is served, so the sentences above
    // are refusals of an absent name rather than of every name.
    let served = call(
        &emulator,
        "DescribeCustomDomainAssociations",
        &[pair("CustomDomainName", SERVED_DOMAIN)],
    )
    .await;
    assert_eq!(
        (served.status, served.text("CustomDomainName").as_deref()),
        (200, Some(SERVED_DOMAIN)),
        "case the served name: expected the association to be reported, got {} with body {}",
        served.status,
        served.body
    );
    // And the operations that CHANGE it accept that name.
    let modified = call(
        &emulator,
        "ModifyCustomDomainAssociation",
        &[
            pair("ClusterIdentifier", CLUSTER),
            pair("CustomDomainName", SERVED_DOMAIN),
            ("CustomDomainCertificateArn".to_string(), rotated.clone()),
        ],
    )
    .await;
    assert_eq!(
        modified.status, 200,
        "case modify the served name: expected 200, got {} with body {}",
        modified.status, modified.body
    );
}
