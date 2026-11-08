// SPDX-License-Identifier: CC0-1.0

//! `GetDistributionConfig` end to end over HTTP.
//!
//! The rule under test: `GetDistributionConfig` REPORTS the stored distribution — the
//! configuration the distribution runs under (as submitted, with the members the service
//! itself resolves) and the version token its last mutation handed back — so for every
//! distribution it answers exactly the `DistributionConfig` and `ETag` that the create,
//! the update and `GetDistribution` answer, it changes nothing, and for an id no
//! distribution has it answers `NoSuchDistribution` (HTTP 404).
//!
//! Every row of the table below is one distribution configuration, and the rows differ
//! along everything the rule generalises over: origin count, alternate domain name count,
//! whether the distribution is enabled, which viewer protocol policy its default cache
//! behaviour names, whether the optional `PriceClass` is named at all, and whether the
//! comment is empty — the recorded case is one row of it. The harness (`emulator`, the XML
//! reader, the `Config` table) is the crate's one harness in `distribution_lifecycle.rs`.

use std::time::Duration;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::PROPAGATION_DELAY;

use crate::distribution_lifecycle::{
    call, config_table, create_distribution, delete_distribution, emulator, get_distribution,
    is_version_token, update_distribution, Answer, Config, Node, API_VERSION,
};

/// Read one distribution's configuration: `GET /<version>/distribution/<id>/config`.
async fn get_distribution_config(
    router: &smithy_server_runtime::MultiServiceRouter,
    id: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/distribution/{id}/config"),
        None,
        None,
    )
    .await
}

/// The `DistributionConfig` members compared between `GetDistribution`'s nested
/// configuration and `GetDistributionConfig`'s payload.
///
/// Source: the `DistributionConfig` structure's members in this service's Smithy model
/// (`smithy_query(query="shape",shape="DistributionConfig")`) — every scalar member, plus
/// the self-stated length of each list member and the scalars of the structure members —
/// so a difference in any of them between the two reads fails this suite.
const CONFIG_PATHS: &[&str] = &[
    "CallerReference",
    "Comment",
    "Enabled",
    "PriceClass",
    "HttpVersion",
    "IsIPV6Enabled",
    "DefaultRootObject",
    "WebACLId",
    "Staging",
    "ContinuousDeploymentPolicyId",
    "ConnectionMode",
    "AnycastIpListId",
    "Aliases/Quantity",
    "Origins/Quantity",
    "OriginGroups/Quantity",
    "CacheBehaviors/Quantity",
    "CustomErrorResponses/Quantity",
    "DefaultCacheBehavior/TargetOriginId",
    "DefaultCacheBehavior/ViewerProtocolPolicy",
    "DefaultCacheBehavior/Compress",
    "DefaultCacheBehavior/SmoothStreaming",
    "DefaultCacheBehavior/AllowedMethods/Quantity",
    "DefaultCacheBehavior/TrustedSigners/Enabled",
    "DefaultCacheBehavior/TrustedSigners/Quantity",
    "Logging/Enabled",
    "Logging/IncludeCookies",
    "Logging/Bucket",
    "Logging/Prefix",
    "Restrictions/GeoRestriction/RestrictionType",
    "Restrictions/GeoRestriction/Quantity",
    "ViewerCertificate/CloudFrontDefaultCertificate",
    "ViewerCertificate/MinimumProtocolVersion",
    "ViewerCertificate/CertificateSource",
];

/// The members every stored configuration reports, whatever a request left out: a config
/// read that is missing one of these is not the stored configuration.
const REQUIRED_PATHS: &[&str] = &[
    "CallerReference",
    "Comment",
    "Enabled",
    "Origins/Quantity",
    "DefaultCacheBehavior/TargetOriginId",
    "DefaultCacheBehavior/ViewerProtocolPolicy",
];

