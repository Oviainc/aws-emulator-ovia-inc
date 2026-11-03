// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteAppAuthorization`, driven over the wire
//! (restJson1) through the same router the emulator boots.
//!
//! Each test states a RULE over a TABLE of inputs rather than one recorded call:
//!
//! * a delete removes exactly the app authorization its identifier names from
//!   the bundle that owns it, in either identifier form, and repeating the call
//!   changes nothing further (`@idempotent`);
//! * a delete of a target the named bundle does not hold is a no-op success;
//! * an app bundle that does not exist is the modelled `ResourceNotFoundException`;
//! * the one precondition the operation documents — the associated ingestion must
//!   be deleted first — is refused with the modelled `ValidationException`, and
//!   the authorization survives that refusal;
//! * an ingestion of a DIFFERENT (application, tenant) pair is not the associated
//!   ingestion and does not block the delete.
//!
//! The harness (the router, `Answer`, `encode`, `uuid_of`, the app-bundle and
//! ingestion calls) is the one `app_bundle_ingestion_lifecycle` already builds;
//! only the app-authorization calls and the delete tables are new here.

use serde_json::{json, Value};

use crate::app_bundle_ingestion_lifecycle::{
    arn_of, audit_capture, encode, unknown_bundle_arn, unknown_uuid, uuid_of, Answer, Emulator,
};

/// The applications AppFabric supports, as the model's own documentation of the
/// `app` member lists them (`smithy_query(query="doc",
/// shape="CreateAppAuthorizationRequest", member="app")`: "Valid values are:
/// SLACK ASANA JIRA M365 M365AUDITLOGS ZOOM ZENDESK OKTA GOOGLE DROPBOX
/// SMARTSHEET CISCO"). The tables below walk entries rather than repeating one
/// value, so no row depends on which application an authorization carries.
const APPS: &[&str] = &[
    "ZENDESK", "OKTA", "DROPBOX", "CISCO", "SMARTSHEET", "M365AUDITLOGS", "SLACK", "JIRA", "ZOOM",
    "GOOGLE",
];

/// The two `AuthType` values the model's enum carries (`apiKey` | `oauth2`), so
/// a table can span both kinds of authorization.
const AUTH_TYPES: &[&str] = &["apiKey", "oauth2"];

/// Which of the two forms the model's `Identifier` shape accepts a table row
/// names a resource by (`^arn:.+$|^[a-f0-9]{8}-…$`).
#[derive(Debug, Clone, Copy)]
pub(crate) enum Form {
    Arn,
    Uuid,
}

/// An ARN rendered in the form a row asks for.
pub(crate) fn as_form(arn: &str, form: Form) -> String {
    match form {
        Form::Arn => arn.to_string(),
        Form::Uuid => uuid_of(arn),
    }
}

/// A second UUID-shaped identifier no create call ever minted, so a row can name
/// two distinct unknown resources.
fn other_unknown_uuid() -> String {
    "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee".to_string()
}

/// An app-authorization ARN this service never minted, in the form
/// `CreateAppAuthorization` builds (`appbundle/<uuid>/appauthorization/<uuid>`).
fn unknown_authorization_arn(bundle_uuid: &str, authorization_uuid: &str) -> String {
    format!(
        "{}/appauthorization/{authorization_uuid}",
        unknown_bundle_arn(bundle_uuid)
    )
}

/// The credential body one `authType` takes — the pairing the model's own
/// documentation of `credential` requires ("Specify credentials that match the
/// authorization type for your request").
fn credential_for(auth_type: &str) -> Value {
    if auth_type == "apiKey" {
        json!({"apiKeyCredential": {"apiKey": "placeholder-api-key"}})
    } else {
        json!({"oauth2Credential": {
            "clientId": "placeholder-client-id",
            "clientSecret": "placeholder-client-secret",
        }})
    }
}

