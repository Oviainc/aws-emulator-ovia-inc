// SPDX-License-Identifier: CC0-1.0

//! The ONE identifier-resolution rule every level of AppFabric's resource chain
//! shares, driven over the wire (restJson1) through the same router the emulator
//! boots.
//!
//! RULE: an `Identifier` member names a stored record in either of the two forms
//! the model's shape accepts (`^arn:.+$` or a bare UUID), and both name the same
//! record: an identifier in its ARN form resolves to the record whose own ARN it
//! is, any other identifier IS the record's bare key, and an identifier the
//! collection being searched does not hold resolves to nothing — so the read is
//! refused as the resource it targets.
//!
//! The table spans what the rule generalises over: every level that has an
//! `Identifier` of its own (app bundle, ingestion, ingestion destination, app
//! authorization), each form at every level of the path at once, and, for the
//! not-found half, an identifier of each form that names nothing, an ARN minted
//! for a resource of another KIND, and a child's ARN read under a sibling parent
//! that does not hold it.
//!
//! The harness (the router, `Answer`, `encode`, `arn_of`, `uuid_of`) is the one
//! `app_bundle_ingestion_lifecycle` already builds.

use serde_json::{json, Value};

use crate::app_bundle_ingestion_lifecycle::{
    arn_of, encode, unknown_bundle_arn, unknown_ingestion_arn, unknown_uuid, uuid_of, Answer,
    Emulator,
};

/// The region every call in this suite signs for, and therefore the region of
/// every ARN built by hand below.
const REGION: &str = "us-east-1";

/// The account segment of every ARN a generated emulator emits.
const ACCOUNT: &str = "000000000000";

/// The two forms the model's `Identifier` shape accepts for the same resource.
#[derive(Clone, Copy, Debug)]
enum Form {
    Arn,
    Uuid,
}

/// Both of them, so a table row can be generated for each.
const FORMS: [Form; 2] = [Form::Arn, Form::Uuid];

/// The resource `arn` names, written in `form`.
fn written(arn: &str, form: Form) -> String {
    match form {
        Form::Arn => arn.to_string(),
        Form::Uuid => uuid_of(arn),
    }
}

/// The `(resourceType, message)` pair of each level's reporting not-found
/// refusal, as this crate's state module holds it.
const BUNDLE_NOT_FOUND: (&str, &str) = ("AppBundle", "The app bundle ID was not found.");
const INGESTION_NOT_FOUND: (&str, &str) = ("Ingestion", "The specified ingestion does not exist");
const DESTINATION_NOT_FOUND: (&str, &str) = (
    "IngestionDestination",
    "The specified ingestion destination does not exist",
);
const AUTHORIZATION_NOT_FOUND: (&str, &str) = (
    "AppAuthorization",
    "The app authorization was not found.",
);

/// `GET /appbundles/{appBundleIdentifier}`.
fn bundle_uri(bundle: &str) -> String {
    format!("/appbundles/{}", encode(bundle))
}

/// `GET /appbundles/{appBundleIdentifier}/ingestions/{ingestionIdentifier}`.
fn ingestion_uri(bundle: &str, ingestion: &str) -> String {
    format!(
        "/appbundles/{}/ingestions/{}",
        encode(bundle),
        encode(ingestion)
    )
}

/// `GET /appbundles/{b}/ingestions/{i}/ingestiondestinations/{d}`.
fn destination_uri(bundle: &str, ingestion: &str, destination: &str) -> String {
    format!(
        "/appbundles/{}/ingestions/{}/ingestiondestinations/{}",
        encode(bundle),
        encode(ingestion),
        encode(destination)
    )
}

/// `GET /appbundles/{b}/appauthorizations/{a}`.
fn authorization_uri(bundle: &str, authorization: &str) -> String {
    format!(
        "/appbundles/{}/appauthorizations/{}",
        encode(bundle),
        encode(authorization)
    )
}

/// An ingestion-destination ARN in the form `CreateIngestionDestination` builds
/// (`appbundle/<uuid>/ingestion/<uuid>/ingestiondestination/<uuid>`).
fn destination_arn(bundle_uuid: &str, ingestion_uuid: &str, destination_uuid: &str) -> String {
    format!(
        "arn:aws:appfabric:{REGION}:{ACCOUNT}:appbundle/{bundle_uuid}\
         /ingestion/{ingestion_uuid}/ingestiondestination/{destination_uuid}"
    )
}

