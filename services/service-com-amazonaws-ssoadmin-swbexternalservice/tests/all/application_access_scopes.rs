// SPDX-License-Identifier: CC0-1.0

//! The application / access-scope slice end to end over the `awsJson1_1` wire.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over the one
//! example the issue cited:
//!
//! 1. `CreateApplication` PERSISTS the request under a freshly minted `ApplicationArn`
//!    of the form the model's `ApplicationArn` pattern requires, and
//!    `DescribeApplication` REPORTS exactly what the create stored — for every value of
//!    `Name`, `Description`, `Status`, `PortalOptions` and `Tags` the model admits,
//!    with the members the create did not send settled on their modelled defaults;
//! 2. an access scope of an application is KEYED BY ITS `Scope` NAME: a
//!    `PutApplicationAccessScope` adds that scope when the application does not carry it
//!    and REPLACES its `AuthorizedTargets` when it does, leaving every other scope
//!    alone, and `GetApplicationAccessScope` reports exactly the targets the last put
//!    stored — `AuthorizedTargets` absent for a scope put without any, since
//!    `ScopeTargets` is `length 1..10` and cannot express an empty list;
//! 3. the put is `@idempotent` and the get is a REPORT: repeating either answers
//!    identically and changes nothing a later caller sees;
//! 4. an `ApplicationArn` this account holds no application for, and a `Scope` an
//!    application does not carry, are refused `ResourceNotFoundException` — never a
//!    200, and the refusal stores nothing;
//! 5. `CreateApplication` resolves both resources the request names: an
//!    `ApplicationProviderArn` naming no published provider and an `InstanceArn` this
//!    account holds no instance for are refused `ResourceNotFoundException`, a
//!    `SignInOptions` whose `Origin` is `APPLICATION` with no `ApplicationUrl` is
//!    refused `ValidationException`, and so is a published provider that backs no
//!    customer managed OAuth 2.0 application (`application_create_providers.rs` owns
//!    that rule's own table);
//! 6. the emulator's per-application access-scope ceiling
//!    (`state::MAX_SCOPES_PER_APPLICATION`) bounds caller-driven growth: a put that
//!    would ADD a scope past it is refused `ValidationException`, while a put that
//!    UPDATES a scope the application already carries is never refused by it.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints,
//! the `awsJson1_1` envelope and the `X-Amz-Target` routing are exercised rather than
//! bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    call, error_code, router, seeded_instance_arn, TEST_REGION, UNKNOWN_INSTANCE_ARN,
};

/// The OAuth 2.0 application provider a customer managed application is created under.
///
/// SOURCE: `state::BUILT_IN_APPLICATION_PROVIDERS`, the catalogue
/// `ListApplicationProviders`/`DescribeApplicationProvider` report, whose `custom` entry
/// is the "SSO Custom Application Provider" and federates over `OAUTH`.
const CUSTOM_PROVIDER_ARN: &str = "arn:aws:sso::aws:applicationProvider/custom";

/// A SAML 2.0 provider of the pre-integrated application catalog, which
/// `CreateApplication` does NOT create an application under — the refusal rule has its
/// own suite (`application_create_providers.rs`); here it only keeps a scope put from
/// reaching an application that could not exist. A catalog entry the issue did not cite.
const CATALOG_PROVIDER_ARN: &str = "arn:aws:sso::aws:applicationProvider/catalog/Zendesk";

/// An Amazon Web Services MANAGED provider: its applications are created by the service
/// that owns it on the customer's behalf, so `CreateApplication` does not create one
/// either.
const REDSHIFT_PROVIDER_ARN: &str = "arn:aws:sso::aws:applicationProvider/redshift";

/// One application to create: the parameters, as a row of the table.
#[derive(Debug, Clone)]
struct Application {
    label: String,
    provider_arn: String,
    name: String,
    description: Option<String>,
    status: Option<String>,
    portal_options: Option<Value>,
    tags: Vec<(String, String)>,
    client_token: Option<String>,
}

impl Application {
    fn new(label: &str, name: &str) -> Self {
        Self {
            label: label.to_string(),
            provider_arn: CUSTOM_PROVIDER_ARN.to_string(),
            name: name.to_string(),
            description: None,
            status: None,
            portal_options: None,
            tags: Vec::new(),
            client_token: None,
        }
    }

    fn with_description(mut self, description: &str) -> Self {
        self.description = Some(description.to_string());
        self
    }

