// SPDX-License-Identifier: CC0-1.0

//! `CreateApplication` end to end over the `awsJson1_1` wire — the operation the stub
//! issue reported as an unimplemented scaffold.
//!
//! The application suite already pins what a create stores for each shape of request
//! (`application_access_scopes.rs`) and how its `ClientToken` behaves
//! (`application_assignments.rs`), both against the ONE instance the emulator seeds and
//! from the ONE Region the harness signs for. This suite pins the rules those tables hold
//! fixed, each asserted over a TABLE rather than over a single example:
//!
//! 1. a create stores the application UNDER THE INSTANCE THE REQUEST NAMES: the minted
//!    `ApplicationArn` carries that instance's id and the account of this emulator, and
//!    both the create's own response and a later `DescribeApplication` report that
//!    instance and the identity store connected to it — for the account instance this
//!    emulator seeds and for an organization instance of a different id alike;
//! 2. `CreatedFrom` is the REGION OF THE REQUEST THAT CREATED the application, decided
//!    once at create time: a describe made from any other Region reports the Region the
//!    create ran in, not the Region of the read;
//! 3. a create writes exactly ONE record: every application created before it still
//!    reports exactly what it reported then, and the fresh application carries no access
//!    scopes;
//! 4. a create that cannot be served is refused with the modelled CLIENT error for the
//!    reason it cannot be served, is never answered with the scaffold's HTTP 501, and
//!    writes NOTHING — the store holds exactly the records it held before the call.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints,
//! the `awsJson1_1` envelope and the `X-Amz-Target` routing are exercised rather than
//! bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns, and the
//! `ApplicationArn`-shape check is `application_access_scopes.rs`'s.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::application_access_scopes::is_application_arn_of;
use crate::ssoadmin_permission_sets::{
    call, call_in_region, error_code, organization_router, router, router_over,
    seeded_instance_arn, with_store, TEST_REGION, UNKNOWN_INSTANCE_ARN,
};

/// Application providers an application is created under, spanning both forms of the
/// published catalog.
///
/// SOURCE: `state::BUILT_IN_APPLICATION_PROVIDERS`, the catalog
/// `ListApplicationProviders` / `DescribeApplicationProvider` report — the AWS MANAGED
/// providers, whose id is a service slug, and the APPLICATION CATALOG, whose id is
/// `catalog/<Name>`. These are entries the issue did not cite.
const PROVIDER_CUSTOM: &str = "arn:aws:sso::aws:applicationProvider/custom";
const PROVIDER_GLUE: &str = "arn:aws:sso::aws:applicationProvider/glue";
const PROVIDER_S3: &str = "arn:aws:sso::aws:applicationProvider/s3";
const PROVIDER_CATALOG_DATABRICKS: &str =
    "arn:aws:sso::aws:applicationProvider/catalog/Databricks";
const PROVIDER_CATALOG_GITHUB: &str = "arn:aws:sso::aws:applicationProvider/catalog/GitHub";

/// An `ApplicationProviderArn` of the modelled form that names no published provider, so
/// nothing of the catalog resolves it.
const UNKNOWN_PROVIDER_ARN: &str = "arn:aws:sso::aws:applicationProvider/catalog/NoSuchProvider";

/// One `CreateApplication` request: a row of the table.
#[derive(Debug, Clone)]
struct Row {
    label: &'static str,
    provider_arn: &'static str,
    name: String,
    description: Option<String>,
    status: Option<&'static str>,
    portal_options: Option<Value>,
    tags: Vec<(&'static str, &'static str)>,
    /// The Region the create is signed for — what `CreatedFrom` must report.
    region: &'static str,
}

