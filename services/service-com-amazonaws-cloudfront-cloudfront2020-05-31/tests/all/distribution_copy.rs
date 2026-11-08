// SPDX-License-Identifier: CC0-1.0

//! `CopyDistribution` — creating a staging distribution from a primary one, end to end
//! over HTTP.
//!
//! The rule under test: a copy creates a NEW distribution that runs under the PRIMARY's
//! persisted configuration, with exactly the three members the request may supply applied
//! to it — the new distribution's `CallerReference`, its `Enabled` flag (`True` when the
//! request omits it) and `Staging` `true` — and leaves the primary untouched. The copy is
//! a distribution like any other: it gets its own id, ARN, domain name and version token,
//! it starts `InProgress`, it is readable through `GetDistribution` and listable, and it
//! takes part in the caller-reference rule the other distribution creates follow.
//!
//! Every assertion below spans a TABLE of primaries — the configurations in
//! [`crate::distribution_lifecycle::config_table`], which differ in origin count, alias
//! count, enabled flag, viewer protocol policy, comment and whether the optional
//! `PriceClass` is named — crossed with the `Enabled` and `Staging` values a request can
//! carry, so the rule is asserted over the class rather than over one copy.
//!
//! The harness (the router, the restXml reader and the request helpers) is the one in
//! [`crate::distribution_lifecycle`], so these rules go over the same wire path as the
//! rest of the suite.

use std::time::Duration;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::PROPAGATION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call_with_headers, config, config_table, create_distribution, emulator, get_distribution,
    is_distribution_id, is_version_token, list_distributions, update_distribution, Answer, Config,
    ACCOUNT_ID, API_VERSION,
};

/// The `CopyDistributionRequest` body a client sends: the members the model does NOT bind
/// to the URI or to a header (`CallerReference`, `Enabled`), under the root element the
/// API reference names.
fn copy_body(caller_reference: &str, enabled: Option<bool>) -> String {
    let mut out = format!(
        "<CopyDistributionRequest xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{caller_reference}</CallerReference>"
    );
    if let Some(enabled) = enabled {
        out.push_str(&format!("<Enabled>{enabled}</Enabled>"));
    }
    out.push_str("</CopyDistributionRequest>");
    out
}

/// Call `CopyDistribution` on `primary_id`.
///
/// `staging` is the `Staging` HEADER the model binds the member to, and `if_match` the
/// optional `If-Match` one; `None` leaves the header off the request entirely.
pub(crate) async fn copy_distribution(
    router: &MultiServiceRouter,
    primary_id: &str,
    caller_reference: &str,
    enabled: Option<bool>,
    staging: Option<&str>,
    if_match: Option<&str>,
) -> Answer {
    let mut headers: Vec<(&str, &str)> = Vec::new();
    if let Some(staging) = staging {
        headers.push(("staging", staging));
    }
    if let Some(version) = if_match {
        headers.push(("if-match", version));
    }
    call_with_headers(
        router,
        "POST",
        &format!("/{API_VERSION}/distribution/{primary_id}/copy"),
        Some(&copy_body(caller_reference, enabled)),
        &headers,
    )
    .await
}

/// Create one primary distribution and report `(Id, ETag)`.
async fn create_primary(router: &MultiServiceRouter, row: &Config) -> (String, String) {
    let created = create_distribution(router, &row.xml()).await;
    assert_eq!(
        created.status,
        201,
        "case {}: expected HTTP 201 from CreateDistribution, got {} with {}",
        row.case,
        created.status,
        created.raw
    );
    let id = created.root().text_at("Id").unwrap_or_default().to_string();
    (id, created.header("etag").to_string())
}

/// The `Enabled` values a request can carry, with the flag each one leaves on the copy.
/// `None` is the member left out: "If you omit this field, the default value is `True`."
const ENABLED_CASES: [(Option<bool>, bool); 3] =
    [(None, true), (Some(true), true), (Some(false), false)];

/// The `Staging` header values that select a staging copy: the documented true value, and
/// the header left off. A boolean header is deserialized before any handler runs, so
/// `true` is the only spelling of it a request can carry.
const STAGING_CASES: [Option<&str>; 2] = [None, Some("true")];

