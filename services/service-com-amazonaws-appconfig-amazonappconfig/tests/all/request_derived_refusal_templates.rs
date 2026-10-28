// SPDX-License-Identifier: CC0-1.0

//! The AppConfig refusals whose `Message` is a TEMPLATE filled in from the request, over
//! the sets of requests each template covers.
//!
//! Five rules are under test, each one level more general than any single request. The
//! status and the error code are asserted beside the message in every row, because a
//! template is only part of the refusal a client reads:
//!
//! 1. A tagging call whose `ResourceArn` resolves to no resource this service holds is
//!    HTTP 404 `ResourceNotFoundException` with the ONE template
//!    `Resource with ARN <ResourceArn> could not be found for account <account>.` — the
//!    identifier label is the initialism `ARN`, in upper case, and the three tagging
//!    operations share the one sentence whatever the ARN addresses.
//! 2. A paginated listing handed a `NextToken` it did not mint is HTTP 400
//!    `BadRequestException` with the template `Invalid nextToken parameter <NextToken>.`,
//!    the submitted token echoed as sent — one sentence for every listing of this service.
//! 3. `CreateHostedConfigurationVersion` whose `LatestVersionNumber` is not the version the
//!    profile currently holds is HTTP 409 `ConflictException` with
//!    `Expected latest version <submitted> does not match actual latest version <actual>`,
//!    built from the two numbers alone and with no trailing full stop, and the refused
//!    write stores nothing.
//! 4. `CreateExperimentDefinition` under a `Name` already in use in that application is
//!    HTTP 409 `ConflictException` with
//!    `Experiment definition with name '<Name>' already exists for application <id>` — the
//!    name single-quoted, and the definition already holding it NOT disclosed.
//! 5. A request refused for breaking a modelled `@length` maximum is HTTP 400
//!    `BadRequestException` whose validation clause quotes the value the caller
//!    SUBMITTED — all of it, as sent — rather than a shortened rendering of it.
//!
//! The tables span what those rules generalise over: for 1, the operation (all three) and
//! the resource kind, Region and id shape the ARN addresses; for 2, the listing called
//! (account-level and application-scoped), and the token's shape — text that is no token,
//! text carrying URI-significant characters, a token another listing of this service
//! minted, and a token at the `NextToken` length bound; for 3, the pair of version numbers
//! (below, above and far from the actual one) across two different actual latest versions;
//! for 4, the form of the contested name (short, at the `Name` bound, non-ASCII, carrying
//! spaces and punctuation, carrying the quote character the template uses); and for 5, the
//! operation and member the over-long value is bound to, and its size and encoding — just
//! past the bound, far past the framework's own echo limit, and multi-byte text — with the
//! value AT the bound served as the positive control.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::{
    account, create_application, create_environment, create_profile, json_call, router, send,
    Answer,
};
use crate::experiment_definitions::{body as definition_body, create_definition, flag, scope,
    treatment};
use crate::extensions::{actions, create_extension, get_extension, minted_id};
use crate::resource_tags::{arn, filler, list_tags, pct, tag, untag};
use crate::update_extension::update_extension;

/// The `Message` of a refusal body, or the body itself when it carries none — which is
/// what a failing assertion needs to show.
fn message(answer: &Answer) -> String {
    answer
        .string("Message")
        .or_else(|| answer.string("message"))
        .unwrap_or_else(|| String::from_utf8_lossy(&answer.body).to_string())
}

/// Assert one refusal's status and error code, then hand back its message.
fn refused(case: &str, answer: &Answer, status: u16, code: &str) -> String {
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected {status}, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert!(
        answer.error_code().contains(code),
        "case {case:?}: expected {code}, got {:?}",
        answer.error_code()
    );
    message(answer)
}

// =============================================================================
// 1. the tagging operations' one not-found template
// =============================================================================

