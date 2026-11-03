// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the OUTCOME of AppFabric's connect handshake and for the
//! refusals that hang off it, driven over the wire (restJson1) through the same
//! router the emulator boots.
//!
//! The RULES these tests state over TABLES of inputs:
//!
//! * the connect handshake's outcome is the application's answer, reported in the
//!   record's own `AppAuthorizationStatus`: an `oauth2` authorization's handshake
//!   has to redeem the caller's authorization code with the application to get an
//!   access token, which no application here ever does, so such an authorization
//!   converges on the modelled `ConnectionValidationFailed` and never on
//!   `Connected`, whatever credential it was created with and whatever code the
//!   connect call carries;
//! * a pair whose app authorization did not complete the handshake — created and
//!   never connected, or connected and failed validation — admits no ingestion,
//!   and the refusal carries the service's fixed sentence for the connect step;
//! * a read or mutation naming an app authorization the named bundle does not
//!   hold is refused with the service's fixed not-found sentence, with the
//!   identifier in the shape's own members rather than in the message;
//! * an app bundle that still holds an app authorization cannot be deleted, and
//!   the refusal is the modelled validation error.
//!
//! The harness (the router, `Answer`, `encode`, `arn_of`, `uuid_of`) is the one
//! `app_bundle_ingestion_lifecycle` builds; the identifier forms are
//! `app_authorization_deletion`'s.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_appfabric_fabricfrontendservice::state::OAUTH2_HANDSHAKE_SETTLES_AFTER;

use crate::app_authorization_deletion::{as_form, delete_authorization, Form};
use crate::app_bundle_ingestion_lifecycle::{arn_of, encode, uuid_of, Answer, Emulator};
use crate::ingestion_authorization_refusal_wording::NOT_CONNECTED_MESSAGE;

/// The sentence AppFabric refuses a read or mutation of an app authorization it
/// does not hold with. Spelled out here, so these tests assert the wording
/// itself rather than whatever constant the handler reaches for. It is whole
/// fixed text: no identifier of the request or of the store belongs in it.
const NOT_FOUND_MESSAGE: &str = "The app authorization was not found.";

/// The sentence AppFabric refuses a `DeleteAppBundle` of a bundle that still
/// holds contents with — fixed text too, for the same reason.
const BUNDLE_NOT_DELETABLE_MESSAGE: &str = "The specified app bundle cannot be deleted.";

/// The not-found sentence of the app bundle itself, which the delete leaves
/// behind once it does succeed.
const BUNDLE_NOT_FOUND_MESSAGE: &str = "The app bundle ID was not found.";

/// Long enough past [`OAUTH2_HANDSHAKE_SETTLES_AFTER`] that the outcome the
/// clock owes a handshake is due however coarse the clock is.
fn past_the_handshake() -> Duration {
    OAUTH2_HANDSHAKE_SETTLES_AFTER + Duration::from_millis(250)
}

/// The two values the model's `AppAuthorizationStatus` enum carries for an
/// authorization whose handshake has run (`smithy_query(query="shape",
/// shape="AppAuthorizationStatus")`): the application accepted the configured
/// credential, or it answered a validation exception.
const CONNECTED: &str = "Connected";
const CONNECTION_VALIDATION_FAILED: &str = "ConnectionValidationFailed";

/// The state a created authorization starts in, before any handshake has run.
const PENDING_CONNECT: &str = "PendingConnect";

/// One row of the handshake table: an authorization to create and connect, and
/// the `authType` that decides what its handshake needs from the application.
struct Handshake {
    case: &'static str,
    /// An application from the set the model's own documentation of `app` lists
    /// (`smithy_query(query="doc", shape="CreateAppAuthorizationRequest",
    /// member="app")`: "SLACK ASANA JIRA M365 M365AUDITLOGS ZOOM ZENDESK OKTA
    /// GOOGLE DROPBOX SMARTSHEET CISCO").
    app: &'static str,
    auth_type: &'static str,
    tenant: &'static str,
    /// The credential the authorization is created with, as the union member the
    /// `authType` requires — the values differ per row, because no credential
    /// value may change the outcome.
    credential: Value,
    /// The `authRequest` the connect call carries as `(code, redirectUri)`, or
    /// `None` for an authorization whose type takes none.
    auth_request: Option<(&'static str, &'static str)>,
    /// Which modelled `Identifier` form the connect call names the records by.
    form: Form,
}