    fn with_status(mut self, status: &str) -> Self {
        self.status = Some(status.to_string());
        self
    }

    fn with_portal_options(mut self, portal_options: Value) -> Self {
        self.portal_options = Some(portal_options);
        self
    }

    fn with_tags(mut self, tags: &[(&str, &str)]) -> Self {
        self.tags = tags
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        self
    }

    fn with_client_token(mut self, token: &str) -> Self {
        self.client_token = Some(token.to_string());
        self
    }

    /// The `CreateApplication` request body for this row.
    fn request(&self, instance_arn: &str) -> Value {
        let mut body = json!({
            "InstanceArn": instance_arn,
            "ApplicationProviderArn": self.provider_arn,
            "Name": self.name,
        });
        if let Some(description) = &self.description {
            body["Description"] = json!(description);
        }
        if let Some(status) = &self.status {
            body["Status"] = json!(status);
        }
        if let Some(portal_options) = &self.portal_options {
            body["PortalOptions"] = portal_options.clone();
        }
        if !self.tags.is_empty() {
            body["Tags"] = Value::Array(
                self.tags
                    .iter()
                    .map(|(key, value)| json!({"Key": key, "Value": value}))
                    .collect(),
            );
        }
        if let Some(token) = &self.client_token {
            body["ClientToken"] = json!(token);
        }
        body
    }

    /// The `PortalOptions` a `DescribeApplication` of this row must report: the ones it
    /// sent, with the modelled default for `Visibility`
    /// (`smithy_query(query="shape",shape="ApplicationVisibility")` → `default:
    /// "ENABLED"`) standing in where it sent none.
    fn expected_portal_options(&self) -> Value {
        let mut options = self
            .portal_options
            .clone()
            .unwrap_or_else(|| json!({}));
        if options.get("Visibility").is_none() {
            options["Visibility"] = json!("ENABLED");
        }
        options
    }

    /// The `Status` a describe must report: the one it sent, or `ENABLED` — the status an
    /// application the caller did not disable carries.
    fn expected_status(&self) -> String {
        self.status.clone().unwrap_or_else(|| "ENABLED".to_string())
    }
}

/// The rows differ along what rule 1 generalises over: the length and character class of
/// `Name` (`len 1..100`, `^[\S\s]*$`, so spaces
/// and punctuation are admitted), whether `Description` (`len 1..128`) and `Status`
/// (`ENABLED | DISABLED`) are sent, the shape of `PortalOptions` (absent, `Visibility`
/// only, either `SignInOrigin`, with and without an `ApplicationUrl`) and the number of
/// `Tags` (`len 0..75`). Every constraint is from
/// `smithy_query(query="operation",operation="CreateApplication")` and
/// `smithy_query(query="shape",shape="PortalOptions,SignInOptions")`.
fn applications() -> Vec<Application> {
    vec![
        Application::new("the shortest Name, nothing else sent", "a"),
        Application::new(
            "Name at the 100-character limit, spanning spaces and punctuation",
            &format!("App 1 (prod) - {}", "x".repeat(85)),
        )
        .with_description("The payroll application")
        .with_status("ENABLED")
        .with_tags(&[("owner", "platform"), ("Env", "staging"), ("cost/centre", "42")]),
        Application::new("a DISABLED application with portal visibility off", "disabled-app")
            .with_status("DISABLED")
            .with_portal_options(json!({"Visibility": "DISABLED"}))
            .with_description("d"),
        Application::new(
            "IDENTITY_CENTER sign-in origin, no ApplicationUrl",
            "identity-center-origin",
        )
        .with_portal_options(json!({"SignInOptions": {"Origin": "IDENTITY_CENTER"}})),
        Application::new("APPLICATION sign-in origin with an ApplicationUrl", "application-origin")
            .with_portal_options(json!({
                "SignInOptions": {
                    "Origin": "APPLICATION",
                    "ApplicationUrl": "https://app.example.com/sso/start?tenant=one",
                },
                "Visibility": "ENABLED",
            }))
            .with_description(&"e".repeat(128))
            .with_tags(&[("single", "tag")]),
        Application::new("a single tag and a ClientToken", "tokened-app")
            .with_tags(&[("team", "identity")])
            .with_client_token("apl-token-1"),
    ]
}

