// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ONE rule both of AppFabric's authorization-dependent
//! operations decide by, driven over the wire (restJson1) through the same
//! router the emulator boots.
//!
//! RULE: an app authorization admits dependent work only while it holds the
//! status the model documents as ready to be used (`Connected`); in any other
//! state — none created yet, created and not yet connected, or connected and
//! then invalidated by an applied update — the dependent work is refused.
//! `CreateIngestion` (the dependency check) and `StartUserAccessTasks` (the
//! per-authorization fan-out) are the two operations that ask that question, and
//! both now ask it of `state::AppAuthorizationRecord::is_connected`, so these
//! tests drive the shared path through one operation of each.
//!
//! The table spans every way an authorization can be in each state, over
//! applications and tenants the issue did not cite, and asserts both operations
//! against the SAME set-up bundle so the two can never disagree about whether a
//! pair is usable.
//!
//! The harness (the router, `Answer`, `encode`, `arn_of`, `is_v4_uuid`) is the
//! one `app_bundle_ingestion_lifecycle` already builds.

use serde_json::{json, Value};
use service_com_amazonaws_appfabric_fabricfrontendservice::state::{
    APP_AUTHORIZATION_STATUS_CONNECTED, APP_AUTHORIZATION_STATUS_PENDING_CONNECT,
};

use crate::app_bundle_ingestion_lifecycle::{arn_of, encode, is_v4_uuid, Answer, Emulator};
use crate::ingestion_authorization_refusal_wording::{
    NOT_CONNECTED_MESSAGE, NO_AUTHORIZATION_MESSAGE,
};
use crate::user_access_tasks::{NO_AUTHENTICATION_TOKEN_CODE, NO_AUTHENTICATION_TOKEN_MESSAGE};

/// The email address every start call below searches for.
const EMAIL: &str = "grace.hopper@example.com";

/// How an app authorization for the ingestion's (app, tenant) pair was set up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Setup {
    /// The bundle holds no authorization for the pair at all.
    None,
    /// `CreateAppAuthorization` only, with no connect call after it.
    Created,
    /// `CreateAppAuthorization` then `ConnectAppAuthorization`.
    Connected,
    /// Connected, then an `UpdateAppAuthorization` that applies a change, which
    /// invalidates the handshake.
    ConnectedThenUpdated,
    /// Connected, then an `UpdateAppAuthorization` that applies NOTHING, which
    /// leaves the handshake standing.
    ConnectedThenEmptyUpdate,
}

impl Setup {
    /// The status the bundle holds for the pair after this set-up, or `None`
    /// when it holds no authorization for it. Only the two states the model's
    /// `AppAuthorizationStatus` reaches without an application talking back.
    fn expected_status(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Created | Self::ConnectedThenUpdated => {
                Some(APP_AUTHORIZATION_STATUS_PENDING_CONNECT)
            }
            Self::Connected | Self::ConnectedThenEmptyUpdate => {
                Some(APP_AUTHORIZATION_STATUS_CONNECTED)
            }
        }
    }

    /// Whether dependent work is admitted for a pair set up this way.
    fn admits_dependent_work(self) -> bool {
        self.expected_status() == Some(APP_AUTHORIZATION_STATUS_CONNECTED)
    }
}

/// An app bundle to hang the set-up authorization off, as its ARN.
async fn app_bundle(emulator: &Emulator) -> String {
    let created = emulator.create_app_bundle(json!({})).await;
    assert_eq!(
        created.status, 201,
        "the set-up app bundle must be created, got HTTP {} with body {}",
        created.status, created.body
    );
    arn_of(&created.body, "appBundle")
}

