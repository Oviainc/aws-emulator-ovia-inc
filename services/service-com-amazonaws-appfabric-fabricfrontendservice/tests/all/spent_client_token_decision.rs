// SPDX-License-Identifier: CC0-1.0

//! The ONE spent-`@idempotencyToken` decision every create of this service makes,
//! driven over the wire (restJson1) through the same router the emulator boots.
//!
//! RULE: for every create carrying an `@idempotencyToken`, a token already spent
//! in this Region for this operation decides the call three ways and the same way
//! each time — the same token with DIFFERENT parameters is the documented
//! parameter-mismatch conflict (HTTP 409 `ConflictException`, naming the token and
//! the type of resource the first call created), the same token with the SAME
//! parameters replays exactly the resource that call created (no second resource,
//! and the replay reports what a read of it reports), and a token whose resource
//! has since been deleted is spent on nothing, so the same call creates a NEW
//! resource under it.
//!
//! The table's rows are the four creates the rule holds over — `CreateAppBundle`,
//! `CreateAppAuthorization`, `CreateIngestion` and `CreateIngestionDestination` —
//! each at its own level of AppFabric's resource chain, and each differing in the
//! member the mismatch has to notice (a customer-managed key, a credential, a tag
//! set, a destination configuration).
//!
//! The harness (the router, `Answer`, `encode`, `arn_of`) is the one
//! `app_bundle_ingestion_lifecycle` already builds.

use serde_json::{json, Value};

use crate::app_bundle_ingestion_lifecycle::{arn_of, encode, Emulator};

/// A client token in the form the model's `UUID` pattern requires
/// (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`), built from
/// `seed` so a test can name the same token twice.
fn token(seed: u32) -> String {
    format!("{seed:08x}-3333-4333-8333-333333333333")
}

/// The application and tenant the authorization and ingestion rows name. One
/// (app, tenant) pair per store, because the rule is about the token rather than
/// about which pair is ingested.
const APP: &str = "SLACK";
const TENANT: &str = "t-decision";

/// The creates of this service that carry an `@idempotencyToken`: one row per
/// level of AppFabric's resource chain (bundle → authorization, bundle →
/// ingestion → destination).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Create {
    AppBundle,
    AppAuthorization,
    Ingestion,
    IngestionDestination,
}

