// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHEN a create operation judges a precondition that is a
//! fact about the store rather than about the request, driven over the wire
//! (restJson1) through the same router the emulator boots.
//!
//! RULE: a create operation judges every STATE-dependent precondition — a
//! dependency on another resource, an at-most-one guard, a quota — only AFTER
//! the `clientToken` replay, so a retry carrying a spent token and the same
//! parameters replays the resource the first call created even once that
//! precondition no longer holds; a call that is NOT such a retry (a fresh token)
//! is still refused by it, with the error each operation models.
//!
//! The harness (the router, `encode`, `arn_of`) is the one
//! `app_bundle_ingestion_lifecycle` already builds.

use serde_json::{json, Value};
use service_com_amazonaws_appfabric_fabricfrontendservice::state::{
    APP_AUTHORIZATION_STATUS_CONNECTED, APP_AUTHORIZATION_STATUS_PENDING_CONNECT,
};

use crate::app_bundle_ingestion_lifecycle::{arn_of, encode, Emulator};
use crate::ingestion_authorization_refusal_wording::NOT_CONNECTED_MESSAGE;

/// A client token in the form the model's `UUID` pattern requires
/// (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`), built
/// from `seed` so a test can name the same token twice.
fn token(seed: u32) -> String {
    format!("{seed:08x}-2222-4222-8222-222222222222")
}

/// The app bundle every row hangs its chain off, as its ARN.
async fn app_bundle(emulator: &Emulator) -> String {
    let created = emulator.create_app_bundle(json!({})).await;
    assert_eq!(
        created.status, 201,
        "the set-up app bundle must be created, got HTTP {} with body {}",
        created.status, created.body
    );
    arn_of(&created.body, "appBundle")
}

/// `CreateAppAuthorization` + `ConnectAppAuthorization` for one (app, tenant)
/// pair, as the authorization's ARN: the dependency AppFabric's resource chain
/// puts before an ingestion (authorize → connect → ingest).
async fn connected_authorization(
    emulator: &Emulator,
    bundle: &str,
    app: &str,
    tenant: &str,
    case: &str,
) -> String {
    let created = emulator
        .call(
            "POST",
            &format!("/appbundles/{}/appauthorizations", encode(bundle)),
            Some(json!({
                "app": app,
                "authType": "apiKey",
                "credential": {"apiKeyCredential": {"apiKey": "placeholder-api-key"}},
                "tenant": {"tenantIdentifier": tenant, "tenantDisplayName": format!("{tenant} display")},
            })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: the set-up app authorization must be created, got HTTP {} with body {}",
        created.status, created.body
    );
    let arn = member(
        created
            .body
            .get("appAuthorization")
            .unwrap_or_else(|| panic!("case {case:?}: expected appAuthorization in {}", created.body)),
        "appAuthorizationArn",
    );
    let connect = emulator
        .call(
            "POST",
            &format!(
                "/appbundles/{}/appauthorizations/{}/connect",
                encode(bundle),
                encode(&arn)
            ),
            Some(json!({})),
        )
        .await;
    assert_eq!(
        connect.status, 200,
        "case {case:?}: the set-up app authorization must connect, got HTTP {} with body {}",
        connect.status, connect.body
    );
    arn
}

/// A string member of a JSON object, or `""`.
fn member(value: &Value, name: &str) -> String {
    value
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The status `GetAppAuthorization` reports for `authorization`.
async fn held_status(emulator: &Emulator, bundle: &str, authorization: &str, case: &str) -> String {
    let read = emulator
        .call(
            "GET",
            &format!(
                "/appbundles/{}/appauthorizations/{}",
                encode(bundle),
                encode(authorization)
            ),
            None,
        )
        .await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: the app authorization must be readable, got HTTP {} with body {}",
        read.status, read.body
    );
    member(
        read.body
            .get("appAuthorization")
            .unwrap_or_else(|| panic!("case {case:?}: expected appAuthorization in {}", read.body)),
        "status",
    )
}

/// How an ingestion's app-authorization dependency stops holding AFTER the
/// ingestion was created. Every row is reachable with modelled calls alone:
/// `UpdateAppAuthorization` applies a change whenever it is given a `credential`
/// or a `tenant`, and an applied update invalidates the connect handshake. What
/// no modelled call reaches is the pair losing its authorization ALTOGETHER:
/// while the bundle holds the ingestion, both the delete and a tenant move are
/// refused by the precondition `DeleteAppAuthorization` documents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Broken {
    /// The credential is rotated, which re-pends the handshake.
    CredentialRotated,
    /// Only the tenant's DISPLAY name changes — still an applied update, so the
    /// handshake is re-pended for the same (app, tenant) pair.
    TenantDisplayNameChanged,
}

