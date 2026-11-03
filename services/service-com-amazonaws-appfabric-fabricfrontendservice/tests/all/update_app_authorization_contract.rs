// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateAppAuthorization`, driven over the wire
//! (restJson1) through the same router the emulator boots.
//!
//! Each test states a RULE over a table of inputs rather than one recorded call:
//!
//! * a partial update writes ONLY the members the request supplies, and a call
//!   that supplies none changes nothing;
//! * an applied update invalidates the connect handshake, so the authorization
//!   goes back to `PendingConnect` and `updatedAt` does not go backwards, while
//!   the identifiers, `app`, `authType`, `createdAt` and the tags stay as they
//!   were;
//! * a supplied `credential` must be the kind the STORED `authType` uses, and no
//!   response ever reports credential material back;
//! * either modelled `Identifier` form names the same record, and an identifier
//!   the store does not hold (under this bundle) is not found;
//! * an app bundle holds at most one authorization per (application, tenant)
//!   pair, so an update that would move one onto a pair already taken is
//!   refused and writes nothing.
//!
//! The harness (the router, `Answer`, `encode`, `arn_of`, `uuid_of`) is the one
//! `app_bundle_ingestion_lifecycle` already builds.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_appfabric_fabricfrontendservice::state::service_state;

use crate::app_bundle_ingestion_lifecycle::{
    arn_of, encode, unknown_bundle_arn, unknown_uuid, uuid_of, Answer, Emulator,
};

/// The API key an `apiKey` authorization is created with. Distinct, recognisable
/// strings so a test can assert no response echoes the credential material back.
const API_KEY: &str = "secret-api-key-f3a1";

/// The API key an update supplies, so a test can tell a rotated credential from
/// the one the create call carried.
const ROTATED_API_KEY: &str = "secret-api-key-rotated-55c0";

/// The OAuth2 client id an `oauth2` authorization is created with.
const CLIENT_ID: &str = "secret-client-id-2b7e";

/// The OAuth2 client secret an `oauth2` authorization is created with.
const CLIENT_SECRET: &str = "secret-client-secret-ab19";

/// Every credential string the tests below send, none of which may appear in any
/// response (both `Credential` members are `@sensitive` and `AppAuthorization`
/// models no credential member).
const SECRETS: &[&str] = &[API_KEY, ROTATED_API_KEY, CLIENT_ID, CLIENT_SECRET];

/// The two values the model's `AuthType` enum carries, each with the credential
/// union member it requires — the pairing the model documents on
/// `UpdateAppAuthorizationRequest$credential` ("Specify credentials that match
/// the authorization type of the app authorization to update").
const AUTH_TYPES: &[(&str, &str)] = &[
    ("apiKey", "apiKeyCredential"),
    ("oauth2", "oauth2Credential"),
];

/// A `Credential` union body of one kind.
fn credential_of(kind: &str) -> Value {
    match kind {
        "apiKeyCredential" => json!({"apiKeyCredential": {"apiKey": ROTATED_API_KEY}}),
        "oauth2Credential" => json!({"oauth2Credential": {
            "clientId": CLIENT_ID,
            "clientSecret": CLIENT_SECRET,
        }}),
        other => panic!("not a modelled Credential member: {other:?}"),
    }
}

/// The credential kind one `authType` requires, from [`AUTH_TYPES`].
fn required_kind(auth_type: &str) -> &'static str {
    AUTH_TYPES
        .iter()
        .find(|(modelled, _)| *modelled == auth_type)
        .map(|(_, kind)| *kind)
        .unwrap_or_else(|| panic!("not a modelled AuthType: {auth_type:?}"))
}

/// A `tenant` body.
fn tenant_of(identifier: &str, display: &str) -> Value {
    json!({"tenantIdentifier": identifier, "tenantDisplayName": display})
}

/// An app bundle to hang authorizations off, as its ARN. One tag of its own, so
/// a tag reported for an authorization cannot have come from its parent.
async fn app_bundle(emulator: &Emulator) -> String {
    let created = emulator
        .create_app_bundle(json!({"tags": [{"key": "owner", "value": "the-bundle"}]}))
        .await;
    assert_eq!(
        created.status, 201,
        "the set-up app bundle must be created, got HTTP {} with body {}",
        created.status, created.body
    );
    arn_of(&created.body, "appBundle")
}