impl Create {
    /// The response member holding the created resource.
    fn resource_member(self) -> &'static str {
        match self {
            Self::AppBundle => "appBundle",
            Self::AppAuthorization => "appAuthorization",
            Self::Ingestion => "ingestion",
            Self::IngestionDestination => "ingestionDestination",
        }
    }

    /// The member of that resource holding its ARN. `AppAuthorization` names its
    /// own `appAuthorizationArn`; every other shape names it `arn`.
    fn arn_member(self) -> &'static str {
        match self {
            Self::AppAuthorization => "appAuthorizationArn",
            _ => "arn",
        }
    }

    /// The type of resource the mismatch conflict reports in its `resourceType`.
    fn resource_type(self) -> &'static str {
        match self {
            Self::AppBundle => "AppBundle",
            Self::AppAuthorization => "AppAuthorization",
            Self::Ingestion => "Ingestion",
            Self::IngestionDestination => "IngestionDestination",
        }
    }

    /// The request body this create is first called with, carrying `token`.
    fn body(self, token: &str) -> Value {
        match self {
            Self::AppBundle => json!({"clientToken": token}),
            Self::AppAuthorization => json!({
                "clientToken": token,
                "app": APP,
                "authType": "apiKey",
                "credential": {"apiKeyCredential": {"apiKey": "placeholder-api-key"}},
                "tenant": {"tenantIdentifier": TENANT, "tenantDisplayName": TENANT},
            }),
            Self::Ingestion => json!({
                "clientToken": token,
                "app": APP,
                "tenantId": TENANT,
                "ingestionType": "auditLog",
            }),
            // `schema`/`format` are the model's own closed enums (`Schema = ocsf
            // | raw`, `Format = json | parquet`).
            Self::IngestionDestination => json!({
                "clientToken": token,
                "processingConfiguration": {"auditLog": {"schema": "ocsf", "format": "json"}},
                "destinationConfiguration": {"auditLog": {"destination": {
                    "s3Bucket": {"bucketName": "audit-logs-bucket"}}}},
            }),
        }
    }

    /// The same request with ONE member differing — which is what makes the
    /// second call a different request rather than a retry. Each row differs in a
    /// member of its own shape, so no single member decides the rule.
    fn differing_body(self, token: &str) -> Value {
        let mut body = self.body(token);
        match self {
            Self::AppBundle => {
                body["customerManagedKeyIdentifier"] =
                    json!("11111111-2222-4333-8444-555555555555");
            }
            Self::AppAuthorization => {
                body["credential"] = json!({"apiKeyCredential": {"apiKey": "rotated-api-key"}});
            }
            Self::Ingestion => {
                body["tags"] = json!([{"key": "env", "value": "prod"}]);
            }
            Self::IngestionDestination => {
                body["destinationConfiguration"] = json!({"auditLog": {"destination": {
                    "s3Bucket": {"bucketName": "other-audit-logs-bucket"}}}});
            }
        }
        body
    }

    /// The parent chain this create needs, as `(bundle arn, ingestion arn)`.
    async fn chain(self, emulator: &Emulator, case: &str) -> (Option<String>, Option<String>) {
        if self == Self::AppBundle {
            return (None, None);
        }
        let created = emulator.create_app_bundle(json!({})).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: the set-up app bundle must be created, got HTTP {} with body {}",
            created.status, created.body
        );
        let bundle = arn_of(&created.body, "appBundle");
        if self == Self::AppAuthorization {
            return (Some(bundle), None);
        }
        // authorize → connect → ingest: the dependency an ingestion needs.
        emulator.authorize(&bundle, APP, TENANT).await;
        if self == Self::Ingestion {
            return (Some(bundle), None);
        }
        let ingestion = emulator
            .call(
                "POST",
                &format!("/appbundles/{}/ingestions", encode(&bundle)),
                Some(json!({"app": APP, "tenantId": TENANT, "ingestionType": "auditLog"})),
            )
            .await;
        assert_eq!(
            ingestion.status, 201,
            "case {case:?}: the set-up ingestion must be created, got HTTP {} with body {}",
            ingestion.status, ingestion.body
        );
        let ingestion = arn_of(&ingestion.body, "ingestion");
        (Some(bundle), Some(ingestion))
    }

    /// The collection URI this create posts to, under the chain it needs.
    fn uri(self, bundle: Option<&str>, ingestion: Option<&str>) -> String {
        match self {
            Self::AppBundle => "/appbundles".to_string(),
            Self::AppAuthorization => format!(
                "/appbundles/{}/appauthorizations",
                encode(bundle.unwrap_or_default())
            ),
            Self::Ingestion => format!(
                "/appbundles/{}/ingestions",
                encode(bundle.unwrap_or_default())
            ),
            Self::IngestionDestination => format!(
                "/appbundles/{}/ingestions/{}/ingestiondestinations",
                encode(bundle.unwrap_or_default()),
                encode(ingestion.unwrap_or_default())
            ),
        }
    }
}

