// SPDX-License-Identifier: CC0-1.0

//! The custom-domain association and the partner-integration families, end to end
//! over the awsQuery wire.
//!
//! Two rules, each asserted over a table of inputs.
//!
//! **A custom domain association is a property of the CLUSTER that serves it**
//! (`Cluster$CustomDomainName`, `$CustomDomainCertificateArn`,
//! `$CustomDomainCertificateExpiryDate`), so creating one is observable through
//! `DescribeClusters` as well as `DescribeCustomDomainAssociations`, modifying one
//! replaces the certificate of the association that exists, and deleting one takes
//! all three members off the cluster. Every one of the four operations resolves the
//! cluster against this service's store first — an identifier naming no cluster is
//! `ClusterNotFound` — and the three that act on an existing association refuse a
//! domain name the cluster does not serve with `CustomDomainAssociationNotFoundFault`.
//!
//! **A partner integration is keyed by the (cluster, database, partner) triple**,
//! so adding one twice leaves one record, the status the PARTNER reports is what
//! `UpdatePartnerStatus` writes onto it, `DescribePartners` narrows by the database
//! and partner the request names, and deleting one takes it out of the report.
//! Every one of the four operations resolves the cluster first, for the same reason.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter` the
//! production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator};

/// The cluster the rows associate domains with and add partners to.
const CLUSTER: &str = "domains-and-partners";

/// A second cluster, so the rows can span more than one holder of an association.
const OTHER_CLUSTER: &str = "second-warehouse";

/// A cluster identifier no row creates.
const UNKNOWN_CLUSTER: &str = "never-provisioned";

/// An ACM certificate ARN of the form the member's own pattern admits
/// (`^arn:[\w+=/,.@-]+:acm:[\w+=/,.@-]*:[0-9]+:[\w+=,.@-]+(/[\w+=,.@-]+)*$`).
fn certificate_arn(token: &str) -> String {
    format!("arn:aws:acm:us-west-2:000000000000:certificate/{token}")
}

/// The account that owns this emulator's clusters, which every partner call
/// names.
fn account_id() -> String {
    engine::emulator_account_id().to_string()
}

/// Drive one action with owned parameters.
async fn call(emulator: &Emulator, action: &str, params: &[(String, String)]) -> Answer {
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    emulator.call(action, &borrowed).await
}

/// An emulator holding the two clusters the rows name.
async fn seeded() -> Emulator {
    let emulator = Emulator::new();
    for identifier in [CLUSTER, OTHER_CLUSTER] {
        let created = emulator.create_minimal(identifier).await;
        assert_eq!(
            created.status, 200,
            "case seed {identifier}: expected 200, got {} with body {}",
            created.status, created.body
        );
    }
    emulator
}

/// One row of the custom-domain table: the cluster it associates, the domain name
/// and the certificate.
struct DomainCase {
    case: &'static str,
    cluster: &'static str,
    domain: &'static str,
    certificate: String,
}

