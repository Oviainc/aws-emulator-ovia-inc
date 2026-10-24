// SPDX-License-Identifier: CC0-1.0

//! Which application provider `CreateApplication` will mint an application under, end to
//! end over the `awsJson1_1` wire.
//!
//! The rule under test: the application this operation creates is an OAuth 2.0 CUSTOMER
//! MANAGED one, so it mints an application only under the published provider through
//! which a customer manages an OAuth 2.0 application; every other published provider —
//! one that federates over SAML 2.0, and an Amazon Web Services managed provider whose
//! applications the service that owns it creates on the customer's behalf — is refused
//! with the modelled `ValidationException` (HTTP 400) and stores nothing, while an ARN
//! naming no published provider at all stays the `ResourceNotFoundException` (404) a
//! provider lookup answers. Refusing a create changes nothing about the provider itself:
//! `DescribeApplicationProvider` reports every entry of the catalogue either way.
//!
//! The tables span what the rule generalises over rather than the one provider the issue
//! cited: EVERY entry of the published catalogue (read from the crate, so a provider added
//! to it is covered without editing this test — the pre-integrated catalog applications
//! and the Amazon Web Services service providers alike, including entries the issue did
//! not cite), the partition the provider ARN is written in (the modelled pattern admits
//! `arn:aws(-[a-z]{1,5}){0,3}`), and, on the accepted side, the `Name`, `Description`,
//! `Status`, `PortalOptions` and `Tags` forms the model admits.
//!
//! The expectation is NOT read from the predicate under test: a create is expected to
//! succeed exactly for the custom application provider
//! (`arn:aws:sso::aws:applicationProvider/custom`, "SSO Custom Application Provider"),
//! which is the provider a customer managed application is created under, and the SAML
//! half is cross-checked against each provider's own `FederationProtocol` as
//! `DescribeApplicationProvider` reports it.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints,
//! the `awsJson1_1` envelope and the `X-Amz-Target` routing are exercised rather than
//! bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    call, error_code, router, router_over, seeded_instance_arn, with_store,
};

/// The id of the one provider a CUSTOMER MANAGED OAuth 2.0 application is created under:
/// IAM Identity Center's custom application provider, the "SSO Custom Application
/// Provider" entry of the published catalogue (`state::BUILT_IN_APPLICATION_PROVIDERS`)
/// and the provider `CreateApplication`'s own documentation describes the operation with.
const CUSTOMER_MANAGED_PROVIDER_ID: &str = "custom";

/// `FederationProtocol::SAML`, as `DescribeApplicationProvider` reports it: the protocol
/// of a provider whose applications are SAML 2.0 ones, which this API does not create.
const FEDERATION_SAML: &str = "SAML";

/// A provider ARN in the given partition, for an id of the published catalogue.
fn provider_arn(partition: &str, id: &str) -> String {
    format!("arn:{partition}:sso::aws:applicationProvider/{id}")
}

/// The `CreateApplication` request for one provider ARN.
fn create_request(instance_arn: &str, provider: &str, name: &str) -> Value {
    json!({
        "InstanceArn": instance_arn,
        "ApplicationProviderArn": provider,
        "Name": name,
    })
}

/// One `CreateApplication` call: `(status, body)`.
async fn create(router: &MultiServiceRouter, request: &Value) -> (u16, Value) {
    call(router, "CreateApplication", request).await
}

/// `DescribeApplicationProvider` for one provider ARN.
async fn describe_provider(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(
        router,
        "DescribeApplicationProvider",
        &json!({"ApplicationProviderArn": arn}),
    )
    .await
}

/// `DescribeApplication` for one application ARN.
async fn describe_application(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(router, "DescribeApplication", &json!({"ApplicationArn": arn})).await
}

/// Every entry of the published catalogue, as `(id, whether a create may be minted under
/// it)`. The verdict comes from the rule — only the custom application provider backs a
/// customer managed OAuth 2.0 application — and not from the crate's own predicate, so
/// this is an independent expectation; the ids come from the crate so a provider added to
/// the catalogue is covered here.
fn catalogue() -> Vec<(&'static str, bool)> {
    state::BUILT_IN_APPLICATION_PROVIDERS
        .iter()
        .map(|provider| (provider.id, provider.id == CUSTOMER_MANAGED_PROVIDER_ID))
        .collect()
}