/// An app-authorization ARN in the form `CreateAppAuthorization` builds
/// (`appbundle/<uuid>/appauthorization/<uuid>`).
fn authorization_arn(bundle_uuid: &str, authorization_uuid: &str) -> String {
    format!(
        "arn:aws:appfabric:{REGION}:{ACCOUNT}:appbundle/{bundle_uuid}\
         /appauthorization/{authorization_uuid}"
    )
}

/// An ingestion under `bundle` for `tenant`, as its ARN.
async fn create_ingestion(emulator: &Emulator, bundle: &str, tenant: &str) -> String {
    let created = emulator
        .create_ingestion(
            bundle,
            json!({"app": "SLACK", "tenantId": tenant, "ingestionType": "auditLog"}),
        )
        .await;
    assert!(
        created.status < 300,
        "fixture ingestion for tenant {tenant:?}: expected success, got {} with body {}",
        created.status,
        created.body
    );
    arn_of(&created.body, "ingestion")
}

/// An ingestion destination under `ingestion` delivering to `bucket`, as its ARN.
async fn create_destination(
    emulator: &Emulator,
    bundle: &str,
    ingestion: &str,
    bucket: &str,
) -> String {
    let created = emulator
        .call(
            "POST",
            &format!(
                "/appbundles/{}/ingestions/{}/ingestiondestinations",
                encode(bundle),
                encode(ingestion)
            ),
            Some(json!({
                "processingConfiguration": {"auditLog": {"schema": "ocsf", "format": "json"}},
                "destinationConfiguration": {
                    "auditLog": {"destination": {"s3Bucket": {"bucketName": bucket}}}
                },
            })),
        )
        .await;
    assert!(
        created.status < 300,
        "fixture destination for bucket {bucket:?}: expected success, got {} with body {}",
        created.status,
        created.body
    );
    arn_of(&created.body, "ingestionDestination")
}

/// An app authorization under `bundle` for `(app, tenant)`, as its ARN.
async fn create_authorization(
    emulator: &Emulator,
    bundle: &str,
    app: &str,
    tenant: &str,
) -> String {
    let created = emulator
        .call(
            "POST",
            &format!("/appbundles/{}/appauthorizations", encode(bundle)),
            Some(json!({
                "app": app,
                "authType": "apiKey",
                "credential": {"apiKeyCredential": {"apiKey": "placeholder-api-key"}},
                "tenant": {"tenantIdentifier": tenant, "tenantDisplayName": tenant},
            })),
        )
        .await;
    assert!(
        created.status < 300,
        "fixture authorization for {app:?}/{tenant:?}: expected success, got {} with body {}",
        created.status,
        created.body
    );
    created
        .body
        .get("appAuthorization")
        .and_then(|authorization| authorization.get("appAuthorizationArn"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "expected appAuthorization.appAuthorizationArn in {}",
                created.body
            )
        })
        .to_string()
}