/// Whether `arn` has the shape the model's `ApplicationArn` pattern requires for an
/// application of `instance_arn`:
/// `^arn:aws(-[a-z]{1,5}){0,3}:sso::\d{12}:application/(sso)?ins-[a-zA-Z0-9-.]{16}/apl-[a-zA-Z0-9]{16}$`.
pub(crate) fn is_application_arn_of(arn: &str, instance_arn: &str) -> bool {
    let instance_id = instance_arn.rsplit('/').next().unwrap_or_default();
    let Some(rest) = arn.strip_prefix("arn:aws:sso::") else {
        return false;
    };
    let Some((account, resource)) = rest.split_once(':') else {
        return false;
    };
    if account.len() != 12 || !account.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let Some(suffix) = resource.strip_prefix(&format!("application/{instance_id}/apl-")) else {
        return false;
    };
    suffix.len() == 16 && suffix.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Create one application and return `(status, body)`.
async fn create(
    router: &MultiServiceRouter,
    instance_arn: &str,
    application: &Application,
) -> (u16, Value) {
    call(
        router,
        "CreateApplication",
        &application.request(instance_arn),
    )
    .await
}

/// Create one application that must succeed, and return its `ApplicationArn`.
async fn create_ok(
    router: &MultiServiceRouter,
    instance_arn: &str,
    application: &Application,
) -> String {
    let (status, body) = create(router, instance_arn, application).await;
    assert_eq!(
        status, 200,
        "case {:?}: CreateApplication must answer 200, got {status} {body}",
        application.label
    );
    body["ApplicationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateApplication returned no ApplicationArn: {body}"))
        .to_string()
}

/// `DescribeApplication` for `arn`.
async fn describe(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(router, "DescribeApplication", &json!({"ApplicationArn": arn})).await
}

/// `PutApplicationAccessScope`: `targets` is `None` for a put that sends no
/// `AuthorizedTargets` at all.
async fn put_scope(
    router: &MultiServiceRouter,
    arn: &str,
    scope: &str,
    targets: Option<&[String]>,
) -> (u16, Value) {
    let mut body = json!({"ApplicationArn": arn, "Scope": scope});
    if let Some(targets) = targets {
        body["AuthorizedTargets"] = json!(targets);
    }
    call(router, "PutApplicationAccessScope", &body).await
}

/// `GetApplicationAccessScope` for one scope of one application.
async fn get_scope(router: &MultiServiceRouter, arn: &str, scope: &str) -> (u16, Value) {
    call(
        router,
        "GetApplicationAccessScope",
        &json!({"ApplicationArn": arn, "Scope": scope}),
    )
    .await
}

/// The body a get of `scope` must answer with when `targets` were put for it.
fn expected_scope_body(scope: &str, targets: Option<&[String]>) -> Value {
    match targets {
        // `ScopeTargets` is `length 1..10`: a scope put without authorized targets is
        // reported with the member ABSENT, not with an empty list.
        None => json!({"Scope": scope}),
        Some(targets) => json!({"Scope": scope, "AuthorizedTargets": targets}),
    }
}

/// `count` ARNs that are valid `ScopeTarget`s of this account: the instance's own ARN
/// first, then applications created for the purpose, so every target names a resource
/// that really exists.
///
/// SOURCE of the form: `smithy_query(query="shape",shape="ScopeTarget")` —
/// `^arn:aws(-[a-z]{1,5}){0,3}:sso::(\d{12}:application/(sso)?ins-[a-zA-Z0-9-.]{16}/apl-[a-zA-Z0-9]{16}|:instance/(sso)?ins-[a-zA-Z0-9-.]{16})$`.
async fn target_pool(
    router: &MultiServiceRouter,
    instance_arn: &str,
    count: usize,
) -> Vec<String> {
    let mut targets = vec![instance_arn.to_string()];
    for index in 0..count.saturating_sub(1) {
        let application = Application::new("a scope target", &format!("target-{index}"));
        targets.push(create_ok(router, instance_arn, &application).await);
    }
    targets.truncate(count);
    targets
}