/// A copy runs under the primary's stored configuration, with the request's
/// `CallerReference`, its `Enabled` flag and `Staging` `true` applied — and nothing else
/// of the request in it. Every member of the copy is read back through
/// `GetDistribution`, and the primary is unchanged by the call.
#[tokio::test]
async fn copy_runs_under_the_primarys_configuration_with_the_requested_members_applied() {
    let router = emulator();
    for row in config_table() {
        let (primary_id, primary_etag) = create_primary(&router, &row).await;
        let primary_before = get_distribution(&router, &primary_id).await;

        for (enabled, expected_enabled) in ENABLED_CASES {
            for staging in STAGING_CASES {
                let case = format!(
                    "{} (Enabled {enabled:?}, Staging {staging:?})",
                    row.case
                );
                let reference = format!("copy-{}-{enabled:?}-{staging:?}", row.case);
                let copied =
                    copy_distribution(&router, &primary_id, &reference, enabled, staging, None)
                        .await;
                assert_eq!(
                    copied.status, 201,
                    "case {case}: expected HTTP 201 from CopyDistribution, got {} with {}",
                    copied.status, copied.raw
                );

                let copy_id = copied.root().text_at("Id").unwrap_or_default().to_string();
                assert!(
                    is_distribution_id(&copy_id),
                    "case {case}: expected the copy's Id to have a distribution id's shape, \
                     got {copy_id:?}"
                );
                assert_ne!(
                    copy_id, primary_id,
                    "case {case}: expected the copy to be a new distribution, got the \
                     primary's id {copy_id:?}"
                );
                let copy_etag = copied.header("etag").to_string();
                assert!(
                    is_version_token(&copy_etag),
                    "case {case}: expected the copy's ETag to be a version token, got \
                     {copy_etag:?}"
                );
                assert_ne!(
                    copy_etag, primary_etag,
                    "case {case}: expected the copy to carry its own version, got the \
                     primary's {copy_etag:?}"
                );
                let expected_location =
                    format!("https://cloudfront.amazonaws.com/{API_VERSION}/distribution/{copy_id}");
                assert_eq!(
                    copied.header("location"),
                    expected_location,
                    "case {case}: expected Location {expected_location:?}, got {:?}",
                    copied.header("location")
                );
                let expected_arn =
                    format!("arn:aws:cloudfront::{ACCOUNT_ID}:distribution/{copy_id}");
                assert_eq!(
                    copied.root().text_at("ARN"),
                    Some(expected_arn.as_str()),
                    "case {case}: expected ARN {expected_arn:?}, got {:?}",
                    copied.root().text_at("ARN")
                );

                // The copy's configuration: the primary's, with the three members this
                // request speaks about applied to it.
                let expected = Config {
                    caller_reference: reference.clone(),
                    enabled: expected_enabled,
                    ..row.clone()
                };
                {
                    let config = copied
                        .root()
                        .at("DistributionConfig")
                        .expect("the answer carries a DistributionConfig");
                    expected.assert_matches(config, "CopyDistribution");
                    assert_eq!(
                        config.text_at("Staging"),
                        Some("true"),
                        "case {case} (CopyDistribution): expected the copy to be a staging \
                         distribution, got Staging {:?}",
                        config.text_at("Staging")
                    );
                }

                // Read back through the implemented reader: the copy is persisted, so the
                // configuration it runs under is the one a later read answers with.
                let read = get_distribution(&router, &copy_id).await;
                assert_eq!(
                    read.status, 200,
                    "case {case}: expected HTTP 200 from GetDistribution on the copy, got \
                     {} with {}",
                    read.status, read.raw
                );
                let read_config = read
                    .root()
                    .at("DistributionConfig")
                    .expect("the read carries a DistributionConfig");
                expected.assert_matches(read_config, "GetDistribution");
                assert_eq!(
                    read_config.text_at("Staging"),
                    Some("true"),
                    "case {case} (GetDistribution): expected Staging true, got {:?}",
                    read_config.text_at("Staging")
                );
                assert_eq!(
                    read.header("etag"),
                    copy_etag,
                    "case {case}: expected GetDistribution to report the version the copy \
                     minted ({copy_etag:?}), got {:?}",
                    read.header("etag")
                );

                // The copy is a distribution CloudFront has just accepted, so the clock —
                // not this call — owes it its deployment.
                assert_eq!(
                    read.root().text_at("Status"),
                    Some("InProgress"),
                    "case {case}: expected the copy to report InProgress, got {:?}",
                    read.root().text_at("Status")
                );
            }
        }

        // Nothing about the primary changed: it keeps its version, its configuration and
        // its own `Staging` flag.
        let primary_after = get_distribution(&router, &primary_id).await;
        assert_eq!(
            primary_after.header("etag"),
            primary_etag,
            "case {}: expected the primary to keep version {primary_etag:?} across the \
             copies, got {:?}",
            row.case,
            primary_after.header("etag")
        );
        assert_eq!(
            primary_after.raw, primary_before.raw,
            "case {}: expected the primary to read back unchanged after the copies, got \
             {}",
            row.case, primary_after.raw
        );
        row.assert_matches(
            primary_after
                .root()
                .at("DistributionConfig")
                .expect("the primary read carries a DistributionConfig"),
            "primary after CopyDistribution",
        );
        assert_eq!(
            primary_after
                .root()
                .at("DistributionConfig")
                .and_then(|config| config.text_at("Staging")),
            Some("false"),
            "case {}: expected the primary to stay a non-staging distribution, got {:?}",
            row.case,
            primary_after
                .root()
                .at("DistributionConfig")
                .and_then(|config| config.text_at("Staging"))
        );
    }
}

