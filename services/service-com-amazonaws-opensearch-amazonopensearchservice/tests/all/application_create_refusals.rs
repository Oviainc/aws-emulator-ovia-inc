// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what `CreateApplication` does with a request whose input
//! breaks a constraint the model declares on one of its OWN members.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`]; this suite adds no second copy of it, and leaves the
//! round trip of an accepted create to [`super::applications`] and the shape of a
//! constraint refusal's SENTENCE to [`super::collection_element_refusals`].
//!
//! The rule under test: a create whose input breaks a constraint the model declares is
//! refused with the modelled `ValidationException` (HTTP 400) and MINTS NOTHING, so the
//! account's applications are exactly those its accepted creates made, while a value AT
//! a published bound is admitted and mints one application.
//!
//! So the assertions span a SET of inputs rather than one call: each constrained member
//! of `CreateApplicationRequest` — the name, the client token, the customer-managed key
//! and the configuration settings — broken in each way its constraints can be broken
//! (one past each published length bound, outside a pattern, outside an enum), and then
//! each of those bounds driven at the bound itself.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::APP_CONFIG_TYPE_VALUES;

use super::application_listing::listed_ids;
use super::application_updates::{app_configs, read_list};
use super::domain_lifecycle::{error_code, string_member, Answer, Emulator, PREFIX};

/// A `kmsKeyArn` of the form `KmsKeyArn`'s own pattern admits
/// (`smithy_query(query="shape",shape="KmsKeyArn")`), so a row that is not about the key
/// carries one nothing can refuse.
const KMS_KEY: &str = "arn:aws:kms:us-east-1:123456789012:key/1234abcd-12ab-34cd-56ef-1234567890ab";

/// The published bounds of `appConfigs`: `@length(min: 0, max: 200)`
/// (`smithy_query(query="shape",shape="AppConfigs")`).
const APP_CONFIGS_MAX: usize = 200;

/// The published upper bound of `clientToken`: `@length(min: 1, max: 64)`
/// (`smithy_query(query="shape",shape="ClientToken")`).
const CLIENT_TOKEN_MAX: usize = 64;

/// `CreateApplication`: `POST /2021-01-01/opensearch/application`.
async fn create_application(emulator: &Emulator, body: Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/application"),
            Some(body),
        )
        .await
}

/// `GetApplication`: `GET /2021-01-01/opensearch/application/{id}`.
async fn get_application(emulator: &Emulator, id: &str) -> Answer {
    emulator
        .call("GET", &format!("{PREFIX}/opensearch/application/{id}"), None)
        .await
}

/// `ListApplications`: `GET /2021-01-01/opensearch/list-applications`.
async fn list_applications(emulator: &Emulator) -> Answer {
    emulator
        .call(
            "GET",
            &format!("{PREFIX}/opensearch/list-applications"),
            None,
        )
        .await
}