/// Rule 1 (and rule 3): every row round-trips through the store, under an ARN of the
/// modelled form that no other row shares.
#[tokio::test]
async fn every_created_application_reads_back_exactly_what_was_stored() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let mut minted: Vec<String> = Vec::new();

    for application in applications() {
        let case = application.label.clone();
        let (status, body) = create(&router, &instance_arn, &application).await;
        assert_eq!(
            status, 200,
            "case {case:?}: create must answer 200, got {status} {body}"
        );
        let arn = body["ApplicationArn"].as_str().unwrap_or_default().to_string();
        assert!(
            is_application_arn_of(&arn, &instance_arn),
            "case {case:?}: minted ARN is not a modelled ApplicationArn of this instance, \
             got {arn:?}"
        );
        assert!(
            !minted.contains(&arn),
            "case {case:?}: each create mints a fresh ARN, got the already-minted {arn:?}"
        );
        assert_eq!(
            body["InstanceArn"].as_str(),
            Some(instance_arn.as_str()),
            "case {case:?}: create must answer with the instance it created under, got {body}"
        );
        assert!(
            body["IdentityStoreArn"]
                .as_str()
                .is_some_and(|value| value.contains(":identitystore/")),
            "case {case:?}: create must answer with the instance's identity store, got {body}"
        );
        minted.push(arn.clone());

        let (status, described) = describe(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: describe of the created application must answer 200, got \
             {status} {described}"
        );
        assert_eq!(
            described["ApplicationArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: describe must report the ARN it was asked about, got {described}"
        );
        assert_eq!(
            described["Name"].as_str(),
            Some(application.name.as_str()),
            "case {case:?}: describe must report the stored Name, got {described}"
        );
        assert_eq!(
            described["Description"].as_str(),
            application.description.as_deref(),
            "case {case:?}: describe must report the stored Description, got {described}"
        );
        assert_eq!(
            described["Status"].as_str(),
            Some(application.expected_status().as_str()),
            "case {case:?}: describe must report the stored Status, got {described}"
        );
        assert_eq!(
            described["PortalOptions"],
            application.expected_portal_options(),
            "case {case:?}: describe must report the stored PortalOptions, got {described}"
        );
        assert_eq!(
            described["ApplicationProviderArn"].as_str(),
            Some(application.provider_arn.as_str()),
            "case {case:?}: describe must report the provider it was created under, got \
             {described}"
        );
        assert_eq!(
            described["InstanceArn"].as_str(),
            Some(instance_arn.as_str()),
            "case {case:?}: describe must report the owning instance, got {described}"
        );
        assert_eq!(
            described["ApplicationAccount"].as_str(),
            arn.split(':').nth(4),
            "case {case:?}: ApplicationAccount must be the account of the ARN, got {described}"
        );
        assert_eq!(
            described["CreatedFrom"].as_str(),
            Some(TEST_REGION),
            "case {case:?}: CreatedFrom must be the Region the create ran in, got {described}"
        );
        assert!(
            described["CreatedDate"].is_number(),
            "case {case:?}: describe must report a CreatedDate, got {described}"
        );

        // Rule 3: the describe reported rather than mutated, so a second one answers
        // identically.
        let (status, again) = describe(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a repeated describe must answer 200, got {status} {again}"
        );
        assert_eq!(
            again, described,
            "case {case:?}: a describe must change nothing a later caller sees, got {again}"
        );
    }

    // Every earlier application is still readable after the later creates: a create adds
    // a record, it does not replace the store.
    for (application, arn) in applications().iter().zip(&minted) {
        let (status, body) = describe(&router, arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: an application created earlier must still describe, got {status} {body}",
            application.label
        );
    }
}