/// Drive `setup` through the operations that reach each state, and answer the
/// status `GetAppAuthorization` reports afterwards (`None` when nothing was
/// created), so the table's own expectation is checked against the store.
async fn set_up_authorization(
    emulator: &Emulator,
    bundle: &str,
    app: &str,
    tenant: &str,
    setup: Setup,
    case: &str,
) -> Option<String> {
    if setup == Setup::None {
        return None;
    }

    let created = emulator
        .call(
            "POST",
            &format!("/appbundles/{}/appauthorizations", encode(bundle)),
            Some(json!({
                "app": app,
                "authType": "apiKey",
                "credential": {"apiKeyCredential": {"apiKey": "placeholder-api-key"}},
                "tenant": {"tenantIdentifier": tenant, "tenantDisplayName": format!("{tenant} (display)")},
            })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: the set-up app authorization must be created, got HTTP {} with body {}",
        created.status, created.body
    );
    let authorization = created
        .body
        .get("appAuthorization")
        .and_then(|authorization| authorization.get("appAuthorizationArn"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected appAuthorization.appAuthorizationArn in {}",
                created.body
            )
        })
        .to_string();

    if setup != Setup::Created {
        let connect = emulator
            .call(
                "POST",
                &format!(
                    "/appbundles/{}/appauthorizations/{}/connect",
                    encode(bundle),
                    encode(&authorization)
                ),
                Some(json!({})),
            )
            .await;
        assert_eq!(
            connect.status, 200,
            "case {case:?}: the set-up app authorization must connect, got HTTP {} with body {}",
            connect.status, connect.body
        );
    }

    if matches!(
        setup,
        Setup::ConnectedThenUpdated | Setup::ConnectedThenEmptyUpdate
    ) {
        // An update that supplies a credential applies a change; one that
        // supplies nothing applies none.
        let body = if setup == Setup::ConnectedThenUpdated {
            json!({"credential": {"apiKeyCredential": {"apiKey": "replacement-api-key"}}})
        } else {
            json!({})
        };
        let updated = emulator
            .call(
                "PATCH",
                &format!(
                    "/appbundles/{}/appauthorizations/{}",
                    encode(bundle),
                    encode(&authorization)
                ),
                Some(body),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: the set-up update must be accepted, got HTTP {} with body {}",
            updated.status, updated.body
        );
    }

    let read_back = emulator
        .call(
            "GET",
            &format!(
                "/appbundles/{}/appauthorizations/{}",
                encode(bundle),
                encode(&authorization)
            ),
            None,
        )
        .await;
    assert_eq!(
        read_back.status, 200,
        "case {case:?}: the set-up app authorization must be readable, got HTTP {} with body {}",
        read_back.status, read_back.body
    );
    Some(
        read_back
            .body
            .get("appAuthorization")
            .and_then(|authorization| authorization.get("status"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    )
}

/// `CreateIngestion` with no authorize-first helper in front of it, which is
/// what the dependency rule is about.
async fn create_ingestion(emulator: &Emulator, bundle: &str, app: &str, tenant: &str) -> Answer {
    emulator
        .call(
            "POST",
            &format!("/appbundles/{}/ingestions", encode(bundle)),
            Some(json!({"app": app, "tenantId": tenant, "ingestionType": "auditLog"})),
        )
        .await
}

/// `StartUserAccessTasks` over the whole bundle.
async fn start_tasks(emulator: &Emulator, bundle: &str) -> Answer {
    emulator
        .call(
            "POST",
            "/useraccess/start",
            Some(json!({"appBundleIdentifier": bundle, "email": EMAIL})),
        )
        .await
}

/// A string member of a JSON object, or `""`.
fn member(value: &Value, name: &str) -> String {
    value
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// RULE: both authorization-dependent operations decide by the SAME state — an
/// app authorization admits dependent work only while its status is the one the
/// model documents as ready to be used. For every pair whose authorization is in
/// that state, `CreateIngestion` creates an ingestion (readable back through
/// `GetIngestion`) and `StartUserAccessTasks` mints a task for it; for every
/// other state — including none at all — `CreateIngestion` is refused with the
/// modelled `ValidationException` (HTTP 400) carrying the service's fixed
/// sentence for the step the caller still owes, and the start call reports that
/// authorization with the modelled `error` — the service's own fixed no-token
/// error — and no `taskId`.
///
/// The table spans every set-up that reaches each state (never created, created
/// and not connected, connected, connected then invalidated by an applied
/// update, connected then left standing by an update that applies nothing), over
/// applications and tenants the issue did not cite.
#[tokio::test]
async fn one_connect_handshake_rule_decides_both_dependent_operations() {
    // (case, app, tenant, set-up)
    let cases: Vec<(&str, &str, &str, Setup)> = vec![
        ("no authorization for the pair", "ZOOM", "tenant-zoom", Setup::None),
        (
            "authorization created, not connected",
            "OKTA",
            "tenant-okta",
            Setup::Created,
        ),
        (
            "authorization connected",
            "SMARTSHEET",
            "tenant-smartsheet",
            Setup::Connected,
        ),
        (
            "connected, then an applied update re-pends it",
            "DROPBOX",
            "tenant-dropbox",
            Setup::ConnectedThenUpdated,
        ),
        (
            "connected, then an update that applies nothing",
            "M365AUDITLOGS",
            "tenant-m365",
            Setup::ConnectedThenEmptyUpdate,
        ),
    ];

    for (case, app, tenant, setup) in cases {
        // One bundle per case: a Region holds at most one app bundle.
        let emulator = Emulator::new();
        let bundle = app_bundle(&emulator).await;
        let status = set_up_authorization(&emulator, &bundle, app, tenant, setup, case).await;
        assert_eq!(
            status.as_deref(),
            setup.expected_status(),
            "case {case:?}: the set-up must leave the authorization in the expected state, got {status:?}"
        );

        // The first operation that asks the question: the ingestion dependency.
        let ingestion = create_ingestion(&emulator, &bundle, app, tenant).await;
        if setup.admits_dependent_work() {
            assert_eq!(
                ingestion.status, 201,
                "case {case:?}: a connected authorization must admit the ingestion, got HTTP {} with body {}",
                ingestion.status, ingestion.body
            );
            let arn = arn_of(&ingestion.body, "ingestion");
            let read_back = emulator.get_ingestion(&bundle, &arn).await;
            assert_eq!(
                read_back.status, 200,
                "case {case:?}: the created ingestion must be readable back, got HTTP {} with body {}",
                read_back.status, read_back.body
            );
            assert_eq!(
                arn_of(&read_back.body, "ingestion"),
                arn,
                "case {case:?}: GetIngestion must report the ingestion the create call wrote, got {}",
                read_back.body
            );
        } else {
            assert_eq!(
                ingestion.status, 400,
                "case {case:?}: a pair with no connected authorization must be refused, got HTTP {} with body {}",
                ingestion.status, ingestion.body
            );
            assert!(
                ingestion.error_code().contains("ValidationException"),
                "case {case:?}: the refusal must be the modelled ValidationException, got {} with body {}",
                ingestion.error_code(),
                ingestion.body
            );
            let message = member(&ingestion.body, "message");
            // The refusal carries the service's FIXED sentence for the step the
            // caller still owes — the create step while the pair holds no
            // authorization, the connect step once it holds one — and no value
            // of the request or of the store. See
            // `ingestion_authorization_refusal_wording` for the whole class.
            let expected = match setup.expected_status() {
                Some(_) => NOT_CONNECTED_MESSAGE,
                None => NO_AUTHORIZATION_MESSAGE,
            };
            assert_eq!(
                message, expected,
                "case {case:?}: expected the service's fixed sentence, got {message:?}"
            );
            assert!(
                !message.contains(app) && !message.contains(tenant),
                "case {case:?}: the sentence must carry neither the app nor the tenant, got {message:?}"
            );
        }

        // The second operation that asks it: the user-access fan-out over the
        // same bundle's authorizations.
        let started = start_tasks(&emulator, &bundle).await;
        assert_eq!(
            started.status, 201,
            "case {case:?}: StartUserAccessTasks must answer HTTP 201, got {} with body {}",
            started.status, started.body
        );
        let items = started
            .body
            .get("userAccessTasksList")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected userAccessTasksList in {}", started.body));
        let expected_items = usize::from(setup != Setup::None);
        assert_eq!(
            items.len(),
            expected_items,
            "case {case:?}: expected {expected_items} item(s), one per app authorization, got {items:?}"
        );
        for item in &items {
            assert_eq!(
                member(item, "app"),
                app,
                "case {case:?}: the reported item must name the authorization's app, got {item}"
            );
            if setup.admits_dependent_work() {
                assert!(
                    is_v4_uuid(&member(item, "taskId")),
                    "case {case:?}: a connected authorization must get a UUID-shaped taskId, got {item}"
                );
                assert!(
                    item.get("error").is_none(),
                    "case {case:?}: a started task must carry no error, got {item}"
                );
            } else {
                assert!(
                    item.get("taskId").is_none(),
                    "case {case:?}: an authorization that is not connected must start no task, got {item}"
                );
                let error = item
                    .get("error")
                    .unwrap_or_else(|| panic!("case {case:?}: expected an error in {item}"));
                assert_eq!(
                    (
                        member(error, "errorCode"),
                        member(error, "errorMessage")
                    ),
                    (
                        NO_AUTHENTICATION_TOKEN_CODE.to_string(),
                        NO_AUTHENTICATION_TOKEN_MESSAGE.to_string()
                    ),
                    "case {case:?}: the reported item must carry the service's fixed no-token error, got {item}"
                );
                let error_message = member(error, "errorMessage");
                assert!(
                    !error_message.contains(app)
                        && !error_message.contains(tenant)
                        && !error_message.contains(APP_AUTHORIZATION_STATUS_PENDING_CONNECT),
                    "case {case:?}: the sentence must carry no value of the request or the store, got {error_message:?}"
                );
            }
        }
    }
}