/// The status a connect leaves an authorization of this `authType` reading
/// straight away, and the terminal one its handshake converges on.
fn expected_statuses(auth_type: &str) -> (&'static str, &'static str) {
    if auth_type == "apiKey" {
        // An `apiKey` authorization carries its credential outright: there is no
        // grant to redeem, so its handshake completes with the call.
        (CONNECTED, CONNECTED)
    } else {
        // An `oauth2` one has to redeem its code with the application, which
        // never answers here, so the outcome is the validation failure and it
        // falls due after the call.
        (PENDING_CONNECT, CONNECTION_VALIDATION_FAILED)
    }
}

/// `CreateAppAuthorization` under `bundle`, with the row's own credential.
async fn create_authorization(
    emulator: &Emulator,
    bundle: &str,
    app: &str,
    auth_type: &str,
    tenant: &str,
    credential: &Value,
    case: &str,
) -> String {
    let created = emulator
        .call(
            "POST",
            &format!("/appbundles/{}/appauthorizations", encode(bundle)),
            Some(json!({
                "app": app,
                "authType": auth_type,
                "credential": credential,
                "tenant": {
                    "tenantIdentifier": tenant,
                    "tenantDisplayName": format!("{tenant} display"),
                },
            })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: expected HTTP 201 from CreateAppAuthorization, got {} with body {}",
        created.status, created.body
    );
    assert_eq!(
        status_of(&created.body, "appAuthorization"),
        PENDING_CONNECT,
        "case {case:?}: a created authorization awaits the handshake, got {}",
        created.body
    );
    arn_of_authorization(&created.body, case)
}

/// The `appAuthorizationArn` of a create response.
fn arn_of_authorization(body: &Value, case: &str) -> String {
    body.get("appAuthorization")
        .and_then(|authorization| authorization.get("appAuthorizationArn"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected appAuthorization.appAuthorizationArn in {body}")
        })
        .to_string()
}

/// `ConnectAppAuthorization`, carrying the row's `authRequest` or no body member
/// at all.
async fn connect(
    emulator: &Emulator,
    bundle: &str,
    authorization: &str,
    auth_request: Option<(&str, &str)>,
) -> Answer {
    let body = match auth_request {
        Some((code, redirect_uri)) => {
            json!({"authRequest": {"code": code, "redirectUri": redirect_uri}})
        }
        None => json!({}),
    };
    emulator
        .call(
            "POST",
            &format!(
                "/appbundles/{}/appauthorizations/{}/connect",
                encode(bundle),
                encode(authorization)
            ),
            Some(body),
        )
        .await
}

/// `GetAppAuthorization`.
async fn get_authorization(emulator: &Emulator, bundle: &str, authorization: &str) -> Answer {
    emulator
        .call(
            "GET",
            &format!(
                "/appbundles/{}/appauthorizations/{}",
                encode(bundle),
                encode(authorization)
            ),
            None,
        )
        .await
}

/// `UpdateAppAuthorization` with `body`.
async fn update_authorization(
    emulator: &Emulator,
    bundle: &str,
    authorization: &str,
    body: &Value,
) -> Answer {
    emulator
        .call(
            "PATCH",
            &format!(
                "/appbundles/{}/appauthorizations/{}",
                encode(bundle),
                encode(authorization)
            ),
            Some(body.clone()),
        )
        .await
}

/// The `status` of a named resource in a response body, or `""`.
fn status_of(body: &Value, resource: &str) -> String {
    body.get(resource)
        .and_then(|record| record.get("status"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A timestamp member of a record, as epoch seconds.
fn stamp(record: &Value, name: &str, case: &str) -> f64 {
    record
        .get(name)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: expected the timestamp {name} in {record}"))
}

/// A string member of a JSON object, or `""`.
fn member(value: &Value, name: &str) -> String {
    value
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The status `ListAppAuthorizations` reports for one authorization of a bundle,
/// so the listing is checked to agree with the read.
async fn listed_status(emulator: &Emulator, bundle: &str, arn: &str, case: &str) -> String {
    let answer = emulator
        .call(
            "GET",
            &format!("/appbundles/{}/appauthorizations", encode(bundle)),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: ListAppAuthorizations must answer HTTP 200, got {} with body {}",
        answer.status, answer.body
    );
    answer
        .body
        .get("appAuthorizationSummaryList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected appAuthorizationSummaryList in {}",
                answer.body
            )
        })
        .iter()
        .find(|summary| member(summary, "appAuthorizationArn") == arn)
        .map(|summary| member(summary, "status"))
        .unwrap_or_else(|| panic!("case {case:?}: expected a summary for {arn} in {}", answer.body))
}

/// The handshake table: both `AuthType` values, applications and tenants the
/// issue did not cite, credential values differing in kind, size and shape, and
/// both modelled identifier forms. No credential value, and no authorization
/// code, may change which outcome the handshake reaches.
fn handshakes() -> Vec<Handshake> {
    vec![
        Handshake {
            case: "oauth2 for SLACK, short credential values",
            app: "SLACK",
            auth_type: "oauth2",
            tenant: "tenant-slack",
            credential: json!({"oauth2Credential": {"clientId": "c", "clientSecret": "s"}}),
            auth_request: Some(("code-1", "https://example.com/oauth/callback")),
            form: Form::Arn,
        },
        Handshake {
            case: "oauth2 for ZENDESK, provider-shaped credential values",
            app: "ZENDESK",
            auth_type: "oauth2",
            tenant: "tenant-zendesk",
            credential: json!({"oauth2Credential": {
                "clientId": "1234567890123.1234567890123",
                "clientSecret": "0123456789abcdef0123456789abcdef",
            }}),
            auth_request: Some((
                "1234567890123.1234567890123.abcdef0123456789abcdef0123456789",
                "https://us-east-1.console.aws.amazon.com/appfabric/oauth2",
            )),
            form: Form::Uuid,
        },
        Handshake {
            case: "oauth2 for GOOGLE, long credential values",
            app: "GOOGLE",
            auth_type: "oauth2",
            tenant: "tenant-google",
            credential: json!({"oauth2Credential": {
                "clientId": "i".repeat(2048),
                "clientSecret": "s".repeat(2048),
            }}),
            auth_request: Some(("c".repeat(2048).leak(), "https://example.org/cb?state=1")),
            form: Form::Arn,
        },
        Handshake {
            case: "apiKey for OKTA, short key",
            app: "OKTA",
            auth_type: "apiKey",
            tenant: "tenant-okta",
            credential: json!({"apiKeyCredential": {"apiKey": "k"}}),
            auth_request: None,
            form: Form::Uuid,
        },
        Handshake {
            case: "apiKey for SMARTSHEET, long key",
            app: "SMARTSHEET",
            auth_type: "apiKey",
            tenant: "tenant-smartsheet",
            credential: json!({"apiKeyCredential": {"apiKey": "k".repeat(2048)}}),
            auth_request: None,
            form: Form::Arn,
        },
    ]
}

/// RULE: the connect handshake's outcome is the application's answer, which
/// AppFabric reports in the authorization's own status. An `oauth2`
/// authorization's handshake must redeem the caller's authorization code with the
/// application to obtain an access token; nothing here answers for the
/// application, so that handshake converges on the modelled
/// `ConnectionValidationFailed` — never on `Connected` — whatever credential the
/// authorization carries and whatever code the connect call brings, and a retry
/// of the connect reaches the same outcome. An `apiKey` authorization has no
/// grant to redeem and is connected by the call itself.
///
/// The table is [`handshakes`]: both `AuthType` values, applications and tenants
/// the issue did not cite, credential values differing in kind, size and shape
/// (including the model's 2048-character bound), and both identifier forms.
#[tokio::test]
async fn the_connect_handshake_converges_on_the_outcome_the_application_answers() {
    let emulator = Emulator::new();
    let bundle = emulator.fixture_bundle(0).await;
    let rows = handshakes();
    let mut stored: Vec<(String, Value)> = Vec::new();

    for row in &rows {
        let case = row.case;
        let arn = create_authorization(
            &emulator,
            &bundle,
            row.app,
            row.auth_type,
            row.tenant,
            &row.credential,
            case,
        )
        .await;
        let before = get_authorization(&emulator, &bundle, &arn).await;
        let (at_once, _) = expected_statuses(row.auth_type);

        let connected = connect(
            &emulator,
            &as_form(&bundle, row.form),
            &as_form(&arn, row.form),
            row.auth_request,
        )
        .await;
        assert_eq!(
            connected.status, 200,
            "case {case:?}: expected HTTP 200 from ConnectAppAuthorization, got {} with body {}",
            connected.status, connected.body
        );
        // What the call reports is the record it left behind: the handshake of an
        // `oauth2` authorization is still in flight here, so the record still
        // awaits its outcome.
        assert_eq!(
            status_of(&connected.body, "appAuthorizationSummary"),
            at_once,
            "case {case:?}: expected the summary to report {at_once}, got {}",
            connected.body
        );
        let read = get_authorization(&emulator, &bundle, &arn).await;
        assert_eq!(
            status_of(&read.body, "appAuthorization"),
            at_once,
            "case {case:?}: expected the record to read {at_once} straight after the call, got {}",
            read.body
        );
        stored.push((
            arn,
            before
                .body
                .get("appAuthorization")
                .cloned()
                .unwrap_or(Value::Null),
        ));
    }

    // ONE wait for every row: the outcome each handshake owes is stored, not
    // slept for, so it falls due for all of them at once.
    tokio::time::sleep(past_the_handshake()).await;

    for (row, (arn, created_record)) in rows.iter().zip(stored.iter()) {
        let case = row.case;
        let (_, terminal) = expected_statuses(row.auth_type);
        let read = get_authorization(&emulator, &bundle, arn).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: GetAppAuthorization must answer HTTP 200, got {} with body {}",
            read.status, read.body
        );
        let record = read
            .body
            .get("appAuthorization")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            member(&record, "status"),
            terminal,
            "case {case:?}: expected the handshake to converge on {terminal}, got {record}"
        );
        assert_eq!(
            listed_status(&emulator, &bundle, arn, case).await,
            terminal,
            "case {case:?}: the listing must report the status the read reports ({terminal})"
        );
        // The outcome is a CHANGE to the record, so it dates it: the handshake
        // landed after the create call, and `createdAt` never moves.
        assert!(
            stamp(&record, "updatedAt", case) > stamp(created_record, "updatedAt", case),
            "case {case:?}: the handshake's outcome must date the record, got {record} after \
             {created_record}"
        );
        assert_eq!(
            stamp(&record, "createdAt", case),
            stamp(created_record, "createdAt", case),
            "case {case:?}: createdAt must not move, got {record}"
        );

        // A retry of the connect — the model's own remedy for a failed
        // validation ("try to connect the app authorization again") — reaches
        // the same outcome: a credential the application never validated cannot
        // become one it did.
        let again = connect(
            &emulator,
            &as_form(&bundle, row.form),
            &as_form(arn, row.form),
            row.auth_request,
        )
        .await;
        assert_eq!(
            again.status, 200,
            "case {case:?}: a repeated connect must be served, got {} with body {}",
            again.status, again.body
        );
        assert_eq!(
            status_of(&again.body, "appAuthorizationSummary"),
            terminal,
            "case {case:?}: a repeated connect must reach {terminal} again, got {}",
            again.body
        );
    }
}

/// RULE: an ingestion is created only for an (application, tenant) pair whose app
/// authorization completed the connect handshake, so a pair whose authorization
/// never connected — created and left alone, or connected and failed validation —
/// is refused with the modelled `ValidationException` (HTTP 400) carrying the
/// service's fixed sentence for the connect step, and nothing is written; the
/// pair whose authorization IS connected is admitted.
///
/// The table spans both ways a pair can hold an unconnected authorization, the
/// connected case, and applications and tenants the issue did not cite.
#[tokio::test]
async fn a_pair_whose_handshake_did_not_connect_admits_no_ingestion() {
    // (case, app, tenant, authType, whether the set-up runs the connect call)
    let cases: &[(&str, &str, &str, &str, bool)] = &[
        ("created and never connected", "JIRA", "tenant-jira", "apiKey", false),
        (
            "connected, and its validation failed",
            "SLACK",
            "tenant-slack",
            "oauth2",
            true,
        ),
        (
            "created and never connected, oauth2",
            "DROPBOX",
            "tenant-dropbox",
            "oauth2",
            false,
        ),
        ("connected", "CISCO", "tenant-cisco", "apiKey", true),
    ];

    for (case, app, tenant, auth_type, run_connect) in cases {
        let emulator = Emulator::new();
        let bundle = emulator.fixture_bundle(0).await;
        let credential = if *auth_type == "apiKey" {
            json!({"apiKeyCredential": {"apiKey": "placeholder-api-key"}})
        } else {
            json!({"oauth2Credential": {
                "clientId": "placeholder-client-id",
                "clientSecret": "placeholder-client-secret",
            }})
        };
        let arn = create_authorization(
            &emulator, &bundle, app, auth_type, tenant, &credential, case,
        )
        .await;
        if *run_connect {
            let auth_request = if *auth_type == "apiKey" {
                None
            } else {
                Some(("placeholder-code", "https://example.com/oauth/callback"))
            };
            let connected = connect(&emulator, &bundle, &arn, auth_request).await;
            assert_eq!(
                connected.status, 200,
                "case {case:?}: the set-up connect must be served, got {} with body {}",
                connected.status, connected.body
            );
            // Let whatever outcome the handshake owes fall due, so the ingestion
            // call below sees the state the handshake really reached.
            tokio::time::sleep(past_the_handshake()).await;
        }

        let held = member(
            &get_authorization(&emulator, &bundle, &arn)
                .await
                .body
                .get("appAuthorization")
                .cloned()
                .unwrap_or(Value::Null),
            "status",
        );
        let connected_pair = held == CONNECTED;
        let answer = emulator
            .call(
                "POST",
                &format!("/appbundles/{}/ingestions", encode(&bundle)),
                Some(json!({"app": app, "tenantId": tenant, "ingestionType": "auditLog"})),
            )
            .await;

        if connected_pair {
            assert_eq!(
                answer.status, 201,
                "case {case:?}: a connected pair must admit the ingestion, got {} with body {}",
                answer.status, answer.body
            );
            let ingestion = arn_of(&answer.body, "ingestion");
            let read = emulator.get_ingestion(&bundle, &ingestion).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: the created ingestion must be readable back, got {} with body {}",
                read.status, read.body
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: a pair holding {held} must be refused, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected the modelled ValidationException, got {}",
                answer.body
            );
            assert_eq!(
                member(&answer.body, "message"),
                NOT_CONNECTED_MESSAGE,
                "case {case:?}: expected the service's fixed connect-step sentence, got {}",
                answer.body
            );
            let listed = emulator.list_ingestions(&bundle, "").await;
            assert_eq!(
                listed
                    .body
                    .get("ingestions")
                    .and_then(Value::as_array)
                    .map(Vec::len),
                Some(0),
                "case {case:?}: a refused create must write nothing, got {}",
                listed.body
            );
        }
    }
}

