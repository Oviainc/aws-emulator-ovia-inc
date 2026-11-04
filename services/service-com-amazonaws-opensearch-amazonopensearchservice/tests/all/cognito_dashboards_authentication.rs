// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Amazon Cognito authentication a domain's OpenSearch
//! Dashboards is asked to be reached through — the `CognitoOptions` container
//! `CreateDomain` and `UpdateDomainConfig` both take.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees. The user pools the rows name are created through the Cognito user-pool
//! service's OWN `CreateUserPool`, mounted on the same shared engine
//! ([`Emulator::with_user_pools`]), so the pools under test are the ones that service
//! stores and a deleted pool is one its `DeleteUserPool` removed.
//!
//! The rule under test, asserted over a table rather than one call: a `CognitoOptions`
//! container that ENABLES Cognito authentication is accepted only when it names all
//! three resources the feature is served by — the user pool, the identity pool and the
//! role — and when the user pool it names both exists and hosts the managed login pages
//! the Dashboards sign-in redirects to, which a pool does only once a user pool domain
//! (prefix or custom) is created on it; anything else is refused with
//! `ValidationException` (HTTP 400) and neither creates nor changes a domain. A
//! container that leaves the feature disabled configures no sign-in path, so none of
//! that is required of it.
//!
//! The rows differ along what the rule generalises over: which required member is left
//! out, whether the named pool exists (one of several created pools, one never created,
//! one created and then deleted), which kind of hosted domain the pool holds (a prefix
//! domain, a custom domain, none at all, one created and then deleted), and whether the
//! container enables the feature at all — each row driven through BOTH operations that
//! take the container.

use bytes::Bytes;
use serde_json::{json, Value};

use super::domain_configuration_rules::{assert_no_domain, assert_refusal};
use super::domain_lifecycle::{
    baseline_create, create_baseline, Answer, Emulator, REGION, SETTLE_WAIT,
};
use super::service_software_update::message;

/// The error every refusal below carries: the operation's modelled 400 for a
/// configuration the service will not provision.
const REFUSAL: &str = "ValidationException";

/// A user pool id of the minted form (`UserPoolIdType`'s pattern `^[\w-]+_[0-9a-zA-Z]+$`)
/// that no `CreateUserPool` call in these tests created.
const UNCREATED_POOL: &str = "us-east-1_AAAAAAAAA";

/// A second uncreated id, of the same form but naming another Region and another
/// suffix: nothing resolves it either, and the refusal has to name the id the request
/// carried rather than one case's value.
const UNCREATED_POOL_ELSEWHERE: &str = "eu-west-1_z9Q8r7S6t";

/// What the service answers a container that enables Cognito authentication against a
/// user pool holding no hosted domain: the pool's own `Domain` is what is missing, so
/// the sentence names that member and carries no pool id.
const NO_HOSTED_DOMAIN: &str = "Domain needs to be specified for user pool";

/// One row of the table: the container to send, the case it stands for, and the refusal
/// sentence the service answers it with — `None` where the rule accepts it.
struct Row {
    case: &'static str,
    options: Value,
    refusal: Option<String>,
}

/// An identity pool id of the form `CognitoOptions$IdentityPoolId` takes (its modelled
/// pattern is `^[\w-]+:[0-9a-f-]+$`), with `suffix` as the pool's own identifier.
fn identity_pool_id(suffix: &str) -> String {
    format!("{REGION}:{suffix}")
}

/// The ARN of the role OpenSearch Service configures the pools with
/// (`CognitoOptions$RoleArn`, whose modelled pattern is an `iam::<account>:role/` ARN),
/// named `role` in the account every emulated resource belongs to.
fn role_arn(role: &str) -> String {
    format!("arn:aws:iam::{}:role/{role}", ::engine::emulator_account_id())
}

/// Call `operation` of the Cognito user-pool service mounted on the same router, the
/// way an SDK drives it: `POST /` with
/// `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`, signed for that service so
/// the router can name it.
async fn user_pool_call(emulator: &Emulator, operation: &str, body: Value) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cognito-idp.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/cognito-idp/aws4_request, \
                 SignedHeaders=host, Signature=0"
            ),
        )
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    emulator.dispatch(request).await
}

/// Create a user pool named `name` through the user-pool service and answer with the
/// `UserPoolId` it minted — the id a `CognitoOptions` container names.
async fn created_user_pool(emulator: &Emulator, name: &str) -> String {
    let answer = user_pool_call(emulator, "CreateUserPool", json!({ "PoolName": name })).await;
    assert_eq!(
        answer.status, 200,
        "case CreateUserPool({name:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case CreateUserPool({name:?}): expected a minted Id, got {:?}",
                answer.body
            )
        })
        .to_owned()
}

