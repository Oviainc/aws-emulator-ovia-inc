// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the (application, tenant) pair that ties an ingestion to
//! its app authorization, driven over the wire (restJson1) through the same
//! router the emulator boots.
//!
//! Each test states a RULE over a TABLE of inputs rather than one recorded call:
//!
//! * no accepted call takes an app authorization OFF a pair the bundle still
//!   holds an ingestion for — `UpdateAppAuthorization` is refused with the same
//!   precondition `DeleteAppAuthorization` documents, so the two operations that
//!   can break the association both refuse it;
//! * a call that leaves the pair alone (a renamed tenant display name, a rotated
//!   credential) breaks no association and is applied;
//! * whatever sequence of updates a caller sends, every ingestion the bundle
//!   lists is still matched by an app authorization for its own pair — the state
//!   `CreateIngestion` requires.
//!
//! The harness (the router, `Answer`, `encode`, `arn_of`, `uuid_of`, the
//! app-bundle and ingestion calls) is the one `app_bundle_ingestion_lifecycle`
//! already builds.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::app_bundle_ingestion_lifecycle::{arn_of, encode, uuid_of, Answer, Emulator};

/// Which of the two forms the model's `Identifier` shape accepts a table row
/// names a resource by (`^arn:.+$|^[a-f0-9]{8}-…$`).
#[derive(Debug, Clone, Copy)]
enum Form {
    Arn,
    Uuid,
}

/// An ARN rendered in the form a row asks for.
fn as_form(arn: &str, form: Form) -> String {
    match form {
        Form::Arn => arn.to_string(),
        Form::Uuid => uuid_of(arn),
    }
}

/// The credential body one `authType` takes — the pairing the model's own
/// documentation of `credential` requires ("Specify credentials that match the
/// authorization type of the app authorization to update").
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

/// `POST /appbundles/{appBundleIdentifier}/appauthorizations`, as its ARN.
async fn create_authorization(
    emulator: &Emulator,
    bundle: &str,
    app: &str,
    auth_type: &str,
    tenant: &str,
    case: &str,
) -> String {
    let created = emulator
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
        .await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: expected HTTP 201 from CreateAppAuthorization, got {} with body {}",
        created.status, created.body
    );
    created
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
        .to_string()
}

/// `POST …/appauthorizations/{id}/connect`, which is what an ingestion of the
/// authorization's pair requires: it carries the OAuth2 `authRequest` an
/// `oauth2` authorization needs and nothing for an `apiKey` one.
async fn connect_authorization(
    emulator: &Emulator,
    bundle: &str,
    authorization: &str,
    auth_type: &str,
    case: &str,
) {
    let body = if auth_type == "apiKey" {
        json!({})
    } else {
        json!({"authRequest": {
            "redirectUri": "https://example.invalid/oauth/callback",
            "code": "placeholder-authorization-code",
        }})
    };
    let connected = emulator
        .call(
            "POST",
            &format!(
                "/appbundles/{}/appauthorizations/{}/connect",
                encode(bundle),
                encode(authorization)
            ),
            Some(body),
        )
        .await;
    assert_eq!(
        connected.status, 200,
        "case {case:?}: expected HTTP 200 from ConnectAppAuthorization, got {} with body {}",
        connected.status, connected.body
    );
}

/// `PATCH /appbundles/{appBundleIdentifier}/appauthorizations/{appAuthorizationIdentifier}`.
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

/// `GET /appbundles/{appBundleIdentifier}/appauthorizations/{appAuthorizationIdentifier}`.
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

/// `DELETE /appbundles/{appBundleIdentifier}/appauthorizations/{appAuthorizationIdentifier}`.
async fn delete_authorization(emulator: &Emulator, bundle: &str, authorization: &str) -> Answer {
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

/// The stored `appAuthorization`, read back through `GetAppAuthorization`.
async fn stored(emulator: &Emulator, bundle: &str, authorization: &str, case: &str) -> Value {
    let answer = get_authorization(emulator, bundle, authorization).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: GetAppAuthorization must answer HTTP 200, got {} with body {}",
        answer.status, answer.body
    );
    answer
        .body
        .get("appAuthorization")
        .unwrap_or_else(|| panic!("case {case:?}: expected appAuthorization in {}", answer.body))
        .clone()
}