impl Row {
    /// The request body for this row, under `instance_arn`.
    fn request(&self, instance_arn: &str) -> Value {
        let mut body = json!({
            "InstanceArn": instance_arn,
            "ApplicationProviderArn": self.provider_arn,
            "Name": self.name,
        });
        if let Some(description) = &self.description {
            body["Description"] = json!(description);
        }
        if let Some(status) = self.status {
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
        body
    }

    /// The `Status` a describe must report: the one sent, or the status an application the
    /// caller did not disable carries.
    fn expected_status(&self) -> &str {
        self.status.unwrap_or("ENABLED")
    }

    /// The `PortalOptions` a describe must report: the ones sent, with the model's own
    /// default for `Visibility` (`smithy_query(query="shape",shape="PortalOptions")` →
    /// `Visibility` `default: "ENABLED"`) standing in where none was sent.
    fn expected_portal_options(&self) -> Value {
        let mut options = self.portal_options.clone().unwrap_or_else(|| json!({}));
        if options.get("Visibility").is_none() {
            options["Visibility"] = json!("ENABLED");
        }
        options
    }
}

/// The rows the rules are asserted over.
///
/// They differ along what rules 1 and 2 generalise over: the Region the create is signed
/// for (two, three and four segment Region names, all inside `RegionName`'s
/// `^([a-z]+-){2,3}\d$`), the provider the application runs under (both forms of the
/// published catalog), the length of `Name` at both published bounds (`len 1..100`,
/// `^[\S\s]*$`, so spaces and punctuation are admitted), whether `Description`
/// (`len 1..128`, asserted at both bounds) and `Status` (`ENABLED | DISABLED`) are sent,
/// the shape of `PortalOptions` (absent, `Visibility` only, each `SignInOrigin`) and the
/// number of `Tags` (`len 0..75`). Every constraint is from
/// `smithy_query(query="operation",operation="CreateApplication")` and
/// `smithy_query(query="shape",shape="PortalOptions,SignInOptions,ApplicationStatus")`.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "only the required members, from the harness Region",
            provider_arn: PROVIDER_CUSTOM,
            // `Name` at its lower length bound.
            name: "a".to_string(),
            description: None,
            status: None,
            portal_options: None,
            tags: Vec::new(),
            region: TEST_REGION,
        },
        Row {
            label: "every member set, from another Region, under an AWS managed provider",
            provider_arn: PROVIDER_GLUE,
            // `Name` at its upper length bound (100), spanning spaces and punctuation.
            name: format!("Data lake (prod) - {}", "x".repeat(81)),
            // `Description` at its upper length bound (128).
            description: Some("d".repeat(128)),
            status: Some("ENABLED"),
            portal_options: Some(json!({
                "SignInOptions": {
                    "Origin": "APPLICATION",
                    "ApplicationUrl": "https://lake.example.com/sso/start?tenant=one",
                },
                "Visibility": "ENABLED",
            })),
            tags: vec![("owner", "platform"), ("Env", "staging"), ("cost/centre", "42")],
            region: "eu-west-2",
        },
        Row {
            label: "a DISABLED application hidden from the portal, from a third Region",
            provider_arn: PROVIDER_S3,
            name: "disabled-app".to_string(),
            // `Description` at its lower length bound (1).
            description: Some("d".to_string()),
            status: Some("DISABLED"),
            portal_options: Some(json!({"Visibility": "DISABLED"})),
            tags: vec![("team", "identity")],
            region: "ap-southeast-2",
        },
        Row {
            label: "IDENTITY_CENTER sign-in origin, from a four-segment Region",
            provider_arn: PROVIDER_CATALOG_DATABRICKS,
            name: "identity center origin".to_string(),
            description: None,
            status: None,
            portal_options: Some(json!({"SignInOptions": {"Origin": "IDENTITY_CENTER"}})),
            tags: Vec::new(),
            region: "us-gov-east-1",
        },
        Row {
            label: "a catalog provider, no portal options, one tag",
            provider_arn: PROVIDER_CATALOG_GITHUB,
            name: "catalog-app".to_string(),
            description: Some("created under a pre-integrated catalog provider".to_string()),
            status: Some("DISABLED"),
            portal_options: None,
            tags: vec![("single", "tag")],
            region: "eu-central-1",
        },
    ]
}

/// A Region that is NOT `region`, for a read that must report the Region of the CREATE
/// rather than its own.
fn another_region(region: &str) -> &'static str {
    if region == TEST_REGION {
        "eu-west-3"
    } else {
        TEST_REGION
    }
}

/// `DescribeApplication` for `arn`, signed for `region`.
async fn describe(router: &MultiServiceRouter, region: &str, arn: &str) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "DescribeApplication",
        &json!({"ApplicationArn": arn}),
    )
    .await
}