/// One resolving row: the case, the read's URI, and the response member whose
/// value must be the record the all-ARN read of that level answered.
type Resolves = (String, String, &'static str);

/// One refused row: the case, the read's URI, the refusal's `resourceType` and
/// `message`, and the identifier it must name in `resourceId`.
type Refused = (String, String, (&'static str, &'static str), String);

/// The response member each level's reporting read answers its record in.
const BUNDLE_MEMBER: &str = "appBundle";
const INGESTION_MEMBER: &str = "ingestion";
const DESTINATION_MEMBER: &str = "ingestionDestination";
const AUTHORIZATION_MEMBER: &str = "appAuthorization";

/// RULE (see the module docs): at EVERY level of the chain, either `Identifier`
/// form names the same stored record, and an identifier the level does not hold
/// names none.
#[tokio::test]
async fn either_identifier_form_names_the_same_record_at_every_level() {
    let emulator = Emulator::new();
    let bundle = arn_of(
        &emulator.create_app_bundle(json!({})).await.body,
        BUNDLE_MEMBER,
    );
    let ingestion = create_ingestion(&emulator, &bundle, "t-1").await;
    let destination = create_destination(&emulator, &bundle, &ingestion, "audit-logs-bucket").await;

    // A sibling ingestion with a destination of its own: a child ARN that
    // exists, but not in the collection the read searches.
    let sibling_ingestion = create_ingestion(&emulator, &bundle, "t-2").await;
    let sibling_destination =
        create_destination(&emulator, &bundle, &sibling_ingestion, "other-audit-bucket").await;

    let authorization = create_authorization(&emulator, &bundle, "ASANA", "t-asana").await;

    // What each level answers when every identifier on the path is an ARN: the
    // record every other form combination must answer too.
    let canonical: Vec<(&str, Value)> = vec![
        (
            BUNDLE_MEMBER,
            emulator.call("GET", &bundle_uri(&bundle), None).await.body,
        ),
        (
            INGESTION_MEMBER,
            emulator
                .call("GET", &ingestion_uri(&bundle, &ingestion), None)
                .await
                .body,
        ),
        (
            DESTINATION_MEMBER,
            emulator
                .call(
                    "GET",
                    &destination_uri(&bundle, &ingestion, &destination),
                    None,
                )
                .await
                .body,
        ),
        (
            AUTHORIZATION_MEMBER,
            emulator
                .call("GET", &authorization_uri(&bundle, &authorization), None)
                .await
                .body,
        ),
    ];
    for (member, body) in &canonical {
        assert!(
            body.get(member).is_some(),
            "the all-ARN read of {member:?} must answer its record, got {body}"
        );
    }
    let expected = |member: &str| -> Value {
        canonical
            .iter()
            .find(|(name, _)| *name == member)
            .and_then(|(_, body)| body.get(member).cloned())
            .unwrap_or_else(|| panic!("no canonical record for {member:?}"))
    };

    // Every form of every identifier on every level's path.
    let mut resolving: Vec<Resolves> = Vec::new();
    for bundle_form in FORMS {
        let bundle_id = written(&bundle, bundle_form);
        resolving.push((
            format!("GetAppBundle by {bundle_form:?}"),
            bundle_uri(&bundle_id),
            BUNDLE_MEMBER,
        ));
        for ingestion_form in FORMS {
            let ingestion_id = written(&ingestion, ingestion_form);
            resolving.push((
                format!("GetIngestion by bundle {bundle_form:?}, ingestion {ingestion_form:?}"),
                ingestion_uri(&bundle_id, &ingestion_id),
                INGESTION_MEMBER,
            ));
            for destination_form in FORMS {
                let destination_id = written(&destination, destination_form);
                resolving.push((
                    format!(
                        "GetIngestionDestination by bundle {bundle_form:?}, \
                         ingestion {ingestion_form:?}, destination {destination_form:?}"
                    ),
                    destination_uri(&bundle_id, &ingestion_id, &destination_id),
                    DESTINATION_MEMBER,
                ));
            }
        }
        for authorization_form in FORMS {
            let authorization_id = written(&authorization, authorization_form);
            resolving.push((
                format!(
                    "GetAppAuthorization by bundle {bundle_form:?}, \
                     authorization {authorization_form:?}"
                ),
                authorization_uri(&bundle_id, &authorization_id),
                AUTHORIZATION_MEMBER,
            ));
        }
    }

    for (case, uri, member) in resolving {
        let answer = emulator.call("GET", &uri, None).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body.get(member),
            Some(&expected(member)),
            "case {case:?}: expected the record {}, got {}",
            expected(member),
            answer.body
        );
    }

    // The other half of the same rule: an identifier the searched collection
    // does not hold resolves to nothing, in either form.
    let unknown = unknown_uuid();
    let bundle_uuid = uuid_of(&bundle);
    let ingestion_uuid = uuid_of(&ingestion);
    let refused: Vec<Refused> = vec![
        (
            "GetAppBundle with a bundle uuid nothing holds".to_string(),
            bundle_uri(&unknown),
            BUNDLE_NOT_FOUND,
            unknown.clone(),
        ),
        (
            "GetAppBundle with a bundle arn nothing holds".to_string(),
            bundle_uri(&unknown_bundle_arn(&unknown)),
            BUNDLE_NOT_FOUND,
            unknown_bundle_arn(&unknown),
        ),
        (
            "GetAppBundle with an arn of another kind".to_string(),
            bundle_uri(&ingestion),
            BUNDLE_NOT_FOUND,
            ingestion.clone(),
        ),
        (
            "GetIngestion with an ingestion uuid this bundle does not hold".to_string(),
            ingestion_uri(&bundle, &unknown),
            INGESTION_NOT_FOUND,
            unknown.clone(),
        ),
        (
            "GetIngestion with an ingestion arn this bundle does not hold".to_string(),
            ingestion_uri(&bundle, &unknown_ingestion_arn(&bundle_uuid, &unknown)),
            INGESTION_NOT_FOUND,
            unknown_ingestion_arn(&bundle_uuid, &unknown),
        ),
        (
            "GetIngestion with an arn of another kind".to_string(),
            ingestion_uri(&bundle, &destination),
            INGESTION_NOT_FOUND,
            destination.clone(),
        ),
        (
            "GetIngestionDestination with a destination uuid this ingestion does not hold"
                .to_string(),
            destination_uri(&bundle, &ingestion, &unknown),
            DESTINATION_NOT_FOUND,
            unknown.clone(),
        ),
        (
            "GetIngestionDestination with a destination arn this ingestion does not hold"
                .to_string(),
            destination_uri(
                &bundle,
                &ingestion,
                &destination_arn(&bundle_uuid, &ingestion_uuid, &unknown),
            ),
            DESTINATION_NOT_FOUND,
            destination_arn(&bundle_uuid, &ingestion_uuid, &unknown),
        ),
        (
            "GetIngestionDestination with a sibling ingestion's destination arn".to_string(),
            destination_uri(&bundle, &ingestion, &sibling_destination),
            DESTINATION_NOT_FOUND,
            sibling_destination.clone(),
        ),
        (
            "GetIngestionDestination with a sibling ingestion's destination uuid".to_string(),
            destination_uri(&bundle, &ingestion, &uuid_of(&sibling_destination)),
            DESTINATION_NOT_FOUND,
            uuid_of(&sibling_destination),
        ),
        (
            "GetAppAuthorization with an authorization uuid this bundle does not hold".to_string(),
            authorization_uri(&bundle, &unknown),
            AUTHORIZATION_NOT_FOUND,
            unknown.clone(),
        ),
        (
            "GetAppAuthorization with an authorization arn this bundle does not hold".to_string(),
            authorization_uri(&bundle, &authorization_arn(&bundle_uuid, &unknown)),
            AUTHORIZATION_NOT_FOUND,
            authorization_arn(&bundle_uuid, &unknown),
        ),
        (
            "GetAppAuthorization with an arn of another kind".to_string(),
            authorization_uri(&bundle, &ingestion),
            AUTHORIZATION_NOT_FOUND,
            ingestion.clone(),
        ),
    ];

    for (case, uri, (resource_type, message), resource_id) in refused {
        let answer: Answer = emulator.call("GET", &uri, None).await;
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
            Some(resource_type),
            "case {case:?}: expected the refusal to be about {resource_type}, got {}",
            answer.body
        );
        assert_eq!(
            answer.body.get("resourceId").and_then(Value::as_str),
            Some(resource_id.as_str()),
            "case {case:?}: expected {resource_id:?} in resourceId, got {}",
            answer.body
        );
        assert_eq!(
            answer.body.get("message").and_then(Value::as_str),
            Some(message),
            "case {case:?}: expected the message {message:?}, got {}",
            answer.body
        );
    }

    // None of those reads changed what the store holds: each level still reports
    // exactly the record the all-ARN read answered before them.
    for (member, body) in &canonical {
        let uri = match *member {
            BUNDLE_MEMBER => bundle_uri(&bundle),
            INGESTION_MEMBER => ingestion_uri(&bundle, &ingestion),
            DESTINATION_MEMBER => destination_uri(&bundle, &ingestion, &destination),
            _ => authorization_uri(&bundle, &authorization),
        };
        let reread = emulator.call("GET", &uri, None).await;
        assert_eq!(
            reread.body.get(member),
            body.get(member),
            "re-reading {member:?}: expected the stored record {}, got {}",
            body,
            reread.body
        );
    }
}