/// Rule 2 and rule 3: each scope row is stored under its own name and read back exactly,
/// a repeated put changes nothing, and putting one scope leaves the others alone.
#[tokio::test]
async fn every_put_access_scope_reads_back_exactly_the_targets_it_stored() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let application_arn = create_ok(
        &router,
        &instance_arn,
        &Application::new("the application under test", "scoped-app"),
    )
    .await;
    let pool = target_pool(&router, &instance_arn, 10).await;

    // The rows differ along what rule 2 generalises over: the segment count and
    // character class of `Scope`
    // (`^([A-Za-z0-9_]{1,50})(:[A-Za-z0-9_]{1,50}){0,1}(:[A-Za-z0-9_]{1,50}){0,1}$`) and
    // the number and kind of `AuthorizedTargets` (absent, one, the modelled maximum of
    // ten, an instance ARN, application ARNs).
    let rows: Vec<(&str, String, Option<Vec<String>>)> = vec![
        ("a one-segment scope with no AuthorizedTargets", "openid".to_string(), None),
        (
            "a two-segment scope with one application target",
            "sso:account".to_string(),
            Some(vec![pool[1].clone()]),
        ),
        (
            "a three-segment scope with an instance ARN target",
            "s3:access_grants:read_write".to_string(),
            Some(vec![pool[0].clone()]),
        ),
        (
            "a scope at the modelled maximum of ten targets",
            "redshift:connect".to_string(),
            Some(pool.clone()),
        ),
        (
            "segments at the 50-character limit of the Scope pattern",
            format!("{}:{}", "A".repeat(50), "b_0".repeat(10)),
            Some(vec![pool[2].clone(), pool[3].clone()]),
        ),
        (
            "a single-underscore scope name",
            "_".to_string(),
            Some(vec![pool[4].clone()]),
        ),
    ];

    let mut stored: Vec<(String, Option<Vec<String>>)> = Vec::new();
    for (case, scope, targets) in &rows {
        let targets_slice = targets.as_deref();
        let (status, body) = put_scope(&router, &application_arn, scope, targets_slice).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the put must answer 200, got {status} {body}"
        );

        let (status, read) = get_scope(&router, &application_arn, scope).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the get of the put scope must answer 200, got {status} {read}"
        );
        assert_eq!(
            read,
            expected_scope_body(scope, targets_slice),
            "case {case:?}: the get must report exactly the targets the put stored, got {read}"
        );

        // Rule 3: the put is idempotent and the get is a report.
        let (status, body) = put_scope(&router, &application_arn, scope, targets_slice).await;
        assert_eq!(
            status, 200,
            "case {case:?}: repeating the put must answer 200, got {status} {body}"
        );
        let (status, again) = get_scope(&router, &application_arn, scope).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the get after the repeated put must answer 200, got {status} {again}"
        );
        assert_eq!(
            again, read,
            "case {case:?}: repeating the put must leave the same scope unchanged, got {again}"
        );

        stored.push((scope.clone(), targets.clone()));
        // Rule 2: a put of one scope leaves every scope put earlier alone.
        for (earlier_scope, earlier_targets) in &stored {
            let (status, earlier) = get_scope(&router, &application_arn, earlier_scope).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the earlier scope {earlier_scope:?} must still read, got \
                 {status} {earlier}"
            );
            assert_eq!(
                earlier,
                expected_scope_body(earlier_scope, earlier_targets.as_deref()),
                "case {case:?}: the earlier scope {earlier_scope:?} must keep its own \
                 targets, got {earlier}"
            );
        }
    }
}

/// One replacement to observe: the case name, the targets the first put sends and the
/// targets the second one replaces them with (`None` for a put that sends none).
type Transition = (&'static str, Option<Vec<String>>, Option<Vec<String>>);

/// Rule 2: a put of a scope the application already carries REPLACES its authorized
/// targets rather than merging into them, in every direction.
#[tokio::test]
async fn a_second_put_replaces_the_authorized_targets_of_the_same_scope() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let application_arn = create_ok(
        &router,
        &instance_arn,
        &Application::new("the application under test", "replaced-scope-app"),
    )
    .await;
    let pool = target_pool(&router, &instance_arn, 4).await;

    // Each row is (first put, second put): the transitions a replacement can take —
    // growing, shrinking, swapping a target for another, dropping the member entirely
    // and adding it where there was none.
    let transitions: Vec<Transition> = vec![
        (
            "one target replaced by another",
            Some(vec![pool[0].clone()]),
            Some(vec![pool[1].clone()]),
        ),
        (
            "one target grown to three",
            Some(vec![pool[1].clone()]),
            Some(vec![pool[1].clone(), pool[2].clone(), pool[3].clone()]),
        ),
        (
            "three targets shrunk to one",
            Some(vec![pool[1].clone(), pool[2].clone(), pool[3].clone()]),
            Some(vec![pool[2].clone()]),
        ),
        (
            "targets dropped by a put that sends none",
            Some(vec![pool[0].clone(), pool[3].clone()]),
            None,
        ),
        (
            "targets added to a scope that had none",
            None,
            Some(vec![pool[3].clone()]),
        ),
        (
            "the order of the targets replaced",
            Some(vec![pool[0].clone(), pool[2].clone()]),
            Some(vec![pool[2].clone(), pool[0].clone()]),
        ),
    ];

    for (index, (case, first, second)) in transitions.iter().enumerate() {
        // One scope per row, so each replacement is observed on its own scope.
        let scope = format!("scope_{index}");
        let (status, body) = put_scope(&router, &application_arn, &scope, first.as_deref()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first put must answer 200, got {status} {body}"
        );
        let (status, read) = get_scope(&router, &application_arn, &scope).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the get after the first put must answer 200, got {status} {read}"
        );
        assert_eq!(
            read,
            expected_scope_body(&scope, first.as_deref()),
            "case {case:?}: the first put's targets must read back, got {read}"
        );

        let (status, body) = put_scope(&router, &application_arn, &scope, second.as_deref()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the second put must answer 200, got {status} {body}"
        );
        let (status, replaced) = get_scope(&router, &application_arn, &scope).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the get after the second put must answer 200, got {status} \
             {replaced}"
        );
        assert_eq!(
            replaced,
            expected_scope_body(&scope, second.as_deref()),
            "case {case:?}: the second put must REPLACE the targets, got {replaced}"
        );
    }
}