/// Delete the pool `user_pool_id` names through the user-pool service, so the id names
/// no pool afterwards.
async fn deleted_user_pool(emulator: &Emulator, user_pool_id: &str) {
    let answer = user_pool_call(
        emulator,
        "DeleteUserPool",
        json!({ "UserPoolId": user_pool_id }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case DeleteUserPool({user_pool_id:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
}

/// The ARN of the Certificate Manager certificate a CUSTOM user pool domain is served
/// with (`CustomDomainConfigType$CertificateArn`), in the account every emulated
/// resource belongs to.
fn certificate_arn(certificate: &str) -> String {
    format!(
        "arn:aws:acm:{REGION}:{}:certificate/{certificate}",
        ::engine::emulator_account_id()
    )
}

/// Create the user pool domain `domain` on `user_pool_id` through the user-pool
/// service's OWN `CreateUserPoolDomain` — a PREFIX domain when `certificate` is `None`
/// and a CUSTOM one when it names the certificate the custom domain is served with, the
/// two kinds of hosted domain a pool can hold.
async fn created_user_pool_domain(
    emulator: &Emulator,
    user_pool_id: &str,
    domain: &str,
    certificate: Option<&str>,
) {
    let mut body = json!({ "UserPoolId": user_pool_id, "Domain": domain });
    if let Some(certificate) = certificate {
        body["CustomDomainConfig"] = json!({ "CertificateArn": certificate_arn(certificate) });
    }
    let answer = user_pool_call(emulator, "CreateUserPoolDomain", body).await;
    assert_eq!(
        answer.status, 200,
        "case CreateUserPoolDomain({domain:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
}

/// Delete the hosted domain `domain` of `user_pool_id`, so the pool still exists but
/// hosts no managed login pages afterwards.
async fn deleted_user_pool_domain(emulator: &Emulator, user_pool_id: &str, domain: &str) {
    let answer = user_pool_call(
        emulator,
        "DeleteUserPoolDomain",
        json!({ "UserPoolId": user_pool_id, "Domain": domain }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case DeleteUserPoolDomain({domain:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
}

/// One emulator with both services mounted, and the pools the table below is written
/// against: one hosting a PREFIX domain, one hosting a CUSTOM domain, one hosting no
/// domain at all, one whose domain was created and then deleted, and one created-then-
/// deleted pool.
struct Fixture {
    emulator: Emulator,
    rows: Vec<Row>,
}

impl Fixture {
    async fn new() -> Self {
        let emulator = Emulator::with_user_pools();
        let prefix_hosted = created_user_pool(&emulator, "dashboards-users").await;
        created_user_pool_domain(&emulator, &prefix_hosted, "dashboards-login", None).await;
        let custom_hosted = created_user_pool(&emulator, "dashboards-users-two").await;
        created_user_pool_domain(
            &emulator,
            &custom_hosted,
            "dashboards-login-custom",
            Some("11111111-2222-3333-4444-555555555555"),
        )
        .await;
        let unhosted = created_user_pool(&emulator, "dashboards-users-unhosted").await;
        let domain_removed = created_user_pool(&emulator, "dashboards-users-domain-gone").await;
        created_user_pool_domain(&emulator, &domain_removed, "dashboards-login-gone", None).await;
        deleted_user_pool_domain(&emulator, &domain_removed, "dashboards-login-gone").await;
        let removed = created_user_pool(&emulator, "dashboards-users-gone").await;
        deleted_user_pool(&emulator, &removed).await;
        let rows = rows(
            &prefix_hosted,
            &custom_hosted,
            &unhosted,
            &domain_removed,
            &removed,
        );
        Self { emulator, rows }
    }
}

/// The table: `first` hosts a prefix domain and `second` a custom one, `unhosted` is a
/// pool that exists and hosts no domain, `domain_removed` one whose domain was deleted,
/// and `removed` an id whose pool was deleted.
fn rows(
    first: &str,
    second: &str,
    unhosted: &str,
    domain_removed: &str,
    removed: &str,
) -> Vec<Row> {
    vec![
        Row {
            case: "enabled, naming a pool hosting a prefix domain, an identity pool and a role",
            options: json!({
                "Enabled": true,
                "UserPoolId": first,
                "IdentityPoolId": identity_pool_id("11111111-2222-3333-4444-555555555555"),
                "RoleArn": role_arn("AmazonOpenSearchServiceCognitoAccess"),
            }),
            refusal: None,
        },
        Row {
            case: "enabled, naming a pool hosting a custom domain, another identity pool and \
                   another role",
            options: json!({
                "Enabled": true,
                "UserPoolId": second,
                "IdentityPoolId": identity_pool_id("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"),
                "RoleArn": role_arn("dashboards-cognito-access"),
            }),
            refusal: None,
        },
        Row {
            case: "enabled, naming a pool that hosts no domain",
            options: json!({
                "Enabled": true,
                "UserPoolId": unhosted,
                "IdentityPoolId": identity_pool_id("11111111-2222-3333-4444-555555555555"),
                "RoleArn": role_arn("AmazonOpenSearchServiceCognitoAccess"),
            }),
            refusal: Some(NO_HOSTED_DOMAIN.to_string()),
        },
        Row {
            case: "enabled, naming a pool whose hosted domain was deleted",
            options: json!({
                "Enabled": true,
                "UserPoolId": domain_removed,
                "IdentityPoolId": identity_pool_id("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"),
                "RoleArn": role_arn("dashboards-cognito-access"),
            }),
            refusal: Some(NO_HOSTED_DOMAIN.to_string()),
        },
        Row {
            case: "enabled, with no RoleArn",
            options: json!({
                "Enabled": true,
                "UserPoolId": first,
                "IdentityPoolId": identity_pool_id("11111111-2222-3333-4444-555555555555"),
            }),
            refusal: Some("RoleArn needs to be specified".to_string()),
        },
        Row {
            case: "enabled, with no IdentityPoolId",
            options: json!({
                "Enabled": true,
                "UserPoolId": first,
                "RoleArn": role_arn("AmazonOpenSearchServiceCognitoAccess"),
            }),
            refusal: Some("IdentityPoolId needs to be specified".to_string()),
        },
        Row {
            case: "enabled, with no UserPoolId",
            options: json!({
                "Enabled": true,
                "IdentityPoolId": identity_pool_id("11111111-2222-3333-4444-555555555555"),
                "RoleArn": role_arn("AmazonOpenSearchServiceCognitoAccess"),
            }),
            refusal: Some("UserPoolId needs to be specified".to_string()),
        },
        Row {
            case: "enabled, carrying nothing else",
            options: json!({ "Enabled": true }),
            refusal: Some("UserPoolId needs to be specified".to_string()),
        },
        Row {
            case: "enabled, naming a user pool nothing created",
            options: json!({
                "Enabled": true,
                "UserPoolId": UNCREATED_POOL,
                "IdentityPoolId": identity_pool_id("11111111-2222-3333-4444-555555555555"),
                "RoleArn": role_arn("AmazonOpenSearchServiceCognitoAccess"),
            }),
            refusal: Some(format!("User pool {UNCREATED_POOL} does not exist.")),
        },
        Row {
            case: "enabled, naming another uncreated pool of another Region",
            options: json!({
                "Enabled": true,
                "UserPoolId": UNCREATED_POOL_ELSEWHERE,
                "IdentityPoolId": identity_pool_id("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"),
                "RoleArn": role_arn("dashboards-cognito-access"),
            }),
            refusal: Some(format!("User pool {UNCREATED_POOL_ELSEWHERE} does not exist.")),
        },
        Row {
            case: "enabled, naming a pool that was created and then deleted",
            options: json!({
                "Enabled": true,
                "UserPoolId": removed,
                "IdentityPoolId": identity_pool_id("11111111-2222-3333-4444-555555555555"),
                "RoleArn": role_arn("AmazonOpenSearchServiceCognitoAccess"),
            }),
            refusal: Some(format!("User pool {removed} does not exist.")),
        },
        Row {
            case: "disabled, naming an uncreated pool and carrying no role",
            options: json!({ "Enabled": false, "UserPoolId": UNCREATED_POOL }),
            refusal: None,
        },
        Row {
            case: "Enabled omitted, naming an uncreated pool and carrying no role",
            options: json!({ "UserPoolId": UNCREATED_POOL }),
            refusal: None,
        },
        Row {
            case: "disabled, naming a pool that exists and hosts no domain",
            options: json!({ "Enabled": false, "UserPoolId": unhosted }),
            refusal: None,
        },
        Row {
            case: "an empty container",
            options: json!({}),
            refusal: None,
        },
    ]
}

/// The `CognitoOptions` container the service HOLDS once a request has sent `options`:
/// the members as sent, with `Enabled` filled in where the request left it out — the
/// service provisions a domain with a whole configuration, and Cognito authentication
/// for OpenSearch Dashboards is off unless a request turns it on.
fn held(options: &Value) -> Value {
    let mut held = options.clone();
    if held.get("Enabled").is_none() {
        held["Enabled"] = json!(false);
    }
    held
}

/// The `CognitoOptions` container `DescribeDomainConfig` reports for `name`, which is
/// `Value::Null` for a domain that carries none.
async fn stored_options(emulator: &Emulator, name: &str, case: &str) -> Value {
    let answer = emulator.describe_config(name).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: DescribeDomainConfig expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["DomainConfig"]["CognitoOptions"]["Options"].clone()
}

#[tokio::test]
async fn create_domain_judges_the_cognito_authentication_it_is_handed() {
    let fixture = Fixture::new().await;

    for (index, row) in fixture.rows.iter().enumerate() {
        let name = format!("cognito-create-{index}");
        let case = format!("CreateDomain({}, {})", name, row.case);
        let mut body = baseline_create(&name);
        body["CognitoOptions"] = row.options.clone();
        let answer = fixture.emulator.create(body).await;

        let Some(expected) = row.refusal.as_deref() else {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected the configuration to be accepted, got {} with {:?}",
                answer.status, answer.body
            );
            // The accepted container is stored and reported back member for member,
            // through both readers of the record.
            assert_eq!(
                answer.body["DomainStatus"]["CognitoOptions"],
                held(&row.options),
                "case {case}: expected the sent container, got {:?}",
                answer.body["DomainStatus"]["CognitoOptions"]
            );
            let described = fixture.emulator.describe(&name).await;
            assert_eq!(
                described.body["DomainStatus"]["CognitoOptions"],
                held(&row.options),
                "case {case}: DescribeDomain expected the sent container, got {:?}",
                described.body["DomainStatus"]["CognitoOptions"]
            );
            assert_eq!(
                stored_options(&fixture.emulator, &name, &case).await,
                held(&row.options),
                "case {case}: DescribeDomainConfig expected the sent container"
            );
            continue;
        };

        assert_refusal(&answer, REFUSAL, &case);
        assert_eq!(
            message(&answer, &case),
            expected,
            "case {case}: expected the refusal to state what is wrong with the container"
        );
        // A refused create stores nothing: the domain the request named does not exist.
        assert_no_domain(&fixture.emulator, &name).await;
    }
}

#[tokio::test]
async fn update_domain_config_judges_the_cognito_authentication_it_is_handed() {
    let fixture = Fixture::new().await;
    let name = "cognito-update";
    create_baseline(&fixture.emulator, name).await;
    // The service applies one change of a domain at a time, so the change the create
    // started has to land before a row's update can start another.
    tokio::time::sleep(SETTLE_WAIT).await;

    for row in &fixture.rows {
        let case = format!("UpdateDomainConfig({})", row.case);
        let before = stored_options(&fixture.emulator, name, &case).await;
        let answer = fixture
            .emulator
            .update_config(name, json!({ "CognitoOptions": row.options.clone() }))
            .await;

        let Some(expected) = row.refusal.as_deref() else {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected the configuration to be accepted, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.body["DomainConfig"]["CognitoOptions"]["Options"],
                held(&row.options),
                "case {case}: expected the sent container, got {:?}",
                answer.body["DomainConfig"]["CognitoOptions"]["Options"]
            );
            assert_eq!(
                stored_options(&fixture.emulator, name, &case).await,
                held(&row.options),
                "case {case}: DescribeDomainConfig expected the sent container"
            );
            // And this row's change lands before the next row starts another.
            tokio::time::sleep(SETTLE_WAIT).await;
            continue;
        };

        assert_refusal(&answer, REFUSAL, &case);
        assert_eq!(
            message(&answer, &case),
            expected,
            "case {case}: expected the refusal to state what is wrong with the container"
        );
        // A refused update changes nothing: the domain keeps the configuration it had.
        assert_eq!(
            stored_options(&fixture.emulator, name, &case).await,
            before,
            "case {case}: a refused update must leave the stored container untouched"
        );
    }
}

#[tokio::test]
async fn a_dry_run_update_is_judged_by_the_same_rule() {
    let fixture = Fixture::new().await;
    let name = "cognito-dry-run";
    create_baseline(&fixture.emulator, name).await;

    for row in fixture.rows.iter().filter(|row| row.refusal.is_some()) {
        let case = format!("UpdateDomainConfig(DryRun, {})", row.case);
        let expected = row.refusal.as_deref().expect("the row carries a refusal");
        let answer = fixture
            .emulator
            .update_config(
                name,
                json!({ "CognitoOptions": row.options.clone(), "DryRun": true }),
            )
            .await;
        assert_refusal(&answer, REFUSAL, &case);
        assert_eq!(
            message(&answer, &case),
            expected,
            "case {case}: a dry run reports what applying the change would do, and this \
             change cannot be applied"
        );
    }
}