/// `POST /appbundles/{appBundleIdentifier}/appauthorizations`.
pub(crate) async fn create_authorization(
    emulator: &Emulator,
    bundle: &str,
    app: &str,
    auth_type: &str,
    tenant: &str,
) -> Answer {
    emulator
        .call(
            "POST",
            &format!("/appbundles/{}/appauthorizations", encode(bundle)),
            Some(json!({
                "app": app,
                "authType": auth_type,
                "credential": credential_for(auth_type),
                "tenant": {
                    "tenantIdentifier": tenant,
                    "tenantDisplayName": format!("{tenant} display"),
                },
            })),
        )
        .await
}

/// `POST …/appauthorizations/{id}/connect`, carrying the OAuth2 `authRequest` an
/// `oauth2` authorization requires and nothing for an `apiKey` one.
pub(crate) async fn connect_authorization(
    emulator: &Emulator,
    bundle: &str,
    authorization: &str,
    auth_type: &str,
) -> Answer {
    let body = if auth_type == "apiKey" {
        json!({})
    } else {
        json!({"authRequest": {
            "redirectUri": "https://example.com/oauth/callback",
            "code": "placeholder-authorization-code",
        }})
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

/// `GET /appbundles/{appBundleIdentifier}/appauthorizations/{appAuthorizationIdentifier}`.
pub(crate) async fn get_authorization(
    emulator: &Emulator,
    bundle: &str,
    authorization: &str,
) -> Answer {
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

/// `DELETE /appbundles/{appBundleIdentifier}/appauthorizations/{appAuthorizationIdentifier}`.
pub(crate) async fn delete_authorization(
    emulator: &Emulator,
    bundle: &str,
    authorization: &str,
) -> Answer {
    emulator
        .call(
            "DELETE",
            &format!(
                "/appbundles/{}/appauthorizations/{}",
                encode(bundle),
                encode(authorization)
            ),
            None,
        )
        .await
}

/// The `appAuthorizationArn` a create (or get) answer reports.
pub(crate) fn authorization_arn(answer: &Answer, case: &str) -> String {
    answer
        .body
        .get("appAuthorization")
        .and_then(|authorization| authorization.get("appAuthorizationArn"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected appAuthorization.appAuthorizationArn in {}",
                answer.body
            )
        })
        .to_string()
}

/// The ARNs `ListAppAuthorizations` reports for one bundle, in the order it
/// reports them.
pub(crate) async fn authorization_arns(
    emulator: &Emulator,
    bundle: &str,
    case: &str,
) -> Vec<String> {
    let answer = emulator
        .call(
            "GET",
            &format!("/appbundles/{}/appauthorizations", encode(bundle)),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from ListAppAuthorizations, got {} with body {}",
        answer.status, answer.body
    );
    answer
        .body
        .get("appAuthorizationSummaryList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: no appAuthorizationSummaryList in {}",
                answer.body
            )
        })
        .iter()
        .map(|summary| {
            summary
                .get("appAuthorizationArn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a summary without an arn in {summary}")
                })
                .to_string()
        })
        .collect()
}

/// Create one app bundle and `total` app authorizations under it, each for its
/// own (application, tenant) pair, and answer `(bundle arn, authorization arns in
/// creation order)`. `connect` runs the connect handshake on each, so a table can
/// span both of the statuses a stored authorization can be in.
///
/// `bundle_index` picks the Region the bundle is created in, so a fixture needing
/// two bundles at once gives them different indices: an account holds one app
/// bundle per Region.
pub(crate) async fn bundle_with_authorizations(
    emulator: &Emulator,
    bundle_index: usize,
    total: usize,
    connect: bool,
    case: &str,
) -> (String, Vec<String>) {
    let bundle = emulator.fixture_bundle(bundle_index).await;
    let mut authorizations = Vec::with_capacity(total);
    for index in 0..total {
        let auth_type = AUTH_TYPES[index % AUTH_TYPES.len()];
        let created = create_authorization(
            emulator,
            &bundle,
            APPS[index % APPS.len()],
            auth_type,
            &format!("tenant-{bundle_index}-{index}"),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201 from CreateAppAuthorization, got {} with body {}",
            created.status, created.body
        );
        let arn = authorization_arn(&created, case);
        if connect {
            let connected = connect_authorization(emulator, &bundle, &arn, auth_type).await;
            assert_eq!(
                connected.status, 200,
                "case {case:?}: expected HTTP 200 from ConnectAppAuthorization, got {} with body {}",
                connected.status, connected.body
            );
        }
        authorizations.push(arn);
    }
    (bundle, authorizations)
}