/// A string member of a JSON object, or `""`.
fn member(value: &Value, name: &str) -> String {
    value
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The ARN of the resource a create answered with.
fn created_arn(create: Create, body: &Value, case: &str) -> String {
    let resource = body
        .get(create.resource_member())
        .unwrap_or_else(|| panic!("case {case:?}: expected {} in {body}", create.resource_member()));
    let arn = member(resource, create.arn_member());
    assert!(
        !arn.is_empty(),
        "case {case:?}: expected a non-empty {} in {resource}",
        create.arn_member()
    );
    arn
}

/// RULE: every create of this service makes the SAME decision about a spent
/// idempotency token — mismatched parameters are the HTTP 409 `ConflictException`
/// that names the token and the resource type, matching parameters replay the
/// resource the first call created (which a read reports identically), and a token
/// whose resource was deleted is free again, so the same call creates a new one.
#[tokio::test]
async fn every_create_decides_a_spent_client_token_the_same_way() {
    for (index, create) in [
        Create::AppBundle,
        Create::AppAuthorization,
        Create::Ingestion,
        Create::IngestionDestination,
    ]
    .into_iter()
    .enumerate()
    {
        let case = format!("{create:?}");
        // A fresh store per row: a Region holds at most one app bundle.
        let emulator = Emulator::new();
        let (bundle, ingestion) = create.chain(&emulator, &case).await;
        let uri = create.uri(bundle.as_deref(), ingestion.as_deref());
        let spent = token(0x500 + index as u32);
        let body = create.body(&spent);

        let created = emulator.call("POST", &uri, Some(body.clone())).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201 from the first create, got {} with body {}",
            created.status, created.body
        );
        let arn = created_arn(create, &created.body, &case);

        // 1. The same token with one member differing: the documented mismatch.
        let conflict = emulator
            .call("POST", &uri, Some(create.differing_body(&spent)))
            .await;
        assert_eq!(
            conflict.status, 409,
            "case {case:?}: expected HTTP 409 for a parameter mismatch, got {} with body {}",
            conflict.status, conflict.body
        );
        assert_eq!(
            conflict.error_code(),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {} with body {}",
            conflict.error_code(),
            conflict.body
        );
        assert_eq!(
            member(&conflict.body, "message"),
            format!("The clientToken {spent} was already used with different request parameters."),
            "case {case:?}: expected the mismatch sentence for {spent:?}, got {}",
            conflict.body
        );
        assert_eq!(
            member(&conflict.body, "resourceId"),
            spent,
            "case {case:?}: the mismatch must name the token it judged, got {}",
            conflict.body
        );
        assert_eq!(
            member(&conflict.body, "resourceType"),
            create.resource_type(),
            "case {case:?}: expected resourceType {:?}, got {}",
            create.resource_type(),
            conflict.body
        );

        // 2. The same token with the same parameters replays the one resource,
        //    however often the retry comes, and reports what a read reports.
        let read_uri = format!("{uri}/{}", encode(&arn));
        for retry in 1..=2 {
            let replay = emulator.call("POST", &uri, Some(body.clone())).await;
            assert_eq!(
                replay.status, 201,
                "case {case:?} retry {retry}: expected HTTP 201 from a replay, got {} with body {}",
                replay.status, replay.body
            );
            assert_eq!(
                created_arn(create, &replay.body, &case),
                arn,
                "case {case:?} retry {retry}: expected the replayed resource {arn:?}, got {}",
                replay.body
            );
            let read = emulator.call("GET", &read_uri, None).await;
            assert_eq!(
                read.status, 200,
                "case {case:?} retry {retry}: the replayed resource must be readable, got HTTP {} \
                 with body {}",
                read.status, read.body
            );
            assert_eq!(
                read.body.get(create.resource_member()),
                replay.body.get(create.resource_member()),
                "case {case:?} retry {retry}: the replay must report the stored record, got {}",
                read.body
            );
        }

        // 3. Once the resource is deleted the token is spent on nothing, so the
        //    same call creates a new resource rather than replaying the gone one.
        let deleted = emulator.call("DELETE", &read_uri, None).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: expected HTTP 204 from the delete, got {} with body {}",
            deleted.status, deleted.body
        );
        let gone = emulator.call("GET", &read_uri, None).await;
        assert_eq!(
            gone.status, 404,
            "case {case:?}: the deleted resource must stay absent, got HTTP {} with body {}",
            gone.status, gone.body
        );

        let afresh = emulator.call("POST", &uri, Some(body)).await;
        assert_eq!(
            afresh.status, 201,
            "case {case:?}: expected HTTP 201 after the delete freed the token, got {} with body {}",
            afresh.status, afresh.body
        );
        let fresh_arn = created_arn(create, &afresh.body, &case);
        assert_ne!(
            fresh_arn, arn,
            "case {case:?}: a token spent on a deleted resource must mint a new one, got {arn:?} \
             again"
        );
        let read_fresh = emulator
            .call("GET", &format!("{uri}/{}", encode(&fresh_arn)), None)
            .await;
        assert_eq!(
            read_fresh.status, 200,
            "case {case:?}: the newly created resource must be readable, got HTTP {} with body {}",
            read_fresh.status, read_fresh.body
        );
        assert_eq!(
            read_fresh.body.get(create.resource_member()),
            afresh.body.get(create.resource_member()),
            "case {case:?}: the new create must report the stored record, got {}",
            read_fresh.body
        );
    }
}
