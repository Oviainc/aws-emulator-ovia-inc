// SPDX-License-Identifier: CC0-1.0

//! The two state-machine rules CloudFront's distribution identity and teardown follow,
//! end to end over HTTP.
//!
//! * A distribution may be DELETED only once its stored configuration has it disabled
//!   AND that configuration has finished propagating (the distribution reports
//!   `Deployed`). Any other state answers `DistributionNotDisabled` (HTTP 409) and leaves
//!   the record exactly as readable as it was — "Before you can delete a distribution,
//!   you must disable it", and the disable is not in force until it is deployed.
//! * A create is refused ONLY when its `CallerReference` is one the store already holds
//!   AND the configuration under it differs from the one that reference already names.
//!   Both branches are what the recorded service does: the corpus fixture
//!   `cfval-caller-reference-reuse-conflict` answers a repeat under an already-used
//!   reference with 201 and the `Distribution` that reference names, and answers the reuse
//!   that changes the configuration with 409 `DistributionAlreadyExists` — the reference
//!   being "associated with ANOTHER distribution", as the error shape puts it. So a fresh
//!   reference creates a distribution whatever the configuration says, the same reference
//!   with the same configuration is the same request again, and only the same reference
//!   with a different configuration is the modelled conflict.
//!
//! The harness (the router, the restXml reader, the request helpers and the
//! `DistributionConfig` builder) is the one in [`crate::distribution_lifecycle`], so
//! these rules are asserted over the same wire path as the rest of the suite.

use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    config, create_distribution, delete_distribution, emulator, get_distribution,
    list_distributions, past_propagation, update_distribution, Answer, Config, API_VERSION, HOST,
};

/// Create one distribution and report `(Id, ETag)`.
async fn create(router: &MultiServiceRouter, row: &Config) -> (String, String) {
    let created = create_distribution(router, &row.xml()).await;
    assert_eq!(
        created.status,
        201,
        "case {}: expected HTTP 201 from CreateDistribution, got {} with {}",
        row.case,
        created.status,
        created.raw
    );
    let id = created
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();
    (id, created.header("etag").to_string())
}

/// Disable (or re-enable) a stored distribution and report the version the update minted.
async fn set_enabled(
    router: &MultiServiceRouter,
    id: &str,
    row: &Config,
    enabled: bool,
    if_match: &str,
) -> String {
    let changed = Config {
        enabled,
        ..row.clone()
    };
    let updated = update_distribution(router, id, &changed.update_xml(), Some(if_match)).await;
    assert_eq!(
        updated.status,
        200,
        "case {}: expected HTTP 200 from UpdateDistribution(Enabled={enabled}), got {} with {}",
        row.case,
        updated.status,
        updated.raw
    );
    assert_eq!(
        updated.root().text_at("DistributionConfig/Enabled"),
        Some(if enabled { "true" } else { "false" }),
        "case {}: expected the update to store Enabled={enabled}, got {:?}",
        row.case,
        updated.root().text_at("DistributionConfig/Enabled")
    );
    updated.header("etag").to_string()
}

/// How a row brings its distribution into the state the delete is issued against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reach {
    /// Created in this state and left alone.
    AsCreated,
    /// Created, deployed, then moved into this state by an update.
    ByUpdate,
}

/// One row of the delete-gate table: a distribution state and what a delete does to it.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    /// `Enabled` in the stored configuration the delete is issued against.
    enabled: bool,
    reach: Reach,
    /// Whether the test waits for the last mutation's propagation before deleting.
    deployed: bool,
    /// The status the delete is expected to answer with.
    status: u16,
}

/// Every combination of the two conditions the gate reads — is the stored configuration
/// disabled, and has it propagated — reached both ways a configuration can be reached,
/// plus the re-enable that takes a deployed distribution back out of the deletable state.
const DELETE_TABLE: &[Row] = &[
    Row {
        case: "created-enabled-propagating",
        enabled: true,
        reach: Reach::AsCreated,
        deployed: false,
        status: 409,
    },
    Row {
        case: "created-enabled-deployed",
        enabled: true,
        reach: Reach::AsCreated,
        deployed: true,
        status: 409,
    },
    Row {
        case: "created-disabled-propagating",
        enabled: false,
        reach: Reach::AsCreated,
        deployed: false,
        status: 409,
    },
    Row {
        case: "created-disabled-deployed",
        enabled: false,
        reach: Reach::AsCreated,
        deployed: true,
        status: 204,
    },
    Row {
        case: "disabled-by-update-propagating",
        enabled: false,
        reach: Reach::ByUpdate,
        deployed: false,
        status: 409,
    },
    Row {
        case: "disabled-by-update-deployed",
        enabled: false,
        reach: Reach::ByUpdate,
        deployed: true,
        status: 204,
    },
    Row {
        case: "re-enabled-by-update-deployed",
        enabled: true,
        reach: Reach::ByUpdate,
        deployed: true,
        status: 409,
    },
];

