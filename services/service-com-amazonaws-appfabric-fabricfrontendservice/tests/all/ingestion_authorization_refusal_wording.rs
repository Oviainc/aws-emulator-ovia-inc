// SPDX-License-Identifier: CC0-1.0

//! The WORDING `CreateIngestion` refuses an unsatisfied app-authorization
//! dependency with, driven over the wire (restJson1) through the same router the
//! emulator boots.
//!
//! RULE: the refusal of an ingestion whose (application, tenant) pair has no
//! connected app authorization is HTTP 400 `ValidationException` carrying the
//! service's FIXED sentence for the precondition the caller still owes — the
//! not-found sentence while the bundle holds no authorization for the pair, the
//! not-connected sentence once it holds one that has not completed the connect
//! handshake — and neither sentence has a variable part: the application, the
//! tenant, the app bundle identifier and the status the store holds for the pair
//! appear nowhere in it, so two refusals of two different pairs read
//! identically.
//!
//! The table spans the class rather than one recorded call: applications from
//! the model's own `app` list (of which the issue's own application is only one
//! row), tenant identifiers of several forms and
//! sizes (a word, a uuid, a host name, and one near the modelled 1024-character
//! bound), crossed with every way a pair can lack a connected authorization —
//! nothing authorized at all, a connected authorization for another tenant of
//! the same application, one created and never connected, and one connected and
//! then re-pended by an applied update.
//!
//! The harness (the router, `Answer`, `encode`, `arn_of`) is the one
//! `app_bundle_ingestion_lifecycle` already builds.

use serde_json::{json, Value};
use service_com_amazonaws_appfabric_fabricfrontendservice::state::{
    APP_AUTHORIZATION_STATUS_CONNECTED, APP_AUTHORIZATION_STATUS_PENDING_CONNECT,
};

use crate::app_bundle_ingestion_lifecycle::{arn_of, encode, uuid_of, Answer, Emulator};

/// The sentence the service answers for a pair its app bundle holds NO app
/// authorization for. Spelled out here, so these tests assert the wording itself
/// rather than whatever constant the handler happens to reach for.
pub(crate) const NO_AUTHORIZATION_MESSAGE: &str =
    "The app authorization for the specified application and tenant was not found. \
     Create an app authorization for the application and tenant before creating an ingestion.";

/// The sibling sentence for a pair whose app authorization exists but has not
/// completed the connect handshake — the same value-free form with the connect
/// step as its remediation.
pub(crate) const NOT_CONNECTED_MESSAGE: &str =
    "The app authorization for the specified application and tenant was not connected. \
     Connect the app authorization for the application and tenant before creating an ingestion.";

/// How a bundle's app authorizations were set up before the ingestion call —
/// every way a pair can lack a connected one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Setup {
    /// The bundle holds no app authorization at all.
    Nothing,
    /// The bundle holds a CONNECTED authorization for the same application and
    /// another tenant, so it holds authorizations but none for this pair.
    ConnectedForAnotherTenant,
    /// `CreateAppAuthorization` for the pair, with no connect call after it.
    Created,
    /// Created and connected, then re-pended by an update that applies a change.
    ConnectedThenUpdated,
}