/// RULE 1: all three tagging operations refuse an unresolvable `ResourceArn` with the one
/// template, whose identifier label is `ARN`.
#[tokio::test]
async fn a_tagging_lookup_that_resolves_to_nothing_states_the_arn_template() {
    let router = router();
    let account = account();

    // Rows differ along the resource kind the ARN addresses, its depth under an
    // application, the Region it names and the shape of its ids — none of which changes
    // the sentence, because the lookup failed for all of them.
    let unresolvable: Vec<(&str, String)> = vec![
        ("an application", arn("application/zzzzzzz")),
        (
            "an environment under an application",
            arn("application/zzzzzzz/environment/yyyyyyy"),
        ),
        (
            "a configuration profile under an application",
            arn("application/zzzzzzz/configurationprofile/xxxxxxx"),
        ),
        ("a deployment strategy", arn("deploymentstrategy/wwwwwww")),
        ("an extension", arn("extension/vvvvvvv")),
        (
            "an application in another Region",
            engine::emulator_arn("appconfig", "eu-west-1", "application/zzzzzzz"),
        ),
        (
            "a long, id-shaped resource path",
            arn(&format!("application/{}", filler(32, 3))),
        ),
    ];

    for (kind, resource_arn) in &unresolvable {
        let expected =
            format!("Resource with ARN {resource_arn} could not be found for account {account}.");
        // Every operation that reaches this lookup, each with the members it binds.
        let answers: Vec<(String, Answer)> = vec![
            (
                format!("TagResource on {kind}"),
                tag(&router, resource_arn, &json!({ "owner": "platform" })).await,
            ),
            (
                format!("ListTagsForResource on {kind}"),
                list_tags(&router, resource_arn).await,
            ),
            (
                format!("UntagResource on {kind}"),
                untag(&router, resource_arn, &["owner"]).await,
            ),
        ];
        for (case, answer) in &answers {
            let stated = refused(case, answer, 404, "ResourceNotFoundException");
            assert_eq!(
                stated, expected,
                "case {case:?}: expected message {expected:?}, got {stated:?}"
            );
        }
    }
}

// =============================================================================
// 2. the one malformed-NextToken template
// =============================================================================

/// `GET <uri>` with `next_token` set to `token`, which travels percent-encoded as a query
/// value.
async fn list_with_token(router: &MultiServiceRouter, uri: &str, token: &str) -> Answer {
    let separator = if uri.contains('?') { '&' } else { '?' };
    json_call(
        router,
        "GET",
        &format!("{uri}{separator}next_token={}", pct(token)),
        &Value::Null,
    )
    .await
}