/// The `IdentityStoreArn` of an instance, read the way a client reads it.
async fn identity_store_arn(router: &MultiServiceRouter, instance_arn: &str) -> String {
    let (status, body) = call(
        router,
        "DescribeInstance",
        &json!({"InstanceArn": instance_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeInstance of {instance_arn} expected 200, got {status} {body}"
    );
    body["IdentityStoreArn"]
        .as_str()
        .unwrap_or_else(|| panic!("DescribeInstance reported no IdentityStoreArn: {body}"))
        .to_string()
}

/// The two instances the rules are asserted against: the ACCOUNT instance this emulator
/// seeds, and an ORGANIZATION instance held under a different instance id — so the
/// "under the instance the request names" half of rule 1 varies over the instance instead
/// of being read off the only one that exists.
async fn instance_fixtures() -> Vec<(&'static str, MultiServiceRouter, String)> {
    let account = router();
    let account_instance = seeded_instance_arn(&account).await;
    let (organization, organization_instance) = organization_router();
    assert_ne!(
        account_instance, organization_instance,
        "the two fixtures must hold instances of different ids, got {account_instance}"
    );
    vec![
        ("the seeded account instance", account, account_instance),
        (
            "an organization instance of another id",
            organization,
            organization_instance,
        ),
    ]
}

/// Rules 1 and 2: every row is stored under the instance it names, with the Region of the
/// request that created it, and reads back exactly what the create stored.
#[tokio::test]
async fn a_created_application_belongs_to_the_named_instance_and_the_requests_region() {
    let mut minted: Vec<String> = Vec::new();

    for (fixture, router, instance_arn) in instance_fixtures().await {
        let identity_store = identity_store_arn(&router, &instance_arn).await;

        for row in rows() {
            let case = format!("{fixture}: {}", row.label);
            let (status, created) = call_in_region(
                &router,
                row.region,
                "CreateApplication",
                &row.request(&instance_arn),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: CreateApplication must answer 200, got {status} {created}"
            );

            let arn = created["ApplicationArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no ApplicationArn in {created}"))
                .to_string();
            assert!(
                is_application_arn_of(&arn, &instance_arn),
                "case {case:?}: the minted ARN must be a modelled ApplicationArn of \
                 {instance_arn}, got {arn:?}"
            );
            assert!(
                !minted.contains(&arn),
                "case {case:?}: every create mints a fresh ARN, got the already-minted {arn:?}"
            );
            minted.push(arn.clone());
            assert_eq!(
                created["InstanceArn"].as_str(),
                Some(instance_arn.as_str()),
                "case {case:?}: the create must answer with the instance it created under, \
                 got {created}"
            );
            assert_eq!(
                created["IdentityStoreArn"].as_str(),
                Some(identity_store.as_str()),
                "case {case:?}: the create must answer with that instance's identity store, \
                 got {created}"
            );

            // Read from ANOTHER Region: the record is the answer, not the request's scope.
            let read_region = another_region(row.region);
            let (status, described) = describe(&router, read_region, &arn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: describing the created application must answer 200, got \
                 {status} {described}"
            );
            assert_eq!(
                described["ApplicationArn"].as_str(),
                Some(arn.as_str()),
                "case {case:?}: describe must report the ARN it was asked about, got {described}"
            );
            assert_eq!(
                described["InstanceArn"].as_str(),
                Some(instance_arn.as_str()),
                "case {case:?}: describe must report the owning instance, got {described}"
            );
            assert_eq!(
                described["IdentityStoreArn"].as_str(),
                Some(identity_store.as_str()),
                "case {case:?}: describe must report that instance's identity store, got \
                 {described}"
            );
            assert_eq!(
                described["ApplicationAccount"].as_str(),
                arn.split(':').nth(4),
                "case {case:?}: ApplicationAccount must be the account of the ARN, got \
                 {described}"
            );
            assert_eq!(
                described["ApplicationProviderArn"].as_str(),
                Some(row.provider_arn),
                "case {case:?}: describe must report the provider it was created under, got \
                 {described}"
            );
            assert_eq!(
                described["Name"].as_str(),
                Some(row.name.as_str()),
                "case {case:?}: describe must report the stored Name, got {described}"
            );
            assert_eq!(
                described["Description"].as_str(),
                row.description.as_deref(),
                "case {case:?}: describe must report the stored Description, got {described}"
            );
            assert_eq!(
                described["Status"].as_str(),
                Some(row.expected_status()),
                "case {case:?}: describe must report the stored Status, got {described}"
            );
            assert_eq!(
                described["PortalOptions"],
                row.expected_portal_options(),
                "case {case:?}: describe must report the stored PortalOptions, got {described}"
            );
            // Rule 2: the Region of the CREATE, from a read made in another one.
            assert_eq!(
                described["CreatedFrom"].as_str(),
                Some(row.region),
                "case {case:?}: CreatedFrom must be the Region of the create, not the \
                 {read_region:?} of this read, got {described}"
            );
            assert!(
                described["CreatedDate"].as_f64().is_some_and(|date| date > 0.0),
                "case {case:?}: describe must report when the application was created, got \
                 {described}"
            );
        }
    }
}

/// Rule 3: a create writes one record and leaves every earlier one as it was, and the
/// application it minted carries no access scopes.
#[tokio::test]
async fn a_create_disturbs_no_application_already_stored() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;

    // (case, ARN, the describe body the application answered with when it was created).
    let mut stored: Vec<(String, String, Value)> = Vec::new();
    let table = rows();

    for row in &table {
        let case = row.label.to_string();
        let (status, created) = call_in_region(
            &router,
            row.region,
            "CreateApplication",
            &row.request(&instance_arn),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateApplication must answer 200, got {status} {created}"
        );
        let arn = created["ApplicationArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no ApplicationArn in {created}"))
            .to_string();

        // Every application created BEFORE this one still answers exactly as it did.
        for (earlier_case, earlier_arn, snapshot) in &stored {
            let (status, described) = describe(&router, TEST_REGION, earlier_arn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: {earlier_case:?} must still describe after a later create, \
                 got {status} {described}"
            );
            assert_eq!(
                &described, snapshot,
                "case {case:?}: creating it must not have altered {earlier_case:?}, got \
                 {described}"
            );
        }

        let (status, described) = describe(&router, TEST_REGION, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the new application must describe, got {status} {described}"
        );

        // A fresh application carries NO access scopes: `PutApplicationAccessScope` is what
        // adds them, so a get of any scope name finds none.
        let (status, scope) = call(
            &router,
            "GetApplicationAccessScope",
            &json!({"ApplicationArn": arn, "Scope": "openid"}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a fresh application carries no access scope, so the get must be \
             refused, got {status} {scope}"
        );
        assert_eq!(
            error_code(&scope),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException for an unset scope, got {scope}"
        );

        stored.push((case, arn, described));
    }

    assert_eq!(
        stored.len(),
        table.len(),
        "every row must have been stored, got {} of {}",
        stored.len(),
        table.len()
    );
}