impl Setup {
    /// The sentence the rule requires for this set-up: the remediation the
    /// caller still owes is the create step while the pair holds nothing, and
    /// the connect step once it holds something.
    fn expected_message(self) -> &'static str {
        match self {
            Self::Nothing | Self::ConnectedForAnotherTenant => NO_AUTHORIZATION_MESSAGE,
            Self::Created | Self::ConnectedThenUpdated => NOT_CONNECTED_MESSAGE,
        }
    }

    /// The status the store holds for the pair after this set-up, or `None` when
    /// it holds no authorization for it. Asserted absent from the message.
    fn held_status(self) -> Option<&'static str> {
        match self {
            Self::Nothing | Self::ConnectedForAnotherTenant => None,
            Self::Created | Self::ConnectedThenUpdated => {
                Some(APP_AUTHORIZATION_STATUS_PENDING_CONNECT)
            }
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

/// One app bundle, as its ARN. A Region holds at most one, so each case builds
/// its own emulator around one bundle.
async fn app_bundle(emulator: &Emulator) -> String {
    let created = emulator.create_app_bundle(json!({})).await;
    assert_eq!(
        created.status, 201,
        "the set-up app bundle must be created, got HTTP {} with body {}",
        created.status, created.body
    );
    arn_of(&created.body, "appBundle")
}

/// `CreateAppAuthorization` for one (application, tenant) pair, as the created
/// authorization's ARN.
async fn create_authorization(
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
                "tenant": {"tenantIdentifier": tenant, "tenantDisplayName": "a tenant"},
            })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: the set-up app authorization must be created, got HTTP {} with body {}",
        created.status, created.body
    );
    created
        .body
        .get("appAuthorization")
        .and_then(|authorization| authorization.get("appAuthorizationArn"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected an appAuthorizationArn in {}", created.body))
        .to_string()
}

/// `ConnectAppAuthorization` for a stored authorization.
async fn connect(emulator: &Emulator, bundle: &str, authorization: &str, case: &str) {
    let connected = emulator
        .call(
            "POST",
            &format!(
                "/appbundles/{}/appauthorizations/{}/connect",
                encode(bundle),
                encode(authorization)
            ),
            Some(json!({})),
        )
        .await;
    assert_eq!(
        connected.status, 200,
        "case {case:?}: the set-up app authorization must connect, got HTTP {} with body {}",
        connected.status, connected.body
    );
}

/// `UpdateAppAuthorization` supplying a replacement credential, which applies a
/// change and so invalidates the handshake.
async fn re_pend(emulator: &Emulator, bundle: &str, authorization: &str, case: &str) {
    let updated = emulator
        .call(
            "PATCH",
            &format!(
                "/appbundles/{}/appauthorizations/{}",
                encode(bundle),
                encode(authorization)
            ),
            Some(json!({"credential": {"apiKeyCredential": {"apiKey": "replacement-api-key"}}})),
        )
        .await;
    assert_eq!(
        updated.status, 200,
        "case {case:?}: the set-up update must be accepted, got HTTP {} with body {}",
        updated.status, updated.body
    );
}

/// Drive `setup` for the pair (`app`, `tenant`) under `bundle`.
async fn set_up(
    emulator: &Emulator,
    bundle: &str,
    app: &str,
    tenant: &str,
    setup: Setup,
    case: &str,
) {
    match setup {
        Setup::Nothing => {}
        Setup::ConnectedForAnotherTenant => {
            let other = format!("{tenant}-another");
            let authorization = create_authorization(emulator, bundle, app, &other, case).await;
            connect(emulator, bundle, &authorization, case).await;
        }
        Setup::Created => {
            create_authorization(emulator, bundle, app, tenant, case).await;
        }
        Setup::ConnectedThenUpdated => {
            let authorization = create_authorization(emulator, bundle, app, tenant, case).await;
            connect(emulator, bundle, &authorization, case).await;
            re_pend(emulator, bundle, &authorization, case).await;
        }
    }
}

/// `CreateIngestion` with nothing in front of it, which is what the dependency
/// refusal is about.
async fn create_ingestion(emulator: &Emulator, bundle: &str, app: &str, tenant: &str) -> Answer {
    emulator
        .call(
            "POST",
            &format!("/appbundles/{}/ingestions", encode(bundle)),
            Some(json!({"app": app, "tenantId": tenant, "ingestionType": "auditLog"})),
        )
        .await
}

/// The ingestion ARNs `ListIngestions` reports for a bundle.
async fn listed_ingestions(emulator: &Emulator, bundle: &str) -> Vec<String> {
    let listed = emulator
        .call(
            "GET",
            &format!("/appbundles/{}/ingestions", encode(bundle)),
            None,
        )
        .await;
    listed
        .body
        .get("ingestions")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| member(item, "arn"))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

/// A tenant identifier near the modelled bound: `TenantIdentifier` is
/// `len 1..1024` (`smithy_query(query="shape",shape="TenantIdentifier")`).
fn long_tenant() -> String {
    format!("tenant-{}", "x".repeat(1_000))
}

/// RULE: every refusal of an ingestion whose (application, tenant) pair has no
/// connected app authorization answers HTTP 400 `ValidationException` with the
/// service's fixed sentence for the step the caller still owes, and that sentence
/// carries no value of the request or of the store: not the application, not the
/// tenant, not the app bundle identifier, not the held status.
#[tokio::test]
async fn the_ingestion_dependency_refusal_carries_the_services_fixed_sentence() {
    // (case, application, tenant identifier, set-up)
    let long = long_tenant();
    let cases: Vec<(&str, &str, &str, Setup)> = vec![
        ("nothing authorized", "ASANA", "tenant-asana", Setup::Nothing),
        (
            "nothing authorized, uuid-shaped tenant",
            "CISCO",
            "11111111-2222-4333-8444-555555555555",
            Setup::Nothing,
        ),
        (
            "nothing authorized, host-name tenant",
            "GOOGLE",
            "fabric.example.com",
            Setup::Nothing,
        ),
        (
            "nothing authorized, tenant near the modelled bound",
            "JIRA",
            long.as_str(),
            Setup::Nothing,
        ),
        (
            "another tenant of the same application is connected",
            "ZENDESK",
            "tenant-zendesk",
            Setup::ConnectedForAnotherTenant,
        ),
        (
            "created and never connected",
            "M365AUDITLOGS",
            "tenant-m365auditlogs",
            Setup::Created,
        ),
        (
            "created and never connected, uuid-shaped tenant",
            "SMARTSHEET",
            "66666666-7777-4888-8999-aaaaaaaaaaaa",
            Setup::Created,
        ),
        (
            "connected, then re-pended by an applied update",
            "DROPBOX",
            "tenant-dropbox",
            Setup::ConnectedThenUpdated,
        ),
        (
            // The application the recorded run carried is ONE row of this table.
            "an application the model's own `app` list names, nothing authorized",
            "SLACK",
            "tenant-first",
            Setup::Nothing,
        ),
    ];

    for (case, app, tenant, setup) in cases {
        let emulator = Emulator::new();
        let bundle = app_bundle(&emulator).await;
        set_up(&emulator, &bundle, app, tenant, setup, case).await;

        let refused = create_ingestion(&emulator, &bundle, app, tenant).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {}",
            refused.body
        );
        assert_eq!(
            refused.body.get("reason").and_then(Value::as_str),
            Some("other"),
            "case {case:?}: the store, not a request member, forbids the call, got {}",
            refused.body
        );
        assert!(
            refused.body.get("fieldList").is_none(),
            "case {case:?}: no request member is at fault, got {}",
            refused.body
        );

        let message = member(&refused.body, "message");
        assert_eq!(
            message,
            setup.expected_message(),
            "case {case:?}: expected the service's fixed sentence, got {message:?}"
        );
        for (what, value) in [
            ("the application", app.to_string()),
            ("the tenant", tenant.to_string()),
            ("the app bundle ARN", bundle.clone()),
            ("the app bundle uuid", uuid_of(&bundle)),
        ] {
            assert!(
                !message.contains(&value),
                "case {case:?}: the sentence must not carry {what} ({value:?}), got {message:?}"
            );
        }
        for status in [
            APP_AUTHORIZATION_STATUS_PENDING_CONNECT,
            APP_AUTHORIZATION_STATUS_CONNECTED,
        ] {
            assert!(
                !message.contains(status),
                "case {case:?}: the sentence must not carry the stored status {status:?} \
                 (the pair holds {:?}), got {message:?}",
                setup.held_status()
            );
        }

        assert!(
            listed_ingestions(&emulator, &bundle).await.is_empty(),
            "case {case:?}: a refused create must write no ingestion"
        );
    }
}

