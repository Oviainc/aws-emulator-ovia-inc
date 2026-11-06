// SPDX-License-Identifier: CC0-1.0

//! What `EnableBaseline` and `UpdateEnabledBaseline` ACCEPT, end to end over HTTP.
//!
//! Drives the calls through the same
//! [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter) the production
//! emulator dispatches with, so what these tests exercise is the wire behaviour a client
//! sees — status codes, error codes and JSON bodies — not the handlers' Rust signatures.
//!
//! The rule under test, asserted over TABLES of inputs rather than one case: an
//! enablement is accepted only when it names a baseline of the service's catalog, at a
//! version that baseline offers, on an AWS Organizations organizational unit, configured
//! only by parameters that baseline declares. Every other request is refused — 404
//! `ResourceNotFoundException` for a baseline ARN naming no catalog baseline, 400
//! `ValidationException` for everything else — and persists nothing, so neither the store
//! nor the enablement quota is touched by a refusal.
//!
//! The baselines, the versions each offers and the parameter keys each declares are read
//! off `state::BASELINE_CATALOG` — the catalog `ListBaselines`/`GetBaseline` publish — so
//! every case covers entries beyond the one an issue happened to cite, and no row is
//! written out by hand against a value this emulator could drift from.

use service_com_amazonaws_controltower_awscontroltowerapis::state::{
    CatalogBaseline, BASELINE_CATALOG,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::landing_zone_lifecycle::{call, past_the_settle_span, set_up_emulator, Answer};

/// The ARN this endpoint serves a catalog baseline under, as `GetBaseline` reports it.
fn baseline_arn(baseline_id: &str) -> String {
    format!("arn:aws:controltower:us-west-2::baseline/{baseline_id}")
}

/// An organizational unit ARN, in the form the AWS Organizations documentation publishes.
/// `suffix` keeps the rows of one case on distinct targets.
///
/// Shared with `crate::enabled_baseline_update_validation`, which asserts the same
/// accepting rule from the update side, so the two suites cannot disagree about the forms
/// they drive.
pub(crate) fn ou(suffix: &str) -> String {
    format!("arn:aws:organizations::123456789012:ou/o-exampleorg/ou-vl11-{suffix}")
}

/// POST `/enable-baseline`, omitting `parameters` when the caller does.
pub(crate) async fn enable(
    router: &MultiServiceRouter,
    baseline_identifier: &str,
    baseline_version: &str,
    target_identifier: &str,
    parameters: Option<serde_json::Value>,
) -> Answer {
    let mut body = serde_json::json!({
        "baselineIdentifier": baseline_identifier,
        "baselineVersion": baseline_version,
        "targetIdentifier": target_identifier,
    });
    if let Some(parameters) = parameters {
        body["parameters"] = parameters;
    }
    call(router, "/enable-baseline", body).await
}

/// POST `/update-enabled-baseline`, omitting `parameters` when the caller does.
pub(crate) async fn update(
    router: &MultiServiceRouter,
    enabled_baseline_identifier: &str,
    baseline_version: &str,
    parameters: Option<serde_json::Value>,
) -> Answer {
    let mut body = serde_json::json!({
        "enabledBaselineIdentifier": enabled_baseline_identifier,
        "baselineVersion": baseline_version,
    });
    if let Some(parameters) = parameters {
        body["parameters"] = parameters;
    }
    call(router, "/update-enabled-baseline", body).await
}

/// POST `/get-enabled-baseline` and hand back its `enabledBaselineDetails` member.
pub(crate) async fn enabled_baseline_details(
    router: &MultiServiceRouter,
    identifier: &str,
) -> (Answer, serde_json::Value) {
    let answer = call(
        router,
        "/get-enabled-baseline",
        serde_json::json!({ "enabledBaselineIdentifier": identifier }),
    )
    .await;
    let details = answer.get("enabledBaselineDetails").clone();
    (answer, details)
}

/// A member of a JSON object, as a `&str`.
pub(crate) fn member<'a>(value: &'a serde_json::Value, name: &str) -> Option<&'a str> {
    value.get(name).and_then(serde_json::Value::as_str)
}