/// Rule 6: the per-application access-scope ceiling bounds how far a caller can grow one
/// application's scopes, and refuses only a NEW scope — an update of one it already
/// carries is not growth.
#[tokio::test]
async fn the_access_scope_ceiling_refuses_a_new_scope_but_never_an_update() {
    use service_com_amazonaws_ssoadmin_swbexternalservice::state::MAX_SCOPES_PER_APPLICATION;

    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let application_arn = create_ok(
        &router,
        &instance_arn,
        &Application::new("the application under test", "ceiling-app"),
    )
    .await;

    for index in 0..MAX_SCOPES_PER_APPLICATION {
        let scope = format!("scope_{index}");
        let (status, body) = put_scope(&router, &application_arn, &scope, None).await;
        assert_eq!(
            status, 200,
            "scope {scope:?} is within the ceiling, so its put must answer 200, got \
             {status} {body}"
        );
    }

    // One past the ceiling: refused, and nothing stored for it.
    let beyond = format!("scope_{MAX_SCOPES_PER_APPLICATION}");
    let (status, body) = put_scope(&router, &application_arn, &beyond, None).await;
    assert_eq!(
        status, 400,
        "case {beyond:?}: a scope beyond the ceiling must be refused 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case {beyond:?}: expected ValidationException, got {body}"
    );
    let (status, body) = get_scope(&router, &application_arn, &beyond).await;
    assert_eq!(
        status, 400,
        "case {beyond:?}: the refused put must have stored nothing, got {status} {body}"
    );

    // An UPDATE of a scope the application already carries adds no key, so the ceiling
    // never refuses it — at either published bound of `ScopeTargets` (`length 1..10`).
    let pool = target_pool(&router, &instance_arn, 10).await;
    for (case, targets) in [
        ("one target, the minimum ScopeTargets admits", &pool[..1]),
        ("ten targets, the maximum ScopeTargets admits", &pool[..]),
    ] {
        let scope = "scope_0";
        let (status, body) = put_scope(&router, &application_arn, scope, Some(targets)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: updating a stored scope at the ceiling must answer 200, got \
             {status} {body}"
        );
        let (status, read) = get_scope(&router, &application_arn, scope).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the updated scope must read back, got {status} {read}"
        );
        assert_eq!(
            read,
            expected_scope_body(scope, Some(targets)),
            "case {case:?}: the update must have replaced the targets, got {read}"
        );
    }
}