/// The rule over the WHOLE published catalogue: a create under the custom application
/// provider is minted, a create under any other published provider is refused 400
/// `ValidationException`, and the catalogue entry is unchanged either way.
#[tokio::test]
async fn only_the_customer_managed_provider_backs_a_create() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(engine.clone());
    let instance_arn = seeded_instance_arn(&router).await;

    let mut minted = 0usize;
    let mut refused = 0usize;
    for (id, accepted) in catalogue() {
        let arn = provider_arn("aws", id);
        let case = format!("provider {id}");

        // The provider itself: it exists, and it is reported whatever the create answers.
        let (status, provider) = describe_provider(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the catalogue entry must be described, got {status} {provider}"
        );
        let federation_protocol = provider["FederationProtocol"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let (status, body) = create(
            &router,
            &create_request(&instance_arn, &arn, &format!("app-under-{minted}-{refused}")),
        )
        .await;

        if accepted {
            minted += 1;
            assert_eq!(
                status, 200,
                "case {case:?}: a customer managed OAuth 2.0 application must be created, \
                 got {status} {body}"
            );
            let application_arn = body["ApplicationArn"].as_str().unwrap_or_default();
            assert!(
                !application_arn.is_empty(),
                "case {case:?}: an accepted create must mint an ApplicationArn, got {body}"
            );
            let (status, described) = describe_application(&router, application_arn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the created application must read back, got {status} \
                 {described}"
            );
            assert_eq!(
                described["ApplicationProviderArn"].as_str(),
                Some(arn.as_str()),
                "case {case:?}: the describe must report the provider the create ran \
                 under, got {described}"
            );
        } else {
            refused += 1;
            assert_eq!(
                status, 400,
                "case {case:?}: a provider that backs no customer managed OAuth 2.0 \
                 application must be refused 400, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body}"
            );
            assert!(
                body["ApplicationArn"].is_null(),
                "case {case:?}: a refused create mints no ApplicationArn, got {body}"
            );
            let message = body["Message"].as_str().unwrap_or_default();
            assert!(
                message.contains(&arn),
                "case {case:?}: the refusal must name the provider it refused, got {body}"
            );
        }

        // The SAML half of the rule, read from the wire rather than from the expectation
        // above: a provider that federates over SAML 2.0 never backs a create.
        if federation_protocol == FEDERATION_SAML {
            assert!(
                !accepted,
                "case {case:?}: a SAML 2.0 provider must back no create, yet the \
                 expectation accepted it"
            );
        }

        // The entry still reports the same thing after the create was answered.
        let (status, again) = describe_provider(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the provider must survive the create, got {status} {again}"
        );
        assert_eq!(
            again, provider,
            "case {case:?}: a create must not change what the provider reports, got {again}"
        );
    }

    assert_eq!(
        minted, 1,
        "exactly one entry of the catalogue backs a create, the custom application \
         provider; the expectation accepted {minted}"
    );
    assert!(
        refused > 1,
        "the catalogue must hold providers that back no create, it refused {refused}"
    );
    assert_eq!(
        with_store(&engine, |store| store.applications.len()),
        minted,
        "every refused create must have stored nothing"
    );
}

/// The same rule in every partition the modelled `ApplicationProviderArn` pattern admits
/// (`^arn:aws(-[a-z]{1,5}){0,3}:…`): the provider's id is what decides, not the partition
/// the ARN is written in.
#[tokio::test]
async fn the_rule_holds_in_every_partition_the_pattern_admits() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;

    // Partitions the pattern admits, spanning no suffix, one suffix and three.
    for (index, partition) in ["aws", "aws-cn", "aws-us-gov", "aws-iso-b-x"]
        .into_iter()
        .enumerate()
    {
        let accepted = provider_arn(partition, CUSTOMER_MANAGED_PROVIDER_ID);
        let (status, body) = create(
            &router,
            &create_request(&instance_arn, &accepted, &format!("partition-app-{index}")),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {partition:?}: the custom provider backs a create in this partition, \
             got {status} {body}"
        );

        // A pre-integrated SAML 2.0 catalog application and an Amazon Web Services
        // managed service provider, written in the same partition.
        for id in ["catalog/Statuspage", "athena"] {
            let arn = provider_arn(partition, id);
            let case = format!("{id} in {partition}");
            let (status, body) = create(
                &router,
                &create_request(&instance_arn, &arn, "refused-partition-app"),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: this provider backs no create, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body}"
            );
        }
    }
}