/// Assert that `config` — a `DistributionConfig` element — agrees with `reference` on
/// every member of [`CONFIG_PATHS`].
fn assert_same_config(case: &str, reference: &Node, config: &Node, context: &str) {
    for path in CONFIG_PATHS {
        assert_eq!(
            config.text_at(path),
            reference.text_at(path),
            "case {case} ({context}): expected {path} {:?}, got {:?}",
            reference.text_at(path),
            config.text_at(path)
        );
    }
    for path in REQUIRED_PATHS {
        assert!(
            config.at(path).is_some(),
            "case {case} ({context}): the configuration read reports no {path}"
        );
    }
    // The ordered members a `Quantity` only counts: the origin ids and the alternate
    // domain names, in the order they are stored.
    let origins = |node: &Node| -> Vec<String> {
        node.at("Origins/Items")
            .map(|items| {
                items
                    .children_named("Origin")
                    .into_iter()
                    .filter_map(|origin| origin.text_at("Id").map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let aliases = |node: &Node| -> Vec<String> {
        node.at("Aliases/Items")
            .map(|items| {
                items
                    .children_named("CNAME")
                    .into_iter()
                    .map(|cname| cname.text().to_string())
                    .collect()
            })
            .unwrap_or_default()
    };
    assert_eq!(
        origins(config),
        origins(reference),
        "case {case} ({context}): expected the origin ids {:?}, got {:?}",
        origins(reference),
        origins(config)
    );
    assert_eq!(
        aliases(config),
        aliases(reference),
        "case {case} ({context}): expected the alternate domain names {:?}, got {:?}",
        aliases(reference),
        aliases(config)
    );
}

/// The configuration that updates `row`: the SAME caller reference (CloudFront refuses a
/// change to it), with every other member the rule generalises over moved.
fn updated(row: &Config) -> Config {
    Config {
        comment: if row.comment.is_empty() {
            "now commented".to_string()
        } else {
            format!("{} (updated)", row.comment)
        },
        enabled: !row.enabled,
        aliases: if row.aliases.is_empty() {
            vec!["moved.example.com".to_string()]
        } else {
            Vec::new()
        },
        viewer_protocol_policy: if row.viewer_protocol_policy == "allow-all" {
            "https-only"
        } else {
            "allow-all"
        },
        price_class: match row.price_class {
            None => Some("PriceClass_200"),
            Some(_) => None,
        },
        ..row.clone()
    }
}

#[tokio::test]
async fn the_config_read_reports_the_stored_configuration_and_its_version() {
    let router = emulator();
    for row in config_table() {
        let case = row.case;

        let created = create_distribution(&router, &row.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected the create to be accepted, got {} ({})",
            created.status, created.raw
        );
        let id = created.root().text_at("Id").unwrap_or_default().to_string();
        let created_etag = created.header("etag").to_string();

        let read = get_distribution_config(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected the config read to succeed, got {} ({})",
            read.status, read.raw
        );
        // The payload IS the configuration: `DistributionConfig` is this output's
        // `@httpPayload`, so it is the document's root element.
        assert!(
            read.root().at("CallerReference").is_some(),
            "case {case}: expected a DistributionConfig payload, got {}",
            read.raw
        );
        row.assert_matches(read.root(), "GetDistributionConfig");
        assert_eq!(
            read.header("etag"),
            created_etag,
            "case {case}: expected the config read to report the version the create handed \
             back ({created_etag:?}), got {:?}",
            read.header("etag")
        );
        assert!(
            is_version_token(&created_etag),
            "case {case}: expected a CloudFront version token, got {created_etag:?}"
        );

        // Parity with `GetDistribution`: both reads answer from the ONE stored record.
        let whole = get_distribution(&router, &id).await;
        let nested = whole
            .root()
            .at("DistributionConfig")
            .unwrap_or_else(|| panic!("case {case}: GetDistribution carried no config"));
        assert_same_config(case, nested, read.root(), "GetDistribution parity");
        assert_eq!(
            read.header("etag"),
            whole.header("etag"),
            "case {case}: expected GetDistribution and GetDistributionConfig to report one \
             version, got {:?} and {:?}",
            whole.header("etag"),
            read.header("etag")
        );

        // A read changes nothing a later caller sees: reading again answers the same
        // document and the same version.
        let again = get_distribution_config(&router, &id).await;
        assert_eq!(
            again.status, 200,
            "case {case}: expected the second config read to succeed, got {} ({})",
            again.status, again.raw
        );
        assert_eq!(
            again.raw, read.raw,
            "case {case}: expected a repeated config read to answer the same document, got \
             {} then {}",
            read.raw, again.raw
        );
        assert_eq!(
            again.header("etag"),
            created_etag,
            "case {case}: expected the version to survive a read, got {:?}",
            again.header("etag")
        );

        // After an update the config read follows the stored record: the new
        // configuration under the new version token.
        let next = updated(&row);
        let update = update_distribution(&router, &id, &next.update_xml(), Some(&created_etag)).await;
        assert_eq!(
            update.status, 200,
            "case {case}: expected the update to be accepted, got {} ({})",
            update.status, update.raw
        );
        let updated_etag = update.header("etag").to_string();
        assert_ne!(
            updated_etag, created_etag,
            "case {case}: expected the update to mint a new version, got {updated_etag:?}"
        );
        let reread = get_distribution_config(&router, &id).await;
        assert_eq!(
            reread.status, 200,
            "case {case}: expected the config read after the update to succeed, got {} ({})",
            reread.status, reread.raw
        );
        next.assert_matches(reread.root(), "GetDistributionConfig after the update");
        assert_eq!(
            reread.header("etag"),
            updated_etag,
            "case {case}: expected the config read to report the version the update handed \
             back ({updated_etag:?}), got {:?}",
            reread.header("etag")
        );
    }
}

#[tokio::test]
async fn a_config_read_of_an_unknown_distribution_is_refused() {
    let router = emulator();
    // Ids no distribution has: one shaped like a CloudFront distribution id, one that is
    // not, and one that belonged to a distribution that was deleted.
    let created = create_distribution(
        &router,
        &Config {
            enabled: false,
            ..config_table()[0].clone()
        }
        .xml(),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "expected the create to be accepted, got {} ({})",
        created.status, created.raw
    );
    let deleted_id = created.root().text_at("Id").unwrap_or_default().to_string();
    let etag = created.header("etag").to_string();
    // The distribution is disabled; once its configuration has propagated it is
    // deletable, which the delete gate suite covers. Here it is only a source of an id
    // that no longer names a distribution.
    tokio::time::sleep(PROPAGATION_DELAY + Duration::from_millis(250)).await;
    let deleted = delete_distribution(&router, &deleted_id, Some(etag.as_str())).await;
    assert_eq!(
        deleted.status, 204,
        "expected the delete to be accepted, got {} ({})",
        deleted.status, deleted.raw
    );

    for (case, id) in [
        ("never-created", "EABCDEFGHIJKLM".to_string()),
        ("not-an-id-shape", "no-such-distribution".to_string()),
        ("deleted", deleted_id),
    ] {
        let read = get_distribution_config(&router, &id).await;
        assert_eq!(
            read.status, 404,
            "case {case}: expected HTTP 404 for an id no distribution has, got {} ({})",
            read.status, read.raw
        );
        assert_eq!(
            read.error_code(),
            "NoSuchDistribution",
            "case {case}: expected NoSuchDistribution, got {:?} ({})",
            read.error_code(),
            read.raw
        );
    }
}