/// `POST /appbundles/{appBundleIdentifier}/appauthorizations`, as its ARN.
async fn authorization(
    emulator: &Emulator,
    bundle: &str,
    app: &str,
    auth_type: &str,
    tenant: &str,
) -> String {
    let credential = match required_kind(auth_type) {
        "apiKeyCredential" => json!({"apiKeyCredential": {"apiKey": API_KEY}}),
        _ => json!({"oauth2Credential": {"clientId": CLIENT_ID, "clientSecret": CLIENT_SECRET}}),
    };
    let created = emulator
        .call(
            "POST",
            &format!("/appbundles/{}/appauthorizations", encode(bundle)),
            Some(json!({
                "app": app,
                "authType": auth_type,
                "credential": credential,
                "tenant": tenant_of(tenant, &format!("{tenant} display")),
                "tags": [{"key": "env", "value": "prod"}],
            })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "the set-up app authorization must be created, got HTTP {} with body {}",
        created.status, created.body
    );
    created
        .body
        .get("appAuthorization")
        .and_then(|authorization| authorization.get("appAuthorizationArn"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected appAuthorization.appAuthorizationArn in {}", created.body))
        .to_string()
}

/// `POST …/appauthorizations/{id}/connect`, which is what moves an authorization
/// to `Connected` — the state the documented update behaviour is about.
async fn connect(emulator: &Emulator, bundle: &str, authorization: &str) {
    let connected = emulator
        .call(
            "POST",
            &format!(
                "/appbundles/{}/appauthorizations/{}/connect",
                encode(bundle),
                encode(authorization)
            ),
            Some(json!({"authRequest": {"redirectUri": "https://example.invalid/", "code": "code"}})),
        )
        .await;
    assert_eq!(
        connected.status, 200,
        "the set-up connect must answer HTTP 200, got {} with body {}",
        connected.status, connected.body
    );
}

/// `PATCH /appbundles/{appBundleIdentifier}/appauthorizations/{appAuthorizationIdentifier}`.
async fn update(emulator: &Emulator, bundle: &str, authorization: &str, body: &Value) -> Answer {
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
async fn read(emulator: &Emulator, bundle: &str, authorization: &str) -> Answer {
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

/// The `appAuthorization` member of a success body.
fn record(answer: &Answer, case: &str) -> Value {
    answer
        .body
        .get("appAuthorization")
        .unwrap_or_else(|| panic!("case {case:?}: expected appAuthorization in {}", answer.body))
        .clone()
}

/// The stored record, read back through `GetAppAuthorization`.
async fn stored(emulator: &Emulator, bundle: &str, authorization: &str, case: &str) -> Value {
    let answer = read(emulator, bundle, authorization).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: GetAppAuthorization must answer HTTP 200, got {} with body {}",
        answer.status, answer.body
    );
    record(&answer, case)
}

/// A string member of a record, or `""`.
fn member(record: &Value, name: &str) -> String {
    record
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A timestamp member, as the epoch seconds restJson1 renders it as.
fn stamp(record: &Value, name: &str, case: &str) -> f64 {
    record
        .get(name)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: expected a numeric {name} in {record}"))
}

/// The `tenantIdentifier` / `tenantDisplayName` pair of a record.
fn tenant_pair(record: &Value, case: &str) -> (String, String) {
    let tenant = record
        .get("tenant")
        .unwrap_or_else(|| panic!("case {case:?}: expected tenant in {record}"));
    (member(tenant, "tenantIdentifier"), member(tenant, "tenantDisplayName"))
}

/// Every tag `ListTagsForResource` reports for an ARN, as a sorted list of pairs.
async fn tags_of(emulator: &Emulator, arn: &str, case: &str) -> Vec<(String, String)> {
    let answer = emulator
        .call("GET", &format!("/tags/{}", encode(arn)), None)
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: ListTagsForResource must answer HTTP 200 for {arn:?}, got {} with body {}",
        answer.status, answer.body
    );
    let mut tags: Vec<(String, String)> = answer
        .body
        .get("tags")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected tags in {}", answer.body))
        .iter()
        .map(|tag| (member(tag, "key"), member(tag, "value")))
        .collect();
    tags.sort();
    tags
}

/// The `status` of every summary `ListAppAuthorizations` reports for a bundle,
/// keyed by `appAuthorizationArn`.
async fn listed(emulator: &Emulator, bundle: &str, case: &str) -> Vec<(String, String, String)> {
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
        .unwrap_or_else(|| panic!("case {case:?}: expected appAuthorizationSummaryList in {}", answer.body))
        .iter()
        .map(|summary| {
            let (tenant, _) = tenant_pair(summary, case);
            (
                member(summary, "appAuthorizationArn"),
                member(summary, "status"),
                tenant,
            )
        })
        .collect()
}

/// Refuse any response that carries credential material.
fn assert_no_secrets(answer: &Answer, case: &str, which: &str) {
    let rendered = answer.body.to_string();
    for secret in SECRETS {
        assert!(
            !rendered.contains(secret),
            "case {case:?}: {which} must not report the credential material {secret:?}, got {rendered}"
        );
    }
}

/// One row of the table in
/// [`an_update_applies_only_the_supplied_members_and_requires_reconnecting`]: the
/// case name, the PATCH body, and the `tenant` pair the record must carry
/// afterwards (`None` meaning "the one it already had").
type PartialUpdateCase = (
    &'static str,
    Value,
    Option<(&'static str, &'static str)>,
);

/// RULE: `UpdateAppAuthorization` applies ONLY the members the request supplies
/// (`credential` and `tenant` are both optional) and leaves every other member
/// of the record alone; an applied update puts the authorization back in
/// `PendingConnect` — the operation's documented behaviour for one that was
/// connected — with `updatedAt` no earlier than before, while a call that
/// supplies NEITHER member changes nothing at all. What the call returns is what
/// `GetAppAuthorization` and `ListAppAuthorizations` then report.
///
/// The table spans the combinations of supplied members (neither, each one, both)
/// over both of the model's `AuthType` values and applications the issue did not
/// cite.
#[tokio::test]
async fn an_update_applies_only_the_supplied_members_and_requires_reconnecting() {
    // (case, the PATCH body, the tenant pair expected afterwards or None for "unchanged")
    let cases: Vec<PartialUpdateCase> = vec![
        ("no member supplied", json!({}), None),
        (
            "tenant only",
            json!({"tenant": tenant_of("tenant-moved", "moved display")}),
            Some(("tenant-moved", "moved display")),
        ),
        ("credential only", json!({"credential": Value::Null}), None),
        (
            "both members",
            json!({"tenant": tenant_of("tenant-both", "both display"), "credential": Value::Null}),
            Some(("tenant-both", "both display")),
        ),
        (
            "tenant display name only",
            json!({"tenant": tenant_of("tenant-a", "renamed display")}),
            Some(("tenant-a", "renamed display")),
        ),
    ];

    for (app, auth_type) in [("OKTA", "apiKey"), ("ZOOM", "oauth2"), ("GOOGLE", "apiKey")] {
        for (label, patch, expected_tenant) in &cases {
            let case = format!("{app} {auth_type}: {label}");
            let emulator = Emulator::new();
            let bundle = app_bundle(&emulator).await;
            let arn = authorization(&emulator, &bundle, app, auth_type, "tenant-a").await;
            connect(&emulator, &bundle, &arn).await;
            let before = stored(&emulator, &bundle, &arn, &case).await;
            // The handshake of an `apiKey` authorization completes with the
            // connect call; an `oauth2` one has its outcome owed to the clock,
            // so the set-up record still awaits it.
            let setup_status = if auth_type == "apiKey" {
                "Connected"
            } else {
                "PendingConnect"
            };
            assert_eq!(
                member(&before, "status"),
                setup_status,
                "case {case:?}: the set-up authorization must read {setup_status}, got {before}"
            );

            // A `credential` of `null` in the table means "the kind this
            // authType requires", which only this row knows.
            let mut body = patch.clone();
            if body.get("credential").is_some_and(Value::is_null) {
                body.as_object_mut()
                    .expect("a JSON object request body")
                    .insert("credential".to_string(), credential_of(required_kind(auth_type)));
            }
            let supplied = body.get("credential").is_some() || body.get("tenant").is_some();

            let updated = update(&emulator, &bundle, &arn, &body).await;
            assert_eq!(
                updated.status, 200,
                "case {case:?}: expected HTTP 200, got {} with body {}",
                updated.status, updated.body
            );
            assert_no_secrets(&updated, &case, "the update response");
            let after = record(&updated, &case);

            if supplied {
                assert_eq!(
                    member(&after, "status"),
                    "PendingConnect",
                    "case {case:?}: an applied update must require reconnecting, got {after}"
                );
                assert!(
                    stamp(&after, "updatedAt", &case) >= stamp(&before, "updatedAt", &case),
                    "case {case:?}: updatedAt must not go backwards, got {} after {}",
                    stamp(&after, "updatedAt", &case),
                    stamp(&before, "updatedAt", &case)
                );
                assert!(
                    stamp(&after, "updatedAt", &case) >= stamp(&after, "createdAt", &case),
                    "case {case:?}: updatedAt must be at or after createdAt, got {after}"
                );
            } else {
                assert_eq!(
                    after, before,
                    "case {case:?}: a call supplying no member must change nothing, expected {before}, got {after}"
                );
            }

            // Only the supplied members moved: everything else belongs to the
            // resource, not to this request.
            for immutable in [
                "appAuthorizationArn",
                "appBundleArn",
                "app",
                "authType",
                "persona",
            ] {
                assert_eq!(
                    member(&after, immutable),
                    member(&before, immutable),
                    "case {case:?}: {immutable} must not change, got {after}"
                );
            }
            assert_eq!(
                stamp(&after, "createdAt", &case),
                stamp(&before, "createdAt", &case),
                "case {case:?}: createdAt must not change, got {after}"
            );
            let expected_pair = expected_tenant
                .map(|(identifier, display)| (identifier.to_string(), display.to_string()))
                .unwrap_or_else(|| tenant_pair(&before, &case));
            assert_eq!(
                tenant_pair(&after, &case),
                expected_pair,
                "case {case:?}: expected tenant {expected_pair:?}, got {after}"
            );

            // Round trip: the record the update reported is the record the store
            // holds, for either identifier form, and the listing agrees.
            for identifier in [arn.clone(), uuid_of(&arn)] {
                let read_back = read(&emulator, &bundle, &identifier).await;
                assert_eq!(
                    read_back.status, 200,
                    "case {case:?} read by {identifier:?}: expected HTTP 200, got {} with body {}",
                    read_back.status, read_back.body
                );
                assert_eq!(
                    record(&read_back, &case),
                    after,
                    "case {case:?} read by {identifier:?}: expected the stored record {after}, got {}",
                    read_back.body
                );
                assert_no_secrets(&read_back, &case, "the read response");
            }
            let summaries = listed(&emulator, &bundle, &case).await;
            assert_eq!(
                summaries,
                vec![(
                    arn.clone(),
                    member(&after, "status"),
                    tenant_pair(&after, &case).0
                )],
                "case {case:?}: the listing must report the updated record, got {summaries:?}"
            );

            // The authorization's own tags are untouched by an update that names
            // none, and so are its parent bundle's.
            let authorization_tags = tags_of(&emulator, &arn, &case).await;
            assert_eq!(
                authorization_tags,
                vec![("env".to_string(), "prod".to_string())],
                "case {case:?}: the authorization's tags must be untouched, got {authorization_tags:?}"
            );
            let bundle_tags = tags_of(&emulator, &bundle, &case).await;
            assert_eq!(
                bundle_tags,
                vec![("owner".to_string(), "the-bundle".to_string())],
                "case {case:?}: the bundle's tags must be untouched, got {bundle_tags:?}"
            );
        }
    }
}

/// RULE: a `credential` supplied to an update must be the union member the
/// STORED `authType` uses — the pairing the model documents on the member. A
/// matching credential is applied; a crossed one is refused with the modelled
/// `ValidationException` (HTTP 400) naming the `credential` member and writes
/// NOTHING, so the authorization keeps the state and tenant it had.
///
/// The table is every (stored `AuthType`, supplied credential kind) combination
/// the model allows — both matching and both crossed — over applications the
/// issue did not cite, each row also supplying a `tenant` so a refused call can
/// be seen to have written neither member.
#[tokio::test]
async fn a_supplied_credential_must_match_the_stored_auth_type() {
    for app in ["DROPBOX", "SMARTSHEET"] {
        for (auth_type, matching_kind) in AUTH_TYPES {
            for (_, supplied_kind) in AUTH_TYPES {
                let case = format!("{app} stored {auth_type} updated with {supplied_kind}");
                let emulator = Emulator::new();
                let bundle = app_bundle(&emulator).await;
                let arn = authorization(&emulator, &bundle, app, auth_type, "tenant-a").await;
                connect(&emulator, &bundle, &arn).await;
                let before = stored(&emulator, &bundle, &arn, &case).await;

                let body = json!({
                    "credential": credential_of(supplied_kind),
                    "tenant": tenant_of("tenant-next", "next display"),
                });
                let updated = update(&emulator, &bundle, &arn, &body).await;
                assert_no_secrets(&updated, &case, "the update response");

                if supplied_kind == matching_kind {
                    assert_eq!(
                        updated.status, 200,
                        "case {case:?}: a matching credential must be accepted, got HTTP {} with body {}",
                        updated.status, updated.body
                    );
                    let after = stored(&emulator, &bundle, &arn, &case).await;
                    assert_eq!(
                        member(&after, "status"),
                        "PendingConnect",
                        "case {case:?}: an applied update must require reconnecting, got {after}"
                    );
                    assert_eq!(
                        tenant_pair(&after, &case),
                        ("tenant-next".to_string(), "next display".to_string()),
                        "case {case:?}: expected the supplied tenant to persist, got {after}"
                    );
                } else {
                    assert_eq!(
                        updated.status, 400,
                        "case {case:?}: a crossed credential must be refused, got HTTP {} with body {}",
                        updated.status, updated.body
                    );
                    assert!(
                        updated.error_code().contains("ValidationException"),
                        "case {case:?}: expected a ValidationException, got {:?} with body {}",
                        updated.error_code(),
                        updated.body
                    );
                    assert!(
                        member(&updated.body, "message").contains("credential"),
                        "case {case:?}: the refusal must name the credential member, got {}",
                        updated.body
                    );
                    let after = stored(&emulator, &bundle, &arn, &case).await;
                    assert_eq!(
                        after, before,
                        "case {case:?}: a refused update must write nothing, expected {before}, got {after}"
                    );
                }
            }
        }
    }
}

/// RULE: an update names its target by either modelled `Identifier` form (ARN or
/// bare UUID) under the bundle that owns it; an identifier that names no record
/// of this store — an unknown bundle, an unknown authorization, or one that
/// belongs to ANOTHER bundle — is the modelled `ResourceNotFoundException`
/// (HTTP 404), and writes nothing.
///
/// The table spans both identifier forms for both path labels, and both ways an
/// identifier can fail to resolve (never minted, and minted under another
/// bundle).
#[tokio::test]
async fn either_identifier_form_updates_the_same_record_and_an_unknown_one_is_not_found() {
    let emulator = Emulator::new();
    // Two bundles AT ONCE, so a cross-bundle identifier can be tried: AppFabric
    // holds one bundle per Region, so each is created in its own.
    let first = emulator.fixture_bundle(0).await;
    let second = emulator.fixture_bundle(1).await;
    let arn = authorization(&emulator, &first, "JIRA", "apiKey", "tenant-a").await;
    let other = authorization(&emulator, &second, "JIRA", "apiKey", "tenant-a").await;

    // (case, bundle identifier, authorization identifier, expected status)
    let cases: Vec<(&str, String, String, u16)> = vec![
        ("both ARNs", first.clone(), arn.clone(), 200),
        ("both UUIDs", uuid_of(&first), uuid_of(&arn), 200),
        ("bundle ARN, authorization UUID", first.clone(), uuid_of(&arn), 200),
        ("bundle UUID, authorization ARN", uuid_of(&first), arn.clone(), 200),
        ("an unminted bundle UUID", unknown_uuid(), arn.clone(), 404),
        (
            "an unminted bundle ARN",
            unknown_bundle_arn(&unknown_uuid()),
            arn.clone(),
            404,
        ),
        ("an unminted authorization UUID", first.clone(), unknown_uuid(), 404),
        (
            "an authorization of another bundle",
            first.clone(),
            other.clone(),
            404,
        ),
        (
            "this authorization under another bundle",
            second.clone(),
            arn.clone(),
            404,
        ),
    ];

    for (case, bundle_identifier, authorization_identifier, expected) in cases {
        let before = stored(&emulator, &first, &arn, case).await;
        let body = json!({"tenant": tenant_of("tenant-a", &format!("display for {case}"))});
        let answer = update(&emulator, &bundle_identifier, &authorization_identifier, &body).await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} with body {}",
            answer.status, answer.body
        );

        let after = stored(&emulator, &first, &arn, case).await;
        if expected == 200 {
            assert_eq!(
                tenant_pair(&after, case).1,
                format!("display for {case}"),
                "case {case:?}: the named record must carry the update, got {after}"
            );
        } else {
            assert!(
                answer.error_code().contains("ResourceNotFoundException"),
                "case {case:?}: expected a ResourceNotFoundException, got {:?} with body {}",
                answer.error_code(),
                answer.body
            );
            assert_eq!(
                after, before,
                "case {case:?}: a refused update must write nothing, expected {before}, got {after}"
            );
        }
    }

    // The other bundle's authorization was never touched by any row above.
    let untouched = stored(&emulator, &second, &other, "the other bundle").await;
    assert_eq!(
        tenant_pair(&untouched, "the other bundle"),
        ("tenant-a".to_string(), "tenant-a display".to_string()),
        "the other bundle's authorization must be untouched, got {untouched}"
    );
}

/// Every value the model's `AppAuthorizationStatus` enum carries, from
/// `smithy_query(query="shape", shape="AppAuthorizationStatus")`: the state a
/// create leaves a record in, the one a connect reaches, and the two
/// post-handshake failures no implemented operation reaches.
const APP_AUTHORIZATION_STATUSES: &[&str] = &[
    "PendingConnect",
    "Connected",
    "ConnectionValidationFailed",
    "TokenAutoRotationFailed",
];

/// Put the stored app authorization into one `AppAuthorizationStatus`.
///
/// The status is written straight into the SAME store the handlers read
/// (`state::service_state` on the engine this emulator was mounted on), because
/// two of the four values above are states no implemented operation reaches, so
/// the set-up is the real record rather than a double of one.
fn set_status(
    engine: &engine::InMemoryEngine,
    bundle: &str,
    authorization: &str,
    status: &str,
    case: &str,
) {
    let store =
        service_state(engine).unwrap_or_else(|| panic!("case {case:?}: no appfabric store"));
    let mut store = store.lock();
    let record = store
        .resolve_bundle_mut(bundle)
        .unwrap_or_else(|| panic!("case {case:?}: bundle {bundle} is not stored"))
        .resolve_app_authorization_mut(authorization)
        .unwrap_or_else(|| panic!("case {case:?}: authorization {authorization} is not stored"));
    record.status = status.to_string();
}

/// RULE: an APPLIED update leaves the authorization awaiting the connect
/// handshake — `PendingConnect` — whichever status it held before, because the
/// credential or tenant every other status reports on is the one this call
/// replaced; a call that applies nothing keeps the status it found.
///
/// The table is every value of the model's `AppAuthorizationStatus` enum as the
/// starting state (including the two post-handshake failures the issue did not
/// cite), crossed with the members an update can supply.
#[tokio::test]
async fn an_applied_update_puts_every_status_back_in_pending_connect() {
    // (case, the PATCH body)
    let patches: Vec<(&str, Value)> = vec![
        ("no member", json!({})),
        (
            "a tenant",
            json!({"tenant": tenant_of("tenant-moved", "moved display")}),
        ),
        (
            "a credential",
            json!({"credential": {"apiKeyCredential": {"apiKey": ROTATED_API_KEY}}}),
        ),
    ];

    for status in APP_AUTHORIZATION_STATUSES {
        for (label, patch) in &patches {
            let case = format!("{status} updated with {label}");
            let engine = Arc::new(engine::InMemoryEngine::new());
            let emulator = Emulator::with_engine(&engine);
            let bundle = app_bundle(&emulator).await;
            let arn = authorization(&emulator, &bundle, "OKTA", "apiKey", "tenant-a").await;
            set_status(&engine, &bundle, &arn, status, &case);
            let before = stored(&emulator, &bundle, &arn, &case).await;
            assert_eq!(
                member(&before, "status"),
                *status,
                "case {case:?}: the set-up status must be stored, got {before}"
            );

            let answer = update(&emulator, &bundle, &arn, patch).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200, got {} with body {}",
                answer.status, answer.body
            );
            assert_no_secrets(&answer, &case, "the update response");

            let applied = patch.get("tenant").is_some() || patch.get("credential").is_some();
            let expected = if applied { "PendingConnect" } else { *status };
            let after = stored(&emulator, &bundle, &arn, &case).await;
            assert_eq!(
                member(&after, "status"),
                expected,
                "case {case:?}: expected status {expected:?}, got {after}"
            );
            assert_eq!(
                record(&answer, &case),
                after,
                "case {case:?}: the call must report the stored record {after}, got {}",
                answer.body
            );
        }
    }
}

/// tenant) pair, so an update that would move one onto a pair the bundle already
/// holds ANOTHER authorization for is refused with the modelled
/// `ValidationException` (HTTP 400) naming the `tenant` member and writes
/// nothing; re-stating an authorization's own tenant, or taking a tenant that is
/// only used by a DIFFERENT application, is accepted.
///
/// The table spans the three ways a supplied tenant can relate to the pairs the
/// bundle already holds: taken by another authorization of the same application,
/// held by this authorization itself, and held only for another application.
#[tokio::test]
async fn an_update_cannot_move_an_authorization_onto_a_pair_the_bundle_already_holds() {
    // (case, the application of the authorization updated, the tenant supplied,
    //  the expected status)
    let cases: Vec<(&str, &str, &str, u16)> = vec![
        ("a pair another authorization holds", "ASANA", "tenant-a", 400),
        ("its own pair", "ASANA", "tenant-b", 200),
        ("a free pair", "ASANA", "tenant-free", 200),
        ("a tenant only another application uses", "CISCO", "tenant-a", 200),
    ];

    for (case, app, tenant, expected) in cases {
        let emulator = Emulator::new();
        let bundle = app_bundle(&emulator).await;
        // The bundle holds (ASANA, tenant-a) already; the authorization under
        // test is this application's with tenant-b.
        let held = authorization(&emulator, &bundle, "ASANA", "apiKey", "tenant-a").await;
        let target = authorization(&emulator, &bundle, app, "apiKey", "tenant-b").await;
        let before = stored(&emulator, &bundle, &target, case).await;
        let held_before = stored(&emulator, &bundle, &held, case).await;

        let body = json!({"tenant": tenant_of(tenant, &format!("{tenant} display"))});
        let answer = update(&emulator, &bundle, &target, &body).await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} with body {}",
            answer.status, answer.body
        );

        let after = stored(&emulator, &bundle, &target, case).await;
        if expected == 200 {
            assert_eq!(
                tenant_pair(&after, case),
                (tenant.to_string(), format!("{tenant} display")),
                "case {case:?}: expected the supplied tenant to persist, got {after}"
            );
        } else {
            assert!(
                answer.error_code().contains("ValidationException"),
                "case {case:?}: expected a ValidationException, got {:?} with body {}",
                answer.error_code(),
                answer.body
            );
            assert!(
                member(&answer.body, "message").contains("tenant"),
                "case {case:?}: the refusal must name the tenant member, got {}",
                answer.body
            );
            assert_eq!(
                after, before,
                "case {case:?}: a refused update must write nothing, expected {before}, got {after}"
            );
        }
        let held_after = stored(&emulator, &bundle, &held, case).await;
        assert_eq!(
            held_after, held_before,
            "case {case:?}: the authorization that was not named must be untouched, expected {held_before}, got {held_after}"
        );
    }
}
