// SPDX-License-Identifier: CC0-1.0

//! The HSM client-certificate lifecycle, end to end over the awsQuery wire.
//!
//! The rule under test: **`CreateHsmClientCertificate` mints the certificate the
//! request names and reports it with the public key the service generated for it;
//! that key is the certificate's own, so every later read of the certificate
//! carries the same blob, and the IDENTIFIER is what an account's certificates are
//! unique on — a second create under one identifier is refused with
//! `HsmClientCertificateAlreadyExistsFault` and overwrites nothing.** The reads
//! and the delete are the same rule from the other side: an identifier that names
//! no certificate is refused rather than reported as an empty page, and a
//! certificate a live cluster connects to its HSM with is not the account's to
//! remove.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter` the
//! production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! The rows span the classes the rule generalises over rather than one recorded
//! request: the shape of the identifier, how many tags the create carries, the
//! `TagKeys` / `TagValues` filter, and the `MaxRecords` window and `Marker` cursor
//! the collection is paged through.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator};

/// One row of the create table: the identifier it mints a certificate under and
/// the tags it carries.
struct CreateCase {
    case: &'static str,
    identifier: &'static str,
    tags: Vec<(&'static str, &'static str)>,
}

/// `CreateHsmClientCertificate` with `identifier` and `tags`.
async fn create(
    emulator: &Emulator,
    identifier: &str,
    tags: &[(&'static str, &'static str)],
) -> Answer {
    let mut params: Vec<(String, String)> = vec![(
        "HsmClientCertificateIdentifier".to_string(),
        identifier.to_string(),
    )];
    for (position, (key, value)) in tags.iter().enumerate() {
        params.push((
            format!("Tags.Tag.{}.Key", position + 1),
            (*key).to_string(),
        ));
        params.push((
            format!("Tags.Tag.{}.Value", position + 1),
            (*value).to_string(),
        ));
    }
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    emulator
        .call("CreateHsmClientCertificate", &borrowed)
        .await
}

/// `DescribeHsmClientCertificates` with these parameters.
async fn describe(emulator: &Emulator, params: &[(&str, &str)]) -> Answer {
    emulator
        .call("DescribeHsmClientCertificates", params)
        .await
}

