// SPDX-License-Identifier: CC0-1.0

//! `UpdateDistributionWithStagingConfig` — promoting a staging distribution's
//! configuration onto its primary distribution, end to end over HTTP.
//!
//! The rule under test: a promotion makes the PRIMARY's stored configuration the STAGING
//! distribution's stored configuration, except for the members that belong to the primary
//! itself — its `CallerReference`, its `Aliases`, its `ContinuousDeploymentPolicyId` and
//! its `Staging` flag, which it retains — then disables the continuous deployment policy
//! the primary names, puts the primary back into propagation and hands back a fresh
//! version token, while the staging distribution is left exactly as it was. The two
//! version tokens the request states (`<primary ETag>, <staging ETag>`) are the
//! precondition: each one is checked against the distribution it names.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path
//! a client takes, and each promoted configuration is read back through
//! `GetDistribution`.
//!
//! The rows span what the rule generalises over: how many alternate domain names the
//! primary retains (none, one, two), how the staging configuration differs from the
//! primary's (origin count and ids, comment, viewer protocol policy, whether the optional
//! `PriceClass` is named at all, and whether it is enabled), whether the request names the
//! staging distribution or leaves the member out, and every shape of the version list a
//! request can state.
//!
//! The harness (the router, the restXml reader, the distribution helpers, the copy call
//! and the policy calls) is the one the rest of the suite uses, reused rather than
//! copied.

use smithy_server_runtime::MultiServiceRouter;

use crate::continuous_deployment_policies::{create_policy, get_policy};
use crate::distribution_copy::copy_distribution;
use crate::distribution_lifecycle::{
    call_with_headers, config, create_distribution, emulator, get_distribution, is_version_token,
    update_distribution, Answer, Config, API_VERSION,
};

// ---------------------------------------------------------------------------
// Calling the promotion
// ---------------------------------------------------------------------------

/// `PUT /2020-05-31/distribution/{Id}/promote-staging-config`, from the operation's `http`
/// binding.
///
/// `staging_id` is the `StagingDistributionId` QUERY parameter the model binds the member
/// to, and `if_match` the `If-Match` header; `None` leaves each off the request entirely.
/// The operation binds no payload, so the request carries no body.
async fn promote(
    router: &MultiServiceRouter,
    primary_id: &str,
    staging_id: Option<&str>,
    if_match: Option<&str>,
) -> Answer {
    let query = staging_id
        .map(|id| format!("?StagingDistributionId={id}"))
        .unwrap_or_default();
    let headers: Vec<(&str, &str)> = if_match
        .map(|version| vec![("if-match", version)])
        .unwrap_or_default();
    call_with_headers(
        router,
        "PUT",
        &format!("/{API_VERSION}/distribution/{primary_id}/promote-staging-config{query}"),
        None,
        &headers,
    )
    .await
}

/// The `DistributionConfig` document for `row` with `extra` appended as its last members.
///
/// `ContinuousDeploymentPolicyId` and `Staging` are the last two members
/// `DistributionConfig` declares (`smithy_query(query="shape",shape="DistributionConfig")`),
/// so appending them to the row's document states them in the order the model declares.
fn with_members(row: &Config, extra: &str) -> String {
    row.update_xml()
        .replace("</DistributionConfig>", &format!("{extra}</DistributionConfig>"))
}

/// A `ContinuousDeploymentPolicyConfig` routing traffic to `dns_name`.
///
/// The traffic configuration is the single-header one, whose `Header` "must begin with
/// `aws-cf-cd-`" (`SingleHeaderConfig$Header`).
fn policy_xml(dns_name: &str, enabled: bool) -> String {
    format!(
        "<ContinuousDeploymentPolicyConfig \
          xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <StagingDistributionDnsNames><Quantity>1</Quantity>\
         <Items><DnsName>{dns_name}</DnsName></Items></StagingDistributionDnsNames>\
         <Enabled>{enabled}</Enabled>\
         <TrafficConfig><SingleHeaderConfig><Header>aws-cf-cd-staging</Header>\
         <Value>true</Value></SingleHeaderConfig><Type>SingleHeader</Type></TrafficConfig>\
         </ContinuousDeploymentPolicyConfig>"
    )
}