impl Broken {
    /// The `UpdateAppAuthorization` body that breaks the dependency this way.
    fn update_body(self, tenant: &str) -> Value {
        match self {
            Self::CredentialRotated => {
                json!({"credential": {"apiKeyCredential": {"apiKey": "rotated-api-key"}}})
            }
            Self::TenantDisplayNameChanged => json!({"tenant": {
                "tenantIdentifier": tenant,
                "tenantDisplayName": format!("{tenant} renamed"),
            }}),
        }
    }
}

/// RULE: once an ingestion exists, a retry carrying its spent `clientToken` and
/// the same parameters replays THAT ingestion, whatever the app authorization it
/// depends on has become in the meantime — while a call that is not a retry (a
/// fresh token, same parameters) is still refused with the modelled
/// `ValidationException` (HTTP 400), and no second ingestion is ever written.
///
/// The table spans every way an applied `UpdateAppAuthorization` can stop the
/// dependency from holding (credential rotated, tenant display name changed),
/// over applications and tenants the issue did not cite and over a body with and
/// without `tags`.
#[tokio::test]
async fn a_spent_token_replays_its_ingestion_after_the_dependency_stops_holding() {
    // (case, app, tenant, tags, how the dependency is broken)
    let cases: Vec<(&str, &str, &str, Value, Broken)> = vec![
        (
            "credential rotated, no tags",
            "ASANA",
            "tenant-asana",
            Value::Null,
            Broken::CredentialRotated,
        ),
        (
            "credential rotated, two tags",
            "JIRA",
            "tenant-jira",
            json!([{"key": "env", "value": "prod"}, {"key": "team", "value": "data"}]),
            Broken::CredentialRotated,
        ),
        (
            "tenant display name changed",
            "ZENDESK",
            "tenant-zendesk",
            Value::Null,
            Broken::TenantDisplayNameChanged,
        ),
        (
            "tenant display name changed, one tag",
            "CISCO",
            "tenant-cisco",
            json!([{"key": "only", "value": "one"}]),
            Broken::TenantDisplayNameChanged,
        ),
        (
            "credential rotated, another application",
            "GOOGLE",
            "tenant-google",
            Value::Null,
            Broken::CredentialRotated,
        ),
    ];

    for (index, (case, app, tenant, tags, broken)) in cases.into_iter().enumerate() {
        // One bundle per row: a Region holds at most one app bundle.
        let emulator = Emulator::new();
        let bundle = app_bundle(&emulator).await;
        let authorization = connected_authorization(&emulator, &bundle, app, tenant, case).await;
        assert_eq!(
            held_status(&emulator, &bundle, &authorization, case).await,
            APP_AUTHORIZATION_STATUS_CONNECTED,
            "case {case:?}: the set-up must leave the authorization connected"
        );

        let spent = token(0x100 + index as u32);
        let mut body = json!({
            "clientToken": spent,
            "app": app,
            "tenantId": tenant,
            "ingestionType": "auditLog",
        });
        if !tags.is_null() {
            body["tags"] = tags;
        }
        let uri = format!("/appbundles/{}/ingestions", encode(&bundle));

        let created = emulator.call("POST", &uri, Some(body.clone())).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: a connected authorization must admit the ingestion, got HTTP {} with body {}",
            created.status, created.body
        );
        let arn = arn_of(&created.body, "ingestion");

        // Break the dependency with the modelled update.
        let updated = emulator
            .call(
                "PATCH",
                &format!(
                    "/appbundles/{}/appauthorizations/{}",
                    encode(&bundle),
                    encode(&authorization)
                ),
                Some(broken.update_body(tenant)),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: the breaking update must be accepted, got HTTP {} with body {}",
            updated.status, updated.body
        );
        assert_eq!(
            held_status(&emulator, &bundle, &authorization, case).await,
            APP_AUTHORIZATION_STATUS_PENDING_CONNECT,
            "case {case:?}: an applied update must re-pend the handshake"
        );

        // The dependency really does not hold any more: the SAME parameters
        // under a token this Region never spent are refused.
        let mut fresh = body.clone();
        fresh["clientToken"] = Value::String(token(0x200 + index as u32));
        let refused = emulator.call("POST", &uri, Some(fresh)).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a fresh token must still be refused for a pair with no connected \
             authorization, got HTTP {} with body {}",
            refused.status, refused.body
        );
        assert!(
            refused.error_code().contains("ValidationException"),
            "case {case:?}: the refusal must be the modelled ValidationException, got {} with \
             body {}",
            refused.error_code(),
            refused.body
        );
        let refusal_message = member(&refused.body, "message");
        // The refusal carries the service's fixed sentence for the connect step
        // the pair now owes again, with no value of the request or of the store
        // in it — see `ingestion_authorization_refusal_wording`.
        assert_eq!(
            refusal_message, NOT_CONNECTED_MESSAGE,
            "case {case:?}: expected the service's fixed sentence for an unconnected pair, got \
             {refusal_message:?}"
        );

        // The retry, however often it comes, replays the ingestion the first
        // call created rather than being refused by the broken dependency.
        for retry in 1..=2 {
            let replay = emulator.call("POST", &uri, Some(body.clone())).await;
            assert_eq!(
                replay.status, 201,
                "case {case:?} retry {retry}: a spent token must replay its ingestion, got HTTP \
                 {} with body {}",
                replay.status, replay.body
            );
            assert_eq!(
                arn_of(&replay.body, "ingestion"),
                arn,
                "case {case:?} retry {retry}: expected the replayed ingestion {arn:?}, got {}",
                replay.body
            );
            let read_back = emulator.get_ingestion(&bundle, &arn).await;
            assert_eq!(
                read_back.status, 200,
                "case {case:?} retry {retry}: the replayed ingestion must still be readable, got \
                 HTTP {} with body {}",
                read_back.status, read_back.body
            );
            assert_eq!(
                read_back.body.get("ingestion"),
                replay.body.get("ingestion"),
                "case {case:?} retry {retry}: the replay must report the stored record, got {}",
                read_back.body
            );
        }

        // Nothing was written by any of the calls above: the bundle still holds
        // exactly the one ingestion the first call created.
        let listed = emulator.list_ingestions(&bundle, "").await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: ListIngestions must answer HTTP 200, got {} with body {}",
            listed.status, listed.body
        );
        let summaries = listed
            .body
            .get("ingestions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected ingestions in {}", listed.body));
        assert_eq!(
            summaries.len(),
            1,
            "case {case:?}: expected exactly the one created ingestion, got {summaries:?}"
        );
        assert_eq!(
            member(&summaries[0], "arn"),
            arn,
            "case {case:?}: expected the stored ingestion {arn:?}, got {:?}",
            summaries[0]
        );
    }
}