#[tokio::test]
async fn a_created_certificate_is_reported_with_the_key_the_service_minted() {
    let emulator = Emulator::new();

    let rows: Vec<CreateCase> = vec![
        CreateCase {
            case: "a plain identifier, no tags",
            identifier: "hsm-cert-plain",
            tags: Vec::new(),
        },
        CreateCase {
            case: "an identifier carrying digits and hyphens, one tag",
            identifier: "hsm-cert-2-of-3",
            tags: vec![("team", "analytics")],
        },
        CreateCase {
            case: "a mixed-case identifier with two tags",
            identifier: "HsmCertMixedCase",
            tags: vec![("env", "prod"), ("owner", "data-platform")],
        },
    ];

    for row in &rows {
        let created = create(&emulator, row.identifier, &row.tags).await;
        assert_eq!(
            created.status, 200,
            "case {}: expected 200, got {} with body {}",
            row.case, created.status, created.body
        );
        assert_eq!(
            created.text("HsmClientCertificateIdentifier").as_deref(),
            Some(row.identifier),
            "case {}: expected the identifier back, got {:?}",
            row.case,
            created.text("HsmClientCertificateIdentifier")
        );
        // The public key is the service's to mint — the request carries no key
        // material at all — and it is the certificate's own, derived from its
        // identifier, so the blob the create answered with is the blob every
        // later read of it carries.
        let minted = state::hsm_client_certificate_public_key(row.identifier);
        assert_eq!(
            created.text("HsmClientCertificatePublicKey").as_deref(),
            Some(minted.as_str()),
            "case {}: expected the minted public key, got {:?}",
            row.case,
            created.text("HsmClientCertificatePublicKey")
        );
        assert_eq!(
            all_text(&created.body, "Key"),
            row.tags
                .iter()
                .map(|(key, _)| (*key).to_string())
                .collect::<Vec<String>>(),
            "case {}: expected the tag keys the request carried, got body {}",
            row.case,
            created.body
        );

        // ROUND TRIP: the record the create persisted is what a read reports,
        // blob for blob.
        let read = describe(
            &emulator,
            &[("HsmClientCertificateIdentifier", row.identifier)],
        )
        .await;
        assert_eq!(
            (
                read.status,
                read.text("HsmClientCertificateIdentifier").as_deref(),
                read.text("HsmClientCertificatePublicKey").as_deref()
            ),
            (200, Some(row.identifier), Some(minted.as_str())),
            "case {}: expected the read to report the created certificate, got body {}",
            row.case,
            read.body
        );

        // A second create under the same identifier is refused, and the stored
        // certificate is untouched.
        let duplicate = create(&emulator, row.identifier, &[("later", "tag")]).await;
        assert_eq!(
            (duplicate.status, duplicate.error_code().as_deref()),
            (400, Some("HsmClientCertificateAlreadyExistsFault")),
            "case {}: expected 400 HsmClientCertificateAlreadyExistsFault, got {} / {:?}",
            row.case,
            duplicate.status,
            duplicate.error_code()
        );
        let after = describe(
            &emulator,
            &[("HsmClientCertificateIdentifier", row.identifier)],
        )
        .await;
        assert_eq!(
            all_text(&after.body, "Key"),
            row.tags
                .iter()
                .map(|(key, _)| (*key).to_string())
                .collect::<Vec<String>>(),
            "case {}: expected the refused create to overwrite nothing, got body {}",
            row.case,
            after.body
        );
    }

    // Every certificate the account holds, in creation order.
    let listed = describe(&emulator, &[]).await;
    assert_eq!(
        all_text(&listed.body, "HsmClientCertificateIdentifier"),
        rows.iter()
            .map(|row| row.identifier.to_string())
            .collect::<Vec<String>>(),
        "case list every certificate: expected creation order, got body {}",
        listed.body
    );

    // The tag filter selects CERTIFICATES: one matching tag reports the whole
    // record.
    let filtered = describe(&emulator, &[("TagKeys.TagKey.1", "env")]).await;
    assert_eq!(
        all_text(&filtered.body, "HsmClientCertificateIdentifier"),
        vec!["HsmCertMixedCase".to_string()],
        "case filter by tag key: expected only the tagged certificate, got body {}",
        filtered.body
    );
    let by_value = describe(&emulator, &[("TagValues.TagValue.1", "analytics")]).await;
    assert_eq!(
        all_text(&by_value.body, "HsmClientCertificateIdentifier"),
        vec!["hsm-cert-2-of-3".to_string()],
        "case filter by tag value: expected only the matching certificate, got body {}",
        by_value.body
    );

    // The `MaxRecords` window and the `Marker` cursor page the same collection.
    let page = describe(
        &emulator,
        &[("MaxRecords", &state::MAX_RECORDS_MIN.to_string())],
    )
    .await;
    assert_eq!(
        all_text(&page.body, "HsmClientCertificateIdentifier").len(),
        rows.len(),
        "case one page: expected the whole collection inside the window, got body {}",
        page.body
    );
    assert_eq!(
        page.text("Marker"),
        None,
        "case one page: expected no cursor when nothing remains, got {:?}",
        page.text("Marker")
    );
    let refused = describe(&emulator, &[("MaxRecords", "1")]).await;
    assert_eq!(
        (refused.status, refused.error_code().as_deref()),
        (400, Some("InvalidParameterValue")),
        "case a MaxRecords below the window: expected a refusal, got {} / {:?}",
        refused.status,
        refused.error_code()
    );

    // An identifier that names no certificate is a refusal, not an empty page.
    let unknown = describe(
        &emulator,
        &[("HsmClientCertificateIdentifier", "hsm-cert-never-made")],
    )
    .await;
    assert_eq!(
        (unknown.status, unknown.error_code().as_deref()),
        (400, Some("HsmClientCertificateNotFoundFault")),
        "case an unknown identifier: expected 400 HsmClientCertificateNotFoundFault, got {} / {:?}",
        unknown.status,
        unknown.error_code()
    );
    assert_eq!(
        unknown.text("Message").as_deref(),
        Some(
            state::hsm_client_certificate_not_found_message(
                state::HsmClientCertificateNotFoundWording::IdentifierThenNotFound,
                "hsm-cert-never-made",
            )
            .as_str()
        ),
        "case an unknown identifier: expected the message to name it, got {:?}",
        unknown.text("Message")
    );
}

