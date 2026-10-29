// SPDX-License-Identifier: CC0-1.0

//! How an `ApplicationProviderArn` that names no PUBLISHED application provider is
//! REPORTED, end to end over the `awsJson1_1` wire.
//!
//! The rule under test: an `ApplicationProviderArn` outside the published catalogue is ONE
//! condition — the provider does not exist — so every operation that resolves such an ARN
//! states it with the service's single detail text for that condition, the text naming the
//! ARN the request sent, whichever operation a client called. A client that branches on
//! the detail text — the only member these error shapes carry — therefore reads one
//! provider ARN the same way everywhere. The condition stays distinct from the other
//! refusal a provider ARN can earn: a provider the catalogue DOES publish but which backs
//! no customer managed OAuth 2.0 application is a `ValidationException` naming that
//! provider, not this not-found.
//!
//! The tables span what the rule generalises over: every operation of this service that
//! resolves an `ApplicationProviderArn` (`DescribeApplicationProvider`, which looks one up
//! directly, and `CreateApplication`, which resolves one before minting an application),
//! crossed with the ARN forms the modelled `ApplicationProviderArn` pattern
//! `^arn:aws(-[a-z]{1,5}){0,3}:sso::aws:applicationProvider/[a-zA-Z0-9-/]+$` admits — a
//! bare id, a `catalog/<Name>` id, a published id with a suffix, an id spanning the whole
//! `[a-zA-Z0-9-/]` class, and several partitions. The issue's one ARN is one row of it.
//!
//! The expected sentence is NOT read from the code under test: it is the recorded AWS
//! wording for this error class (`DescribeApplicationProvider` of an ARN the catalogue
//! does not publish answers `ResourceNotFoundException` with "No application provider with
//! arn '<arn>' was found"), written out here as a template whose only variable part is the
//! ARN the request sent. The status is the 400 this awsJson1_1 service answers every
//! refusal of a request with (`crate::scaffolded::modeled_error_status`).
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes. The harness is the one `ssoadmin_permission_sets.rs` owns.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{call, error_code, router_over, seeded_instance_arn, with_store};

/// Provider ARNs the published catalogue holds nothing for. SOURCE: the model's
/// `ApplicationProviderArn` pattern
/// `^arn:aws(-[a-z]{1,5}){0,3}:sso::aws:applicationProvider/[a-zA-Z0-9-/]+$` (len
/// 10..1224) — the rows span a bare id, a `catalog/<Name>` id, a published id carrying a
/// suffix, an id built from the whole `[a-zA-Z0-9-/]` class, a one-character id and three
/// partitions the pattern admits. Each is checked against the catalogue below, so a
/// provider added to it cannot silently turn a row into a published one.
const UNKNOWN_PROVIDER_ARNS: [&str; 6] = [
    // The shape of the ARN the issue reported.
    "arn:aws:sso::aws:applicationProvider/no-such-provider",
    "arn:aws:sso::aws:applicationProvider/catalog/NotAPublishedApplication",
    "arn:aws:sso::aws:applicationProvider/customx",
    "arn:aws-cn:sso::aws:applicationProvider/Zz09-aA/9",
    "arn:aws-us-gov:sso::aws:applicationProvider/q",
    "arn:aws-iso-b-x:sso::aws:applicationProvider/catalog/NotAProviderEither",
];

/// The provider a CUSTOMER MANAGED OAuth 2.0 application is created under, so a row that
/// has to succeed is not confused with a row that has to be refused.
const CUSTOMER_MANAGED_PROVIDER_ARN: &str = "arn:aws:sso::aws:applicationProvider/custom";

/// The detail text AWS answers this one condition with, as a template over the ARN the
/// request sent. SOURCE: the recorded `DescribeApplicationProvider` response for a
/// syntactically valid ARN the catalogue does not publish.
fn expected_message(application_provider_arn: &str) -> String {
    format!("No application provider with arn '{application_provider_arn}' was found")
}

/// One call that resolves an `ApplicationProviderArn`: the operation and the body it takes.
struct Call {
    label: String,
    operation: &'static str,
    body: Value,
}