/// Rule 4: an `ApplicationArn` nothing holds is refused by both the put and the get, and
/// the refused put stores nothing.
#[tokio::test]
async fn an_application_arn_no_create_minted_is_refused_not_found() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let instance_id = instance_arn.rsplit('/').next().unwrap_or_default().to_string();
    // One application exists, so the not-found is about the ARN asked for and not about
    // an empty store.
    let stored = create_ok(
        &router,
        &instance_arn,
        &Application::new("the one stored application", "stored-app"),
    )
    .await;
    let account = stored.split(':').nth(4).unwrap_or_default().to_string();
    let suffix = stored.rsplit("/apl-").next().unwrap_or_default().to_string();
    let application_arn = |partition: &str, account: &str, instance: &str, suffix: &str| {
        format!("arn:{partition}:sso::{account}:application/{instance}/apl-{suffix}")
    };

    // The rows span the parts of the ARN that identify the application: the `apl-`
    // suffix, the instance segment, the account segment and the partition.
    let cases: Vec<(&str, String)> = vec![
        (
            "a pattern-valid apl- suffix that was never minted",
            application_arn("aws", &account, &instance_id, "0000000000000000"),
        ),
        (
            "the stored suffix under another instance",
            application_arn("aws", &account, "ssoins-0000000000000000", &suffix),
        ),
        (
            "the stored application under another account",
            application_arn("aws", "210987654321", &instance_id, &suffix),
        ),
        (
            "the stored application in another partition",
            application_arn("aws-us-gov", &account, &instance_id, &suffix),
        ),
    ];

    for (case, arn) in cases {
        assert_ne!(
            arn, stored,
            "case {case:?}: the row must name a DIFFERENT ARN than the stored one"
        );
        let (status, body) = put_scope(&router, &arn, "openid", None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a put under an unminted ARN must be refused 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException from the put, got {body}"
        );

        let (status, body) = get_scope(&router, &arn, "openid").await;
        assert_eq!(
            status, 400,
            "case {case:?}: a get under an unminted ARN must be refused 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException from the get, got {body}"
        );

        let (status, body) = describe(&router, &arn).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a describe of an unminted ARN must be refused 400, got {status} \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException from the describe, got {body}"
        );
    }

    // The refused puts stored nothing, on the stored application or anywhere else.
    let (status, body) = get_scope(&router, &stored, "openid").await;
    assert_eq!(
        status, 400,
        "a refused put must store no scope on the stored application, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException for a scope no put stored, got {body}"
    );
}

/// Rule 4: a `Scope` an application does not carry is a not-found, never a 200 with no
/// targets — which is what a scope put WITHOUT targets answers with.
#[tokio::test]
async fn a_scope_the_application_does_not_carry_is_refused_not_found() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let application_arn = create_ok(
        &router,
        &instance_arn,
        &Application::new("the application under test", "unknown-scope-app"),
    )
    .await;
    // A second application, so a scope put on one is not readable on the other.
    let other_arn = create_ok(
        &router,
        &instance_arn,
        &Application::new("another application", "other-app"),
    )
    .await;

    let (status, body) = put_scope(&router, &application_arn, "openid", None).await;
    assert_eq!(status, 200, "the seed put must answer 200, got {status} {body}");

    // The rows differ in how they MISS the one stored scope: an unrelated name, a
    // case-shifted one, a prefix, an extra segment, and the stored name on another
    // application.
    let cases: Vec<(&str, &str, &str)> = vec![
        ("an unrelated scope name", &application_arn, "profile"),
        ("the stored name in another case", &application_arn, "OpenId"),
        ("the stored name with an extra segment", &application_arn, "openid:read"),
        ("a prefix of the stored name", &application_arn, "open"),
        ("the stored scope on another application", &other_arn, "openid"),
    ];

    for (case, arn, scope) in cases {
        let (status, body) = get_scope(&router, arn, scope).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a scope nothing put must be refused 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
    }

    // The refusals left the stored scope readable, with no targets.
    let (status, body) = get_scope(&router, &application_arn, "openid").await;
    assert_eq!(
        status, 200,
        "the stored scope must survive the refusals, got {status} {body}"
    );
    assert_eq!(
        body,
        expected_scope_body("openid", None),
        "a scope put without AuthorizedTargets must report none, got {body}"
    );
}

/// Rule 5: what `CreateApplication` refuses, and that a refusal stores nothing.
#[tokio::test]
async fn creating_an_application_is_refused_where_the_request_cannot_be_served() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;

    // (case, the request, the expected error code) — the rows span the reasons a
    // syntactically valid create cannot be served.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "an ApplicationProviderArn naming no published provider",
            json!({
                "InstanceArn": instance_arn,
                "ApplicationProviderArn": "arn:aws:sso::aws:applicationProvider/nosuchprovider",
                "Name": "unknown-provider",
            }),
            "ResourceNotFoundException",
        ),
        (
            "an InstanceArn this account holds no instance for",
            json!({
                "InstanceArn": UNKNOWN_INSTANCE_ARN,
                "ApplicationProviderArn": CUSTOM_PROVIDER_ARN,
                "Name": "unheld-instance",
            }),
            "ResourceNotFoundException",
        ),
        (
            "an APPLICATION sign-in origin with no ApplicationUrl",
            json!({
                "InstanceArn": instance_arn,
                "ApplicationProviderArn": CUSTOM_PROVIDER_ARN,
                "Name": "no-application-url",
                "PortalOptions": {"SignInOptions": {"Origin": "APPLICATION"}},
            }),
            "ValidationException",
        ),
        (
            "a provider of the pre-integrated SAML 2.0 catalog",
            json!({
                "InstanceArn": instance_arn,
                "ApplicationProviderArn": CATALOG_PROVIDER_ARN,
                "Name": "saml-provider",
            }),
            "ValidationException",
        ),
        (
            "an Amazon Web Services managed provider",
            json!({
                "InstanceArn": instance_arn,
                "ApplicationProviderArn": REDSHIFT_PROVIDER_ARN,
                "Name": "aws-managed-provider",
            }),
            "ValidationException",
        ),
    ];

    for (case, request, expected) in cases {
        let (status, body) = call(&router, "CreateApplication", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the create must be refused 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {body}"
        );
        assert!(
            body["ApplicationArn"].is_null(),
            "case {case:?}: a refusal mints no ApplicationArn, got {body}"
        );
    }
}