/// RULE 2: every paginated listing refuses a token it did not mint with AWS's one
/// template, echoing the submitted token.
#[tokio::test]
async fn a_next_token_no_listing_minted_is_refused_with_the_submitted_token_echoed() {
    let router = router();
    let application = create_application(&router, "token-echo").await;
    let second_application = create_application(&router, "token-echo-other").await;
    assert!(
        !second_application.is_empty(),
        "a second application must exist so the applications listing hands back a token"
    );
    let profile = create_profile(&router, &application, "config").await;
    assert!(
        !profile.is_empty(),
        "the profile must exist so its listing has something to page"
    );
    for environment in ["prod", "staging"] {
        let created = create_environment(&router, &application, environment).await;
        assert!(
            !created.is_empty(),
            "the environments must exist so their listing hands back a token"
        );
    }

    /// A real cursor one listing minted, read off a page it left entries after.
    async fn minted_cursor(router: &MultiServiceRouter, listing: &str, uri: &str) -> String {
        let page = json_call(router, "GET", uri, &Value::Null).await;
        assert_eq!(
            page.status,
            200,
            "the {listing} fixture page must be served, got {} / {}",
            page.status,
            String::from_utf8_lossy(&page.body)
        );
        page.json()
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| {
                panic!("{listing} must hand back a NextToken while entries are left over")
            })
    }

    // Two real cursors, each from a DIFFERENT listing: a cursor belongs to the listing
    // that minted it, so every row below is handed one the listing it calls did not mint.
    let applications_cursor = minted_cursor(
        &router,
        "ListApplications",
        "/applications?max_results=1",
    )
    .await;
    let environments_cursor = minted_cursor(
        &router,
        "ListEnvironments",
        &format!("/applications/{application}/environments?max_results=1"),
    )
    .await;

    // Token shapes: text that is no token at all, text carrying URI-significant
    // characters, hex that decodes to nothing this service minted, a token another
    // listing of this service minted, and a token at the modelled `NextToken` maximum
    // (`smithy_query(query="shape", shape="NextToken")` — length 1..2048).
    let tokens: Vec<(&str, String)> = vec![
        ("plain text", "not-a-real-continuation-token".to_string()),
        (
            "text carrying URI-significant characters",
            "not-a-real-pagination-token!!".to_string(),
        ),
        ("a single character", "z".to_string()),
        ("hex that decodes to no cursor", "6162636465".to_string()),
        ("a token at the NextToken maximum", filler(2048, 7)),
    ];

    // Listings: the two the recordings cover, plus an account-level and two
    // application-scoped ones, so the sentence is the listing-independent rule it claims.
    let listings: Vec<(&str, String)> = vec![
        ("ListExtensions", "/extensions".to_string()),
        (
            "ListDeploymentStrategies",
            "/deploymentstrategies".to_string(),
        ),
        ("ListApplications", "/applications".to_string()),
        (
            "ListEnvironments",
            format!("/applications/{application}/environments"),
        ),
        (
            "ListConfigurationProfiles",
            format!("/applications/{application}/configurationprofiles"),
        ),
    ];

    for (listing, uri) in &listings {
        // The real cursor this listing did not mint: the applications one for every
        // listing but that one, which is handed the environments cursor instead.
        let foreign = if *listing == "ListApplications" {
            &environments_cursor
        } else {
            &applications_cursor
        };
        let rows = tokens
            .iter()
            .map(|(shape, token)| (*shape, token))
            .chain(std::iter::once(("a token another listing minted", foreign)));
        for (shape, token) in rows {
            let case = format!("{listing} with {shape}");
            let answer = list_with_token(&router, uri, token).await;
            let stated = refused(&case, &answer, 400, "BadRequestException");
            let expected = format!("Invalid nextToken parameter {token}.");
            assert_eq!(
                stated, expected,
                "case {case:?}: expected message {expected:?}, got {stated:?}"
            );
        }
        // The positive control: the same listing without a token is served, so the rows
        // above are refused for the token and nothing else.
        let served = json_call(&router, "GET", uri, &Value::Null).await;
        assert_eq!(
            served.status,
            200,
            "case {listing}: the token-less listing must be served, got {} / {}",
            served.status,
            String::from_utf8_lossy(&served.body)
        );
    }
}

// =============================================================================
// 3. the hosted configuration version's optimistic-lock template
// =============================================================================

/// `POST …/hostedconfigurationversions`, with `LatestVersionNumber` travelling as the
/// `Latest-Version-Number` header the model binds it to.
async fn host_version(
    router: &MultiServiceRouter,
    application: &str,
    profile: &str,
    content: &str,
    latest_version_number: Option<i32>,
) -> Answer {
    let headers: Vec<(&str, String)> = latest_version_number
        .map(|number| vec![("Latest-Version-Number", number.to_string())])
        .unwrap_or_default();
    send(
        router,
        "POST",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
        ),
        "application/json",
        content.as_bytes().to_vec(),
        &headers,
    )
    .await
}

/// How many versions the profile holds, read back through `ListHostedConfigurationVersions`.
async fn hosted_version_count(
    router: &MultiServiceRouter,
    case: &str,
    application: &str,
    profile: &str,
) -> usize {
    let listed = json_call(
        router,
        "GET",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
        ),
        &Value::Null,
    )
    .await;
    assert_eq!(
        listed.status,
        200,
        "case {case:?}: the listing must be served, got {} / {}",
        listed.status,
        String::from_utf8_lossy(&listed.body)
    );
    listed
        .json()
        .get("Items")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default()
}