#[tokio::test]
async fn a_create_breaking_a_modelled_constraint_is_refused_and_mints_nothing() {
    let emulator = Emulator::new();

    // (case, the request body). Each row breaks ONE constraint of one member of
    // `CreateApplicationRequest`, and the rows differ along what the rule generalises
    // over: which member carries the offending value, and whether the value is one past
    // a published length bound, outside a published pattern or outside a published enum.
    let rows: Vec<(&str, Value)> = vec![
        (
            // `ApplicationName` is `@length(min: 3, max: 30)`.
            "a name one character short of the 3-character lower bound",
            json!({ "name": "ab" }),
        ),
        (
            // 31 characters: one past the upper bound.
            "a name one character past the 30-character upper bound",
            json!({ "name": "abcdefghij-klmnopqrst-uvwxyz123" }),
        ),
        (
            // `ApplicationName`'s pattern is `^[a-z][a-z0-9\-]+$`.
            "a name carrying an upper-case letter",
            json!({ "name": "Search-ui" }),
        ),
        (
            "a name starting with a digit",
            json!({ "name": "1search-ui" }),
        ),
        (
            "a name carrying an underscore",
            json!({ "name": "search_ui" }),
        ),
        (
            // `ClientToken` is `@length(min: 1, max: 64)`.
            "an empty clientToken, short of its 1-character lower bound",
            json!({ "name": "token-floor-ui", "clientToken": "" }),
        ),
        (
            "a clientToken one character past its 64-character upper bound",
            json!({
                "name": "token-ceiling-ui",
                "clientToken": "t".repeat(CLIENT_TOKEN_MAX + 1),
            }),
        ),
        (
            "a kmsKeyArn outside its published pattern",
            json!({
                "name": "key-pattern-ui",
                "kmsKeyArn": "arn:aws:kms:us-east-1:123456789012:key/not-a-key-identifier",
            }),
        ),
        (
            // `KmsKeyArn` is `@length(min: 20, max: 2048)`; this value is 13 characters.
            "a kmsKeyArn shorter than its 20-character lower bound",
            json!({ "name": "key-floor-ui", "kmsKeyArn": "arn:aws:kms:k" }),
        ),
        (
            "a configuration key outside the two values the model declares",
            json!({
                "name": "config-key-ui",
                "appConfigs": [{
                    "key": "opensearchDashboards.dashboardAdmin.admins",
                    "value": "arn:aws:iam::123456789012:user/ana",
                }],
            }),
        ),
        (
            // `AppConfigValue` is `@length(min: 1, max: 4096)`.
            "a configuration value short of its 1-character lower bound",
            json!({
                "name": "config-value-ui",
                "appConfigs": [{ "key": APP_CONFIG_TYPE_VALUES[0], "value": "" }],
            }),
        ),
        (
            "one configuration setting past the 200-entry upper bound",
            json!({
                "name": "config-count-ui",
                "appConfigs": app_configs(APP_CONFIGS_MAX + 1),
                "kmsKeyArn": KMS_KEY,
            }),
        ),
    ];

    for (case, body) in rows {
        let answer = create_application(&emulator, body).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400 from CreateApplication, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.body
        );
        assert!(
            answer.body["id"].is_null() && answer.body["arn"].is_null(),
            "case {case:?}: a refused create reports no application, got {:?}",
            answer.body
        );

        let listing = list_applications(&emulator).await;
        assert_eq!(
            listed_ids(&listing, case),
            Vec::<String>::new(),
            "case {case:?}: a refused create mints nothing, got {:?}",
            listing.body
        );
    }

    // Every row above was refused before anything was stored, so the names they asked
    // for are still free: the corrected form of one of them creates the account's first
    // application.
    let accepted = create_application(&emulator, json!({ "name": "search-ui" })).await;
    assert_eq!(
        accepted.status, 200,
        "a create whose input satisfies every constraint is accepted, got {} with {:?}",
        accepted.status, accepted.body
    );
    let id = string_member(&accepted.body, "id", "the corrected create");
    let listing = list_applications(&emulator).await;
    assert_eq!(
        listed_ids(&listing, "the corrected create"),
        vec![id],
        "the account holds exactly the one application its accepted create made, got {:?}",
        listing.body
    );
}

#[tokio::test]
async fn a_value_at_a_published_bound_is_admitted_and_mints_one_application() {
    let emulator = Emulator::new();

    // (case, the request body). Each row drives one constrained member AT a published
    // bound — the value the row above it is one step past — so the refusals asserted
    // there are the constraint rather than a handler refusing the bound itself.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a clientToken at its 1-character lower bound",
            json!({ "name": "token-floor-ok", "clientToken": "t" }),
        ),
        (
            "a clientToken at its 64-character upper bound",
            json!({
                "name": "token-ceiling-ok",
                "clientToken": "t".repeat(CLIENT_TOKEN_MAX),
            }),
        ),
        (
            "no configuration setting at all, the 0-entry lower bound",
            json!({ "name": "config-floor-ok", "appConfigs": app_configs(0) }),
        ),
        (
            "configuration settings at the 200-entry upper bound",
            json!({
                "name": "config-ceiling-ok",
                "appConfigs": app_configs(APP_CONFIGS_MAX),
                "kmsKeyArn": KMS_KEY,
            }),
        ),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (case, body) in rows {
        let created = create_application(&emulator, body.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateApplication, got {} with {:?}",
            created.status, created.body
        );
        let id = string_member(&created.body, "id", case);

        // The create's answer and a later read answer from ONE stored record, so the
        // settings a bound-case named come back from the read as well.
        let read = get_application(&emulator, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GetApplication, got {} with {:?}",
            read.status, read.body
        );
        // `appConfigs` is `@length(min: 0, max: 200)`, so the read reports the settings
        // the application holds — the EMPTY list at the 0-entry lower bound and for a row
        // that named none — and never a setting no request asked for.
        assert_eq!(
            read.body["appConfigs"],
            read_list(body.get("appConfigs")),
            "case {case:?}: the read reports the configuration settings the create named, \
             got {:?}",
            read.body["appConfigs"]
        );
        assert_eq!(
            read.body["name"], body["name"],
            "case {case:?}: the read reports the name the create named, got {:?}",
            read.body
        );
        minted.push(id);
    }

    let listing = list_applications(&emulator).await;
    let mut listed = listed_ids(&listing, "every bound case");
    listed.sort();
    minted.sort();
    assert_eq!(
        listed, minted,
        "each accepted create mints exactly one application, got {:?}",
        listing.body
    );
}