/// RULE: deleting an app authorization removes exactly the record its identifier
/// names from the bundle that owns it — the reader no longer reports it, every
/// sibling survives in creation order — and, the operation being `@idempotent`,
/// repeating the call answers HTTP 204 again without changing anything further.
///
/// The table spans how many authorizations the bundle holds, which one is deleted
/// (first, middle, last), whether they completed the connect handshake and the
/// identifier form each of the two labels is named by, because none of those may
/// change the outcome.
#[tokio::test]
async fn deleting_an_app_authorization_removes_exactly_that_authorization() {
    // (case, authorizations created, index deleted, connected, bundle form, authorization form)
    let cases: Vec<(&str, usize, usize, bool, Form, Form)> = vec![
        ("one, both by arn", 1, 0, false, Form::Arn, Form::Arn),
        ("one, both by uuid", 1, 0, false, Form::Uuid, Form::Uuid),
        ("one connected, by uuid", 1, 0, true, Form::Uuid, Form::Uuid),
        ("two, delete the last", 2, 1, false, Form::Arn, Form::Uuid),
        ("three, delete the first", 3, 0, true, Form::Uuid, Form::Arn),
        ("three, delete the middle", 3, 1, false, Form::Arn, Form::Arn),
        ("four, delete the last", 4, 3, true, Form::Uuid, Form::Uuid),
        ("five, delete the third", 5, 2, false, Form::Arn, Form::Uuid),
    ];

    for (case, total, index, connected, bundle_form, authorization_form) in cases {
        let emulator = Emulator::new();
        let (bundle, authorizations) =
            bundle_with_authorizations(&emulator, 0, total, connected, case).await;
        let doomed = authorizations[index].clone();
        let survivors: Vec<String> = authorizations
            .iter()
            .filter(|arn| **arn != doomed)
            .cloned()
            .collect();

        let deleted = delete_authorization(
            &emulator,
            &as_form(&bundle, bundle_form),
            &as_form(&doomed, authorization_form),
        )
        .await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: expected HTTP 204, got {} with body {}",
            deleted.status, deleted.body
        );
        assert!(
            deleted
                .body
                .as_object()
                .is_none_or(serde_json::Map::is_empty),
            "case {case:?}: a 204 carries no response members, got {}",
            deleted.body
        );

        // Gone through the reader, in either identifier form.
        for form in [doomed.clone(), uuid_of(&doomed)] {
            let read = get_authorization(&emulator, &bundle, &form).await;
            assert_eq!(
                read.status, 404,
                "case {case:?} read by {form:?}: expected HTTP 404 after the delete, got {} with body {}",
                read.status, read.body
            );
            assert_eq!(
                read.error_code(),
                "ResourceNotFoundException",
                "case {case:?} read by {form:?}: expected ResourceNotFoundException, got {}",
                read.body
            );
        }

        // And every sibling is untouched.
        let listed = authorization_arns(&emulator, &bundle, case).await;
        assert_eq!(
            listed, survivors,
            "case {case:?}: expected the surviving authorizations {survivors:?}, got {listed:?}"
        );
        for survivor in &survivors {
            let read = get_authorization(&emulator, &bundle, survivor).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: survivor {survivor:?} must still be readable, got {} with body {}",
                read.status, read.body
            );
        }

        let again = delete_authorization(
            &emulator,
            &as_form(&bundle, bundle_form),
            &as_form(&doomed, authorization_form),
        )
        .await;
        assert_eq!(
            again.status, 204,
            "case {case:?}: a repeated delete is idempotent, got {} with body {}",
            again.status, again.body
        );
        let listed = authorization_arns(&emulator, &bundle, case).await;
        assert_eq!(
            listed, survivors,
            "case {case:?}: the repeated delete must change nothing, expected {survivors:?}, got {listed:?}"
        );
    }
}