#[tokio::test]
async fn a_certificate_a_cluster_uses_cannot_be_deleted_until_the_cluster_is_gone() {
    let emulator = Emulator::new();
    let identifier = "hsm-cert-in-use";

    let created = create(&emulator, identifier, &[]).await;
    assert_eq!(
        created.status, 200,
        "case create the certificate: expected 200, got {} with body {}",
        created.status, created.body
    );

    // A cluster configured to retrieve its encryption keys from the HSM through
    // this certificate.
    let cluster = emulator
        .call(
            "CreateCluster",
            &[
                ("ClusterIdentifier", "hsm-cert-user"),
                ("NodeType", "ra3.xlplus"),
                ("MasterUsername", "admin"),
                ("HsmClientCertificateIdentifier", identifier),
            ],
        )
        .await;
    assert_eq!(
        cluster.status, 200,
        "case create the cluster using it: expected 200, got {} with body {}",
        cluster.status, cluster.body
    );

    let refused = emulator
        .call(
            "DeleteHsmClientCertificate",
            &[("HsmClientCertificateIdentifier", identifier)],
        )
        .await;
    assert_eq!(
        (refused.status, refused.error_code().as_deref()),
        (400, Some("InvalidHsmClientCertificateStateFault")),
        "case delete a certificate in use: expected 400 InvalidHsmClientCertificateStateFault, got {} / {:?} with body {}",
        refused.status,
        refused.error_code(),
        refused.body
    );
    let still_there = describe(
        &emulator,
        &[("HsmClientCertificateIdentifier", identifier)],
    )
    .await;
    assert_eq!(
        still_there.status, 200,
        "case delete a certificate in use: expected it to survive the refusal, got body {}",
        still_there.body
    );

    // Once no cluster uses it, the same delete succeeds and the record is gone.
    let deleted_cluster = emulator
        .call(
            "DeleteCluster",
            &[
                ("ClusterIdentifier", "hsm-cert-user"),
                ("SkipFinalClusterSnapshot", "true"),
            ],
        )
        .await;
    assert_eq!(
        deleted_cluster.status, 200,
        "case delete the cluster: expected 200, got {} with body {}",
        deleted_cluster.status, deleted_cluster.body
    );
    let deleted = emulator
        .call(
            "DeleteHsmClientCertificate",
            &[("HsmClientCertificateIdentifier", identifier)],
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "case delete the certificate once free: expected 200, got {} / {:?} with body {}",
        deleted.status,
        deleted.error_code(),
        deleted.body
    );
    let gone = describe(
        &emulator,
        &[("HsmClientCertificateIdentifier", identifier)],
    )
    .await;
    assert_eq!(
        (gone.status, gone.error_code().as_deref()),
        (400, Some("HsmClientCertificateNotFoundFault")),
        "case read after delete: expected the certificate to be gone, got {} / {:?}",
        gone.status,
        gone.error_code()
    );
    let unknown_delete = emulator
        .call(
            "DeleteHsmClientCertificate",
            &[("HsmClientCertificateIdentifier", "hsm-cert-never-made")],
        )
        .await;
    assert_eq!(
        (unknown_delete.status, unknown_delete.error_code().as_deref()),
        (400, Some("HsmClientCertificateNotFoundFault")),
        "case delete an unknown identifier: expected the NotFound refusal, got {} / {:?}",
        unknown_delete.status,
        unknown_delete.error_code()
    );
}