#[tokio::test]
async fn a_custom_domain_association_belongs_to_the_cluster_that_serves_it() {
    let emulator = seeded().await;

    let rows: Vec<DomainCase> = vec![
        DomainCase {
            case: "a two-label domain",
            cluster: CLUSTER,
            domain: "warehouse.example.com",
            certificate: certificate_arn("11111111-2222-3333-4444-555555555555"),
        },
        DomainCase {
            case: "a deeper domain on another cluster",
            cluster: OTHER_CLUSTER,
            domain: "reports.eu.example.org",
            certificate: certificate_arn("66666666-7777-8888-9999-aaaaaaaaaaaa"),
        },
    ];

    for row in &rows {
        let created = call(
            &emulator,
            "CreateCustomDomainAssociation",
            &[
                ("ClusterIdentifier".to_string(), row.cluster.to_string()),
                ("CustomDomainName".to_string(), row.domain.to_string()),
                (
                    "CustomDomainCertificateArn".to_string(),
                    row.certificate.clone(),
                ),
            ],
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {}: expected 200, got {} with body {}",
            row.case, created.status, created.body
        );
        for (element, expected) in [
            ("CustomDomainName", row.domain.to_string()),
            ("CustomDomainCertificateArn", row.certificate.clone()),
            ("ClusterIdentifier", row.cluster.to_string()),
        ] {
            assert_eq!(
                created.text(element).as_deref(),
                Some(expected.as_str()),
                "case {}: expected {element}={expected:?} back, got {:?}",
                row.case,
                created.text(element)
            );
        }
        // The certificate's expiry is reported as an ISO-8601 instant in the
        // future: it is the certificate's own, so the create says when the
        // association's certificate runs out.
        let expiry = created
            .text("CustomDomainCertExpiryTime")
            .unwrap_or_else(|| panic!("case {}: expected an expiry, got {}", row.case, created.body));
        assert!(
            expiry.ends_with('Z') && expiry.contains('T'),
            "case {}: expected an ISO-8601 expiry, got {expiry:?}",
            row.case
        );

        // THE RULE: the association is the cluster's, so the cluster reports it.
        let described = emulator
            .call("DescribeClusters", &[("ClusterIdentifier", row.cluster)])
            .await;
        assert_eq!(
            (
                described.text("CustomDomainName").as_deref(),
                described.text("CustomDomainCertificateArn").as_deref()
            ),
            (Some(row.domain), Some(row.certificate.as_str())),
            "case {}: expected the cluster to report its association, got body {}",
            row.case,
            described.body
        );
        assert!(
            described.text("CustomDomainCertificateExpiryDate").is_some(),
            "case {}: expected the cluster to report the certificate's expiry, got body {}",
            row.case,
            described.body
        );
    }

    // Every association the account holds, one record per CERTIFICATE, each
    // listing the (domain, cluster) pairs tied to it.
    let listed = emulator.call("DescribeCustomDomainAssociations", &[]).await;
    assert_eq!(
        all_text(&listed.body, "CustomDomainCertificateArn"),
        rows.iter()
            .map(|row| row.certificate.clone())
            .collect::<Vec<String>>(),
        "case list every association: expected one record per certificate, got body {}",
        listed.body
    );
    assert_eq!(
        all_text(&listed.body, "CustomDomainName"),
        rows.iter()
            .map(|row| row.domain.to_string())
            .collect::<Vec<String>>(),
        "case list every association: expected each certificate's own pairs, got body {}",
        listed.body
    );

    // Each filter selects the pairs it names.
    let by_domain = emulator
        .call(
            "DescribeCustomDomainAssociations",
            &[("CustomDomainName", rows[0].domain)],
        )
        .await;
    assert_eq!(
        all_text(&by_domain.body, "ClusterIdentifier"),
        vec![rows[0].cluster.to_string()],
        "case filter by domain: expected only that pair, got body {}",
        by_domain.body
    );
    let by_certificate = call(
        &emulator,
        "DescribeCustomDomainAssociations",
        &[(
            "CustomDomainCertificateArn".to_string(),
            rows[1].certificate.clone(),
        )],
    )
    .await;
    assert_eq!(
        all_text(&by_certificate.body, "CustomDomainName"),
        vec![rows[1].domain.to_string()],
        "case filter by certificate: expected only that certificate's pairs, got body {}",
        by_certificate.body
    );

    // A name that selects nothing is a refusal, not an empty page.
    let unknown = emulator
        .call(
            "DescribeCustomDomainAssociations",
            &[("CustomDomainName", "nothing.example.net")],
        )
        .await;
    assert_eq!(
        (unknown.status, unknown.error_code().as_deref()),
        (404, Some("CustomDomainAssociationNotFoundFault")),
        "case an unknown domain: expected 404 CustomDomainAssociationNotFoundFault, got {} / {:?}",
        unknown.status,
        unknown.error_code()
    );
    assert_eq!(
        unknown.text("Message").as_deref(),
        Some(
            state::custom_domain_association_not_found_message("nothing.example.net").as_str()
        ),
        "case an unknown domain: expected the message to name it, got {:?}",
        unknown.text("Message")
    );

    // A domain name another cluster already serves cannot be associated twice.
    let taken = call(
        &emulator,
        "CreateCustomDomainAssociation",
        &[
            ("ClusterIdentifier".to_string(), OTHER_CLUSTER.to_string()),
            ("CustomDomainName".to_string(), rows[0].domain.to_string()),
            (
                "CustomDomainCertificateArn".to_string(),
                rows[0].certificate.clone(),
            ),
        ],
    )
    .await;
    assert_eq!(
        (taken.status, taken.error_code().as_deref()),
        (400, Some("CustomCnameAssociationFault")),
        "case a domain another cluster serves: expected 400 CustomCnameAssociationFault, got {} / {:?}",
        taken.status,
        taken.error_code()
    );

    // MODIFY replaces the certificate of the association that exists.
    let rotated = certificate_arn("bbbbbbbb-cccc-dddd-eeee-ffffffffffff");
    let modified = call(
        &emulator,
        "ModifyCustomDomainAssociation",
        &[
            ("ClusterIdentifier".to_string(), CLUSTER.to_string()),
            ("CustomDomainName".to_string(), rows[0].domain.to_string()),
            ("CustomDomainCertificateArn".to_string(), rotated.clone()),
        ],
    )
    .await;
    assert_eq!(
        modified.status, 200,
        "case rotate the certificate: expected 200, got {} with body {}",
        modified.status, modified.body
    );
    let after_modify = emulator
        .call("DescribeClusters", &[("ClusterIdentifier", CLUSTER)])
        .await;
    assert_eq!(
        after_modify.text("CustomDomainCertificateArn").as_deref(),
        Some(rotated.as_str()),
        "case rotate the certificate: expected the new certificate to persist, got {:?}",
        after_modify.text("CustomDomainCertificateArn")
    );

    // A modify naming a domain the cluster does not serve creates nothing.
    let not_served = call(
        &emulator,
        "ModifyCustomDomainAssociation",
        &[
            ("ClusterIdentifier".to_string(), CLUSTER.to_string()),
            (
                "CustomDomainName".to_string(),
                "not-served.example.com".to_string(),
            ),
            ("CustomDomainCertificateArn".to_string(), rotated.clone()),
        ],
    )
    .await;
    assert_eq!(
        (not_served.status, not_served.error_code().as_deref()),
        (404, Some("CustomDomainAssociationNotFoundFault")),
        "case modify a domain the cluster does not serve: expected the NotFound refusal, got {} / {:?}",
        not_served.status,
        not_served.error_code()
    );

    // DELETE takes all three members off the cluster.
    let deleted = call(
        &emulator,
        "DeleteCustomDomainAssociation",
        &[
            ("ClusterIdentifier".to_string(), CLUSTER.to_string()),
            ("CustomDomainName".to_string(), rows[0].domain.to_string()),
        ],
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "case delete the association: expected 200, got {} with body {}",
        deleted.status, deleted.body
    );
    let after_delete = emulator
        .call("DescribeClusters", &[("ClusterIdentifier", CLUSTER)])
        .await;
    for element in [
        "CustomDomainName",
        "CustomDomainCertificateArn",
        "CustomDomainCertificateExpiryDate",
    ] {
        assert_eq!(
            after_delete.text(element),
            None,
            "case delete the association: expected no {element} on the cluster, got {:?}",
            after_delete.text(element)
        );
    }

    // Every one of the four operations resolves the CLUSTER first.
    let unknown_cluster_cases: Vec<(&str, Vec<(String, String)>)> = vec![
        (
            "CreateCustomDomainAssociation",
            vec![
                ("ClusterIdentifier".to_string(), UNKNOWN_CLUSTER.to_string()),
                ("CustomDomainName".to_string(), "x.example.com".to_string()),
                ("CustomDomainCertificateArn".to_string(), rotated.clone()),
            ],
        ),
        (
            "ModifyCustomDomainAssociation",
            vec![
                ("ClusterIdentifier".to_string(), UNKNOWN_CLUSTER.to_string()),
                ("CustomDomainName".to_string(), "x.example.com".to_string()),
                ("CustomDomainCertificateArn".to_string(), rotated.clone()),
            ],
        ),
        (
            "DeleteCustomDomainAssociation",
            vec![
                ("ClusterIdentifier".to_string(), UNKNOWN_CLUSTER.to_string()),
                ("CustomDomainName".to_string(), "x.example.com".to_string()),
            ],
        ),
    ];
    for (action, params) in &unknown_cluster_cases {
        let answer = call(&emulator, action, params).await;
        assert_eq!(
            (answer.status, answer.error_code().as_deref()),
            (404, Some("ClusterNotFound")),
            "case {action} against an unknown cluster: expected 404 ClusterNotFound, got {} / {:?} with body {}",
            answer.status,
            answer.error_code(),
            answer.body
        );
    }
}