/// The two refusal channels stay distinct: a published provider that backs no create is a
/// `ValidationException`, while an ARN naming no published provider at all is a
/// `ResourceNotFoundException` — and neither stores anything.
#[tokio::test]
async fn an_unpublished_provider_stays_a_not_found() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(engine.clone());
    let instance_arn = seeded_instance_arn(&router).await;

    // (case, provider id, the error code the create must answer with) — published
    // providers of each kind, then ids the catalogue does not publish at all.
    let cases: &[(&str, &str, &str)] = &[
        (
            "a pre-integrated SAML 2.0 catalog application",
            "catalog/Zoom",
            "ValidationException",
        ),
        (
            "an Amazon Web Services managed provider with access scopes",
            "s3",
            "ValidationException",
        ),
        (
            "an Amazon Web Services managed provider without access scopes",
            "emr-serverless",
            "ValidationException",
        ),
        (
            "an id the catalogue does not publish",
            "not-a-published-provider",
            "ResourceNotFoundException",
        ),
        (
            "a catalog id the catalogue does not publish",
            "catalog/NotAPublishedApplication",
            "ResourceNotFoundException",
        ),
        (
            "the custom provider's id with a suffix",
            "customx",
            "ResourceNotFoundException",
        ),
    ];

    for (case, id, expected) in cases {
        let arn = provider_arn("aws", id);
        let (status, body) = create(
            &router,
            &create_request(&instance_arn, &arn, "refused-channel-app"),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: the create must be refused 400, got {status} {body}"
        );
        assert_eq!(
            &error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {body}"
        );
        assert!(
            body["ApplicationArn"].is_null(),
            "case {case:?}: a refusal mints no ApplicationArn, got {body}"
        );
    }

    assert_eq!(
        with_store(&engine, |store| store.applications.len()),
        0,
        "every refused create must have stored nothing"
    );
}

/// The accepted side is not narrowed by anything else: under the custom application
/// provider every form of the request the model admits is still created and reads back,
/// with the provider the create ran under reported.
#[tokio::test]
async fn the_accepted_provider_still_serves_every_form_of_the_request() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let provider = provider_arn("aws", CUSTOMER_MANAGED_PROVIDER_ID);

    // The rows differ along the members `CreateApplication` declares beside the provider:
    // the length of `Name` (`len 1..100`), whether `Description` (`len 1..128`) is sent,
    // both values of `ApplicationStatus` (`ENABLED | DISABLED`), the `PortalOptions`
    // forms the model admits (absent, visibility only, each `SignInOrigin`) and the
    // number of `Tags` (`len 0..75`) — all from
    // `smithy_query(query="operation",operation="CreateApplication")`.
    let rows: Vec<(&str, Value)> = vec![
        (
            "the shortest Name the model admits, nothing else sent",
            json!({"InstanceArn": instance_arn, "ApplicationProviderArn": provider, "Name": "a"}),
        ),
        (
            "a DISABLED application with a description and three tags",
            json!({
                "InstanceArn": instance_arn,
                "ApplicationProviderArn": provider,
                "Name": format!("Reporting {}", "y".repeat(89)),
                "Description": "d",
                "Status": "DISABLED",
                "Tags": [
                    {"Key": "owner", "Value": "platform"},
                    {"Key": "Env", "Value": "prod"},
                    {"Key": "cost-centre", "Value": "7"},
                ],
            }),
        ),
        (
            "portal visibility only",
            json!({
                "InstanceArn": instance_arn,
                "ApplicationProviderArn": provider,
                "Name": "visible-app",
                "PortalOptions": {"Visibility": "DISABLED"},
            }),
        ),
        (
            "an IDENTITY_CENTER sign-in origin",
            json!({
                "InstanceArn": instance_arn,
                "ApplicationProviderArn": provider,
                "Name": "idp-initiated",
                "PortalOptions": {"SignInOptions": {"Origin": "IDENTITY_CENTER"}},
            }),
        ),
        (
            "an APPLICATION sign-in origin with an ApplicationUrl",
            json!({
                "InstanceArn": instance_arn,
                "ApplicationProviderArn": provider,
                "Name": "redirecting",
                "PortalOptions": {
                    "SignInOptions": {
                        "Origin": "APPLICATION",
                        "ApplicationUrl": "https://app.example.com/sso/start",
                    },
                    "Visibility": "ENABLED",
                },
            }),
        ),
    ];

    for (case, request) in rows {
        let (status, body) = create(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the create must answer 200, got {status} {body}"
        );
        let application_arn = body["ApplicationArn"].as_str().unwrap_or_default().to_string();
        let (status, described) = describe_application(&router, &application_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the created application must read back, got {status} {described}"
        );
        assert_eq!(
            described["ApplicationProviderArn"].as_str(),
            Some(provider.as_str()),
            "case {case:?}: the describe must report the provider the create ran under, \
             got {described}"
        );
        assert_eq!(
            described["Name"].as_str(),
            request["Name"].as_str(),
            "case {case:?}: the describe must report the stored Name, got {described}"
        );
    }
}