/// The applied `parameters` of an `EnabledBaselineDetails`, as a list — an absent member
/// reads as the empty list, which is what "no parameters are applied" means.
pub(crate) fn applied_parameters(details: &serde_json::Value) -> serde_json::Value {
    details
        .get("parameters")
        .cloned()
        .unwrap_or_else(|| serde_json::json!([]))
}

/// Assert that `refused` is the status and error code the rule names, and that it minted
/// nothing: a refusal reports neither an `EnabledBaseline` ARN nor an operation.
pub(crate) fn assert_refused(case: &str, refused: &Answer, status: u16, code: &str) {
    assert_eq!(
        (refused.status, refused.error_code.as_deref()),
        (status, Some(code)),
        "case {case:?}: expected {status} {code}, got {:?} body {:?}",
        (refused.status, refused.error_code.as_deref()),
        refused.body
    );
    assert!(
        refused.body.get("arn").is_none() && refused.body.get("operationIdentifier").is_none(),
        "case {case:?}: a refused enablement must mint nothing, got {:?}",
        refused.body
    );
}

/// Every version any catalog baseline offers, so a row can ask one baseline for a version
/// only another one has.
pub(crate) fn every_offered_version() -> Vec<&'static str> {
    let mut versions: Vec<&'static str> = BASELINE_CATALOG
        .iter()
        .flat_map(|baseline| baseline.versions.iter().copied())
        .collect();
    versions.sort_unstable();
    versions.dedup();
    versions
}

/// A 16-character baseline resource id the catalog does NOT hold, built from `seed` by
/// replacing its last character — so the rows track the catalog rather than hard-coding a
/// value that could one day become a real entry.
fn uncatalogued_id(seed: &str) -> String {
    let replacement = if seed.ends_with('X') { 'Y' } else { 'X' };
    let mut id: String = seed.chars().take(15).collect();
    id.push(replacement);
    assert!(
        !BASELINE_CATALOG.iter().any(|baseline| baseline.id == id),
        "the uncatalogued id {id:?} must not be a catalog entry"
    );
    id
}

/// RULE (the accepting side): every baseline the catalog publishes enables at every
/// version it offers, with the parameters it declares, and what was accepted is what a
/// later read reports.
#[tokio::test]
async fn every_catalog_baseline_enables_at_every_version_it_offers() {
    let router = set_up_emulator().await;

    // One row per baseline/version pair, each with the parameters that baseline declares
    // (an empty list for a baseline that declares none), on its own target.
    let mut accepted: Vec<(String, String, String, String, serde_json::Value)> = Vec::new();
    for (baseline_index, baseline) in BASELINE_CATALOG.iter().enumerate() {
        for (version_index, version) in baseline.versions.iter().enumerate() {
            let case = format!("{} at {version}", baseline.name);
            let identifier = baseline_arn(baseline.id);
            let target = ou(&format!("b{baseline_index}v{version_index}"));
            let parameters = serde_json::Value::Array(
                baseline
                    .parameters
                    .iter()
                    .map(|key| serde_json::json!({ "key": key, "value": true }))
                    .collect(),
            );

            let answer = enable(
                &router,
                &identifier,
                version,
                &target,
                Some(parameters.clone()),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} body {:?}",
                answer.status, answer.body
            );
            accepted.push((case, answer.string("arn"), identifier, target, parameters));
        }
    }
    assert!(
        accepted.len() >= BASELINE_CATALOG.len(),
        "expected at least one row per catalog baseline, got {}",
        accepted.len()
    );

    // The enables are asynchronous; the read below is about what was PERSISTED, so wait
    // for the clock to make them due first.
    tokio::time::sleep(past_the_settle_span()).await;

    for (case, arn, identifier, target, parameters) in accepted {
        let (read, details) = enabled_baseline_details(&router, &arn).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the enablement to read back, got {} body {:?}",
            read.status, read.body
        );
        assert_eq!(
            (
                member(&details, "baselineIdentifier"),
                member(&details, "targetIdentifier"),
                applied_parameters(&details)
            ),
            (
                Some(identifier.as_str()),
                Some(target.as_str()),
                parameters.clone()
            ),
            "case {case:?}: expected the accepted enablement reported back, got {details:?}"
        );
    }
}