/// Rule 4: a create that cannot be served is refused — never 501, never 200 — and writes
/// nothing.
#[tokio::test]
async fn a_refused_create_stores_nothing() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));
    let instance_arn = seeded_instance_arn(&router).await;

    // One application that WAS created, so "the store is unchanged" is a claim about a
    // non-empty store.
    let (status, created) = call(
        &router,
        "CreateApplication",
        &rows()[0].request(&instance_arn),
    )
    .await;
    assert_eq!(
        status, 200,
        "the seed create must answer 200, got {status} {created}"
    );
    let seeded_arn = created["ApplicationArn"]
        .as_str()
        .expect("the seed create mints an ApplicationArn")
        .to_string();
    let before = with_store(&engine, |store| {
        store.applications.keys().cloned().collect::<Vec<String>>()
    });

    // (case, the request, the modelled error it is refused with) — the rows span the
    // reasons a create whose members all satisfy the model cannot be served.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "an InstanceArn this account holds no instance for",
            json!({
                "InstanceArn": UNKNOWN_INSTANCE_ARN,
                "ApplicationProviderArn": PROVIDER_CUSTOM,
                "Name": "unheld-instance",
            }),
            "ResourceNotFoundException",
        ),
        (
            "an ApplicationProviderArn naming no published provider",
            json!({
                "InstanceArn": instance_arn,
                "ApplicationProviderArn": UNKNOWN_PROVIDER_ARN,
                "Name": "unknown-provider",
            }),
            "ResourceNotFoundException",
        ),
        (
            "an APPLICATION sign-in origin with no ApplicationUrl",
            json!({
                "InstanceArn": instance_arn,
                "ApplicationProviderArn": PROVIDER_GLUE,
                "Name": "no-application-url",
                "PortalOptions": {"SignInOptions": {"Origin": "APPLICATION"}},
            }),
            "ValidationException",
        ),
        (
            "an APPLICATION sign-in origin on an otherwise complete request",
            json!({
                "InstanceArn": instance_arn,
                "ApplicationProviderArn": PROVIDER_CATALOG_GITHUB,
                "Name": "no-application-url-either",
                "Description": "a complete request but for the sign-in URL",
                "Status": "DISABLED",
                "PortalOptions": {
                    "SignInOptions": {"Origin": "APPLICATION"},
                    "Visibility": "DISABLED",
                },
                "Tags": [{"Key": "owner", "Value": "platform"}],
            }),
            "ValidationException",
        ),
    ];

    for (case, request, expected) in cases {
        let (status, body) = call(&router, "CreateApplication", &request).await;
        // 400, so neither a 200 nor the scaffold's 501: the framework serialises these
        // awsJson client errors with the protocol's client-error status, which is the 400
        // every sibling suite of this crate asserts, and the error CODE below is what a
        // client branches on.
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

        let after = with_store(&engine, |store| {
            store.applications.keys().cloned().collect::<Vec<String>>()
        });
        assert_eq!(
            after, before,
            "case {case:?}: a refused create stores no application, the store now holds \
             {after:?}"
        );
        let (status, described) = describe(&router, TEST_REGION, &seeded_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the application already stored must survive the refusal, got \
             {status} {described}"
        );
    }
}