/// A delete lands only on a distribution whose stored configuration is disabled AND
/// deployed; in every other state it is refused with `DistributionNotDisabled` and the
/// distribution stays exactly as readable as it was.
#[tokio::test]
async fn delete_lands_only_on_a_disabled_and_deployed_distribution() {
    for row in DELETE_TABLE {
        let case = row.case;
        let router = emulator();
        // The distribution each row deletes: created in `enabled` when the row reaches
        // its state at creation, created in the opposite state and moved by an update
        // when it reaches it by update.
        let base = Config {
            enabled: match row.reach {
                Reach::AsCreated => row.enabled,
                Reach::ByUpdate => !row.enabled,
            },
            ..config(case, &format!("ref-{case}"))
        };
        let (id, created_etag) = create(&router, &base).await;

        let etag = match row.reach {
            Reach::AsCreated => created_etag,
            Reach::ByUpdate => {
                // An update is accepted at any status, so the created distribution is
                // deployed first only to keep the row's own wait the only one that
                // matters.
                tokio::time::sleep(past_propagation()).await;
                set_enabled(&router, &id, &base, row.enabled, &created_etag).await
            }
        };
        if row.deployed {
            tokio::time::sleep(past_propagation()).await;
        }

        // The status the gate reads is the one a read reports, so pin it first.
        let before = get_distribution(&router, &id).await;
        let expected_status = if row.deployed {
            "Deployed"
        } else {
            "InProgress"
        };
        assert_eq!(
            before.root().text_at("Status"),
            Some(expected_status),
            "case {case}: expected the distribution to be {expected_status} before the delete, got {:?}",
            before.root().text_at("Status")
        );

        let deleted = delete_distribution(&router, &id, Some(&etag)).await;
        assert_eq!(
            deleted.status, row.status,
            "case {case}: expected HTTP {} from DeleteDistribution, got {} with {}",
            row.status, deleted.status, deleted.raw
        );

        if row.status == 204 {
            assert!(
                deleted.raw.trim().is_empty(),
                "case {case}: expected no body on a successful delete, got {}",
                deleted.raw
            );
            assert_deleted(&router, &id, case).await;
        } else {
            assert_eq!(
                deleted.error_code(),
                "DistributionNotDisabled",
                "case {case}: expected DistributionNotDisabled, got {:?} with {}",
                deleted.error_code(),
                deleted.raw
            );
            assert_refusal_message(&deleted, case);
            // A refused delete destroys nothing: the distribution is still readable, at
            // the same version and with the configuration it held.
            let after = get_distribution(&router, &id).await;
            assert_eq!(
                after.status, 200,
                "case {case}: expected the refused delete to leave the distribution readable, got {} with {}",
                after.status, after.raw
            );
            assert_eq!(
                after.header("etag"),
                etag,
                "case {case}: expected the refused delete to leave the version at {etag}, got {:?}",
                after.header("etag")
            );
            assert_eq!(
                after.root().text_at("DistributionConfig/Enabled"),
                Some(if row.enabled { "true" } else { "false" }),
                "case {case}: expected the stored Enabled to be {}, got {:?}",
                row.enabled,
                after.root().text_at("DistributionConfig/Enabled")
            );
            let listed = list_distributions(&router, "").await;
            assert_eq!(
                listed.root().text_at("Quantity"),
                Some("1"),
                "case {case}: expected the refused delete to leave the distribution listed, got {}",
                listed.raw
            );
        }
    }
}

/// A refused delete carries the message CloudFront answers this error code with: a
/// constant describing the distribution's state, not a sentence built from the request.
fn assert_refusal_message(answer: &Answer, case: &str) {
    let message = answer.root().find_text("Message").unwrap_or_default();
    assert_eq!(
        message, "The distribution you are trying to delete has not been disabled.",
        "case {case}: expected the DistributionNotDisabled message, got {message:?}"
    );
}