/// RULE: a delete whose target the named bundle does not hold is a SUCCESS that
/// changes nothing (`@idempotent`: the store is already in the state the request
/// asks for), whichever form the absent identifier takes and whether it is
/// unknown altogether or names a resource living under a different parent.
#[tokio::test]
async fn deleting_an_app_authorization_the_bundle_does_not_hold_is_a_noop_success() {
    let emulator = Emulator::new();
    let (bundle, authorizations) =
        bundle_with_authorizations(&emulator, 0, 2, false, "set-up").await;
    let (other_bundle, other_authorizations) =
        bundle_with_authorizations(&emulator, 1, 1, false, "set-up sibling").await;
    let foreign = other_authorizations[0].clone();

    // (case, bundle identifier, authorization identifier the bundle does not hold)
    let cases: Vec<(&str, String, String)> = vec![
        ("an unknown uuid", bundle.clone(), unknown_uuid()),
        (
            "another unknown uuid, bundle by uuid",
            uuid_of(&bundle),
            other_unknown_uuid(),
        ),
        (
            "an authorization arn this service never minted",
            bundle.clone(),
            unknown_authorization_arn(&uuid_of(&bundle), &unknown_uuid()),
        ),
        (
            "another bundle's authorization, by arn",
            bundle.clone(),
            foreign.clone(),
        ),
        (
            "another bundle's authorization, by uuid",
            uuid_of(&bundle),
            uuid_of(&foreign),
        ),
    ];

    for (case, bundle_identifier, authorization_identifier) in cases {
        let answer =
            delete_authorization(&emulator, &bundle_identifier, &authorization_identifier).await;
        assert_eq!(
            answer.status, 204,
            "case {case:?}: expected HTTP 204, got {} with body {}",
            answer.status, answer.body
        );
        assert!(
            answer
                .body
                .as_object()
                .is_none_or(serde_json::Map::is_empty),
            "case {case:?}: a 204 carries no response members, got {}",
            answer.body
        );

        let listed = authorization_arns(&emulator, &bundle, case).await;
        assert_eq!(
            listed, authorizations,
            "case {case:?}: the no-op must change nothing, expected {authorizations:?}, got {listed:?}"
        );
        let read = get_authorization(&emulator, &other_bundle, &foreign).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the other bundle's authorization must survive, got {} with body {}",
            read.status, read.body
        );
    }
}

/// RULE: the app bundle in the path is a resource of its own, so an identifier
/// naming no stored bundle is the modelled `ResourceNotFoundException` (HTTP 404)
/// reporting the `AppBundle` — never a silent success — whatever the app
/// authorization identifier beside it names.
#[tokio::test]
async fn deleting_under_an_unknown_app_bundle_is_not_found() {
    let emulator = Emulator::new();
    let (bundle, authorizations) =
        bundle_with_authorizations(&emulator, 0, 1, false, "set-up").await;
    let real = authorizations[0].clone();

    // (case, unknown bundle identifier, authorization identifier)
    let cases: Vec<(&str, String, String)> = vec![
        (
            "unknown bundle uuid, unknown authorization uuid",
            unknown_uuid(),
            other_unknown_uuid(),
        ),
        (
            "unknown bundle uuid, a real authorization of another bundle",
            unknown_uuid(),
            real.clone(),
        ),
        (
            "a bundle arn this service never minted",
            unknown_bundle_arn(&unknown_uuid()),
            uuid_of(&real),
        ),
        (
            "another unknown bundle uuid, authorization by arn",
            other_unknown_uuid(),
            real.clone(),
        ),
    ];

    for (case, bundle_identifier, authorization_identifier) in cases {
        let answer =
            delete_authorization(&emulator, &bundle_identifier, &authorization_identifier).await;
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
        assert_eq!(
            answer.body.get("resourceType").and_then(Value::as_str),
            Some("AppBundle"),
            "case {case:?}: the refusal reports the app bundle, got {}",
            answer.body
        );
        assert_eq!(
            answer.body.get("resourceId").and_then(Value::as_str),
            Some(bundle_identifier.as_str()),
            "case {case:?}: the refusal carries the supplied bundle identifier, got {}",
            answer.body
        );

        // Nothing was deleted anywhere.
        let listed = authorization_arns(&emulator, &bundle, case).await;
        assert_eq!(
            listed, authorizations,
            "case {case:?}: the refusal must change nothing, expected {authorizations:?}, got {listed:?}"
        );
    }
}

