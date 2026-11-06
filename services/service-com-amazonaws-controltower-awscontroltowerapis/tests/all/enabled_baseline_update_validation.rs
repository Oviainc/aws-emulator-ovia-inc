// SPDX-License-Identifier: CC0-1.0

//! What an `UpdateEnabledBaseline` request may ask for, end to end over HTTP.
//!
//! Driven through the same [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter)
//! the production emulator dispatches with, so what these tests exercise is the wire
//! behaviour a client sees — status codes, error codes and JSON bodies — not the handlers'
//! Rust signatures.
//!
//! The rule under test: an `EnabledBaseline` is updated only to a configuration its OWN
//! baseline publishes — a `baselineVersion` that baseline offers, and `parameters` keyed
//! only by the parameters it declares, each key at most once — and a request outside that
//! is `ValidationException` (400) which mints no operation and leaves the applied version
//! and parameters as they were.
//!
//! The table is `state::BASELINE_CATALOG` itself, the catalog `ListBaselines`/`GetBaseline`
//! publish, so the rule is asserted for EVERY baseline the service publishes — the ones
//! offering several versions and the ones offering one, the ones declaring a parameter and
//! the ones declaring none — rather than for the single baseline the sibling
//! `enable_baseline_validation` suite drives the update path with. The rows differ along
//! what the rule generalises over: which baseline the enablement applies, and the form of
//! the refused value (a version another baseline offers, a version past the newest offered,
//! a differently written spelling of an offered one, an undeclared key, a declared key in
//! another case or given twice). The request helpers and the catalog walk are the sibling
//! suite's, imported rather than copied, so both sides of the rule are driven by one set.
//!
//! `BaselineVersion`'s `@length` 1..10 with `@pattern ^\d+(?:\.\d+){0,2}$` and the modelled
//! `ValidationException` (400) come from
//! `smithy_query(query="operation", operation="UpdateEnabledBaseline")`: the pattern bounds
//! the SHAPE of a version string, not the set of versions a baseline has, so which versions
//! and parameters exist is the baseline's own published catalog.
//!
//! Note on sameness: `smithy_query(query="doc", shape="UpdateEnabledBaseline")` documents
//! only "updates an EnabledBaseline resource's applied parameters or version", with none of
//! the "supply parameters that are different from the currently configured parameters"
//! requirement `smithy_query(query="doc", shape="UpdateEnabledControl")` states for an
//! enabled control — so an update naming the version already applied is a request this
//! service accepts, which the accepting rows below pin for every single-version baseline.

use service_com_amazonaws_controltower_awscontroltowerapis::state::{
    CatalogBaseline, BASELINE_CATALOG,
};

use crate::baseline_catalog::served_arn;
use crate::enable_baseline_validation::{
    applied_parameters, assert_refused, enable, enabled_baseline_details, every_offered_version,
    member, ou, update,
};
use crate::enabled_baseline_update::get_operation;
use crate::landing_zone_lifecycle::{
    is_operation_identifier, past_the_settle_span, set_up_emulator, Answer,
};

/// The acceptable `parameters` form for `baseline`: every key it declares, once each — the
/// empty list for a baseline that declares none.
fn declared_parameters(baseline: &CatalogBaseline) -> serde_json::Value {
    serde_json::Value::Array(
        baseline
            .parameters
            .iter()
            .map(|key| serde_json::json!({ "key": key, "value": true }))
            .collect(),
    )
}

/// Every parameter key SOME catalog baseline declares, deduplicated — the pool the rows for
/// a baseline that declares none are drawn from.
fn every_declared_parameter() -> Vec<&'static str> {
    let mut keys: Vec<&'static str> = BASELINE_CATALOG
        .iter()
        .flat_map(|baseline| baseline.parameters.iter().copied())
        .collect();
    keys.sort_unstable();
    keys.dedup();
    keys
}

/// A version's numeric components, so two published versions can be ordered as versions
/// rather than as strings.
fn components(version: &str) -> Vec<u32> {
    version
        .split('.')
        .map(|component| {
            component
                .parse::<u32>()
                .expect("a published version's components are numbers")
        })
        .collect()
}