/// After a successful delete the distribution is gone from every read.
async fn assert_deleted(router: &MultiServiceRouter, id: &str, case: &str) {
    let fetched = get_distribution(router, id).await;
    assert_eq!(
        fetched.status, 404,
        "case {case}: expected HTTP 404 after the delete, got {} with {}",
        fetched.status, fetched.raw
    );
    assert_eq!(
        fetched.error_code(),
        "NoSuchDistribution",
        "case {case}: expected NoSuchDistribution after the delete, got {:?}",
        fetched.error_code()
    );
    let listed = list_distributions(router, "").await;
    assert_eq!(
        listed.root().text_at("Quantity"),
        Some("0"),
        "case {case}: expected an empty list after the delete, got {}",
        listed.raw
    );
}

/// What a create under one caller reference is expected to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// A fresh reference: a new distribution, with its own identity.
    Created,
    /// A reference the store already holds, carrying the same configuration: the same
    /// request again, answered with the distribution that reference already names.
    Replayed,
    /// A reference the store already holds, carrying a different configuration:
    /// `DistributionAlreadyExists`.
    Conflict,
}

/// One row of the caller-reference table: a configuration to submit and what the
/// reference it carries makes of it.
struct CreateRow {
    case: &'static str,
    /// The configuration submitted, built from the row's own description.
    config: Config,
    expect: Expect,
}