/// RULE 3: a stale `LatestVersionNumber` is refused with both version numbers in AWS's
/// template, and the write stores nothing.
#[tokio::test]
async fn a_stale_latest_version_number_states_the_expected_and_the_actual_version() {
    let router = router();
    let application = create_application(&router, "optimistic-lock").await;
    let profile = create_profile(&router, &application, "config").await;

    // The actual latest version grows as the test hosts more, so each row states the
    // number it expects beside the number that is really latest: the rows differ along
    // the DISTANCE and the DIRECTION between the two (just below, far below, just above,
    // far above) and along the actual latest version itself.
    let mut actual = 0;
    // One version to begin with, so the first round can name a version BELOW the latest
    // as well as above it.
    let seeded = host_version(&router, &application, &profile, "{\"seed\":true}", None).await;
    assert_eq!(
        seeded.status,
        201,
        "the first version must be hosted, got {} / {}",
        seeded.status,
        String::from_utf8_lossy(&seeded.body)
    );
    actual += 1;

    for round in 1..=3 {
        let written = host_version(
            &router,
            &application,
            &profile,
            &format!("{{\"round\":{round}}}"),
            None,
        )
        .await;
        assert_eq!(
            written.status,
            201,
            "round {round}: hosting a version must answer 201, got {} / {}",
            written.status,
            String::from_utf8_lossy(&written.body)
        );
        actual += 1;

        for submitted in [0, actual - 1, actual + 1, actual + 97] {
            let case = format!("expected {submitted} while {actual} is latest");
            let before = hosted_version_count(&router, &case, &application, &profile).await;
            let answer = host_version(
                &router,
                &application,
                &profile,
                "{\"stale\":true}",
                Some(submitted),
            )
            .await;
            let stated = refused(&case, &answer, 409, "ConflictException");
            let expected = format!(
                "Expected latest version {submitted} does not match actual latest version {actual}"
            );
            assert_eq!(
                stated, expected,
                "case {case:?}: expected message {expected:?}, got {stated:?}"
            );
            let after = hosted_version_count(&router, &case, &application, &profile).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused write stores nothing, so the profile must still \
                 hold {before} versions, got {after}"
            );
        }

        // The positive control: naming the version that IS latest is served, so the rows
        // above are refused for the number and nothing else.
        let accepted = host_version(
            &router,
            &application,
            &profile,
            "{\"fresh\":true}",
            Some(actual),
        )
        .await;
        assert_eq!(
            accepted.status,
            201,
            "round {round}: naming the actual latest version must be served, got {} / {}",
            accepted.status,
            String::from_utf8_lossy(&accepted.body)
        );
        actual += 1;
    }
}

// =============================================================================
// 4. the experiment definition's duplicate-name template
// =============================================================================

/// RULE 4: a name already in use in the application is refused with the quoted name and
/// the application id, and the refusal discloses nothing about the definition holding it.
#[tokio::test]
async fn a_duplicate_experiment_definition_name_states_the_quoted_name() {
    let router = router();
    let scope = scope(&router, "name-template").await;

    // Rows differ along the form of the contested name: short, at the 64-character bound
    // the model's `Name` states, non-ASCII, carrying spaces and punctuation, and carrying
    // the single quote character the template itself uses.
    let names: Vec<(&str, String)> = vec![
        ("short", "duplicate".to_string()),
        ("at the Name maximum", filler(64, 5)),
        ("non-ASCII", "重複した名前".to_string()),
        ("spaces and punctuation", "my experiment (v2)".to_string()),
        ("carrying a single quote", "ops'experiment".to_string()),
    ];

    for (shape, name) in &names {
        let case = format!("a name that is {shape}");
        let first = create_definition(
            &router,
            &scope.application,
            &definition_body(
                &scope,
                name,
                vec![treatment(50.0, None, flag(true))],
                treatment(50.0, None, flag(false)),
            ),
        )
        .await;
        assert_eq!(
            first.status,
            201,
            "case {case:?}: the first create must be served, got {} / {}",
            first.status,
            String::from_utf8_lossy(&first.body)
        );
        let holder = first
            .string("Id")
            .unwrap_or_else(|| panic!("case {case:?}: no Id in {}", first.json()));

        let conflict = create_definition(
            &router,
            &scope.application,
            &definition_body(
                &scope,
                name,
                vec![treatment(10.0, Some("a second attempt"), flag(false))],
                treatment(90.0, None, flag(true)),
            ),
        )
        .await;
        let stated = refused(&case, &conflict, 409, "ConflictException");
        let expected = format!(
            "Experiment definition with name '{name}' already exists for application {}",
            scope.application
        );
        assert_eq!(
            stated, expected,
            "case {case:?}: expected message {expected:?}, got {stated:?}"
        );
        assert!(
            !stated.contains(&holder),
            "case {case:?}: the colliding definition's id {holder:?} must not be disclosed, \
             got {stated:?}"
        );
    }
}