/// How the app authorization an operation names comes to be absent from the
/// bundle it names.
#[derive(Clone, Copy, Debug)]
enum Missing {
    /// A well-formed identifier no create call ever minted.
    NeverExisted,
    /// The identifier of an authorization that was created and then deleted.
    Deleted,
    /// A live authorization of ANOTHER app bundle: it exists, but not where the
    /// call looks for it.
    AnotherBundle,
}

/// RULE: a read or mutation naming an app authorization the named app bundle does
/// not hold is refused HTTP 404 `ResourceNotFoundException` with the service's
/// FIXED sentence; the identifier the caller supplied travels in the shape's own
/// required `resourceId`/`resourceType` members, never in the message, so two
/// refusals of two different identifiers read identically.
///
/// The table spans every operation that resolves one (`GetAppAuthorization`,
/// `UpdateAppAuthorization`, `ConnectAppAuthorization`), every way the record can
/// be absent (never minted, deleted, live under another bundle) and both modelled
/// identifier forms.
#[tokio::test]
async fn a_missing_app_authorization_is_refused_with_the_services_fixed_sentence() {
    let mut messages: Vec<String> = Vec::new();

    for missing in [Missing::NeverExisted, Missing::Deleted, Missing::AnotherBundle] {
        for form in [Form::Arn, Form::Uuid] {
            let emulator = Emulator::new();
            let bundle = emulator.fixture_bundle(0).await;
            let identifier = match missing {
                Missing::NeverExisted => {
                    // A live authorization gives the ARN shape; the identifier
                    // asked for is another UUID in that same shape.
                    let live = create_authorization(
                        &emulator,
                        &bundle,
                        "ASANA",
                        "apiKey",
                        "tenant-live",
                        &json!({"apiKeyCredential": {"apiKey": "placeholder-api-key"}}),
                        "never existed",
                    )
                    .await;
                    live.replace(&uuid_of(&live), "6e8bc430-9c3a-11d9-9669-0800200c9a66")
                }
                Missing::Deleted => {
                    let doomed = create_authorization(
                        &emulator,
                        &bundle,
                        "ZOOM",
                        "oauth2",
                        "tenant-doomed",
                        &json!({"oauth2Credential": {
                            "clientId": "placeholder-client-id",
                            "clientSecret": "placeholder-client-secret",
                        }}),
                        "deleted",
                    )
                    .await;
                    let deleted = delete_authorization(&emulator, &bundle, &doomed).await;
                    assert_eq!(
                        deleted.status, 204,
                        "the set-up delete must answer HTTP 204, got {} with body {}",
                        deleted.status, deleted.body
                    );
                    doomed
                }
                Missing::AnotherBundle => {
                    // A second bundle, in its own Region: an account holds one
                    // app bundle per Region.
                    let elsewhere = emulator.fixture_bundle(1).await;
                    create_authorization(
                        &emulator,
                        &elsewhere,
                        "M365",
                        "apiKey",
                        "tenant-elsewhere",
                        &json!({"apiKeyCredential": {"apiKey": "placeholder-api-key"}}),
                        "another bundle",
                    )
                    .await
                }
            };
            let named = as_form(&identifier, form);

            let refusals = [
                (
                    "GetAppAuthorization",
                    get_authorization(&emulator, &bundle, &named).await,
                ),
                (
                    "UpdateAppAuthorization",
                    update_authorization(
                        &emulator,
                        &bundle,
                        &named,
                        &json!({"tenant": {
                            "tenantIdentifier": "tenant-next",
                            "tenantDisplayName": "next display",
                        }}),
                    )
                    .await,
                ),
                (
                    "ConnectAppAuthorization",
                    connect(
                        &emulator,
                        &bundle,
                        &named,
                        Some(("placeholder-code", "https://example.com/oauth/callback")),
                    )
                    .await,
                ),
            ];

            for (operation, answer) in refusals {
                let case = format!("{operation} on an authorization {missing:?}, named by {form:?}");
                assert_eq!(
                    answer.status, 404,
                    "case {case:?}: expected HTTP 404, got {} with body {}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    "ResourceNotFoundException",
                    "case {case:?}: expected ResourceNotFoundException, got {}",
                    answer.body
                );
                let message = member(&answer.body, "message");
                assert_eq!(
                    message, NOT_FOUND_MESSAGE,
                    "case {case:?}: expected the service's fixed sentence, got {message:?}"
                );
                assert!(
                    !message.contains(&named) && !message.contains(&uuid_of(&identifier)),
                    "case {case:?}: the identifier belongs in resourceId, not in {message:?}"
                );
                assert_eq!(
                    member(&answer.body, "resourceType"),
                    "AppAuthorization",
                    "case {case:?}: the refusal names the kind of resource, got {}",
                    answer.body
                );
                assert_eq!(
                    member(&answer.body, "resourceId"),
                    named,
                    "case {case:?}: the refusal carries the identifier as supplied, got {}",
                    answer.body
                );
                messages.push(message);
            }
        }
    }

    // Every refusal of the class reads identically, whichever identifier and
    // whichever operation produced it.
    let first = messages
        .first()
        .cloned()
        .expect("the table refuses at least one call");
    for message in &messages {
        assert_eq!(
            *message, first,
            "every app-authorization not-found refusal must read identically, got {message:?} \
             beside {first:?}"
        );
    }
}