/// RULE (the identifier side): a `baselineIdentifier` that is not a baseline ARN at all —
/// another service's ARN, another resource type, or a value outside `BaselineArn`'s own
/// form — is a bad request, and the refusal persists nothing.
#[tokio::test]
async fn a_baseline_identifier_that_is_not_a_baseline_arn_is_refused() {
    let catalogued = BASELINE_CATALOG
        .first()
        .expect("the catalog publishes at least one baseline");
    let version = catalogued
        .versions
        .first()
        .expect("a catalog baseline offers at least one version");

    let router = set_up_emulator().await;

    // Every row satisfies `Arn` (@length 20..2048, @pattern ^arn:aws[0-9a-zA-Z_\-:\/]+$),
    // the type `EnableBaseline$baselineIdentifier` carries, so the front door passes them
    // and the HANDLER is what refuses them.
    let cases: Vec<(&str, String)> = vec![
        ("an S3 bucket ARN", "arn:aws:s3:::some-bucket-name-x".to_string()),
        (
            "an IAM role ARN",
            "arn:aws:iam::123456789012:role/not-a-baseline".to_string(),
        ),
        (
            "this service's own EnabledBaseline ARN",
            "arn:aws:controltower:us-west-2:123456789012:enabledbaseline/\
             00000000-0000-4000-8000-000000000000"
                .to_string(),
        ),
        (
            "a baseline resource under another service",
            format!("arn:aws:s3:us-west-2::baseline/{}", catalogued.id),
        ),
        (
            "a catalog baseline id in lowercase",
            baseline_arn(&catalogued.id.to_lowercase()),
        ),
        (
            "a baseline id one character short of sixteen",
            baseline_arn(&catalogued.id[..15]),
        ),
        (
            "a baseline id one character past sixteen",
            baseline_arn(&format!("{}A", catalogued.id)),
        ),
        (
            "an account segment longer than twelve digits",
            format!(
                "arn:aws:controltower:us-west-2:1234567890123:baseline/{}",
                catalogued.id
            ),
        ),
        (
            "a region segment outside a region's character set",
            format!("arn:aws:controltower:us_west_2::baseline/{}", catalogued.id),
        ),
        ("the target's ARN in the baseline position", ou("ident")),
    ];

    for (case, identifier) in cases {
        let refused = enable(&router, &identifier, version, &ou("ident"), None).await;
        assert_refused(case, &refused, 400, "ValidationException");
    }

    // None of those refusals wrote an enablement: the pair they all named enables now, so
    // nothing was holding it.
    let accepted = enable(
        &router,
        &baseline_arn(catalogued.id),
        version,
        &ou("ident"),
        None,
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "the valid enablement after every refusal: expected 200, got {} body {:?}",
        accepted.status, accepted.body
    );
}

/// RULE (the catalog side): a well-formed baseline ARN whose resource id the catalog does
/// not hold names a resource that does not exist, so it is 404 and persists nothing.
#[tokio::test]
async fn a_baseline_arn_naming_no_catalog_baseline_is_not_found() {
    let router = set_up_emulator().await;

    // One row per catalog entry, each a 16-character id the catalog does not hold, plus
    // two ids built from nothing in it.
    let mut cases: Vec<(String, String)> = BASELINE_CATALOG
        .iter()
        .map(|baseline| {
            (
                format!("an id one character from {}'s", baseline.name),
                baseline_arn(&uncatalogued_id(baseline.id)),
            )
        })
        .collect();
    cases.push((
        "an all-letter id no baseline has".to_string(),
        baseline_arn("AAAAAAAAAAAAAAAA"),
    ));
    cases.push((
        "an all-digit id no baseline has".to_string(),
        baseline_arn("0000000000000000"),
    ));

    for (case, identifier) in &cases {
        let refused = enable(&router, identifier, "1.0", &ou("catalog"), None).await;
        assert_refused(case, &refused, 404, "ResourceNotFoundException");
    }

    // The refusals wrote nothing: the target they all named takes an enablement of a
    // baseline the catalog does hold.
    let catalogued = BASELINE_CATALOG
        .first()
        .expect("the catalog publishes at least one baseline");
    let accepted = enable(
        &router,
        &baseline_arn(catalogued.id),
        catalogued
            .versions
            .first()
            .expect("a catalog baseline offers at least one version"),
        &ou("catalog"),
        None,
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "the valid enablement after every refusal: expected 200, got {} body {:?}",
        accepted.status, accepted.body
    );
}