// =============================================================================
// 5. the validation clause quotes the value as submitted
// =============================================================================

/// One request whose only fault is an over-long member, and where the clause must place it.
struct TooLong {
    case: String,
    method: &'static str,
    uri: String,
    body: Value,
    /// The lowerCamel member path the clause names.
    member: &'static str,
    /// The value the request sent, which the clause must quote in full.
    value: String,
    /// The `@length` maximum the model declares for that member.
    maximum: usize,
}

/// A string of `count` copies of `glyph`, for a value whose CHARACTER count is what the
/// bound counts while its byte count is several times larger.
fn repeated(glyph: char, count: usize) -> String {
    std::iter::repeat_n(glyph, count).collect()
}

/// RULE 5: a constraint refusal quotes the value the caller submitted, whole.
#[tokio::test]
async fn a_constraint_refusal_quotes_the_value_the_caller_submitted() {
    let router = router();
    let application = create_application(&router, "echoed-values").await;
    let scope = scope(&router, "echoed-values").await;

    // `Description` is `length 0..1024` wherever this model binds it
    // (`smithy_query(query="operation", operation="UpdateExtension,CreateApplication,\
    // CreateConfigurationProfile,CreateExperimentDefinition")`), so every row below is one
    // character or more past 1024.
    const DESCRIPTION_MAXIMUM: usize = 1024;

    let extension_name = "echoed-values-extension";
    let created = create_extension(
        &router,
        &json!({
            "Name": extension_name,
            "Description": "before",
            "Actions": actions(&["ON_DEPLOYMENT_COMPLETE"]),
        }),
        None,
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "the extension under test must be created, got {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    let extension = minted_id(extension_name, &created.json());

    // Rows differ along the operation and the member the over-long value is bound to, and
    // along the value's size and encoding: one character past the bound, far past it, far
    // past the 256 characters the framework's own echo stops at, and multi-byte text whose
    // byte count is three times its character count.
    let one_past = filler(DESCRIPTION_MAXIMUM + 1, 1);
    let far_past = filler(4096, 2);
    let multi_byte = repeated('界', DESCRIPTION_MAXIMUM + 76);
    let profiles = format!("/applications/{application}/configurationprofiles");
    let experiments = format!("/applications/{}/experimentdefinitions", scope.application);

    let rows: Vec<TooLong> = vec![
        TooLong {
            case: "UpdateExtension, one character past the bound".to_string(),
            method: "PATCH",
            uri: format!("/extensions/{}", pct(&extension)),
            body: json!({ "Description": one_past }),
            member: "description",
            value: one_past.clone(),
            maximum: DESCRIPTION_MAXIMUM,
        },
        TooLong {
            case: "UpdateExtension, four times the bound".to_string(),
            method: "PATCH",
            uri: format!("/extensions/{}", pct(&extension)),
            body: json!({ "Description": far_past }),
            member: "description",
            value: far_past.clone(),
            maximum: DESCRIPTION_MAXIMUM,
        },
        TooLong {
            case: "UpdateExtension, multi-byte text".to_string(),
            method: "PATCH",
            uri: format!("/extensions/{}", pct(&extension)),
            body: json!({ "Description": multi_byte }),
            member: "description",
            value: multi_byte.clone(),
            maximum: DESCRIPTION_MAXIMUM,
        },
        TooLong {
            case: "CreateApplication's Description".to_string(),
            method: "POST",
            uri: "/applications".to_string(),
            body: json!({ "Name": "long-description", "Description": one_past }),
            member: "description",
            value: one_past.clone(),
            maximum: DESCRIPTION_MAXIMUM,
        },
        TooLong {
            case: "CreateConfigurationProfile's Description".to_string(),
            method: "POST",
            uri: profiles,
            body: json!({
                "Name": "long-description",
                "LocationUri": "hosted",
                "Description": far_past,
            }),
            member: "description",
            value: far_past.clone(),
            maximum: DESCRIPTION_MAXIMUM,
        },
        TooLong {
            case: "CreateExperimentDefinition's Hypothesis".to_string(),
            method: "POST",
            uri: experiments.clone(),
            body: {
                let mut body = definition_body(
                    &scope,
                    "long-hypothesis",
                    vec![treatment(50.0, None, flag(true))],
                    treatment(50.0, None, flag(false)),
                );
                body["Hypothesis"] = Value::from(one_past.clone());
                body
            },
            member: "hypothesis",
            value: one_past.clone(),
            maximum: DESCRIPTION_MAXIMUM,
        },
        TooLong {
            case: "CreateExperimentDefinition's AudienceDescription".to_string(),
            method: "POST",
            uri: experiments,
            body: {
                let mut body = definition_body(
                    &scope,
                    "long-audience-description",
                    vec![treatment(50.0, None, flag(true))],
                    treatment(50.0, None, flag(false)),
                );
                body["AudienceDescription"] = Value::from(multi_byte.clone());
                body
            },
            member: "audienceDescription",
            value: multi_byte.clone(),
            maximum: DESCRIPTION_MAXIMUM,
        },
    ];

    for row in &rows {
        let case = &row.case;
        let answer = json_call(&router, row.method, &row.uri, &row.body).await;
        let stated = refused(case, &answer, 400, "BadRequestException");
        let expected = format!(
            "1 validation error detected: Value '{}' at '{}' failed to satisfy constraint: \
             Member must have length less than or equal to {}",
            row.value, row.member, row.maximum
        );
        assert_eq!(
            stated, expected,
            "case {case:?}: the clause must quote all {} characters as submitted; expected \
             {expected:?}, got {stated:?}",
            row.value.chars().count()
        );
    }

    // The value AT the bound is served, on the same member and through the same operation,
    // so a row above is refused for its length and nothing else — and the extension that
    // refused three updates still holds the description and version it had.
    let read_back = get_extension(&router, &extension, None).await;
    assert_eq!(
        read_back.string("Description").as_deref(),
        Some("before"),
        "a refused update stores nothing, so the extension must still describe itself as \
         \"before\", got {}",
        read_back.json()
    );
    assert_eq!(
        read_back.json().get("VersionNumber").and_then(Value::as_i64),
        Some(1),
        "a refused update mints no version, got {}",
        read_back.json()
    );

    let at_the_bound = filler(DESCRIPTION_MAXIMUM, 4);
    let accepted = update_extension(
        &router,
        &extension,
        &json!({ "Description": at_the_bound, "VersionNumber": 1 }),
    )
    .await;
    assert_eq!(
        accepted.status,
        200,
        "a description AT the bound must be served, got {} / {}",
        accepted.status,
        String::from_utf8_lossy(&accepted.body)
    );
    assert_eq!(
        accepted.string("Description").as_deref(),
        Some(at_the_bound.as_str()),
        "the accepted description must be the one stored, got {}",
        accepted.json()
    );
}