#[tokio::test]
async fn a_partner_integration_is_keyed_by_its_cluster_database_and_partner() {
    let emulator = seeded().await;
    let account = account_id();

    // One row per class the rule generalises over: two partners on one database,
    // the same partner on another database, and a partner on another cluster.
    let rows: [(&str, &str, &str, &str); 4] = [
        ("one partner on a database", CLUSTER, "analytics", "datadog"),
        ("a second partner on it", CLUSTER, "analytics", "fivetran"),
        ("the same partner on another database", CLUSTER, "reporting", "datadog"),
        ("a partner on another cluster", OTHER_CLUSTER, "analytics", "datadog"),
    ];

    for (case, cluster, database, partner) in rows {
        let added = call(
            &emulator,
            "AddPartner",
            &[
                ("AccountId".to_string(), account.clone()),
                ("ClusterIdentifier".to_string(), cluster.to_string()),
                ("DatabaseName".to_string(), database.to_string()),
                ("PartnerName".to_string(), partner.to_string()),
            ],
        )
        .await;
        assert_eq!(
            added.status, 200,
            "case {case}: expected 200, got {} with body {}",
            added.status, added.body
        );
        assert_eq!(
            (
                added.text("DatabaseName").as_deref(),
                added.text("PartnerName").as_deref()
            ),
            (Some(database), Some(partner)),
            "case {case}: expected the pair back, got body {}",
            added.body
        );
    }

    // THE RULE: the triple is the key, so the cluster's own report lists exactly
    // the partners added to it, in the order they were added, each starting at
    // the status no partner has reported on yet.
    let described = call(
        &emulator,
        "DescribePartners",
        &[
            ("AccountId".to_string(), account.clone()),
            ("ClusterIdentifier".to_string(), CLUSTER.to_string()),
        ],
    )
    .await;
    assert_eq!(
        all_text(&described.body, "PartnerName"),
        vec![
            "datadog".to_string(),
            "fivetran".to_string(),
            "datadog".to_string()
        ],
        "case describe every partner of the cluster: expected the add order, got body {}",
        described.body
    );
    assert_eq!(
        all_text(&described.body, "Status"),
        vec![state::PARTNER_INTEGRATION_INITIAL_STATUS.to_string(); 3],
        "case describe every partner: expected the initial status, got body {}",
        described.body
    );

    // Adding the same triple twice leaves ONE record.
    let again = call(
        &emulator,
        "AddPartner",
        &[
            ("AccountId".to_string(), account.clone()),
            ("ClusterIdentifier".to_string(), CLUSTER.to_string()),
            ("DatabaseName".to_string(), "analytics".to_string()),
            ("PartnerName".to_string(), "datadog".to_string()),
        ],
    )
    .await;
    assert_eq!(again.status, 200, "case re-add: {}", again.body);
    let after_readd = call(
        &emulator,
        "DescribePartners",
        &[
            ("AccountId".to_string(), account.clone()),
            ("ClusterIdentifier".to_string(), CLUSTER.to_string()),
        ],
    )
    .await;
    assert_eq!(
        all_text(&after_readd.body, "PartnerName").len(),
        3,
        "case re-add: expected one record per triple, got body {}",
        after_readd.body
    );

    // Each filter narrows the report: "If database name is not specified, then
    // all databases in the cluster are described", and the same for the partner.
    let by_database = call(
        &emulator,
        "DescribePartners",
        &[
            ("AccountId".to_string(), account.clone()),
            ("ClusterIdentifier".to_string(), CLUSTER.to_string()),
            ("DatabaseName".to_string(), "reporting".to_string()),
        ],
    )
    .await;
    assert_eq!(
        all_text(&by_database.body, "DatabaseName"),
        vec!["reporting".to_string()],
        "case filter by database: expected only that database's partners, got body {}",
        by_database.body
    );
    let by_partner = call(
        &emulator,
        "DescribePartners",
        &[
            ("AccountId".to_string(), account.clone()),
            ("ClusterIdentifier".to_string(), CLUSTER.to_string()),
            ("PartnerName".to_string(), "fivetran".to_string()),
        ],
    )
    .await;
    assert_eq!(
        all_text(&by_partner.body, "PartnerName"),
        vec!["fivetran".to_string()],
        "case filter by partner: expected only that partner, got body {}",
        by_partner.body
    );

    // The status the PARTNER reports is what the update writes, with its message.
    for status in ["Active", "RuntimeFailure"] {
        let updated = call(
            &emulator,
            "UpdatePartnerStatus",
            &[
                ("AccountId".to_string(), account.clone()),
                ("ClusterIdentifier".to_string(), CLUSTER.to_string()),
                ("DatabaseName".to_string(), "analytics".to_string()),
                ("PartnerName".to_string(), "fivetran".to_string()),
                ("Status".to_string(), status.to_string()),
                (
                    "StatusMessage".to_string(),
                    format!("the partner reports {status}"),
                ),
            ],
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case update to {status}: expected 200, got {} with body {}",
            updated.status, updated.body
        );
        let read = call(
            &emulator,
            "DescribePartners",
            &[
                ("AccountId".to_string(), account.clone()),
                ("ClusterIdentifier".to_string(), CLUSTER.to_string()),
                ("PartnerName".to_string(), "fivetran".to_string()),
            ],
        )
        .await;
        assert_eq!(
            (
                read.text("Status").as_deref(),
                read.text("StatusMessage").as_deref()
            ),
            (Some(status), Some(format!("the partner reports {status}").as_str())),
            "case update to {status}: expected the reported status to persist, got body {}",
            read.body
        );
    }

    // A delete takes the triple it names out of the report and leaves the rest.
    let deleted = call(
        &emulator,
        "DeletePartner",
        &[
            ("AccountId".to_string(), account.clone()),
            ("ClusterIdentifier".to_string(), CLUSTER.to_string()),
            ("DatabaseName".to_string(), "analytics".to_string()),
            ("PartnerName".to_string(), "fivetran".to_string()),
        ],
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "case delete a partner: expected 200, got {} with body {}",
        deleted.status, deleted.body
    );
    let after_delete = call(
        &emulator,
        "DescribePartners",
        &[
            ("AccountId".to_string(), account.clone()),
            ("ClusterIdentifier".to_string(), CLUSTER.to_string()),
        ],
    )
    .await;
    assert_eq!(
        all_text(&after_delete.body, "PartnerName"),
        vec!["datadog".to_string(), "datadog".to_string()],
        "case delete a partner: expected only that one to go, got body {}",
        after_delete.body
    );
    let gone = call(
        &emulator,
        "DeletePartner",
        &[
            ("AccountId".to_string(), account.clone()),
            ("ClusterIdentifier".to_string(), CLUSTER.to_string()),
            ("DatabaseName".to_string(), "analytics".to_string()),
            ("PartnerName".to_string(), "fivetran".to_string()),
        ],
    )
    .await;
    assert_eq!(
        (gone.status, gone.error_code().as_deref()),
        (404, Some("PartnerNotFound")),
        "case delete it twice: expected 404 PartnerNotFound, got {} / {:?}",
        gone.status,
        gone.error_code()
    );

    // Every one of the four operations resolves the CLUSTER first.
    for action in [
        "DescribePartners",
        "AddPartner",
        "DeletePartner",
        "UpdatePartnerStatus",
    ] {
        let mut params = vec![
            ("AccountId".to_string(), account.clone()),
            ("ClusterIdentifier".to_string(), UNKNOWN_CLUSTER.to_string()),
            ("DatabaseName".to_string(), "analytics".to_string()),
            ("PartnerName".to_string(), "datadog".to_string()),
        ];
        if action == "UpdatePartnerStatus" {
            params.push(("Status".to_string(), "Active".to_string()));
        }
        let answer = call(&emulator, action, &params).await;
        assert_eq!(
            (answer.status, answer.error_code().as_deref()),
            (404, Some("ClusterNotFound")),
            "case {action} against an unknown cluster: expected 404 ClusterNotFound, got {} / {:?} with body {}",
            answer.status,
            answer.error_code(),
            answer.body
        );
    }

    // An account that owns none of this account's clusters may not integrate a
    // partner with them.
    let unauthorized = call(
        &emulator,
        "DescribePartners",
        &[
            ("AccountId".to_string(), "210987654321".to_string()),
            ("ClusterIdentifier".to_string(), CLUSTER.to_string()),
        ],
    )
    .await;
    assert_eq!(
        (unauthorized.status, unauthorized.error_code().as_deref()),
        (401, Some("UnauthorizedPartnerIntegration")),
        "case another account's partner read: expected 401 UnauthorizedPartnerIntegration, got {} / {:?}",
        unauthorized.status,
        unauthorized.error_code()
    );
}
