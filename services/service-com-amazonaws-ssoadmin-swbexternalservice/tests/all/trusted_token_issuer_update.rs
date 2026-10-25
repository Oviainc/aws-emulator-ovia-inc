// SPDX-License-Identifier: CC0-1.0

//! `UpdateTrustedTokenIssuer` end to end over the `awsJson1_1` wire.
//!
//! The rules under test, each asserted over a TABLE of updates rather than over one
//! example:
//!
//! 1. an update WRITES exactly the members the request carries and leaves every other
//!    stored member alone: an absent `Name` keeps the stored name, an absent member of
//!    `OidcJwtConfiguration` keeps the stored path or option, and a member the update
//!    configuration does not model at all (`IssuerUrl`) survives every update — what the
//!    next `DescribeTrustedTokenIssuer` reports is the stored record after that merge,
//!    never the request echoed back;
//! 2. an update touches ONE issuer: another issuer of the same instance, and the tags of
//!    the updated issuer, read back unchanged;
//! 3. applying the same update again answers identically and changes nothing (the merge
//!    is idempotent);
//! 4. a `TrustedTokenIssuerArn` no create minted is refused `ResourceNotFoundException`
//!    — the same channel and wording `DescribeTrustedTokenIssuer` answers it with — and
//!    stores nothing.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints,
//! the `awsJson1_1` envelope and the `X-Amz-Target` routing are exercised rather than
//! bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{assert_refused, call, router, seeded_instance_arn};

/// The only value the model's `TrustedTokenIssuerType` enum admits
/// (`smithy_query(query="shape",shape="TrustedTokenIssuerType")` → `["OIDC_JWT"]`).
const ISSUER_TYPE: &str = "OIDC_JWT";

/// The only value the model's `JwksRetrievalOption` enum admits
/// (`smithy_query(query="shape",shape="OidcJwtConfiguration")` → member
/// `JwksRetrievalOption` → `["OPEN_ID_DISCOVERY"]`).
const JWKS_RETRIEVAL_OPTION: &str = "OPEN_ID_DISCOVERY";

/// The stored `OidcJwtConfiguration` of one issuer, as the create set it and as a
/// describe must report it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Configuration {
    issuer_url: String,
    claim_attribute_path: String,
    identity_store_attribute_path: String,
    jwks_retrieval_option: String,
}

impl Configuration {
    /// The wire form of `TrustedTokenIssuerConfiguration` for this configuration.
    fn json(&self) -> Value {
        json!({
            "OidcJwtConfiguration": {
                "IssuerUrl": self.issuer_url,
                "ClaimAttributePath": self.claim_attribute_path,
                "IdentityStoreAttributePath": self.identity_store_attribute_path,
                "JwksRetrievalOption": self.jwks_retrieval_option,
            }
        })
    }
}

/// The baseline an issuer is created with, distinguished by `label` so two issuers of the
/// same instance differ in every member an update could write.
fn baseline(label: &str) -> (String, Configuration) {
    (
        format!("tti-{label}"),
        Configuration {
            issuer_url: format!("https://{label}.example.com/oidc"),
            claim_attribute_path: "email".to_string(),
            identity_store_attribute_path: "userName".to_string(),
            jwks_retrieval_option: JWKS_RETRIEVAL_OPTION.to_string(),
        },
    )
}

/// The tags every issuer this suite creates carries, so an update is shown to leave the
/// tag set of the issuer it renames alone.
const BASELINE_TAGS: [(&str, &str); 2] = [("owner", "identity"), ("Env", "test")];

/// One `UpdateTrustedTokenIssuer` request, as a row of the table: which members it
/// carries, and with which values.
#[derive(Debug, Clone, Default)]
struct Update {
    label: String,
    name: Option<String>,
    claim_attribute_path: Option<String>,
    identity_store_attribute_path: Option<String>,
    jwks_retrieval_option: Option<String>,
    /// Send `TrustedTokenIssuerConfiguration` even when its `OidcJwtConfiguration`
    /// carries no member: every member of `OidcJwtUpdateConfiguration` is optional
    /// (`smithy_query(query="shape",shape="OidcJwtUpdateConfiguration")`), so this is a
    /// request the model admits.
    send_empty_configuration: bool,
}

impl Update {
    fn new(label: &str) -> Self {
        Self {
            label: label.to_string(),
            ..Self::default()
        }
    }

    fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    fn with_claim(mut self, path: &str) -> Self {
        self.claim_attribute_path = Some(path.to_string());
        self
    }

    fn with_identity(mut self, path: &str) -> Self {
        self.identity_store_attribute_path = Some(path.to_string());
        self
    }

    fn with_jwks(mut self, option: &str) -> Self {
        self.jwks_retrieval_option = Some(option.to_string());
        self
    }