/// Rule 1's idempotency half: a replayed `ClientToken` answers with the application the
/// first call minted, and the same token carrying a different request is a conflict.
#[tokio::test]
async fn a_replayed_client_token_mints_no_second_application() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;

    for application in applications()
        .into_iter()
        .filter(|row| row.client_token.is_some())
        .chain([
            Application::new("a token on a fully specified request", "token-app")
                .with_description("tokened")
                .with_status("DISABLED")
                .with_portal_options(json!({"Visibility": "DISABLED"}))
                .with_tags(&[("a", "1"), ("b", "2")])
                .with_client_token("apl-token-2"),
        ])
    {
        let case = application.label.clone();
        let first = create_ok(&router, &instance_arn, &application).await;

        let (status, replayed) = create(&router, &instance_arn, &application).await;
        assert_eq!(
            status, 200,
            "case {case:?}: replaying the token must answer 200, got {status} {replayed}"
        );
        assert_eq!(
            replayed["ApplicationArn"].as_str(),
            Some(first.as_str()),
            "case {case:?}: a replay must answer with the first ARN {first:?}, got {replayed}"
        );

        // Tag ORDER is not a parameter difference: a tag set carries no order.
        if application.tags.len() > 1 {
            let mut reordered = application.clone();
            reordered.tags.reverse();
            let (status, body) = create(&router, &instance_arn, &reordered).await;
            assert_eq!(
                body["ApplicationArn"].as_str(),
                Some(first.as_str()),
                "case {case:?}: reordering Tags is the same request, expected {first:?}, got \
                 {status} {body}"
            );
        }

        // One mutated member per row: each is a parameter the token is scoped to, so each
        // is a conflict rather than a replay.
        let mut mutations: Vec<(&str, Application)> = Vec::new();
        let mut renamed = application.clone();
        renamed.name = format!("{}Z", application.name);
        mutations.push(("Name", renamed));
        let mut redescribed = application.clone();
        redescribed.description = Some("a different description".to_string());
        mutations.push(("Description", redescribed));
        let mut restatused = application.clone();
        restatused.status = Some(match application.expected_status().as_str() {
            "ENABLED" => "DISABLED".to_string(),
            _ => "ENABLED".to_string(),
        });
        mutations.push(("Status", restatused));
        let mut reportaled = application.clone();
        reportaled.portal_options = Some(json!({
            "SignInOptions": {
                "Origin": "APPLICATION",
                "ApplicationUrl": "https://changed.example.com/start",
            }
        }));
        mutations.push(("PortalOptions", reportaled));
        let mut retagged = application.clone();
        retagged.tags.push(("extra".to_string(), "tag".to_string()));
        mutations.push(("Tags", retagged));

        for (member, mutated) in mutations {
            let (status, body) = create(&router, &instance_arn, &mutated).await;
            assert_eq!(
                status, 400,
                "case {case:?} changed {member}: the reused token must be refused 400, got \
                 {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ConflictException",
                "case {case:?} changed {member}: expected ConflictException, got {body}"
            );
        }

        // The conflicts changed nothing: the first application still reads back.
        let (status, described) = describe(&router, &first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first application must survive the refusals, got {status} \
             {described}"
        );
        assert_eq!(
            described["Name"].as_str(),
            Some(application.name.as_str()),
            "case {case:?}: a refused create must not have altered the stored Name, got \
             {described}"
        );
    }
}