/// Every operation of this service that resolves an `ApplicationProviderArn`, as the one
/// request each takes for `provider_arn`. Each body satisfies the operation's modelled
/// constraints and names an instance this account holds, so the only thing wrong with it
/// is the provider it names.
fn provider_arn_calls(instance_arn: &str, provider_arn: &str) -> Vec<Call> {
    let rows: Vec<(&'static str, Value)> = vec![
        (
            "DescribeApplicationProvider",
            json!({"ApplicationProviderArn": provider_arn}),
        ),
        (
            "CreateApplication",
            json!({
                "InstanceArn": instance_arn,
                "ApplicationProviderArn": provider_arn,
                "Name": "AnApplication",
            }),
        ),
    ];
    rows.into_iter()
        .map(|(operation, body)| Call {
            label: format!("{operation} on {provider_arn}"),
            operation,
            body,
        })
        .collect()
}

/// The detail text of a refusal, from the member this `awsJson1_1` model carries it in.
fn message(body: &Value) -> String {
    body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// Call `case` and answer the detail text it was refused with, having asserted the refusal
/// is the modelled not-found at the status this service answers a refused request with.
async fn not_found_text(router: &MultiServiceRouter, case: &Call) -> String {
    let (status, body) = call(router, case.operation, &case.body).await;
    assert_eq!(
        status, 400,
        "case {:?}: a provider ARN the catalogue does not publish must be refused 400, got \
         {status} {body}",
        case.label
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case {:?}: expected ResourceNotFoundException, got {body}",
        case.label
    );
    let text = message(&body);
    assert!(
        !text.is_empty(),
        "case {:?}: the refusal must carry its detail text, got {body}",
        case.label
    );
    text
}

/// None of the rows may name a provider the catalogue publishes, or the row would be
/// asserting the wrong condition.
#[test]
fn every_row_names_a_provider_the_catalogue_does_not_publish() {
    for arn in UNKNOWN_PROVIDER_ARNS {
        assert!(
            state::built_in_application_provider(arn).is_none(),
            "case {arn:?}: this row must name no published provider, yet the catalogue \
             holds one"
        );
    }
    assert!(
        state::built_in_application_provider(CUSTOMER_MANAGED_PROVIDER_ARN).is_some(),
        "case {CUSTOMER_MANAGED_PROVIDER_ARN:?}: the catalogue must publish the custom \
         application provider"
    );
}

/// THE RULE: for one provider ARN the catalogue does not publish, every operation that
/// resolves it answers with the SAME detail text — so the text cannot depend on the
/// operation — and that text is AWS's sentence for this condition, naming the ARN sent.
#[tokio::test]
async fn one_unknown_provider_arn_reads_the_same_whichever_operation_is_called() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(engine.clone());
    let instance_arn = seeded_instance_arn(&router).await;

    for provider_arn in UNKNOWN_PROVIDER_ARNS {
        let expected = expected_message(provider_arn);
        let cases = provider_arn_calls(&instance_arn, provider_arn);
        assert!(
            !cases.is_empty(),
            "case {provider_arn:?}: the table must drive at least one operation"
        );
        for case in &cases {
            let text = not_found_text(&router, case).await;
            assert_eq!(
                text, expected,
                "case {:?}: one unknown ApplicationProviderArn must read AWS's sentence \
                 for this condition, got {text:?} instead of {expected:?}",
                case.label
            );
        }
    }

    assert_eq!(
        with_store(&engine, |store| store.applications.len()),
        0,
        "a create refused on its provider must have stored nothing"
    );
}

/// The text is BUILT from the request: two unknown provider ARNs earn two refusals that
/// differ only in the ARN they name, so nothing about the reporting is a constant that
/// happens to contain one recorded ARN.
#[tokio::test]
async fn the_refusal_names_the_provider_arn_the_request_sent() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(engine.clone());
    let instance_arn = seeded_instance_arn(&router).await;

    let mut texts: Vec<(&str, String)> = Vec::new();
    for provider_arn in UNKNOWN_PROVIDER_ARNS {
        let case = Call {
            label: format!("CreateApplication on {provider_arn}"),
            operation: "CreateApplication",
            body: json!({
                "InstanceArn": instance_arn,
                "ApplicationProviderArn": provider_arn,
                "Name": "AnApplication",
            }),
        };
        texts.push((provider_arn, not_found_text(&router, &case).await));
    }

    for (provider_arn, text) in &texts {
        assert!(
            text.contains(provider_arn),
            "case {provider_arn:?}: the refusal must name this ARN, got {text:?}"
        );
        // The template is the same sentence for every row: replacing the ARN with another
        // row's ARN yields exactly that row's text.
        for (other_arn, other_text) in &texts {
            let rewritten = text.replace(provider_arn, other_arn);
            assert_eq!(
                &rewritten, other_text,
                "case {provider_arn:?}: the ARN is the only part of the refusal that \
                 varies, got {rewritten:?} instead of {other_text:?}"
            );
        }
    }
}