/// `GetEnabledBaseline`'s `enabledBaselineDetails` for `identifier`, asserting it resolved.
async fn details(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    identifier: &str,
) -> serde_json::Value {
    let (answer, details) = enabled_baseline_details(router, identifier).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: get-enabled-baseline expected 200, got {} body {:?}",
        answer.status, answer.body
    );
    details
}

/// `statusSummary.status` of a reported record.
fn applied_status(record: &serde_json::Value) -> Option<&str> {
    record
        .get("statusSummary")
        .and_then(|summary| member(summary, "status"))
}

/// Enable every catalog baseline, each on its own organizational unit, at that baseline's
/// OLDEST offered version with `parameters` as `parameters_of` builds them, and hand back
/// the enablements once the clock has made them due — the state an update is asked for from.
async fn deployed_enablements(
    router: &smithy_server_runtime::MultiServiceRouter,
    tag: &str,
    parameters_of: impl Fn(&CatalogBaseline) -> Option<serde_json::Value>,
) -> Vec<(&'static CatalogBaseline, String)> {
    let mut enablements: Vec<(&'static CatalogBaseline, String)> = Vec::new();
    for (index, baseline) in BASELINE_CATALOG.iter().enumerate() {
        let oldest = baseline
            .versions
            .first()
            .copied()
            .expect("a catalog baseline offers at least one version");
        let enabled = enable(
            router,
            &served_arn(baseline),
            oldest,
            &ou(&format!("{tag}{index}")),
            parameters_of(baseline),
        )
        .await;
        assert_eq!(
            enabled.status, 200,
            "case {:?}: enable expected 200, got {} body {:?}",
            baseline.name, enabled.status, enabled.body
        );
        enablements.push((baseline, enabled.string("arn")));
    }

    // The enables are asynchronous too: an enablement still being applied refuses an update
    // as a conflict, which is not the refusal under test here.
    tokio::time::sleep(past_the_settle_span()).await;
    enablements
}

/// Assert an accepted update minted an asynchronous operation, and hand its identifier back.
fn accepted_operation(case: &str, accepted: &Answer) -> String {
    assert_eq!(
        accepted.status, 200,
        "case {case:?}: expected 200, got {} body {:?}",
        accepted.status, accepted.body
    );
    let operation_identifier = accepted.string("operationIdentifier");
    assert!(
        is_operation_identifier(&operation_identifier),
        "case {case:?}: expected a 36-char lowercase-hex operationIdentifier, got \
         {operation_identifier:?}"
    );
    operation_identifier
}

/// RULE (the catalog's own shape): each entry lists the versions it offers oldest first, so
/// a row derived from the first, the last or one release past the last is derived from what
/// the entry means — the derivation every version row below rests on.
#[test]
fn the_catalog_lists_each_baselines_versions_oldest_first() {
    for baseline in BASELINE_CATALOG {
        let ordered: Vec<Vec<u32>> = baseline
            .versions
            .iter()
            .map(|version| components(version))
            .collect();
        let mut sorted = ordered.clone();
        sorted.sort();
        assert_eq!(
            ordered, sorted,
            "case {:?}: expected the offered versions oldest first, got {:?}",
            baseline.name, baseline.versions
        );
    }
}

/// RULE (the version side): an update may name only a version the enablement's OWN baseline
/// offers — so a version another baseline offers, one past that baseline's newest, another
/// spelling of one it does offer, and an arbitrary pattern-shaped value are each refused as
/// a bad request that applies nothing — while a version that baseline does offer is accepted
/// as an asynchronous operation and applied once the clock makes it due.
#[tokio::test]
async fn an_update_to_a_version_the_enablements_baseline_does_not_offer_is_refused() {
    let router = set_up_emulator().await;
    let offered_anywhere = every_offered_version();
    let enablements = deployed_enablements(&router, "uv", |baseline| {
        Some(declared_parameters(baseline))
    })
    .await;

    let mut accepted_updates: Vec<(&CatalogBaseline, String, String)> = Vec::new();
    for (baseline, arn) in &enablements {
        let oldest = baseline
            .versions
            .first()
            .copied()
            .expect("a catalog baseline offers at least one version");
        let newest = baseline
            .versions
            .last()
            .copied()
            .expect("a catalog baseline offers at least one version");
        let next_major: u32 = components(newest)
            .first()
            .copied()
            .expect("a published version has a first component")
            + 1;

        let mut rows: Vec<(String, String)> = offered_anywhere
            .iter()
            .filter(|version| !baseline.versions.contains(*version))
            .map(|version| {
                (
                    format!("{version}, which another baseline offers"),
                    (*version).to_string(),
                )
            })
            .collect();
        rows.push((
            "one release past the newest offered".to_string(),
            format!("{next_major}.0"),
        ));
        rows.push((
            format!("{newest} written with a third component"),
            format!("{newest}.0"),
        ));
        rows.push((
            format!("{newest} written with a leading zero"),
            format!("0{newest}"),
        ));
        rows.push(("an arbitrary version".to_string(), "10.10.1000".to_string()));
        rows.push(("the zero version".to_string(), "0.0".to_string()));

        for (what, version) in rows {
            let case = format!("{} asked for {what}", baseline.name);
            let refused = update(&router, arn, &version, None).await;
            assert_refused(&case, &refused, 400, "ValidationException");

            // The refusal applied nothing and started nothing: the enablement still reads
            // as the deployed configuration it had.
            let record = details(&router, &case, arn).await;
            assert_eq!(
                (member(&record, "baselineVersion"), applied_status(&record)),
                (Some(oldest), Some("SUCCEEDED")),
                "case {case:?}: a refused update must leave the applied version and status \
                 alone, got {record:?}"
            );
            assert_eq!(
                applied_parameters(&record),
                declared_parameters(baseline),
                "case {case:?}: a refused update must leave the applied parameters alone, \
                 got {record:?}"
            );
        }

        // Every refusal above is about the VALUE, not about the enablement: the newest
        // version that baseline offers is accepted on the same enablement, and mints the
        // asynchronous operation the accepted path reports. For a baseline offering a single
        // version that newest version is the one already applied, which this operation's
        // model docs do not refuse (see this module's note), so the minted operation is what
        // distinguishes an accepted update from an ignored one there.
        let case = format!("{} asked for its newest offered version", baseline.name);
        let accepted = update(&router, arn, newest, None).await;
        let operation_identifier = accepted_operation(&case, &accepted);
        accepted_updates.push((*baseline, arn.clone(), operation_identifier));
    }

    tokio::time::sleep(past_the_settle_span()).await;

    for (baseline, arn, operation_identifier) in &accepted_updates {
        let newest = baseline
            .versions
            .last()
            .copied()
            .expect("a catalog baseline offers at least one version");

        let (resolved, operation) = get_operation(&router, operation_identifier).await;
        assert_eq!(
            resolved.status, 200,
            "case {:?}: the minted operation must resolve, got {} body {:?}",
            baseline.name, resolved.status, resolved.body
        );
        assert_eq!(
            (
                member(&operation, "operationType"),
                member(&operation, "status")
            ),
            (Some("UPDATE_ENABLED_BASELINE"), Some("SUCCEEDED")),
            "case {:?}: expected UPDATE_ENABLED_BASELINE SUCCEEDED once due, got \
             {operation:?}",
            baseline.name
        );

        let record = details(&router, baseline.name, arn).await;
        assert_eq!(
            (
                member(&record, "baselineVersion"),
                applied_status(&record),
                record
                    .get("statusSummary")
                    .and_then(|summary| member(summary, "lastOperationIdentifier"))
            ),
            (
                Some(newest),
                Some("SUCCEEDED"),
                Some(operation_identifier.as_str())
            ),
            "case {:?}: expected the offered version applied by the minted operation once \
             it is due, got {record:?}",
            baseline.name
        );
    }
}

/// RULE (the parameter side): an update's `parameters` may be keyed only by the parameters
/// the enablement's OWN baseline declares, each key once — so an undeclared key, a declared
/// key in another case or with trailing whitespace, a declared key given twice, and any
/// parameter at all on a baseline that declares none are each refused as a bad request that
/// applies nothing — while the keys that baseline declares are accepted and applied.
#[tokio::test]
async fn an_update_carrying_a_parameter_the_enablements_baseline_does_not_declare_is_refused() {
    let router = set_up_emulator().await;
    let declared_anywhere = every_declared_parameter();
    // Nothing applied to start with, so a refused update is visible as "still none applied".
    let enablements = deployed_enablements(&router, "up", |_| None).await;

    let mut accepted_updates: Vec<(&CatalogBaseline, String, String)> = Vec::new();
    for (baseline, arn) in &enablements {
        let oldest = baseline
            .versions
            .first()
            .copied()
            .expect("a catalog baseline offers at least one version");
        let newest = baseline
            .versions
            .last()
            .copied()
            .expect("a catalog baseline offers at least one version");

        let mut rows: Vec<(String, serde_json::Value)> = vec![(
            "a key no baseline declares".to_string(),
            serde_json::json!([{ "key": "NoSuchParameter", "value": true }]),
        )];
        for key in &declared_anywhere {
            if baseline.declares_parameter(key) {
                rows.push((
                    format!("the declared key {key} in another case"),
                    serde_json::json!([{ "key": key.to_lowercase(), "value": true }]),
                ));
                rows.push((
                    format!("the declared key {key} with a trailing space"),
                    serde_json::json!([{ "key": format!("{key} "), "value": true }]),
                ));
                rows.push((
                    format!("the declared key {key} given twice"),
                    serde_json::json!([
                        { "key": key, "value": true },
                        { "key": key, "value": false }
                    ]),
                ));
                rows.push((
                    format!("the declared key {key} beside one no baseline declares"),
                    serde_json::json!([
                        { "key": key, "value": true },
                        { "key": "NoSuchParameter", "value": false }
                    ]),
                ));
            } else {
                rows.push((
                    format!("{key}, which another baseline declares"),
                    serde_json::json!([{ "key": key, "value": true }]),
                ));
            }
        }

        for (what, parameters) in rows {
            let case = format!("{} asked for {what}", baseline.name);
            let refused = update(&router, arn, newest, Some(parameters)).await;
            assert_refused(&case, &refused, 400, "ValidationException");

            let record = details(&router, &case, arn).await;
            assert_eq!(
                (
                    member(&record, "baselineVersion"),
                    applied_parameters(&record),
                    applied_status(&record)
                ),
                (Some(oldest), serde_json::json!([]), Some("SUCCEEDED")),
                "case {case:?}: a refused update must apply neither its version nor its \
                 parameters, got {record:?}"
            );
        }

        // The acceptable form of the same request — the keys that baseline declares, each
        // once — is accepted and applied, so the refusals above are about the keys and not
        // about the member being present.
        let case = format!("{} asked for the keys it declares", baseline.name);
        let accepted = update(&router, arn, newest, Some(declared_parameters(baseline))).await;
        let operation_identifier = accepted_operation(&case, &accepted);
        accepted_updates.push((*baseline, arn.clone(), operation_identifier));
    }

    tokio::time::sleep(past_the_settle_span()).await;

    for (baseline, arn, operation_identifier) in &accepted_updates {
        let (_, operation) = get_operation(&router, operation_identifier).await;
        assert_eq!(
            (
                member(&operation, "operationType"),
                member(&operation, "status")
            ),
            (Some("UPDATE_ENABLED_BASELINE"), Some("SUCCEEDED")),
            "case {:?}: expected UPDATE_ENABLED_BASELINE SUCCEEDED once due, got \
             {operation:?}",
            baseline.name
        );

        let record = details(&router, baseline.name, arn).await;
        assert_eq!(
            applied_parameters(&record),
            declared_parameters(baseline),
            "case {:?}: expected the declared parameters applied once the update is due, \
             got {record:?}",
            baseline.name
        );
    }
}

/// Keeps the import of the sibling suite's catalog walk honest: the pool the version rows
/// draw from is every version SOME baseline offers, and it must hold more than one baseline's
/// set — otherwise no row can ask one baseline for a version only another one has.
#[test]
fn the_version_pool_spans_more_than_one_baselines_offered_set() {
    let pool = every_offered_version();
    for baseline in BASELINE_CATALOG {
        assert!(
            baseline
                .versions
                .iter()
                .all(|version| pool.contains(version)),
            "case {:?}: every offered version must be in the pool, got {pool:?}",
            baseline.name
        );
    }
    assert!(
        BASELINE_CATALOG
            .iter()
            .any(|baseline| baseline.versions.len() < pool.len()),
        "the pool must hold a version some baseline does not offer, got {pool:?}"
    );
}