/// RULE: an app bundle that still holds an app authorization cannot be deleted:
/// the call is refused HTTP 400 `ValidationException` with the service's fixed
/// sentence, `reason` naming no member of the request (the bundle's STATE forbids
/// the call) and nothing is written — the bundle and every authorization under it
/// survive. Once the contents are gone the same call succeeds (204) and the
/// bundle reads back as not found.
///
/// The table spans how many authorizations the bundle holds and both modelled
/// identifier forms, because neither may change the answer.
#[tokio::test]
async fn a_bundle_that_still_holds_an_app_authorization_cannot_be_deleted() {
    // (case, how many authorizations the bundle holds, identifier form)
    let cases: &[(&str, usize, Form)] = &[
        ("one authorization, by arn", 1, Form::Arn),
        ("two authorizations, by uuid", 2, Form::Uuid),
        ("three authorizations, by arn", 3, Form::Arn),
    ];
    const APPS: &[&str] = &["SLACK", "ZENDESK", "OKTA"];

    for (case, total, form) in cases {
        let emulator = Emulator::new();
        let bundle = emulator.fixture_bundle(0).await;
        let mut held = Vec::with_capacity(*total);
        for index in 0..*total {
            held.push(
                create_authorization(
                    &emulator,
                    &bundle,
                    APPS[index % APPS.len()],
                    "apiKey",
                    &format!("tenant-{index}"),
                    &json!({"apiKeyCredential": {"apiKey": "placeholder-api-key"}}),
                    case,
                )
                .await,
            );
        }
        let identifier = as_form(&bundle, *form);

        // One refusal per authorization still held: the precondition is about
        // the bundle holding NONE of them.
        for doomed in &held {
            let refused = emulator.delete_app_bundle(&identifier).await;
            assert_eq!(
                refused.status, 400,
                "case {case:?}: expected HTTP 400 while an authorization remains, got {} with \
                 body {}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {}",
                refused.body
            );
            assert_eq!(
                member(&refused.body, "message"),
                BUNDLE_NOT_DELETABLE_MESSAGE,
                "case {case:?}: expected the service's fixed sentence, got {}",
                refused.body
            );
            assert_eq!(
                member(&refused.body, "reason"),
                "other",
                "case {case:?}: the bundle's state, not a member, forbids the call, got {}",
                refused.body
            );
            assert!(
                refused.body.get("fieldList").is_none(),
                "case {case:?}: no request member is at fault, got {}",
                refused.body
            );

            // The refusal wrote nothing: the bundle and the authorization it
            // still holds are both readable.
            let read = emulator.get_app_bundle(&identifier).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: the refused bundle must survive, got {} with body {}",
                read.status, read.body
            );
            let read = get_authorization(&emulator, &bundle, doomed).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: the held authorization must survive, got {} with body {}",
                read.status, read.body
            );

            let deleted = delete_authorization(&emulator, &bundle, doomed).await;
            assert_eq!(
                deleted.status, 204,
                "case {case:?}: expected HTTP 204 from DeleteAppAuthorization, got {} with body {}",
                deleted.status, deleted.body
            );
        }

        let deleted = emulator.delete_app_bundle(&identifier).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: expected HTTP 204 once no authorization remains, got {} with body {}",
            deleted.status, deleted.body
        );
        let read = emulator.get_app_bundle(&identifier).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: the deleted bundle must be gone, got {} with body {}",
            read.status, read.body
        );
        assert_eq!(
            member(&read.body, "message"),
            BUNDLE_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the app bundle's own not-found sentence, got {}",
            read.body
        );
    }
}