/// RULE (the version side): a `baselineVersion` the named baseline does not offer is a bad
/// request — including a version another baseline offers, a version one release past its
/// newest, and a differently written form of one it does offer — and persists nothing.
#[tokio::test]
async fn a_version_the_baseline_does_not_offer_is_refused() {
    let router = set_up_emulator().await;
    let offered_anywhere = every_offered_version();

    for (baseline_index, baseline) in BASELINE_CATALOG.iter().enumerate() {
        let identifier = baseline_arn(baseline.id);
        let target = ou(&format!("ver{baseline_index}"));
        let newest = baseline
            .versions
            .last()
            .expect("a catalog baseline offers at least one version");

        // Rows: every version SOME baseline offers but this one does not, one release past
        // this baseline's newest, a three-component spelling of an offered version (a
        // version is a published string, matched as published), and the arbitrary value an
        // issue reported.
        let mut versions: Vec<(String, String)> = offered_anywhere
            .iter()
            .filter(|version| !baseline.versions.contains(*version))
            .map(|version| {
                (
                    format!("{version}, which another baseline offers"),
                    (*version).to_string(),
                )
            })
            .collect();
        let next_major: u32 = newest
            .split('.')
            .next()
            .and_then(|major| major.parse::<u32>().ok())
            .expect("an offered version starts with a number")
            + 1;
        versions.push((
            "one release past the newest offered".to_string(),
            format!("{next_major}.0"),
        ));
        versions.push((
            format!("{newest} written with a third component"),
            format!("{newest}.0"),
        ));
        versions.push(("an arbitrary version".to_string(), "10.10.1000".to_string()));

        for (what, version) in versions {
            let case = format!("{} asked for {what}", baseline.name);
            let refused = enable(&router, &identifier, &version, &target, None).await;
            assert_refused(&case, &refused, 400, "ValidationException");
        }

        // Every version the baseline DOES offer is accepted on that same target — one at a
        // time, since one baseline/target pair is enabled at most once.
        let accepted = enable(&router, &identifier, newest, &target, None).await;
        assert_eq!(
            accepted.status, 200,
            "{} at its newest offered version {newest}: expected 200, got {} body {:?}",
            baseline.name, accepted.status, accepted.body
        );
    }
}