/// RULE: the operation's documented precondition — "You must delete the
/// associated ingestion before you can delete an app authorization" — is enforced
/// against the STORE: while the bundle still holds an ingestion for the
/// authorization's (application, tenant) pair, the delete is refused with the
/// modelled `ValidationException` (HTTP 400) and the authorization survives;
/// once that ingestion is gone the same call succeeds.
///
/// The table spans the application and tenant of the pair, the identifier forms
/// and whether the ingestion is `disabled` or `enabled` — the precondition is
/// about the ingestion EXISTING, so its state may not change the answer. Every
/// row authorizes with an `apiKey`, because an ingestion exists only for a pair
/// whose app authorization completed the connect handshake and an `oauth2` one
/// never does in this emulator (see `app_authorization_connect_validation`).
#[tokio::test]
async fn deleting_an_app_authorization_with_an_associated_ingestion_is_refused() {
    // (case, app, tenant, authType, ingestion enabled, bundle form, authorization form)
    let cases: Vec<(&str, &str, &str, &str, bool, Form, Form)> = vec![
        (
            "ZENDESK, disabled ingestion, both by arn",
            "ZENDESK",
            "tenant-a",
            "apiKey",
            false,
            Form::Arn,
            Form::Arn,
        ),
        (
            "SLACK, disabled ingestion, both by uuid",
            "SLACK",
            "tenant-b",
            "apiKey",
            false,
            Form::Uuid,
            Form::Uuid,
        ),
        (
            "DROPBOX, enabled ingestion, mixed forms",
            "DROPBOX",
            "tenant-c",
            "apiKey",
            true,
            Form::Uuid,
            Form::Arn,
        ),
        (
            "M365AUDITLOGS, enabled ingestion, mixed forms",
            "M365AUDITLOGS",
            "tenant-d",
            "apiKey",
            true,
            Form::Arn,
            Form::Uuid,
        ),
    ];

    for (case, app, tenant, auth_type, enabled, bundle_form, authorization_form) in cases {
        let emulator = Emulator::new();
        let bundle = emulator.fixture_bundle(0).await;
        let created = create_authorization(&emulator, &bundle, app, auth_type, tenant).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201 from CreateAppAuthorization, got {} with body {}",
            created.status, created.body
        );
        let authorization = authorization_arn(&created, case);
        let connected = connect_authorization(&emulator, &bundle, &authorization, auth_type).await;
        assert_eq!(
            connected.status, 200,
            "case {case:?}: expected HTTP 200 from ConnectAppAuthorization, got {} with body {}",
            connected.status, connected.body
        );

        let ingestion_created = emulator
            .create_ingestion(
                &bundle,
                json!({"app": app, "tenantId": tenant, "ingestionType": "auditLog"}),
            )
            .await;
        assert_eq!(
            ingestion_created.status, 201,
            "case {case:?}: expected HTTP 201 from CreateIngestion, got {} with body {}",
            ingestion_created.status, ingestion_created.body
        );
        let ingestion = arn_of(&ingestion_created.body, "ingestion");
        if enabled {
            let started = emulator
                .call(
                    "POST",
                    &format!(
                        "/appbundles/{}/ingestions/{}/start",
                        encode(&bundle),
                        encode(&ingestion)
                    ),
                    None,
                )
                .await;
            assert_eq!(
                started.status, 200,
                "case {case:?}: expected HTTP 200 from StartIngestion, got {} with body {}",
                started.status, started.body
            );
        }

        let refused = delete_authorization(
            &emulator,
            &as_form(&bundle, bundle_form),
            &as_form(&authorization, authorization_form),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 while the associated ingestion exists, got {} with body {}",
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
            "case {case:?}: the resource, not a member, forbids the call, got {}",
            refused.body
        );
        assert!(
            refused.body.get("fieldList").is_none(),
            "case {case:?}: no request member is at fault, got {}",
            refused.body
        );
        let message = refused
            .body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            message.contains(&ingestion) && message.contains(&authorization),
            "case {case:?}: the refusal must name the authorization {authorization:?} and the \
             associated ingestion {ingestion:?}, got {message:?}"
        );

        // The refusal wrote nothing: the authorization is still there.
        let read = get_authorization(&emulator, &bundle, &authorization).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the refused authorization must survive, got {} with body {}",
            read.status, read.body
        );

        // Satisfy the precondition the way the documentation states it, and the
        // same call now succeeds.
        if enabled {
            let stopped = emulator
                .call(
                    "POST",
                    &format!(
                        "/appbundles/{}/ingestions/{}/stop",
                        encode(&bundle),
                        encode(&ingestion)
                    ),
                    None,
                )
                .await;
            assert_eq!(
                stopped.status, 200,
                "case {case:?}: expected HTTP 200 from StopIngestion, got {} with body {}",
                stopped.status, stopped.body
            );
        }
        let ingestion_deleted = emulator.delete_ingestion(&bundle, &ingestion).await;
        assert_eq!(
            ingestion_deleted.status, 204,
            "case {case:?}: expected HTTP 204 from DeleteIngestion, got {} with body {}",
            ingestion_deleted.status, ingestion_deleted.body
        );

        let deleted = delete_authorization(
            &emulator,
            &as_form(&bundle, bundle_form),
            &as_form(&authorization, authorization_form),
        )
        .await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: expected HTTP 204 once the associated ingestion is gone, got {} with body {}",
            deleted.status, deleted.body
        );
        let read = get_authorization(&emulator, &bundle, &authorization).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: the deleted authorization must be gone, got {} with body {}",
            read.status, read.body
        );
        let listed = authorization_arns(&emulator, &bundle, case).await;
        assert!(
            listed.is_empty(),
            "case {case:?}: the bundle must hold no authorization, got {listed:?}"
        );
    }
}