// ---------------------------------------------------------------------------
// The continuous deployment workflow a promotion ends
// ---------------------------------------------------------------------------

/// The resources the promotion reads, and the version tokens each one currently carries.
#[derive(Debug)]
struct Deployment {
    primary_id: String,
    /// The primary's current version — the first token of the request's `If-Match`.
    primary_etag: String,
    /// A version the primary carried BEFORE it was paired with the policy: a token that
    /// was once current and no longer is.
    primary_stale_etag: String,
    staging_id: String,
    /// The staging distribution's current version — the second token of the `If-Match`.
    staging_etag: String,
    /// A version the staging distribution carried before the staged configuration landed.
    staging_stale_etag: String,
    /// The staging distribution's edge domain name, which is what the policy routes to.
    staging_domain: String,
    policy_id: String,
    policy_etag: String,
}

/// Arrange the workflow a promotion ends: a primary distribution, a staging copy of it
/// carrying `staging` as its tested configuration, a continuous deployment policy routing
/// traffic to that staging distribution, and the primary updated to name the policy.
async fn arrange(router: &MultiServiceRouter, primary: &Config, staging: &Config) -> Deployment {
    let case = primary.case;

    let created = create_distribution(router, &primary.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {case}: expected HTTP 201 from CreateDistribution, got {} with {}",
        created.status, created.raw
    );
    let primary_id = created.root().text_at("Id").unwrap_or_default().to_string();
    let primary_stale_etag = created.header("etag").to_string();

    // The staging distribution is made the way the workflow makes one: a copy of the
    // primary.
    let copied = copy_distribution(
        router,
        &primary_id,
        &staging.caller_reference,
        None,
        Some("true"),
        None,
    )
    .await;
    assert_eq!(
        copied.status, 201,
        "case {case}: expected HTTP 201 from CopyDistribution, got {} with {}",
        copied.status, copied.raw
    );
    let staging_id = copied.root().text_at("Id").unwrap_or_default().to_string();
    let staging_domain = copied
        .root()
        .text_at("DomainName")
        .unwrap_or_default()
        .to_string();
    let staging_stale_etag = copied.header("etag").to_string();

    // The configuration under test lands on the staging distribution — the changes a
    // continuous deployment tests there before they are promoted. `Staging` stays `true`:
    // this update changes the configuration, not the kind of distribution it is.
    let staged = update_distribution(
        router,
        &staging_id,
        &with_members(staging, "<Staging>true</Staging>"),
        Some(&staging_stale_etag),
    )
    .await;
    assert_eq!(
        staged.status, 200,
        "case {case}: expected HTTP 200 staging the configuration, got {} with {}",
        staged.status, staged.raw
    );
    let staging_etag = staged.header("etag").to_string();

    let policy = create_policy(router, &policy_xml(&staging_domain, true)).await;
    assert_eq!(
        policy.status, 201,
        "case {case}: expected HTTP 201 from CreateContinuousDeploymentPolicy, got {} with {}",
        policy.status, policy.raw
    );
    let policy_id = policy.root().text_at("Id").unwrap_or_default().to_string();
    let policy_etag = policy.header("etag").to_string();

    // The primary names the policy, which is what pairs it with the staging distribution.
    let attached = update_distribution(
        router,
        &primary_id,
        &with_members(
            primary,
            &format!("<ContinuousDeploymentPolicyId>{policy_id}</ContinuousDeploymentPolicyId>"),
        ),
        Some(&primary_stale_etag),
    )
    .await;
    assert_eq!(
        attached.status, 200,
        "case {case}: expected HTTP 200 attaching the policy, got {} with {}",
        attached.status, attached.raw
    );
    let primary_etag = attached.header("etag").to_string();

    Deployment {
        primary_id,
        primary_etag,
        primary_stale_etag,
        staging_id,
        staging_etag,
        staging_stale_etag,
        staging_domain,
        policy_id,
        policy_etag,
    }
}