    fn with_empty_configuration(mut self) -> Self {
        self.send_empty_configuration = true;
        self
    }

    /// The `UpdateTrustedTokenIssuer` request body for this row.
    fn request(&self, trusted_token_issuer_arn: &str) -> Value {
        let mut body = json!({"TrustedTokenIssuerArn": trusted_token_issuer_arn});
        if let Some(name) = &self.name {
            body["Name"] = json!(name);
        }
        let mut oidc = Map::new();
        if let Some(path) = &self.claim_attribute_path {
            oidc.insert("ClaimAttributePath".to_string(), json!(path));
        }
        if let Some(path) = &self.identity_store_attribute_path {
            oidc.insert("IdentityStoreAttributePath".to_string(), json!(path));
        }
        if let Some(option) = &self.jwks_retrieval_option {
            oidc.insert("JwksRetrievalOption".to_string(), json!(option));
        }
        if !oidc.is_empty() || self.send_empty_configuration {
            body["TrustedTokenIssuerConfiguration"] =
                json!({"OidcJwtConfiguration": Value::Object(oidc)});
        }
        body
    }

    /// Rule 1 spelled out as a computation: the stored record after this update is the
    /// baseline with exactly the members the request carries overwritten.
    fn merged(&self, name: &str, configuration: &Configuration) -> (String, Configuration) {
        let mut name = name.to_string();
        let mut configuration = configuration.clone();
        if let Some(updated) = &self.name {
            name = updated.clone();
        }
        if let Some(path) = &self.claim_attribute_path {
            configuration.claim_attribute_path = path.clone();
        }
        if let Some(path) = &self.identity_store_attribute_path {
            configuration.identity_store_attribute_path = path.clone();
        }
        if let Some(option) = &self.jwks_retrieval_option {
            configuration.jwks_retrieval_option = option.clone();
        }
        (name, configuration)
    }
}

/// The rows differ along what rule 1 generalises over: WHICH of the updatable members
/// the request carries (each alone, every combination of two, all of them, none of them),
/// and the value classes each member admits — `Name`'s length and character class
/// (`len 1..255`, `^[\w+=,.@-]+$`), the separator and segment count of
/// `ClaimAttributePath` (`^\p{L}+(?:(\.|\_)\p{L}+){0,2}$`) and of
/// `IdentityStoreAttributePath` (`^\p{L}+(?:\.\p{L}+){0,2}$`), and the one value
/// `JwksRetrievalOption` admits. All constraints are from
/// `smithy_query(query="operation",operation="UpdateTrustedTokenIssuer")` and
/// `smithy_query(query="shape",shape="OidcJwtUpdateConfiguration")`.
fn updates() -> Vec<Update> {
    vec![
        Update::new("Name alone, at the shortest length the model admits").with_name("a"),
        Update::new("Name alone, at the 255-character limit, spanning every allowed class")
            .with_name(format!("Tti_0+=,.@-{}", "x".repeat(244))),
        Update::new("ClaimAttributePath alone, a single segment").with_claim("sub"),
        Update::new("ClaimAttributePath alone, three segments joined by the _ separator")
            .with_claim("user_email_address"),
        Update::new("IdentityStoreAttributePath alone, three segments joined by .")
            .with_identity("emails.primary.value"),
        Update::new("JwksRetrievalOption alone, the one value the enum admits")
            .with_jwks(JWKS_RETRIEVAL_OPTION),
        Update::new("Name and both attribute paths at once")
            .with_name("renamed.and-remapped")
            .with_claim("claims.email")
            .with_identity("addresses.formatted"),
        Update::new("both attribute paths, leaving Name alone")
            .with_claim("nickname")
            .with_identity("displayName"),
        Update::new("Name and one attribute path, leaving the other alone")
            .with_name("half+remapped")
            .with_claim("preferred_username"),
        Update::new("the whole configuration, leaving Name alone")
            .with_claim("given_name")
            .with_identity("name.givenName")
            .with_jwks(JWKS_RETRIEVAL_OPTION),
        Update::new("an OidcJwtConfiguration carrying no member: nothing to write")
            .with_empty_configuration(),
        Update::new("Name beside an OidcJwtConfiguration carrying no member")
            .with_name("named-only")
            .with_empty_configuration(),
        Update::new("neither Name nor TrustedTokenIssuerConfiguration: nothing to write"),
    ]
}

/// Create one issuer with `BASELINE_TAGS` and return the `TrustedTokenIssuerArn` minted
/// for it.
async fn create(
    router: &MultiServiceRouter,
    instance_arn: &str,
    name: &str,
    configuration: &Configuration,
) -> String {
    let body = json!({
        "InstanceArn": instance_arn,
        "Name": name,
        "TrustedTokenIssuerType": ISSUER_TYPE,
        "TrustedTokenIssuerConfiguration": configuration.json(),
        "Tags": BASELINE_TAGS
            .iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect::<Vec<Value>>(),
    });
    let (status, body) = call(router, "CreateTrustedTokenIssuer", &body).await;
    assert_eq!(
        status, 200,
        "CreateTrustedTokenIssuer for {name:?} must answer 200, got {status} {body}"
    );
    body["TrustedTokenIssuerArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateTrustedTokenIssuer minted no ARN: {body}"))
        .to_string()
}