/// RULE: this state-changing operation writes ONE structured audit record per
/// call, naming the operation, the presented access key id, the SERVER-MINTED
/// resource and the outcome — and no caller-supplied content (a tenant
/// identifier, a tenant display name).
///
/// The two outcomes a successful delete has are both covered: the removal of a
/// stored record, and the idempotent no-op for a target the bundle does not hold.
#[tokio::test]
async fn deleting_an_app_authorization_writes_an_audit_record() {
    let captured = audit_capture();
    let case = "audit";

    let emulator = Emulator::new();
    let bundle = emulator.fixture_bundle(0).await;
    let created =
        create_authorization(&emulator, &bundle, "ZENDESK", "apiKey", "tenant-audited").await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: expected HTTP 201 from CreateAppAuthorization, got {} with body {}",
        created.status, created.body
    );
    let authorization = authorization_arn(&created, case);
    let authorization_id = uuid_of(&authorization);
    // An absent target named ONLY by this test: the audit capture is
    // process-wide, so a shared never-minted identifier would also match the
    // records other tests in this suite write for their own no-op deletes.
    let absent = "0d0d0d0d-0d0d-4d0d-8d0d-0d0d0d0d0d0d".to_string();

    let deleted = delete_authorization(&emulator, &bundle, &authorization).await;
    assert_eq!(
        deleted.status, 204,
        "case {case:?}: expected HTTP 204, got {} with body {}",
        deleted.status, deleted.body
    );
    let again = delete_authorization(&emulator, &bundle, &absent).await;
    assert_eq!(
        again.status, 204,
        "case {case:?}: expected HTTP 204 for an absent target, got {} with body {}",
        again.status, again.body
    );

    // Scoped to the identifiers THIS test named, so records other tests write
    // into the same process-wide capture are none of its business.
    let records: Vec<String> = captured
        .audit_records()
        .into_iter()
        .filter(|rendered| {
            rendered.contains("operation=DeleteAppAuthorization ")
                && (rendered.contains(&authorization_id) || rendered.contains(&absent))
        })
        .collect();

    // (case, the resource id the record names, the outcome)
    let cases: Vec<(&str, &str, &str)> = vec![
        ("a deleted app authorization", &authorization_id, "deleted"),
        ("a delete of an absent one", &absent, "already absent"),
    ];
    for (row, resource_id, outcome) in &cases {
        let matching: Vec<&String> = records
            .iter()
            .filter(|rendered| {
                rendered.contains(&format!("outcome={outcome} "))
                    && rendered.contains("resource_type=AppAuthorization ")
                    && rendered.contains(resource_id)
            })
            .collect();
        assert_eq!(
            matching.len(),
            1,
            "case {row:?}: expected exactly one audit record for {outcome}, got {matching:?} out of {records:?}"
        );
        assert!(
            matching[0].contains("caller_akid=AKIDEXAMPLE"),
            "case {row:?}: expected the presented access key id in the record, got {:?}",
            matching[0]
        );
    }
    assert_eq!(
        records.len(),
        cases.len(),
        "case {case:?}: expected one audit record per delete, got {records:?}"
    );
    for record in &records {
        assert!(
            !record.contains("tenant-audited"),
            "case {case:?}: an audit record must carry no caller-supplied content, got {record:?}"
        );
    }
}