/// RULE (the target side): a `targetIdentifier` that is not an organizational unit ARN is
/// a bad request — another service's ARN, another Organizations resource, or an identifier
/// outside the published `o-`/`ou-` forms — and persists nothing.
#[tokio::test]
async fn a_target_that_is_not_an_organizational_unit_is_refused() {
    let catalogued = BASELINE_CATALOG
        .first()
        .expect("the catalog publishes at least one baseline");
    let identifier = baseline_arn(catalogued.id);
    let version = catalogued
        .versions
        .first()
        .expect("a catalog baseline offers at least one version");

    let router = set_up_emulator().await;

    // Every row satisfies `Arn`, the type `EnableBaseline$targetIdentifier` carries, so
    // the front door passes them and the HANDLER is what refuses them.
    let cases: Vec<(&str, String)> = vec![
        (
            "an IAM role ARN",
            "arn:aws:iam::123456789012:role/not-a-target".to_string(),
        ),
        ("an S3 bucket ARN", "arn:aws:s3:::some-bucket-name-x".to_string()),
        (
            "an Organizations account ARN",
            "arn:aws:organizations::123456789012:account/o-exampleorg/111122223333".to_string(),
        ),
        (
            "an Organizations root ARN",
            "arn:aws:organizations::123456789012:root/o-exampleorg/r-ab12".to_string(),
        ),
        (
            "an Organizations policy ARN",
            "arn:aws:organizations::123456789012:policy/o-exampleorg/\
             service_control_policy/p-exampl"
                .to_string(),
        ),
        ("the baseline's own ARN", identifier.clone()),
        (
            "an organizational unit with no organization segment",
            "arn:aws:organizations::123456789012:ou/ou-aa11-1a".to_string(),
        ),
        (
            "an organizational unit id missing its second component",
            "arn:aws:organizations::123456789012:ou/o-exampleorg/ou-aa11".to_string(),
        ),
        (
            "an organizational unit id in uppercase",
            "arn:aws:organizations::123456789012:ou/o-exampleorg/ou-AA11-1A".to_string(),
        ),
        (
            "an organization id without its o- prefix",
            "arn:aws:organizations::123456789012:ou/exampleorg/ou-aa11-1a".to_string(),
        ),
        (
            "an account segment that is not twelve digits",
            "arn:aws:organizations::12345:ou/o-exampleorg/ou-aa11-1a".to_string(),
        ),
        (
            "an Organizations ARN carrying a region",
            "arn:aws:organizations:us-west-2:123456789012:ou/o-exampleorg/ou-aa11-1a".to_string(),
        ),
    ];

    for (case, target) in cases {
        let refused = enable(&router, &identifier, version, &target, None).await;
        assert_refused(case, &refused, 400, "ValidationException");
    }

    // The refusals wrote nothing: the same baseline enables on a real organizational unit.
    let accepted = enable(&router, &identifier, version, &ou("target"), None).await;
    assert_eq!(
        accepted.status, 200,
        "the valid enablement after every refusal: expected 200, got {} body {:?}",
        accepted.status, accepted.body
    );
}

/// RULE (the parameter side): a `parameters` entry whose key the named baseline does not
/// declare — or a declared key given twice — is a bad request and persists nothing, while
/// the keys it does declare are accepted.
#[tokio::test]
async fn a_parameter_the_baseline_does_not_declare_is_refused() {
    let declaring: &CatalogBaseline = BASELINE_CATALOG
        .iter()
        .find(|baseline| !baseline.parameters.is_empty())
        .expect("the catalog publishes a baseline that declares a parameter");
    let declaring_none: &CatalogBaseline = BASELINE_CATALOG
        .iter()
        .find(|baseline| baseline.parameters.is_empty())
        .expect("the catalog publishes a baseline that declares no parameters");
    let declared = declaring
        .parameters
        .first()
        .expect("that baseline declares at least one parameter");

    let router = set_up_emulator().await;

    let cases: Vec<(String, &CatalogBaseline, serde_json::Value)> = vec![
        (
            format!("a key {} does not declare", declaring.name),
            declaring,
            serde_json::json!([{ "key": "NoSuchParameter", "value": true }]),
        ),
        (
            format!("a declared key beside one {} does not", declaring.name),
            declaring,
            serde_json::json!([
                { "key": declared, "value": true },
                { "key": "NoSuchParameter", "value": false }
            ]),
        ),
        (
            "a declared key in the wrong case".to_string(),
            declaring,
            serde_json::json!([{ "key": declared.to_lowercase(), "value": true }]),
        ),
        (
            "a declared key with trailing space".to_string(),
            declaring,
            serde_json::json!([{ "key": format!("{declared} "), "value": true }]),
        ),
        (
            "a declared key given twice".to_string(),
            declaring,
            serde_json::json!([
                { "key": declared, "value": true },
                { "key": declared, "value": false }
            ]),
        ),
        (
            format!("a parameter on {}, which declares none", declaring_none.name),
            declaring_none,
            serde_json::json!([{ "key": declared, "value": true }]),
        ),
    ];

    for (index, (case, baseline, parameters)) in cases.into_iter().enumerate() {
        let version = baseline
            .versions
            .first()
            .expect("a catalog baseline offers at least one version");
        let target = ou(&format!("par{index}"));
        let refused = enable(
            &router,
            &baseline_arn(baseline.id),
            version,
            &target,
            Some(parameters),
        )
        .await;
        assert_refused(&case, &refused, 400, "ValidationException");

        // Each refusal wrote nothing: the pair it named takes the acceptable form of the
        // same request — the declared keys for that baseline, each once.
        let acceptable = serde_json::Value::Array(
            baseline
                .parameters
                .iter()
                .map(|key| serde_json::json!({ "key": key, "value": true }))
                .collect(),
        );
        let accepted = enable(
            &router,
            &baseline_arn(baseline.id),
            version,
            &target,
            Some(acceptable),
        )
        .await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: the acceptable form of the same request: expected 200, got {} \
             body {:?}",
            accepted.status, accepted.body
        );
    }
}