/// The create operations of this service that carry an `@idempotencyToken`, each
/// paired with the state-dependent precondition that stops holding once its
/// resource exists.
#[derive(Debug, Clone, Copy)]
enum Create {
    /// `CreateAppBundle`: the created bundle itself fills the one-bundle-per-
    /// Region quota, so a non-retry is the quota error.
    AppBundle,
    /// `CreateAppAuthorization`: the created authorization itself occupies its
    /// (bundle, app, tenant) triple, so a non-retry is the validation refusal of
    /// a second authorization for that triple.
    AppAuthorization,
    /// `CreateIngestion`: the authorization the ingestion depends on is re-pended
    /// by an applied update, so a non-retry is the validation refusal.
    Ingestion,
}

/// RULE (the same one, across every create this service models): a spent token
/// with the same parameters replays its resource BEFORE any state-dependent
/// precondition is judged, while a fresh token with those parameters is refused
/// with the error that operation models for the precondition — a quota for the
/// bundle, the validation refusal for the at-most-one authorization per triple,
/// a validation refusal for the ingestion's authorization dependency.
#[tokio::test]
async fn every_create_replays_its_resource_before_judging_its_state_precondition() {
    for (index, create) in [
        Create::AppBundle,
        Create::AppAuthorization,
        Create::Ingestion,
    ]
    .into_iter()
    .enumerate()
    {
        let case = format!("{create:?}");
        let emulator = Emulator::new();
        let app = "SMARTSHEET";
        let tenant = "tenant-smartsheet";

        // The parent chain each row needs before its own create.
        let bundle = match create {
            Create::AppBundle => None,
            _ => Some(app_bundle(&emulator).await),
        };
        let authorization = match create {
            Create::Ingestion => Some(
                connected_authorization(
                    &emulator,
                    bundle.as_deref().unwrap_or_default(),
                    app,
                    tenant,
                    &case,
                )
                .await,
            ),
            _ => None,
        };

        let spent = token(0x300 + index as u32);
        // (the create's URI, its body, the response member holding the resource,
        //  the member holding that resource's ARN, the refusal a NON-retry gets)
        let (uri, body, resource, arn_member, refusal_status, refusal_code) = match create {
            Create::AppBundle => (
                "/appbundles".to_string(),
                json!({"clientToken": spent}),
                "appBundle",
                "arn",
                402,
                "ServiceQuotaExceededException",
            ),
            Create::AppAuthorization => (
                format!(
                    "/appbundles/{}/appauthorizations",
                    encode(bundle.as_deref().unwrap_or_default())
                ),
                json!({
                    "clientToken": spent,
                    "app": app,
                    "authType": "apiKey",
                    "credential": {"apiKeyCredential": {"apiKey": "placeholder-api-key"}},
                    "tenant": {"tenantIdentifier": tenant, "tenantDisplayName": "display"},
                }),
                "appAuthorization",
                "appAuthorizationArn",
                400,
                "ValidationException",
            ),
            Create::Ingestion => (
                format!(
                    "/appbundles/{}/ingestions",
                    encode(bundle.as_deref().unwrap_or_default())
                ),
                json!({
                    "clientToken": spent,
                    "app": app,
                    "tenantId": tenant,
                    "ingestionType": "auditLog",
                }),
                "ingestion",
                "arn",
                400,
                "ValidationException",
            ),
        };

        let created = emulator.call("POST", &uri, Some(body.clone())).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201, got {} with body {}",
            created.status, created.body
        );
        let arn = member(
            created
                .body
                .get(resource)
                .unwrap_or_else(|| panic!("case {case:?}: expected {resource} in {}", created.body)),
            arn_member,
        );

        // Only the ingestion's precondition needs a further call to stop
        // holding: the other two are already unsatisfiable, because the resource
        // this create minted is what occupies the quota and the triple.
        if let (Create::Ingestion, Some(authorization), Some(bundle)) =
            (create, authorization.as_deref(), bundle.as_deref())
        {
            let updated = emulator
                .call(
                    "PATCH",
                    &format!(
                        "/appbundles/{}/appauthorizations/{}",
                        encode(bundle),
                        encode(authorization)
                    ),
                    Some(Broken::CredentialRotated.update_body(tenant)),
                )
                .await;
            assert_eq!(
                updated.status, 200,
                "case {case:?}: the breaking update must be accepted, got HTTP {} with body {}",
                updated.status, updated.body
            );
        }

        // A fresh token is not a retry, so the precondition still refuses it,
        // with the error this operation models for it.
        let mut fresh = body.clone();
        fresh["clientToken"] = Value::String(token(0x400 + index as u32));
        let refused = emulator.call("POST", &uri, Some(fresh)).await;
        assert_eq!(
            refused.status, refusal_status,
            "case {case:?}: expected HTTP {refusal_status} for a fresh token, got {} with body {}",
            refused.status, refused.body
        );
        assert!(
            refused.error_code().contains(refusal_code),
            "case {case:?}: expected {refusal_code}, got {} with body {}",
            refused.error_code(),
            refused.body
        );

        // The spent token replays its resource, which is still readable back.
        let replay = emulator.call("POST", &uri, Some(body)).await;
        assert_eq!(
            replay.status, 201,
            "case {case:?}: a spent token must replay its resource, got HTTP {} with body {}",
            replay.status, replay.body
        );
        let replayed_arn = member(
            replay
                .body
                .get(resource)
                .unwrap_or_else(|| panic!("case {case:?}: expected {resource} in {}", replay.body)),
            arn_member,
        );
        assert_eq!(
            replayed_arn, arn,
            "case {case:?}: expected the replayed resource {arn:?}, got {}",
            replay.body
        );
        let read_back = emulator
            .call("GET", &format!("{uri}/{}", encode(&arn)), None)
            .await;
        assert_eq!(
            read_back.status, 200,
            "case {case:?}: the replayed resource must be readable, got HTTP {} with body {}",
            read_back.status, read_back.body
        );
        assert_eq!(
            read_back.body.get(resource),
            replay.body.get(resource),
            "case {case:?}: the replay must report the stored record, got {}",
            read_back.body
        );
    }
}