/// RULE: the associated ingestion is the one for the authorization's OWN
/// (application, tenant) pair, so an ingestion of any other pair — a different
/// application, a different tenant, or an ingestion under another bundle
/// altogether — is not a reason to refuse: the delete succeeds and leaves that
/// ingestion and its own authorization untouched.
#[tokio::test]
async fn an_ingestion_of_another_pair_does_not_block_the_delete() {
    // (case, doomed app, doomed tenant, ingestion app, ingestion tenant)
    let cases: Vec<(&str, &str, &str, &str, &str)> = vec![
        (
            "same tenant, another application",
            "ZENDESK",
            "tenant-shared",
            "OKTA",
            "tenant-shared",
        ),
        (
            "same application, another tenant",
            "CISCO",
            "tenant-one",
            "CISCO",
            "tenant-two",
        ),
        (
            "another application and another tenant",
            "SMARTSHEET",
            "tenant-x",
            "GOOGLE",
            "tenant-y",
        ),
    ];

    for (case, doomed_app, doomed_tenant, other_app, other_tenant) in cases {
        let emulator = Emulator::new();
        let bundle = emulator.fixture_bundle(0).await;

        let created =
            create_authorization(&emulator, &bundle, doomed_app, "apiKey", doomed_tenant).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201 from CreateAppAuthorization, got {} with body {}",
            created.status, created.body
        );
        let doomed = authorization_arn(&created, case);

        // The harness's `create_ingestion` authorizes the ingestion's own pair
        // first, which is the chain AppFabric requires; that authorization is
        // NOT the one this test deletes.
        let ingestion_created = emulator
            .create_ingestion(
                &bundle,
                json!({
                    "app": other_app,
                    "tenantId": other_tenant,
                    "ingestionType": "auditLog",
                }),
            )
            .await;
        assert_eq!(
            ingestion_created.status, 201,
            "case {case:?}: expected HTTP 201 from CreateIngestion, got {} with body {}",
            ingestion_created.status, ingestion_created.body
        );
        let ingestion = arn_of(&ingestion_created.body, "ingestion");

        let deleted = delete_authorization(&emulator, &bundle, &doomed).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: an ingestion of another pair must not block the delete, got {} with body {}",
            deleted.status, deleted.body
        );
        let read = get_authorization(&emulator, &bundle, &doomed).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: the deleted authorization must be gone, got {} with body {}",
            read.status, read.body
        );
        let ingestion_read = emulator.get_ingestion(&bundle, &ingestion).await;
        assert_eq!(
            ingestion_read.status, 200,
            "case {case:?}: the other pair's ingestion must survive, got {} with body {}",
            ingestion_read.status, ingestion_read.body
        );
        // The other pair's own authorization, which the ingestion depends on, is
        // still there too: the listing is exactly that one record.
        let listed = authorization_arns(&emulator, &bundle, case).await;
        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: expected only the other pair's authorization, got {listed:?}"
        );
        assert!(
            !listed.contains(&doomed),
            "case {case:?}: the deleted authorization {doomed:?} must not be listed, got {listed:?}"
        );
    }
}