/// The two conditions stay DISTINCT: a provider the catalogue publishes is never reported
/// as missing — it is described, and a create under one that backs no customer managed
/// OAuth 2.0 application is the `ValidationException` naming that provider — so unifying
/// the not-found sentence did not fold the published half into it.
#[tokio::test]
async fn a_published_provider_is_never_reported_as_missing() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(engine.clone());
    let instance_arn = seeded_instance_arn(&router).await;

    // Published providers of each kind the catalogue holds, read from the crate so an
    // entry added to it is covered: the customer managed one, a pre-integrated SAML 2.0
    // catalog application, and an Amazon Web Services managed service provider.
    let published: Vec<String> = state::BUILT_IN_APPLICATION_PROVIDERS
        .iter()
        .map(|provider| format!("arn:aws:sso::aws:applicationProvider/{}", provider.id))
        .collect();
    assert!(
        published.len() > 2,
        "the catalogue must publish several providers, it publishes {}",
        published.len()
    );

    for arn in &published {
        let (status, body) = call(
            &router,
            "DescribeApplicationProvider",
            &json!({"ApplicationProviderArn": arn}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {arn:?}: a published provider must be described, got {status} {body}"
        );
        assert_ne!(
            message(&body),
            expected_message(arn),
            "case {arn:?}: a published provider must never read as missing, got {body}"
        );
    }

    // The published half of the create: refused on the modelled 400 client-input channel
    // for a provider that backs no customer managed OAuth 2.0 application, and otherwise
    // minted — never the not-found sentence.
    for arn in &published {
        let (status, body) = call(
            &router,
            "CreateApplication",
            &json!({
                "InstanceArn": instance_arn,
                "ApplicationProviderArn": arn,
                "Name": "PublishedProviderApp",
            }),
        )
        .await;
        let code = error_code(&body);
        assert_ne!(
            code, "ResourceNotFoundException",
            "case {arn:?}: a published provider must not be reported as missing, got \
             {status} {body}"
        );
        if status == 200 {
            assert_eq!(
                arn, CUSTOMER_MANAGED_PROVIDER_ARN,
                "case {arn:?}: only the custom application provider backs a create, got \
                 {body}"
            );
        } else {
            assert_eq!(
                code, "ValidationException",
                "case {arn:?}: a published provider that backs no create is refused on the \
                 client-input channel, got {status} {body}"
            );
            assert!(
                message(&body).contains(arn.as_str()),
                "case {arn:?}: the refusal must name the provider it refused, got {body}"
            );
        }
    }
}

/// A refusal changes nothing a later caller sees: the same unknown provider ARN reads the
/// same way twice, the catalogue still reports every entry, and nothing was stored.
#[tokio::test]
async fn a_refused_call_leaves_the_catalogue_as_it_was() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(engine.clone());
    let instance_arn = seeded_instance_arn(&router).await;

    let (status, before) = call(
        &router,
        "DescribeApplicationProvider",
        &json!({"ApplicationProviderArn": CUSTOMER_MANAGED_PROVIDER_ARN}),
    )
    .await;
    assert_eq!(
        status, 200,
        "the custom application provider must be described, got {status} {before}"
    );

    for provider_arn in UNKNOWN_PROVIDER_ARNS {
        for case in provider_arn_calls(&instance_arn, provider_arn) {
            let first = not_found_text(&router, &case).await;
            let again = not_found_text(&router, &case).await;
            assert_eq!(
                first, again,
                "case {:?}: the same call must read the same way twice, got {first:?} then \
                 {again:?}",
                case.label
            );
        }
    }

    let (status, after) = call(
        &router,
        "DescribeApplicationProvider",
        &json!({"ApplicationProviderArn": CUSTOMER_MANAGED_PROVIDER_ARN}),
    )
    .await;
    assert_eq!(
        status, 200,
        "the custom application provider must still be described, got {status} {after}"
    );
    assert_eq!(
        before, after,
        "a refused call must change nothing a later caller sees, got {before} then {after}"
    );
    assert_eq!(
        with_store(&engine, |store| store.applications.len()),
        0,
        "every refused create must have stored nothing"
    );
}