/// A string member of a JSON record, or `""`.
fn member(record: &Value, name: &str) -> String {
    record
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `tenantIdentifier` a record carries.
fn tenant_identifier(record: &Value) -> String {
    record
        .get("tenant")
        .map(|tenant| member(tenant, "tenantIdentifier"))
        .unwrap_or_default()
}

/// Create an ingestion for one (application, tenant) pair and answer its ARN.
/// `enabled` runs `StartIngestion`, because the precondition is about the
/// ingestion EXISTING and so may not depend on its state.
async fn create_ingestion(
    emulator: &Emulator,
    bundle: &str,
    app: &str,
    tenant: &str,
    enabled: bool,
    case: &str,
) -> String {
    let created = emulator
        .create_ingestion(
            bundle,
            json!({"app": app, "tenantId": tenant, "ingestionType": "auditLog"}),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: expected HTTP 201 from CreateIngestion, got {} with body {}",
        created.status, created.body
    );
    let ingestion = arn_of(&created.body, "ingestion");
    if enabled {
        let started = emulator
            .call(
                "POST",
                &format!(
                    "/appbundles/{}/ingestions/{}/start",
                    encode(bundle),
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
    ingestion
}

/// The (application, tenant) pairs `ListIngestions` reports for one bundle.
async fn ingestion_pairs(emulator: &Emulator, bundle: &str, case: &str) -> BTreeSet<(String, String)> {
    let answer = emulator.list_ingestions(bundle, "").await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: ListIngestions must answer HTTP 200, got {} with body {}",
        answer.status, answer.body
    );
    answer
        .body
        .get("ingestions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected ingestions in {}", answer.body))
        .iter()
        .map(|summary| (member(summary, "app"), member(summary, "tenantId")))
        .collect()
}

/// The (application, tenant) pairs `ListAppAuthorizations` reports for one bundle.
async fn authorization_pairs(
    emulator: &Emulator,
    bundle: &str,
    case: &str,
) -> BTreeSet<(String, String)> {
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
        .map(|summary| (member(summary, "app"), tenant_identifier(summary)))
        .collect()
}

/// One row of the table in
/// [`an_update_cannot_take_an_authorization_off_a_pair_with_an_ingestion`].
struct MoveCase {
    /// What this row is called in every assertion it fails.
    case: &'static str,
    /// The application and `authType` of the authorization under test.
    app: &'static str,
    auth_type: &'static str,
    /// The tenant it is created with.
    from_tenant: &'static str,
    /// The `tenantIdentifier` the PATCH supplies — equal to `from_tenant` for a
    /// row that renames the tenant without moving the pair.
    to_tenant: &'static str,
    /// Whether the PATCH also rotates the credential, and whether it supplies a
    /// `tenant` member at all.
    supply_tenant: bool,
    supply_credential: bool,
    /// The (application, tenant) pair the bundle's ingestion is held for, if the
    /// row creates one, and whether that ingestion is started.
    ingestion: Option<(&'static str, &'static str)>,
    enabled: bool,
    /// The identifier forms the PATCH names the bundle and the authorization by.
    bundle_form: Form,
    authorization_form: Form,
    /// The HTTP status the PATCH must answer.
    expected: u16,
}

/// RULE: an app authorization's (application, tenant) pair is what ties the
/// bundle's ingestion to it, so a call that would take the authorization OFF a
/// pair the bundle still holds an ingestion for is refused with the modelled
/// `ValidationException` (HTTP 400) stating the precondition AppFabric documents
/// — the associated ingestion must be deleted first — and writes nothing; a call
/// that leaves the stored `tenantIdentifier` where it is (a renamed display name,
/// a rotated credential) breaks no association and is applied, and so is a move
/// whose pair no ingestion is held for.
///
/// The table spans the application and `authType` of the authorization, which
/// members the PATCH supplies, how the bundle's ingestion relates to the pair
/// (the same pair, another tenant, another application, none at all), whether
/// that ingestion is `disabled` or `enabled`, and the identifier form each path
/// label is named by — none of which may change the outcome. A row whose bundle
/// holds an ingestion of the authorization's OWN pair authorizes with an
/// `apiKey`, because such an ingestion exists only for a pair whose handshake
/// completed and an `oauth2` one never does in this emulator (see
/// `app_authorization_connect_validation`).
#[tokio::test]
async fn an_update_cannot_take_an_authorization_off_a_pair_with_an_ingestion() {
    let cases: Vec<MoveCase> = vec![
        MoveCase {
            case: "a move off the associated disabled ingestion, both by arn",
            app: "ZENDESK",
            auth_type: "apiKey",
            from_tenant: "tenant-a",
            to_tenant: "tenant-b",
            supply_tenant: true,
            supply_credential: false,
            ingestion: Some(("ZENDESK", "tenant-a")),
            enabled: false,
            bundle_form: Form::Arn,
            authorization_form: Form::Arn,
            expected: 400,
        },
        MoveCase {
            case: "a move off the associated enabled ingestion, both by uuid",
            app: "SLACK",
            auth_type: "apiKey",
            from_tenant: "tenant-one",
            to_tenant: "tenant-two",
            supply_tenant: true,
            supply_credential: false,
            ingestion: Some(("SLACK", "tenant-one")),
            enabled: true,
            bundle_form: Form::Uuid,
            authorization_form: Form::Uuid,
            expected: 400,
        },
        MoveCase {
            case: "a move that also rotates the credential, mixed forms",
            app: "DROPBOX",
            auth_type: "apiKey",
            from_tenant: "tenant-x",
            to_tenant: "tenant-y",
            supply_tenant: true,
            supply_credential: true,
            ingestion: Some(("DROPBOX", "tenant-x")),
            enabled: false,
            bundle_form: Form::Uuid,
            authorization_form: Form::Arn,
            expected: 400,
        },
        MoveCase {
            case: "a renamed display name over the associated ingestion",
            app: "OKTA",
            auth_type: "apiKey",
            from_tenant: "tenant-kept",
            to_tenant: "tenant-kept",
            supply_tenant: true,
            supply_credential: false,
            ingestion: Some(("OKTA", "tenant-kept")),
            enabled: false,
            bundle_form: Form::Arn,
            authorization_form: Form::Uuid,
            expected: 200,
        },
        MoveCase {
            case: "a rotated credential over the associated ingestion",
            app: "CISCO",
            auth_type: "apiKey",
            from_tenant: "tenant-kept",
            to_tenant: "tenant-kept",
            supply_tenant: false,
            supply_credential: true,
            ingestion: Some(("CISCO", "tenant-kept")),
            enabled: true,
            bundle_form: Form::Uuid,
            authorization_form: Form::Uuid,
            expected: 200,
        },
        MoveCase {
            case: "a move while only another tenant has an ingestion",
            app: "SMARTSHEET",
            auth_type: "apiKey",
            from_tenant: "tenant-m",
            to_tenant: "tenant-n",
            supply_tenant: true,
            supply_credential: false,
            ingestion: Some(("SMARTSHEET", "tenant-other")),
            enabled: false,
            bundle_form: Form::Arn,
            authorization_form: Form::Arn,
            expected: 200,
        },
        MoveCase {
            case: "a move while only another application has an ingestion",
            app: "GOOGLE",
            auth_type: "apiKey",
            from_tenant: "tenant-g",
            to_tenant: "tenant-h",
            supply_tenant: true,
            supply_credential: false,
            ingestion: Some(("JIRA", "tenant-g")),
            enabled: true,
            bundle_form: Form::Uuid,
            authorization_form: Form::Arn,
            expected: 200,
        },
        MoveCase {
            case: "a move with no ingestion in the bundle at all",
            app: "M365AUDITLOGS",
            auth_type: "oauth2",
            from_tenant: "tenant-p",
            to_tenant: "tenant-q",
            supply_tenant: true,
            supply_credential: false,
            ingestion: None,
            enabled: false,
            bundle_form: Form::Arn,
            authorization_form: Form::Uuid,
            expected: 200,
        },
    ];

    for row in cases {
        let case = row.case;
        let emulator = Emulator::new();
        let bundle = emulator.fixture_bundle(0).await;
        let authorization = create_authorization(
            &emulator,
            &bundle,
            row.app,
            row.auth_type,
            row.from_tenant,
            case,
        )
        .await;
        // The handshake an ingestion of this authorization's own pair requires.
        connect_authorization(&emulator, &bundle, &authorization, row.auth_type, case).await;
        let ingestion = match row.ingestion {
            Some((app, tenant)) => Some(
                create_ingestion(&emulator, &bundle, app, tenant, row.enabled, case).await,
            ),
            None => None,
        };
        let before = stored(&emulator, &bundle, &authorization, case).await;

        let mut body = json!({});
        let object = body.as_object_mut().expect("a JSON object request body");
        if row.supply_tenant {
            object.insert(
                "tenant".to_string(),
                json!({
                    "tenantIdentifier": row.to_tenant,
                    "tenantDisplayName": format!("{} renamed", row.to_tenant),
                }),
            );
        }
        if row.supply_credential {
            object.insert("credential".to_string(), credential_for(row.auth_type));
        }

        let answer = update_authorization(
            &emulator,
            &as_form(&bundle, row.bundle_form),
            &as_form(&authorization, row.authorization_form),
            &body,
        )
        .await;
        assert_eq!(
            answer.status, row.expected,
            "case {case:?}: expected HTTP {}, got {} with body {}",
            row.expected, answer.status, answer.body
        );

        let after = stored(&emulator, &bundle, &authorization, case).await;
        if row.expected == 200 {
            if row.supply_tenant {
                assert_eq!(
                    tenant_identifier(&after),
                    row.to_tenant,
                    "case {case:?}: expected the supplied tenant to persist, got {after}"
                );
            } else {
                assert_eq!(
                    tenant_identifier(&after),
                    row.from_tenant,
                    "case {case:?}: a call supplying no tenant must leave the pair alone, got {after}"
                );
            }
        } else {
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {}",
                answer.body
            );
            assert_eq!(
                answer.body.get("reason").and_then(Value::as_str),
                Some("other"),
                "case {case:?}: the stored resource, not a member, forbids the call, got {}",
                answer.body
            );
            assert!(
                answer.body.get("fieldList").is_none(),
                "case {case:?}: no request member is at fault, got {}",
                answer.body
            );
            let message = member(&answer.body, "message");
            let held = ingestion
                .clone()
                .unwrap_or_else(|| panic!("case {case:?}: a refused row holds an ingestion"));
            assert!(
                message.contains(&authorization) && message.contains(&held),
                "case {case:?}: the refusal must name the authorization {authorization:?} and the \
                 associated ingestion {held:?}, got {message:?}"
            );
            assert_eq!(
                after, before,
                "case {case:?}: a refused update must write nothing, expected {before}, got {after}"
            );
        }

        // Whatever the answer, every ingestion the bundle lists still has an app
        // authorization for its own pair — the state `CreateIngestion` requires.
        let ingestions = ingestion_pairs(&emulator, &bundle, case).await;
        let authorizations = authorization_pairs(&emulator, &bundle, case).await;
        for pair in &ingestions {
            assert!(
                authorizations.contains(pair),
                "case {case:?}: the ingestion for {pair:?} must still have an app authorization, \
                 the bundle holds {authorizations:?}"
            );
        }

        // And the delete precondition is not bypassed: while the associated
        // ingestion is held the delete is still refused, and once it is deleted
        // both the move and the delete go through.
        let associated = row.ingestion == Some((row.app, tenant_identifier(&after).as_str()));
        let deleted = delete_authorization(&emulator, &bundle, &authorization).await;
        assert_eq!(
            deleted.status,
            if associated { 400 } else { 204 },
            "case {case:?}: expected the delete to answer {} while associated={associated}, got {} with body {}",
            if associated { 400 } else { 204 },
            deleted.status,
            deleted.body
        );
        if let Some(held) = ingestion {
            if row.enabled {
                let stopped = emulator
                    .call(
                        "POST",
                        &format!(
                            "/appbundles/{}/ingestions/{}/stop",
                            encode(&bundle),
                            encode(&held)
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
            let removed = emulator.delete_ingestion(&bundle, &held).await;
            assert_eq!(
                removed.status, 204,
                "case {case:?}: expected HTTP 204 from DeleteIngestion, got {} with body {}",
                removed.status, removed.body
            );
        }
        if associated {
            let retried = update_authorization(
                &emulator,
                &as_form(&bundle, row.bundle_form),
                &as_form(&authorization, row.authorization_form),
                &body,
            )
            .await;
            assert_eq!(
                retried.status, 200,
                "case {case:?}: the move must be accepted once the associated ingestion is gone, \
                 got {} with body {}",
                retried.status, retried.body
            );
            let moved = stored(&emulator, &bundle, &authorization, case).await;
            assert_eq!(
                tenant_identifier(&moved),
                row.to_tenant,
                "case {case:?}: expected the supplied tenant to persist, got {moved}"
            );
            let deleted = delete_authorization(&emulator, &bundle, &authorization).await;
            assert_eq!(
                deleted.status, 204,
                "case {case:?}: expected HTTP 204 once the associated ingestion is gone, got {} with body {}",
                deleted.status, deleted.body
            );
        }
    }
}

/// RULE: the association holds across a SEQUENCE of updates: however a caller
/// moves the tenants of a bundle's app authorizations, every ingestion the bundle
/// lists is still matched by an authorization for its own (application, tenant)
/// pair, so the store never reaches a state `CreateIngestion` refuses to create
/// and no `DeleteAppAuthorization` precondition is bypassed.
///
/// The sequence walks the moves that could break it — a free tenant, the tenant
/// a sibling authorization of the same application holds, the authorization's own
/// tenant, another free tenant — over authorizations that do and do not have an
/// ingestion of their own.
#[tokio::test]
async fn no_sequence_of_updates_leaves_an_ingestion_without_its_authorization() {
    let case = "a sequence of tenant moves";
    let emulator = Emulator::new();
    let bundle = emulator.fixture_bundle(0).await;

    // Two applications, each with two tenants: the first tenant of each pair
    // carries an ingestion, the second does not. Both authorize with an
    // `apiKey`, the handshake an ingestion's pair needs completed.
    let mut with_ingestion: Vec<String> = Vec::new();
    let mut without_ingestion: Vec<String> = Vec::new();
    for (app, auth_type) in [("ASANA", "apiKey"), ("ZOOM", "apiKey")] {
        for tenant in ["tenant-ingested", "tenant-bare"] {
            let arn =
                create_authorization(&emulator, &bundle, app, auth_type, tenant, case).await;
            connect_authorization(&emulator, &bundle, &arn, auth_type, case).await;
            if tenant == "tenant-ingested" {
                create_ingestion(&emulator, &bundle, app, tenant, false, case).await;
                with_ingestion.push(arn);
            } else {
                without_ingestion.push(arn);
            }
        }
    }

    let targets: Vec<String> = with_ingestion
        .iter()
        .chain(without_ingestion.iter())
        .cloned()
        .collect();
    // (label, the tenantIdentifier the PATCH supplies)
    let moves: &[(&str, &str)] = &[
        ("a free tenant", "tenant-free"),
        ("the tenant its application's bare authorization holds", "tenant-bare"),
        (
            "the tenant its application's ingested authorization holds",
            "tenant-ingested",
        ),
        ("another free tenant", "tenant-free-2"),
    ];

    for target in &targets {
        for (label, tenant) in moves {
            let row = format!("{case}: {label} on {target}");
            let answer = update_authorization(
                &emulator,
                &bundle,
                target,
                &json!({"tenant": {
                    "tenantIdentifier": tenant,
                    "tenantDisplayName": format!("{tenant} display"),
                }}),
            )
            .await;
            assert!(
                answer.status == 200 || answer.status == 400,
                "case {row:?}: an update answers either 200 or the modelled 400, got {} with body {}",
                answer.status,
                answer.body
            );

            // The invariant, after EVERY call: no ingestion is left orphaned.
            let ingestions = ingestion_pairs(&emulator, &bundle, &row).await;
            let authorizations = authorization_pairs(&emulator, &bundle, &row).await;
            assert_eq!(
                ingestions.len(),
                2,
                "case {row:?}: both ingestions must survive an update, got {ingestions:?}"
            );
            for pair in &ingestions {
                assert!(
                    authorizations.contains(pair),
                    "case {row:?}: the ingestion for {pair:?} must still have an app \
                     authorization, the bundle holds {authorizations:?}"
                );
            }
        }
    }

    // The authorizations that carry an ingestion never moved, so each is still
    // refused the delete its documented precondition forbids.
    for arn in &with_ingestion {
        let refused = delete_authorization(&emulator, &bundle, arn).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: the delete of {arn:?} must still be refused, got {} with body {}",
            refused.status, refused.body
        );
    }
}