/// RULE, stated the other way round: the sentence is a property of the
/// PRECONDITION, not of the pair — so two refusals for two different
/// applications and tenants in the same state are byte-identical, the two states
/// are worded differently from each other, and satisfying the precondition
/// replaces the refusal with the created ingestion (readable back through
/// `GetIngestion`).
#[tokio::test]
async fn the_sentence_depends_only_on_which_step_is_owed() {
    // (application, tenant) pairs that differ in every way the rule generalises
    // over, each driven through both unsatisfied states.
    let pairs = [
        ("OKTA", "tenant-okta"),
        ("ZOOM", "99999999-8888-4777-8666-555555555555"),
        ("M365", "fabric.example.org"),
    ];

    let mut no_authorization = Vec::new();
    let mut not_connected = Vec::new();
    for (app, tenant) in pairs {
        let case = format!("{app}/{tenant}");

        // Nothing authorized: the create step is owed.
        let emulator = Emulator::new();
        let bundle = app_bundle(&emulator).await;
        let refused = create_ingestion(&emulator, &bundle, app, tenant).await;
        no_authorization.push(member(&refused.body, "message"));

        // Authorized and not connected: the connect step is owed.
        let authorization = create_authorization(&emulator, &bundle, app, tenant, &case).await;
        let refused = create_ingestion(&emulator, &bundle, app, tenant).await;
        not_connected.push(member(&refused.body, "message"));

        // The precondition satisfied: the same request is served.
        connect(&emulator, &bundle, &authorization, &case).await;
        let created = create_ingestion(&emulator, &bundle, app, tenant).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: a connected authorization must admit the ingestion, got HTTP {} \
             with body {}",
            created.status, created.body
        );
        let arn = arn_of(&created.body, "ingestion");
        let read_back = emulator.get_ingestion(&bundle, &arn).await;
        assert_eq!(
            read_back.status, 200,
            "case {case:?}: the created ingestion must be readable back, got HTTP {} with body {}",
            read_back.status, read_back.body
        );
        assert_eq!(
            arn_of(&read_back.body, "ingestion"),
            arn,
            "case {case:?}: GetIngestion must report the ingestion the create wrote, got {}",
            read_back.body
        );
    }

    for (index, (app, tenant)) in pairs.iter().enumerate() {
        let case = format!("{app}/{tenant}");
        assert_eq!(
            no_authorization[index], NO_AUTHORIZATION_MESSAGE,
            "case {case:?}: expected the not-found sentence, got {:?}",
            no_authorization[index]
        );
        assert_eq!(
            not_connected[index], NOT_CONNECTED_MESSAGE,
            "case {case:?}: expected the not-connected sentence, got {:?}",
            not_connected[index]
        );
        assert_eq!(
            no_authorization[index], no_authorization[0],
            "case {case:?}: every pair with no authorization must read identically, got {:?} \
             beside {:?}",
            no_authorization[index], no_authorization[0]
        );
        assert_eq!(
            not_connected[index], not_connected[0],
            "case {case:?}: every pair with an unconnected authorization must read identically, \
             got {:?} beside {:?}",
            not_connected[index], not_connected[0]
        );
    }
    assert_ne!(
        NO_AUTHORIZATION_MESSAGE, NOT_CONNECTED_MESSAGE,
        "the two owed steps must be worded differently, got {NO_AUTHORIZATION_MESSAGE:?} twice"
    );
}