/// RULE (the same rule, from the update side): `UpdateEnabledBaseline` is checked against
/// the catalog entry of the baseline its enablement applies, so an update to a version
/// that baseline does not offer or with a parameter it does not declare is refused and
/// changes nothing a later read sees.
#[tokio::test]
async fn an_update_is_checked_against_the_enablements_baseline() {
    let declaring: &CatalogBaseline = BASELINE_CATALOG
        .iter()
        .find(|baseline| !baseline.parameters.is_empty() && baseline.versions.len() > 1)
        .expect("the catalog publishes a baseline with a parameter and several versions");
    let declared = declaring
        .parameters
        .first()
        .expect("that baseline declares at least one parameter");
    let applied_version = declaring
        .versions
        .first()
        .expect("a catalog baseline offers at least one version");
    let newer_version = declaring
        .versions
        .last()
        .expect("a catalog baseline offers at least one version");

    let router = set_up_emulator().await;
    let enabled = enable(
        &router,
        &baseline_arn(declaring.id),
        applied_version,
        &ou("update"),
        Some(serde_json::json!([{ "key": declared, "value": true }])),
    )
    .await;
    assert_eq!(
        enabled.status, 200,
        "enable: expected 200, got {} body {:?}",
        enabled.status, enabled.body
    );
    let arn = enabled.string("arn");
    tokio::time::sleep(past_the_settle_span()).await;

    let cases: Vec<(&str, &str, Option<serde_json::Value>)> = vec![
        ("a version no baseline offers", "10.10.1000", None),
        (
            "a version written with a third component",
            "1.0.0",
            None,
        ),
        (
            "a key the baseline does not declare",
            newer_version,
            Some(serde_json::json!([{ "key": "NoSuchParameter", "value": true }])),
        ),
        (
            "a declared key given twice",
            newer_version,
            Some(serde_json::json!([
                { "key": declared, "value": true },
                { "key": declared, "value": false }
            ])),
        ),
    ];

    for (case, version, parameters) in cases {
        let refused = update(&router, &arn, version, parameters).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected 400 ValidationException, got {:?} body {:?}",
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );
        assert!(
            refused.body.get("operationIdentifier").is_none(),
            "case {case:?}: a refused update must mint no operation, got {:?}",
            refused.body
        );

        let (_, details) = enabled_baseline_details(&router, &arn).await;
        assert_eq!(
            (
                member(&details, "baselineVersion"),
                applied_parameters(&details)
            ),
            (
                Some(*applied_version),
                serde_json::json!([{ "key": declared, "value": true }])
            ),
            "case {case:?}: a refused update must change nothing, got {details:?}"
        );
    }

    // The acceptable update — a version this baseline offers, with a key it declares — is
    // applied once the clock makes it due.
    let accepted = update(
        &router,
        &arn,
        newer_version,
        Some(serde_json::json!([{ "key": declared, "value": false }])),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "the acceptable update: expected 200, got {} body {:?}",
        accepted.status, accepted.body
    );
    tokio::time::sleep(past_the_settle_span()).await;

    let (_, details) = enabled_baseline_details(&router, &arn).await;
    assert_eq!(
        (
            member(&details, "baselineVersion"),
            applied_parameters(&details)
        ),
        (
            Some(*newer_version),
            serde_json::json!([{ "key": declared, "value": false }])
        ),
        "once the accepted update is due: expected {newer_version} with the declared \
         parameter applied, got {details:?}"
    );
}