/// A create is refused exactly when its caller reference is one the store already holds
/// AND the configuration under it differs from the one that reference names; a fresh
/// reference always creates, an identical repeat always replays, and a reference is
/// claimed only while the distribution that claimed it is still there.
#[tokio::test]
async fn a_create_is_refused_only_for_a_caller_reference_already_in_use() {
    let router = emulator();
    let first = config("first", "ref-first");
    let (first_id, first_etag) = create(&router, &first).await;
    let first_domain = get_distribution(&router, &first_id)
        .await
        .root()
        .text_at("DomainName")
        .unwrap_or_default()
        .to_string();

    // Every row varies along BOTH halves of the rule: whether the caller reference is
    // one the store holds, and whether the configuration under it is the one that
    // reference already names.
    let rows = vec![
        CreateRow {
            case: "same-config-fresh-reference",
            config: Config {
                caller_reference: "ref-second".to_string(),
                ..first.clone()
            },
            expect: Expect::Created,
        },
        CreateRow {
            case: "identical-request-again",
            config: first.clone(),
            expect: Expect::Replayed,
        },
        CreateRow {
            case: "reused-reference-different-comment",
            config: Config {
                comment: "a different comment".to_string(),
                ..first.clone()
            },
            expect: Expect::Conflict,
        },
        CreateRow {
            case: "reused-reference-different-origins-and-aliases",
            config: Config {
                origins: vec![
                    ("primary".to_string(), "a.example.com".to_string()),
                    ("secondary".to_string(), "b.example.com".to_string()),
                ],
                aliases: vec!["cdn.example.com".to_string()],
                price_class: Some("PriceClass_100"),
                viewer_protocol_policy: "https-only",
                ..first.clone()
            },
            expect: Expect::Conflict,
        },
        CreateRow {
            case: "reused-reference-disabled",
            config: Config {
                enabled: false,
                ..first.clone()
            },
            expect: Expect::Conflict,
        },
        CreateRow {
            case: "fresh-reference-wholly-different-config",
            config: Config {
                caller_reference: "ref-third".to_string(),
                comment: String::new(),
                enabled: false,
                origins: vec![
                    ("one".to_string(), "one.example.com".to_string()),
                    ("two".to_string(), "two.example.com".to_string()),
                    ("three".to_string(), "three.example.com".to_string()),
                ],
                aliases: vec![
                    "one.example.net".to_string(),
                    "two.example.net".to_string(),
                ],
                price_class: Some("PriceClass_All"),
                viewer_protocol_policy: "redirect-to-https",
                ..first.clone()
            },
            expect: Expect::Created,
        },
    ];

    let mut ids = vec![first_id.clone()];
    let mut domains = vec![first_domain];
    for row in &rows {
        let case = row.case;
        let answer = create_distribution(&router, &row.config.xml()).await;
        match row.expect {
            Expect::Created => {
                assert_eq!(
                    answer.status, 201,
                    "case {case}: expected HTTP 201 for a fresh caller reference, got {} with {}",
                    answer.status, answer.raw
                );
                let id = answer
                    .root()
                    .text_at("Id")
                    .unwrap_or_default()
                    .to_string();
                let domain = answer
                    .root()
                    .text_at("DomainName")
                    .unwrap_or_default()
                    .to_string();
                assert!(
                    !ids.contains(&id),
                    "case {case}: expected a distribution of its own, got the id {id} again"
                );
                assert!(
                    !domains.contains(&domain),
                    "case {case}: expected a domain name of its own, got {domain} again"
                );
                // And the new distribution is the one that was submitted, read back
                // through GetDistribution.
                let fetched = get_distribution(&router, &id).await;
                assert_eq!(
                    fetched.status, 200,
                    "case {case}: expected HTTP 200 from GetDistribution, got {} with {}",
                    fetched.status, fetched.raw
                );
                row.config.assert_matches(
                    fetched
                        .root()
                        .child("DistributionConfig")
                        .unwrap_or_else(|| panic!("case {case}: no DistributionConfig")),
                    "GetDistribution after a create under a fresh caller reference",
                );
                ids.push(id);
                domains.push(domain);
            }
            Expect::Replayed => {
                // The same request again: the ORIGINAL distribution, at its current
                // version, and nothing new in the store.
                assert_eq!(
                    answer.status, 201,
                    "case {case}: expected HTTP 201 for the same create again, got {} with {}",
                    answer.status, answer.raw
                );
                assert_eq!(
                    answer.root().text_at("Id"),
                    Some(first_id.as_str()),
                    "case {case}: expected the distribution the first create made ({first_id}), got {:?}",
                    answer.root().text_at("Id")
                );
                assert_eq!(
                    answer.header("etag"),
                    first_etag,
                    "case {case}: expected the original version {first_etag}, got {:?}",
                    answer.header("etag")
                );
                assert_eq!(
                    answer.header("location"),
                    format!("https://{HOST}/{API_VERSION}/distribution/{first_id}"),
                    "case {case}: expected the original distribution's Location, got {:?}",
                    answer.header("location")
                );
                row.config.assert_matches(
                    answer
                        .root()
                        .child("DistributionConfig")
                        .unwrap_or_else(|| panic!("case {case}: no DistributionConfig")),
                    "CreateDistribution repeated verbatim",
                );
            }
            Expect::Conflict => {
                assert_eq!(
                    answer.status, 409,
                    "case {case}: expected HTTP 409 for a caller reference already in use, got {} with {}",
                    answer.status, answer.raw
                );
                assert_eq!(
                    answer.error_code(),
                    "DistributionAlreadyExists",
                    "case {case}: expected DistributionAlreadyExists, got {:?} with {}",
                    answer.error_code(),
                    answer.raw
                );
                assert!(
                    answer.root().text_at("Distribution").is_none(),
                    "case {case}: expected no Distribution payload on a refusal, got {}",
                    answer.raw
                );
            }
        }
    }

    // The store holds the first distribution plus one per fresh reference, and no refused
    // create added anything.
    let listed = list_distributions(&router, "").await;
    assert_eq!(
        listed.root().text_at("Quantity"),
        Some(ids.len().to_string().as_str()),
        "case final-list: expected {} distributions, got {}",
        ids.len(),
        listed.raw
    );

    // "The caller reference you attempted to create the distribution with is associated
    // with ANOTHER distribution": once that distribution is gone, the reference is not
    // associated with anything, so a create may claim it again.
    let disabled_etag = set_enabled(&router, &first_id, &first, false, &first_etag).await;
    tokio::time::sleep(past_propagation()).await;
    let removed = delete_distribution(&router, &first_id, Some(&disabled_etag)).await;
    assert_eq!(
        removed.status, 204,
        "case release-reference: expected HTTP 204, got {} with {}",
        removed.status, removed.raw
    );
    let reclaimed = create_distribution(&router, &first.xml()).await;
    assert_eq!(
        reclaimed.status, 201,
        "case release-reference: expected the freed caller reference to be usable, got {} with {}",
        reclaimed.status, reclaimed.raw
    );
    let reclaimed_id = reclaimed
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();
    assert!(
        !ids.contains(&reclaimed_id),
        "case release-reference: expected a new distribution id, got {reclaimed_id} again"
    );
}