/// `DescribeTrustedTokenIssuer` for `arn`.
async fn describe(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(
        router,
        "DescribeTrustedTokenIssuer",
        &json!({"TrustedTokenIssuerArn": arn}),
    )
    .await
}

/// The `(Key, Value)` pairs `ListTagsForResource` reports for `arn`, sorted so the
/// assertion is about the SET a tagged resource carries.
async fn tags(router: &MultiServiceRouter, arn: &str) -> Vec<(String, String)> {
    let (status, body) = call(
        router,
        "ListTagsForResource",
        &json!({"ResourceArn": arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "ListTagsForResource for {arn:?} must answer 200, got {status} {body}"
    );
    let mut reported: Vec<(String, String)> = body["Tags"]
        .as_array()
        .map(|list| {
            list.iter()
                .map(|tag| {
                    (
                        tag["Key"].as_str().unwrap_or_default().to_string(),
                        tag["Value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    reported.sort();
    reported
}

/// Assert `arn` describes as `name` and `configuration`, under `ISSUER_TYPE` and its own
/// ARN — the whole stored record, so a member an update should have left alone cannot
/// drift unnoticed.
async fn assert_describes_as(
    router: &MultiServiceRouter,
    case: &str,
    what: &str,
    arn: &str,
    name: &str,
    configuration: &Configuration,
) {
    let (status, body) = describe(router, arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: describing the {what} must answer 200, got {status} {body}"
    );
    assert_eq!(
        body["TrustedTokenIssuerArn"].as_str(),
        Some(arn),
        "case {case:?}: the {what} must report the ARN it was asked about, got {body}"
    );
    assert_eq!(
        body["Name"].as_str(),
        Some(name),
        "case {case:?}: the {what} must report the Name {name:?}, got {body}"
    );
    assert_eq!(
        body["TrustedTokenIssuerType"].as_str(),
        Some(ISSUER_TYPE),
        "case {case:?}: the {what}'s stored type is no member of an update, so it must \
         still be {ISSUER_TYPE}, got {body}"
    );
    assert_eq!(
        body["TrustedTokenIssuerConfiguration"],
        configuration.json(),
        "case {case:?}: the {what} must report the configuration {configuration:?}, got \
         {body}"
    );
}

/// Rules 1, 2 and 3: every row writes exactly the members it carries, leaves the other
/// issuer and the tag set alone, and re-applying it changes nothing.
#[tokio::test]
async fn an_update_writes_only_the_members_the_request_carries() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;

    for update in updates() {
        let case = update.label.clone();
        // A fresh target and a BYSTANDER per row, so each row starts from the same
        // baseline and an update of one issuer is shown not to reach the other.
        let (target_name, target_configuration) = baseline("target");
        let target = create(&router, &instance_arn, &target_name, &target_configuration).await;
        let (bystander_name, bystander_configuration) = baseline("bystander");
        let bystander = create(
            &router,
            &instance_arn,
            &bystander_name,
            &bystander_configuration,
        )
        .await;

        let (status, body) = call(
            &router,
            "UpdateTrustedTokenIssuer",
            &update.request(&target),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the update must answer 200, got {status} {body}"
        );
        assert!(
            body.as_object().is_none_or(Map::is_empty),
            "case {case:?}: UpdateTrustedTokenIssuerResponse models no members, got {body}"
        );

        let (expected_name, expected_configuration) =
            update.merged(&target_name, &target_configuration);
        assert_describes_as(
            &router,
            &case,
            "updated issuer",
            &target,
            &expected_name,
            &expected_configuration,
        )
        .await;

        // Rule 2: the other issuer of the same instance is untouched, and so is the tag
        // set of the issuer that was updated.
        assert_describes_as(
            &router,
            &case,
            "bystander issuer",
            &bystander,
            &bystander_name,
            &bystander_configuration,
        )
        .await;
        let expected_tags: Vec<(String, String)> = {
            let mut pairs: Vec<(String, String)> = BASELINE_TAGS
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect();
            pairs.sort();
            pairs
        };
        let reported = tags(&router, &target).await;
        assert_eq!(
            reported, expected_tags,
            "case {case:?}: an update writes no tags, expected {expected_tags:?}, got \
             {reported:?}"
        );

        // Rule 3: the same update again is the same merge.
        let (status, body) = call(
            &router,
            "UpdateTrustedTokenIssuer",
            &update.request(&target),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: repeating the update must answer 200, got {status} {body}"
        );
        assert_describes_as(
            &router,
            &case,
            "re-updated issuer",
            &target,
            &expected_name,
            &expected_configuration,
        )
        .await;
    }
}

/// Rule 1, the other direction: updates ACCUMULATE on the stored record rather than
/// resetting it, so a member written by an earlier update survives a later one that does
/// not carry it.
#[tokio::test]
async fn successive_updates_accumulate_on_the_stored_record() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let (mut name, mut configuration) = baseline("accumulating");
    let arn = create(&router, &instance_arn, &name, &configuration).await;

    // Each step carries ONE member, so after the last one the record holds a value from
    // every step: no step may reset what an earlier one wrote.
    let steps = vec![
        Update::new("step 1: Name").with_name("step-one"),
        Update::new("step 2: ClaimAttributePath").with_claim("claims.email"),
        Update::new("step 3: IdentityStoreAttributePath").with_identity("emails.value"),
        Update::new("step 4: Name again").with_name("step-four"),
        Update::new("step 5: nothing at all"),
    ];
    for step in steps {
        let case = step.label.clone();
        let (status, body) = call(&router, "UpdateTrustedTokenIssuer", &step.request(&arn)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the update must answer 200, got {status} {body}"
        );
        (name, configuration) = step.merged(&name, &configuration);
        assert_describes_as(
            &router,
            &case,
            "accumulating issuer",
            &arn,
            &name,
            &configuration,
        )
        .await;
    }

    // The issuer's own identity never moved: the ARN and the URL no update member names.
    let (_, baseline_configuration) = baseline("accumulating");
    assert_eq!(
        configuration.issuer_url, baseline_configuration.issuer_url,
        "IssuerUrl is no member of OidcJwtUpdateConfiguration, so it must have survived \
         every step, got {:?}",
        configuration.issuer_url
    );
}

/// Rule 4: a `TrustedTokenIssuerArn` no create minted is refused, with the wording the
/// describe of the same ARN answers, and nothing is written for it.
#[tokio::test]
async fn updating_an_arn_no_create_minted_is_refused_and_stores_nothing() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let instance_id = instance_arn
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();
    // One issuer exists, so the refusal is about the ARN asked for and not about an empty
    // store.
    let (stored_name, stored_configuration) = baseline("stored");
    let stored = create(&router, &instance_arn, &stored_name, &stored_configuration).await;

    let account = stored.split(':').nth(4).unwrap_or_default().to_string();
    let uuid = stored.rsplit("/tti-").next().unwrap_or_default().to_string();
    let tti_arn = |partition: &str, account: &str, instance: &str, uuid: &str| {
        format!("arn:{partition}:sso::{account}:trustedTokenIssuer/{instance}/tti-{uuid}")
    };

    // The rows span the parts of the ARN that identify the issuer: the `tti-` UUID, the
    // instance segment, the account segment and the partition.
    let cases: Vec<(&str, String)> = vec![
        (
            "a pattern-valid tti- UUID that was never minted",
            tti_arn(
                "aws",
                &account,
                &instance_id,
                "00000000-0000-4000-8000-000000000000",
            ),
        ),
        (
            "the stored UUID under another instance",
            tti_arn("aws", &account, "ssoins-0000000000000000", &uuid),
        ),
        (
            "the stored issuer under another account",
            tti_arn("aws", "210987654321", &instance_id, &uuid),
        ),
        (
            "the stored issuer in another partition",
            tti_arn("aws-us-gov", &account, &instance_id, &uuid),
        ),
    ];

    for (case, arn) in cases {
        assert_ne!(
            arn, stored,
            "case {case:?}: the row must name a DIFFERENT ARN than the stored one"
        );
        let (status, body) = call(
            &router,
            "UpdateTrustedTokenIssuer",
            &Update::new(case).with_name("should-not-apply").request(&arn),
        )
        .await;
        assert_refused(
            case,
            "updating an unminted TrustedTokenIssuerArn",
            "ResourceNotFoundException",
            status,
            &body,
        );
        let expected = format!("Could not find TrustedTokenIssuer with arn {arn}.");
        assert_eq!(
            body["Message"].as_str(),
            Some(expected.as_str()),
            "case {case:?}: expected the message {expected:?}, got {body}"
        );
        // The refusal wrote nothing: the ARN still describes as absent.
        let (status, described) = describe(&router, &arn).await;
        assert_refused(
            case,
            "describing the ARN the refused update named",
            "ResourceNotFoundException",
            status,
            &described,
        );
    }

    // The refusals left the one stored issuer exactly as its create stored it.
    assert_describes_as(
        &router,
        "after the refusals",
        "stored issuer",
        &stored,
        &stored_name,
        &stored_configuration,
    )
    .await;
}