/// A copy is listed beside the distribution it was copied from, and the clock owes it the
/// terminal status the same way a create does.
#[tokio::test]
async fn a_copy_is_listed_and_deploys_on_the_clock() {
    let router = emulator();
    let row = config("listed-copy", "ref-listed-copy");
    let (primary_id, _) = create_primary(&router, &row).await;
    let copied = copy_distribution(&router, &primary_id, "copy-of-listed", None, Some("true"), None)
        .await;
    assert_eq!(
        copied.status,
        201,
        "case listed-copy: expected HTTP 201 from CopyDistribution, got {} with {}",
        copied.status,
        copied.raw
    );
    let copy_id = copied.root().text_at("Id").unwrap_or_default().to_string();

    let listed = list_distributions(&router, "").await;
    let ids: Vec<String> = listed
        .root()
        .at("Items")
        .map(|items| {
            items
                .children_named("DistributionSummary")
                .into_iter()
                .map(|summary| summary.text_at("Id").unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        ids.contains(&copy_id) && ids.contains(&primary_id),
        "case listed-copy: expected ListDistributions to carry both {primary_id:?} and \
         {copy_id:?}, got {ids:?}"
    );

    tokio::time::sleep(PROPAGATION_DELAY + Duration::from_millis(250)).await;
    let read = get_distribution(&router, &copy_id).await;
    assert_eq!(
        read.root().text_at("Status"),
        Some("Deployed"),
        "case listed-copy: expected the copy to be Deployed once the clock owes it, got {:?}",
        read.root().text_at("Status")
    );
}

/// A copy of a distribution the store does not hold is the modelled 404, whatever the
/// shape of the id asked for — and nothing is created for it.
#[tokio::test]
async fn copying_a_distribution_that_does_not_exist_is_refused() {
    let router = emulator();
    // Rows differ along the shape of the id: one that could have been minted, one that
    // could not, and one that is the version-token namespace rather than an id.
    let cases: [(&str, &str); 3] = [
        ("well-formed-unknown-id", "E1A2B3C4D5E6F7"),
        ("not-an-id-shape", "no-such-distribution"),
        ("lowercase-body", "eabcdefghijklm"),
    ];
    for (case, primary_id) in cases {
        let answer =
            copy_distribution(&router, primary_id, &format!("copy-{case}"), None, Some("true"), None)
                .await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404 copying {primary_id:?}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchDistribution",
            "case {case}: expected NoSuchDistribution copying {primary_id:?}, got {:?}",
            answer.error_code()
        );
    }
    let listed = list_distributions(&router, "").await;
    assert_eq!(
        listed.root().text_at("Quantity"),
        Some("0"),
        "expected no distribution to exist after the refused copies, got {}",
        listed.raw
    );
}

/// `If-Match` names "the version identifier of the primary distribution whose
/// configuration you are copying": the model leaves the header optional, so a request
/// without one copies the configuration as it stands; a header carrying the current
/// version copies too; one carrying a version that is not current fails the precondition;
/// and a value that is no version token at all is an invalid `If-Match` version.
#[tokio::test]
async fn a_copy_under_if_match_lands_only_on_the_version_the_caller_read() {
    let router = emulator();
    let row = config("if-match-copy", "ref-if-match-copy");
    let (primary_id, first_etag) = create_primary(&router, &row).await;

    // A second version of the primary, so there is a STALE token that was once current.
    let changed = Config {
        comment: "changed comment".to_string(),
        ..row.clone()
    };
    let updated =
        update_distribution(&router, &primary_id, &changed.update_xml(), Some(&first_etag)).await;
    assert_eq!(
        updated.status,
        200,
        "expected HTTP 200 from UpdateDistribution, got {} with {}",
        updated.status,
        updated.raw
    );
    let current_etag = updated.header("etag").to_string();

    // (case, If-Match, expected status, expected error code — "" when the copy lands)
    let cases: [(&str, Option<&str>, u16, &str); 6] = [
        ("absent", None, 201, ""),
        ("current", Some(&current_etag), 201, ""),
        ("current-quoted", Some(&current_etag), 201, ""),
        ("stale", Some(&first_etag), 412, "PreconditionFailed"),
        ("not-a-token", Some("not-a-version"), 400, "InvalidIfMatchVersion"),
        ("too-short", Some("E123"), 400, "InvalidIfMatchVersion"),
    ];
    for (case, if_match, expected_status, expected_code) in cases {
        let quoted = format!("\"{current_etag}\"");
        let header = if case == "current-quoted" {
            Some(quoted.as_str())
        } else {
            if_match
        };
        let answer = copy_distribution(
            &router,
            &primary_id,
            &format!("copy-if-match-{case}"),
            None,
            Some("true"),
            header,
        )
        .await;
        assert_eq!(
            answer.status, expected_status,
            "case {case}: expected HTTP {expected_status} for If-Match {header:?}, got {} \
             with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            expected_code,
            "case {case}: expected error code {expected_code:?} for If-Match {header:?}, \
             got {:?}",
            answer.error_code()
        );
        if expected_status == 201 {
            // The configuration copied is the one the primary now runs under, including
            // the comment the update changed.
            let config = answer
                .root()
                .at("DistributionConfig")
                .expect("the copy carries a DistributionConfig");
            assert_eq!(
                config.text_at("Comment"),
                Some(changed.comment.as_str()),
                "case {case}: expected the copy to carry the primary's current comment \
                 {:?}, got {:?}",
                changed.comment,
                config.text_at("Comment")
            );
        }
    }
}

/// `Staging`'s "only valid value is `True`", so the one value outside that domain is
/// refused as an invalid argument and creates nothing.
#[tokio::test]
async fn a_staging_value_other_than_true_is_refused() {
    let router = emulator();
    let row = config("staging-flag", "ref-staging-flag");
    let (primary_id, _) = create_primary(&router, &row).await;

    // `Staging` is a boolean header, so the values that reach this handler at all are
    // `true` and `false`; the one outside the documented domain is refused here, and a
    // spelling outside the boolean grammar (`False`) never reaches it — the framework
    // refuses it as a modelled constraint before the handler runs.
    for (case, staging, expected_code) in [
        ("false", "false", "InvalidArgument"),
        ("False", "False", "ValidationException"),
    ] {
        let answer = copy_distribution(
            &router,
            &primary_id,
            &format!("copy-staging-{case}"),
            None,
            Some(staging),
            None,
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400 for Staging {staging:?}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            expected_code,
            "case {case}: expected {expected_code} for Staging {staging:?}, got {:?}",
            answer.error_code()
        );
    }

    let listed = list_distributions(&router, "").await;
    assert_eq!(
        listed.root().text_at("Quantity"),
        Some("1"),
        "expected the refused copies to have created nothing, got {}",
        listed.raw
    );
}

/// A copy takes part in the caller-reference rule every distribution create follows:
/// repeating the request under the same reference is the SAME request again and answers
/// with the distribution that reference already names, while the same reference over a
/// different configuration — a different primary — is the modelled conflict.
#[tokio::test]
async fn a_repeated_caller_reference_is_the_same_copy_or_a_conflict() {
    let router = emulator();
    let first_row = config("copy-source-a", "ref-copy-source-a");
    let second_row = Config {
        origins: vec![("other".to_string(), "other.example.com".to_string())],
        ..config("copy-source-b", "ref-copy-source-b")
    };
    let (first_id, _) = create_primary(&router, &first_row).await;
    let (second_id, _) = create_primary(&router, &second_row).await;

    let reference = "copy-shared-reference";
    let first = copy_distribution(&router, &first_id, reference, None, Some("true"), None).await;
    assert_eq!(
        first.status,
        201,
        "expected HTTP 201 from the first copy, got {} with {}",
        first.status,
        first.raw
    );
    let copy_id = first.root().text_at("Id").unwrap_or_default().to_string();

    let repeat = copy_distribution(&router, &first_id, reference, None, Some("true"), None).await;
    assert_eq!(
        repeat.status,
        201,
        "case repeat: expected HTTP 201 repeating the copy, got {} with {}",
        repeat.status,
        repeat.raw
    );
    assert_eq!(
        repeat.root().text_at("Id"),
        Some(copy_id.as_str()),
        "case repeat: expected the repeat to answer with {copy_id:?}, got {:?}",
        repeat.root().text_at("Id")
    );

    let conflict = copy_distribution(&router, &second_id, reference, None, Some("true"), None).await;
    assert_eq!(
        conflict.status,
        409,
        "case other-primary: expected HTTP 409 reusing the reference over another \
         primary, got {} with {}",
        conflict.status,
        conflict.raw
    );
    assert_eq!(
        conflict.error_code(),
        "DistributionAlreadyExists",
        "case other-primary: expected DistributionAlreadyExists, got {:?}",
        conflict.error_code()
    );

    // Three distributions exist: the two primaries and the one copy.
    let listed = list_distributions(&router, "").await;
    assert_eq!(
        listed.root().text_at("Quantity"),
        Some("3"),
        "expected the repeat and the conflict to have created nothing, got {}",
        listed.raw
    );
}