/// The version list a request states for `deployment`: `<primary ETag>, <staging ETag>`.
fn version_pair(deployment: &Deployment) -> String {
    format!("{}, {}", deployment.primary_etag, deployment.staging_etag)
}

/// The configuration the primary runs under after promoting `staging`: the staging
/// distribution's, with the primary's own identity retained.
fn promoted_config(primary: &Config, staging: &Config) -> Config {
    Config {
        caller_reference: primary.caller_reference.clone(),
        aliases: primary.aliases.clone(),
        ..staging.clone()
    }
}

/// The (primary, staging) configuration pairs every rule below is asserted over.
///
/// The rows differ along what the rule generalises over: how many alternate domain names
/// the primary retains, how many origins and under which ids the staging configuration
/// names, its comment (including the empty one), its viewer protocol policy, whether it
/// names the optional `PriceClass` at all, and whether the configuration being promoted
/// is enabled.
fn promotion_table() -> Vec<(Config, Config)> {
    vec![
        (
            config("no-aliases", "ref-promote-primary-a"),
            Config {
                origins: vec![
                    ("staged-one".to_string(), "staged-a.example.com".to_string()),
                    ("staged-two".to_string(), "staged-b.example.com".to_string()),
                ],
                comment: "staged two origins".to_string(),
                viewer_protocol_policy: "https-only",
                price_class: Some("PriceClass_100"),
                ..config("no-aliases", "ref-promote-staging-a")
            },
        ),
        (
            Config {
                aliases: vec!["primary-one.example.com".to_string()],
                ..config("one-alias-retained", "ref-promote-primary-b")
            },
            Config {
                aliases: vec!["staging-one.example.com".to_string()],
                comment: "staged behind one alias".to_string(),
                viewer_protocol_policy: "redirect-to-https",
                ..config("one-alias-retained", "ref-promote-staging-b")
            },
        ),
        (
            Config {
                aliases: vec![
                    "primary-two.example.com".to_string(),
                    "primary-three.example.com".to_string(),
                ],
                price_class: Some("PriceClass_200"),
                ..config("two-aliases-disabled-staging", "ref-promote-primary-c")
            },
            Config {
                enabled: false,
                aliases: vec!["staging-two.example.com".to_string()],
                origins: vec![
                    ("staged-x".to_string(), "x.example.com".to_string()),
                    ("staged-y".to_string(), "y.example.com".to_string()),
                    ("staged-z".to_string(), "z.example.com".to_string()),
                ],
                comment: "staged and disabled".to_string(),
                price_class: Some("PriceClass_All"),
                ..config("two-aliases-disabled-staging", "ref-promote-staging-c")
            },
        ),
        (
            Config {
                price_class: Some("PriceClass_200"),
                comment: "primary names a price class".to_string(),
                ..config("empty-comment-unnamed-price-class", "ref-promote-primary-d")
            },
            Config {
                comment: String::new(),
                price_class: None,
                ..config("empty-comment-unnamed-price-class", "ref-promote-staging-d")
            },
        ),
    ]
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// The promoted configuration is the staging distribution's, the primary keeps its own
/// identity, the staging distribution is untouched, and the policy that routed traffic to
/// it is left disabled.
#[tokio::test]
async fn a_promotion_copies_the_staging_configuration_and_retains_the_primarys_identity() {
    let router = emulator();
    for (primary, staging) in promotion_table() {
        let case = primary.case;
        let deployment = arrange(&router, &primary, &staging).await;
        let expected = promoted_config(&primary, &staging);

        let promoted = promote(
            &router,
            &deployment.primary_id,
            Some(&deployment.staging_id),
            Some(&version_pair(&deployment)),
        )
        .await;
        assert_eq!(
            promoted.status, 200,
            "case {case}: expected HTTP 200 from UpdateDistributionWithStagingConfig, got {} \
             with {}",
            promoted.status, promoted.raw
        );
        assert_eq!(
            promoted.root().text_at("Id"),
            Some(deployment.primary_id.as_str()),
            "case {case}: expected the answer to report the primary {:?}, got {:?}",
            deployment.primary_id,
            promoted.root().text_at("Id")
        );
        let promoted_etag = promoted.header("etag").to_string();
        assert!(
            is_version_token(&promoted_etag),
            "case {case}: expected a version token in the ETag header, got {promoted_etag:?}"
        );
        assert_ne!(
            promoted_etag, deployment.primary_etag,
            "case {case}: expected the promotion to mint a fresh version, got the one it \
             matched on {promoted_etag:?}"
        );
        assert_ne!(
            promoted_etag, deployment.staging_etag,
            "case {case}: expected the primary's own fresh version, got the staging \
             distribution's {promoted_etag:?}"
        );
        // The configuration the primary now runs under has to reach the edge locations,
        // so the clock — not this call — owes it its deployment.
        assert_eq!(
            promoted.root().text_at("Status"),
            Some("InProgress"),
            "case {case}: expected the promoted primary to report InProgress, got {:?}",
            promoted.root().text_at("Status")
        );

        for (context, answer) in [
            ("UpdateDistributionWithStagingConfig", &promoted),
            (
                "GetDistribution",
                &get_distribution(&router, &deployment.primary_id).await,
            ),
        ] {
            let reported = answer
                .root()
                .at("DistributionConfig")
                .unwrap_or_else(|| panic!("case {case} ({context}): no DistributionConfig"));
            expected.assert_matches(reported, context);
            assert_eq!(
                reported.text_at("Staging"),
                Some("false"),
                "case {case} ({context}): expected the primary to stay a non-staging \
                 distribution, got {:?}",
                reported.text_at("Staging")
            );
            assert_eq!(
                reported.text_at("ContinuousDeploymentPolicyId"),
                Some(deployment.policy_id.as_str()),
                "case {case} ({context}): expected the primary to retain policy {:?}, got {:?}",
                deployment.policy_id,
                reported.text_at("ContinuousDeploymentPolicyId")
            );
        }

        // The version the promotion minted is the one a later read reports.
        let read = get_distribution(&router, &deployment.primary_id).await;
        assert_eq!(
            read.header("etag"),
            promoted_etag,
            "case {case}: expected GetDistribution to report {promoted_etag:?}, got {:?}",
            read.header("etag")
        );

        // The staging distribution is the SOURCE of the configuration: the promotion
        // neither rewrote nor re-versioned it.
        let staging_read = get_distribution(&router, &deployment.staging_id).await;
        assert_eq!(
            staging_read.header("etag"),
            deployment.staging_etag,
            "case {case}: expected the staging distribution to keep version {:?}, got {:?}",
            deployment.staging_etag,
            staging_read.header("etag")
        );
        let staging_reported = staging_read
            .root()
            .at("DistributionConfig")
            .unwrap_or_else(|| panic!("case {case}: the staging read carries no config"));
        staging.assert_matches(staging_reported, "the staging distribution after promotion");
        assert_eq!(
            staging_reported.text_at("Staging"),
            Some("true"),
            "case {case}: expected the staging distribution to stay one, got {:?}",
            staging_reported.text_at("Staging")
        );

        // "This action will disable the continuous deployment policy": the policy keeps
        // the staging domain it routes to and its traffic configuration, and only its
        // `Enabled` flag changed — which is a change, so it carries a new version.
        let policy = get_policy(&router, &deployment.policy_id).await;
        assert_eq!(
            policy.status, 200,
            "case {case}: expected HTTP 200 reading the policy, got {} with {}",
            policy.status, policy.raw
        );
        let policy_config = policy
            .root()
            .at("ContinuousDeploymentPolicyConfig")
            .unwrap_or_else(|| panic!("case {case}: the policy read carries no config"));
        assert_eq!(
            policy_config.text_at("Enabled"),
            Some("false"),
            "case {case}: expected the promotion to disable the policy, got Enabled {:?}",
            policy_config.text_at("Enabled")
        );
        assert_eq!(
            policy_config.text_at("StagingDistributionDnsNames/Items/DnsName"),
            Some(deployment.staging_domain.as_str()),
            "case {case}: expected the policy to keep routing to {:?}, got {:?}",
            deployment.staging_domain,
            policy_config.text_at("StagingDistributionDnsNames/Items/DnsName")
        );
        assert_eq!(
            policy_config.text_at("TrafficConfig/Type"),
            Some("SingleHeader"),
            "case {case}: expected the policy to keep its traffic configuration, got {:?}",
            policy_config.text_at("TrafficConfig/Type")
        );
        assert_ne!(
            policy.header("etag"),
            deployment.policy_etag,
            "case {case}: expected the disabled policy to carry a fresh version, got {:?}",
            policy.header("etag")
        );
    }
}

/// `StagingDistributionId` is optional: a request that leaves it out promotes the staging
/// distribution the primary is already paired with — the one its continuous deployment
/// policy routes traffic to.
#[tokio::test]
async fn a_request_naming_no_staging_distribution_promotes_the_one_the_policy_routes_to() {
    let router = emulator();
    let (primary, staging) = promotion_table()
        .into_iter()
        .next()
        .expect("the table has rows");
    let deployment = arrange(&router, &primary, &staging).await;
    let case = "staging-distribution-id-omitted";

    let promoted = promote(
        &router,
        &deployment.primary_id,
        None,
        Some(&version_pair(&deployment)),
    )
    .await;
    assert_eq!(
        promoted.status, 200,
        "case {case}: expected HTTP 200 promoting without StagingDistributionId, got {} with {}",
        promoted.status, promoted.raw
    );
    promoted_config(&primary, &staging).assert_matches(
        promoted
            .root()
            .at("DistributionConfig")
            .unwrap_or_else(|| panic!("case {case}: no DistributionConfig")),
        "the policy's staging distribution",
    );

    // A primary paired with nothing, asked to promote nothing, names no configuration to
    // copy at all.
    let lonely = config("no-pairing", "ref-promote-unpaired");
    let created = create_distribution(&router, &lonely.xml()).await;
    assert_eq!(
        created.status, 201,
        "case no-pairing: expected HTTP 201 from CreateDistribution, got {} with {}",
        created.status, created.raw
    );
    let lonely_id = created.root().text_at("Id").unwrap_or_default().to_string();
    let answer = promote(&router, &lonely_id, None, Some(created.header("etag"))).await;
    assert_eq!(
        answer.status, 400,
        "case no-pairing: expected HTTP 400 with no staging distribution to promote, got {} \
         with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        answer.error_code(),
        "InvalidArgument",
        "case no-pairing: expected InvalidArgument, got {:?}",
        answer.error_code()
    );
    let read = get_distribution(&router, &lonely_id).await;
    assert_eq!(
        read.header("etag"),
        created.header("etag"),
        "case no-pairing: the refused promotion must have written nothing, got version {:?}",
        read.header("etag")
    );
}

/// The `If-Match` header states "the current versions (ETag values) of both primary and
/// staging distributions … `<primary ETag>, <staging ETag>`": a value this operation
/// cannot read as that list is an invalid `If-Match` version, a well-formed list naming a
/// version that is not current fails the precondition, and only the current pair promotes.
/// Every refusal writes nothing.
#[tokio::test]
async fn a_promotion_lands_only_on_the_versions_the_caller_read() {
    let router = emulator();
    let (primary, staging) = promotion_table()
        .into_iter()
        .next()
        .expect("the table has rows");
    let deployment = arrange(&router, &primary, &staging).await;
    let current = version_pair(&deployment);
    let primary_etag = deployment.primary_etag.clone();
    let staging_etag = deployment.staging_etag.clone();

    // (case, If-Match, expected status, expected error code — "" when it promotes)
    let cases: [(&str, Option<String>, u16, &str); 9] = [
        ("absent", None, 400, "InvalidIfMatchVersion"),
        ("no-token-at-all", Some(",".to_string()), 400, "InvalidIfMatchVersion"),
        // The header states the versions of BOTH distributions, so one token is not the
        // version list this operation reads.
        (
            "one-version-only",
            Some(primary_etag.clone()),
            400,
            "InvalidIfMatchVersion",
        ),
        (
            "not-a-token",
            Some(format!("{primary_etag}, not-a-version")),
            400,
            "InvalidIfMatchVersion",
        ),
        (
            "three-versions",
            Some(format!("{primary_etag}, {staging_etag}, {primary_etag}")),
            400,
            "InvalidIfMatchVersion",
        ),
        (
            "stale-primary",
            Some(format!(
                "{}, {staging_etag}",
                deployment.primary_stale_etag
            )),
            412,
            "PreconditionFailed",
        ),
        (
            "stale-staging",
            Some(format!(
                "{primary_etag}, {}",
                deployment.staging_stale_etag
            )),
            412,
            "PreconditionFailed",
        ),
        (
            "reversed-order",
            Some(format!("{staging_etag}, {primary_etag}")),
            412,
            "PreconditionFailed",
        ),
        // HTTP entity tags may be quoted, and the current pair promotes.
        (
            "current-quoted",
            Some(format!("\"{primary_etag}\", \"{staging_etag}\"")),
            200,
            "",
        ),
    ];

    for (case, if_match, expected_status, expected_code) in cases {
        let answer = promote(
            &router,
            &deployment.primary_id,
            Some(&deployment.staging_id),
            if_match.as_deref(),
        )
        .await;
        assert_eq!(
            answer.status, expected_status,
            "case {case}: expected HTTP {expected_status} for If-Match {if_match:?}, got {} \
             with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            expected_code,
            "case {case}: expected error code {expected_code:?} for If-Match {if_match:?}, \
             got {:?}",
            answer.error_code()
        );

        let read = get_distribution(&router, &deployment.primary_id).await;
        let comment = read
            .root()
            .at("DistributionConfig")
            .and_then(|config| config.text_at("Comment"))
            .unwrap_or_default()
            .to_string();
        if expected_status == 200 {
            assert_eq!(
                read.header("etag"),
                answer.header("etag"),
                "case {case}: expected the promotion's version {:?} to be the stored one, got \
                 {:?}",
                answer.header("etag"),
                read.header("etag")
            );
            assert_eq!(
                comment, staging.comment,
                "case {case}: expected the staged comment {:?} after the promotion, got \
                 {comment:?}",
                staging.comment
            );
        } else {
            assert_eq!(
                read.header("etag"),
                current.split(',').next().unwrap_or_default().trim(),
                "case {case}: the refused promotion must leave the primary's version alone, \
                 got {:?}",
                read.header("etag")
            );
            assert_eq!(
                comment, primary.comment,
                "case {case}: the refused promotion must leave the primary's configuration \
                 alone, got comment {comment:?}"
            );
        }
    }
}

/// A promotion names two distributions, and each one has to be there and be what the
/// operation reads it as: a distribution the store does not hold is the modelled 404, and
/// one that is not a staging distribution has no staging configuration to promote.
#[tokio::test]
async fn a_promotion_of_a_distribution_that_is_not_there_or_is_not_staging_is_refused() {
    let router = emulator();
    let (primary, staging) = promotion_table()
        .into_iter()
        .next()
        .expect("the table has rows");
    let deployment = arrange(&router, &primary, &staging).await;
    let current = version_pair(&deployment);

    // Rows differ along the shape of the id asked for: one that could have been minted,
    // one that could not, and one from the version-token namespace rather than the id one.
    for (case, unknown_id) in [
        ("well-formed-unknown-id", "E1A2B3C4D5E6F7"),
        ("not-an-id-shape", "no-such-distribution"),
        ("lowercase-body", "eabcdefghijklm"),
    ] {
        let as_primary = promote(
            &router,
            unknown_id,
            Some(&deployment.staging_id),
            Some(&current),
        )
        .await;
        assert_eq!(
            as_primary.status, 404,
            "case {case} (unknown primary): expected HTTP 404, got {} with {}",
            as_primary.status, as_primary.raw
        );
        assert_eq!(
            as_primary.error_code(),
            "NoSuchDistribution",
            "case {case} (unknown primary): expected NoSuchDistribution, got {:?}",
            as_primary.error_code()
        );

        let as_staging = promote(
            &router,
            &deployment.primary_id,
            Some(unknown_id),
            Some(&current),
        )
        .await;
        assert_eq!(
            as_staging.status, 404,
            "case {case} (unknown staging): expected HTTP 404, got {} with {}",
            as_staging.status, as_staging.raw
        );
        assert_eq!(
            as_staging.error_code(),
            "NoSuchDistribution",
            "case {case} (unknown staging): expected NoSuchDistribution, got {:?}",
            as_staging.error_code()
        );
    }

    // The primary itself is a distribution, and not a staging one.
    let not_staging = promote(
        &router,
        &deployment.primary_id,
        Some(&deployment.primary_id),
        Some(&current),
    )
    .await;
    assert_eq!(
        not_staging.status, 400,
        "case primary-as-staging: expected HTTP 400 promoting a non-staging distribution, got \
         {} with {}",
        not_staging.status, not_staging.raw
    );
    assert_eq!(
        not_staging.error_code(),
        "InvalidArgument",
        "case primary-as-staging: expected InvalidArgument, got {:?}",
        not_staging.error_code()
    );

    // None of the refusals wrote anything: the primary still carries the version and the
    // configuration it had.
    let read = get_distribution(&router, &deployment.primary_id).await;
    assert_eq!(
        read.header("etag"),
        deployment.primary_etag,
        "expected the primary to keep version {:?} across the refusals, got {:?}",
        deployment.primary_etag,
        read.header("etag")
    );
    primary.assert_matches(
        read.root()
            .at("DistributionConfig")
            .expect("the read carries a DistributionConfig"),
        "the primary after the refusals",
    );
}

/// The promoted configuration is held to the rule every stored configuration is: the
/// primary keeps its alternate domain names and takes the staging distribution's
/// certificate, so a staging configuration that names no custom certificate cannot be
/// promoted onto a primary that states an alias.
#[tokio::test]
async fn a_promotion_that_would_leave_an_alias_uncovered_is_refused() {
    let router = emulator();
    let primary = Config {
        aliases: vec!["covered.example.com".to_string()],
        ..config("alias-without-certificate", "ref-promote-primary-cert")
    };
    // No alias, so this configuration names no custom certificate and runs under
    // CloudFront's own, which validates no name of the caller's.
    let staging = Config {
        comment: "staged without a certificate".to_string(),
        ..config("alias-without-certificate", "ref-promote-staging-cert")
    };
    let deployment = arrange(&router, &primary, &staging).await;

    let answer = promote(
        &router,
        &deployment.primary_id,
        Some(&deployment.staging_id),
        Some(&version_pair(&deployment)),
    )
    .await;
    assert_eq!(
        answer.status, 400,
        "case alias-without-certificate: expected HTTP 400, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        answer.error_code(),
        "InvalidViewerCertificate",
        "case alias-without-certificate: expected InvalidViewerCertificate, got {:?}",
        answer.error_code()
    );

    let read = get_distribution(&router, &deployment.primary_id).await;
    assert_eq!(
        read.header("etag"),
        deployment.primary_etag,
        "case alias-without-certificate: the refused promotion must have written nothing, got \
         version {:?}",
        read.header("etag")
    );
    primary.assert_matches(
        read.root()
            .at("DistributionConfig")
            .expect("the read carries a DistributionConfig"),
        "the primary after the refusal",
    );
}
