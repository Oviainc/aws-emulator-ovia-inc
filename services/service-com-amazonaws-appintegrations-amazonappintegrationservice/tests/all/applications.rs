// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the AppIntegrations application lifecycle
//! (`CreateApplication` + `GetApplication`).
//!
//! Every assertion goes over the wire through the same [`MultiServiceRouter`] the
//! production emulator mounts (see `harness.rs`), and only the HTTP status, the
//! `x-amzn-errortype` header and the JSON body are inspected. Nothing reaches into the
//! service's state struct, so a response that is not computed from persisted state
//! cannot pass.
//!
//! The suite is a table: each row is one input along an axis the rules under test
//! generalise over (which optional members are present, name and namespace form and
//! length, list sizes, tag count, application type, timeout bound, identifier form,
//! signing region), and the single recorded create from the issue is only one row.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use super::harness::{encode_label, Emulator, Reply, REGION};

/// The values `ApplicationType` accepts, from the model's enum
/// (`smithy_query(query="shape", shape="ApplicationType")`, mirrored in the crate's
/// `ops::types::APPLICATION_TYPE_VALUES`). The issue cited none of them.
const APPLICATION_TYPES: &[&str] = &["A2A_SERVER", "MCP_SERVER", "SERVICE", "STANDARD"];

/// `InitializationTimeout` is `range 1..600000` (model: `CreateApplication` input).
const INITIALIZATION_TIMEOUT_MIN: i64 = 1;
const INITIALIZATION_TIMEOUT_MAX: i64 = 600_000;

/// `POST /applications` with `body`, signed for `region`.
async fn create(emulator: &Emulator, region: &str, body: Value) -> Reply {
    emulator
        .send("POST", "/applications", region, Some(body))
        .await
}

/// `GET /applications/{Arn}`: the identifier travels in a `@httpLabel`, so its `:` and
/// `/` are percent-encoded.
async fn get(emulator: &Emulator, region: &str, identifier: &str) -> Reply {
    emulator
        .send(
            "GET",
            &format!("/applications/{}", encode_label(identifier)),
            region,
            None,
        )
        .await
}

/// The ARN AWS documents for this resource:
/// `arn:aws:app-integrations:<region>:<account>:application/<id>`. Written out here
/// rather than taken from the handler's helper, so the test pins the format.
fn expected_arn(region: &str, id: &str) -> String {
    format!(
        "arn:aws:app-integrations:{region}:{account}:application/{id}",
        account = engine::EMULATOR_ACCOUNT_ID
    )
}

/// Whether `value` is a `UUID` as the model types `Id`:
/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`.
fn is_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => *byte == b'-',
            _ => matches!(*byte, b'0'..=b'9' | b'a'..=b'f'),
        })
}

/// One row of the round-trip table: the label that names it in an assertion, and the
/// `CreateApplication` body it sends. Every member of that body is what a later read
/// must report, so the table needs no second copy of the expectations.
#[derive(Debug, Clone)]
struct Case {
    label: &'static str,
    body: Value,
}

fn case(label: &'static str, body: Value) -> Case {
    Case { label, body }
}

/// The table every round-trip assertion runs over. Rows differ in which optional
/// members are present, in name and namespace form and length (`Name` is
/// `len 1..255 pattern ^[a-zA-Z0-9\/\._ \-]+$`, `Namespace` is `len 1..211 pattern
/// ^[a-zA-Z0-9\/\._\-]+$`, `Description` is `len 0..1000`, so both published bounds
/// appear), in list sizes, in tag count and in application type.
fn round_trip_cases() -> Vec<Case> {
    vec![
        // The shape the issue's recorded create carries: one row of many.
        case(
            "issue's recorded case: required members only",
            json!({
                "Name": "demo-application",
                "Namespace": "demo-namespace",
                "ApplicationSourceConfig": {
                    "ExternalUrlConfig": { "AccessUrl": "https://example.com/app" }
                },
            }),
        ),
        case(
            "shortest legal name and namespace, empty description, one tag, no permissions",
            json!({
                "Name": "a",
                "Namespace": "n",
                "Description": "",
                "ApplicationSourceConfig": {
                    "ExternalUrlConfig": {
                        "AccessUrl": "https://example.com/",
                        "ApprovedOrigins": ["https://only.example.com"],
                    }
                },
                "Permissions": [],
                "Tags": { "owner": "team-a" },
                "IsService": false,
            }),
        ),
        case(
            "every optional member present, name with the space and slash its pattern allows",
            json!({
                "Name": "Mixed Case/App_name-9.v2",
                "Namespace": "Mixed.Case_namespace-9/sub",
                "Description": "an application with every member set",
                "ApplicationSourceConfig": {
                    "ExternalUrlConfig": {
                        "AccessUrl": "https://example.com/full",
                        "ApprovedOrigins": ["https://a.example.com", "https://b.example.com"],
                    }
                },
                "Subscriptions": [
                    { "Event": "Contact::Created", "Description": "new contacts" },
                    { "Event": "Contact::Updated*" },
                ],
                "Publications": [
                    { "Event": "App::Ready", "Schema": "{\"type\":\"object\"}", "Description": "ready" },
                ],
                "Permissions": ["User.Details.View", "Contact.Details.View", "Contact.CustomerName.View"],
                "Tags": { "a": "1", "b": "2", "c": "" },
                "IsService": true,
                "InitializationTimeout": 30_000,
                "ApplicationConfig": { "ContactHandling": { "Scope": "CROSS_CONTACTS" } },
                "IframeConfig": {
                    "Allow": ["microphone", "camera"],
                    "Sandbox": ["allow-forms", "allow-scripts"],
                },
                "ApplicationType": "MCP_SERVER",
                "AuthConfig": {
                    "AuthType": "CONNECT_JWT",
                    "AudienceOverride": "https://audience.example.com",
                },
            }),
        ),
        case(
            "longest legal name, namespace and description, timeout at its upper bound",
            json!({
                "Name": "n".repeat(255),
                "Namespace": "s".repeat(211),
                "Description": "d".repeat(1000),
                "ApplicationSourceConfig": {
                    "ExternalUrlConfig": { "AccessUrl": "https://example.com/long" }
                },
                "Permissions": (0..150).map(|index| format!("Permission.{index}.View")).collect::<Vec<_>>(),
                "InitializationTimeout": INITIALIZATION_TIMEOUT_MAX,
                "ApplicationType": "SERVICE",
            }),
        ),
    ]
}

/// Every member of a `CreateApplication` body, as `GetApplication` must report it.
/// `ClientToken` names the request, not the resource, so it is not a reported member.
///
/// A member the create did NOT send is reported whenever the service fills it in: the
/// three collections and the tag map read back empty, `IframeConfig` reads back as the
/// empty structure, `ApplicationType` as the type the service assigns, and `IsService`
/// as the model's default. Only the members the service has no default for stay absent.
fn assert_reports_the_created_members(label: &str, reply: &Reply, sent: &Value) {
    assert_eq!(
        reply.status, 200,
        "case {label:?}: expected HTTP 200, got {} with body {}",
        reply.status, reply.body
    );
    let sent = sent.as_object().expect("the create body is an object");
    for (member, value) in sent {
        if member == "ClientToken" {
            continue;
        }
        assert_eq!(
            reply.field(member),
            Some(value),
            "case {label:?}: expected member {member} to be reported as {value}, got {:?}",
            reply.field(member)
        );
    }
    let sent_is_service = sent
        .get("IsService")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    // What each server-defaulted member reads back as when the create named none of
    // them: an empty collection, an empty structure, or the assigned application type.
    let defaults: Vec<(&str, Value)> = vec![
        ("Subscriptions", json!([])),
        ("Publications", json!([])),
        ("Permissions", json!([])),
        ("Tags", json!({})),
        ("IframeConfig", json!({})),
        (
            "ApplicationType",
            json!(if sent_is_service { "SERVICE" } else { "STANDARD" }),
        ),
    ];
    for (member, default) in defaults {
        if !sent.contains_key(member) {
            assert_eq!(
                reply.field(member),
                Some(&default),
                "case {label:?}: expected {member} to be reported as the service-assigned \
                 default {default} for a create that sent none, got {:?}",
                reply.field(member)
            );
        }
    }
    // A member the service assigns no default to is absent until a caller sets it.
    for member in [
        "Description",
        "InitializationTimeout",
        "ApplicationConfig",
        "AuthConfig",
    ] {
        if !sent.contains_key(member) {
            assert_eq!(
                reply.field(member),
                None,
                "case {label:?}: expected no {member} member for a create that sent none, got {:?}",
                reply.field(member)
            );
        }
    }
    let reported_is_service = reply
        .field("IsService")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    assert_eq!(
        reported_is_service, sent_is_service,
        "case {label:?}: expected IsService {sent_is_service}, got {reported_is_service}"
    );
}

#[tokio::test]
async fn a_created_application_is_reported_by_get_exactly_as_stored() {
    let emulator = Emulator::new();

    for Case { label, body } in round_trip_cases() {
        let created = create(&emulator, REGION, body.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected create to answer HTTP 200, got {} with body {}",
            created.status, created.body
        );
        let id = created.string("Id").unwrap_or_default();
        assert!(
            is_uuid(&id),
            "case {label:?}: expected create to mint a UUID Id, got {id:?}"
        );
        assert_eq!(
            created.string("Arn"),
            Some(expected_arn(REGION, &id)),
            "case {label:?}: expected create to mint the documented ARN, got {:?}",
            created.string("Arn")
        );

        // Both identifier forms the model's `ArnOrUUID` allows name this application,
        // and a trailing qualifier does not change which one.
        let identifiers = [
            ("bare UUID", id.clone()),
            ("full ARN", expected_arn(REGION, &id)),
            ("UUID with a qualifier", format!("{id}:1")),
        ];
        for (form, identifier) in identifiers {
            let case_label = format!("{label} / {form}");
            let fetched = get(&emulator, REGION, &identifier).await;
            assert_reports_the_created_members(&case_label, &fetched, &body);
            assert_eq!(
                fetched.string("Id"),
                Some(id.clone()),
                "case {case_label:?}: expected the Id create minted, got {:?}",
                fetched.string("Id")
            );
            assert_eq!(
                fetched.string("Arn"),
                created.string("Arn"),
                "case {case_label:?}: expected the ARN create minted, got {:?}",
                fetched.string("Arn")
            );
            assert!(
                fetched.field("CreatedTime").is_some(),
                "case {case_label:?}: expected a CreatedTime, got body {}",
                fetched.body
            );
            assert_eq!(
                fetched.field("LastModifiedTime"),
                fetched.field("CreatedTime"),
                "case {case_label:?}: an application nothing has updated must report \
                 LastModifiedTime equal to CreatedTime, got {:?} and {:?}",
                fetched.field("LastModifiedTime"),
                fetched.field("CreatedTime")
            );

            // Reading twice must report the same thing: a read changes nothing.
            let again = get(&emulator, REGION, &identifier).await;
            assert_eq!(
                again.body, fetched.body,
                "case {case_label:?}: a second read reported {} after the first reported {}",
                again.body, fetched.body
            );
        }
    }
}

#[tokio::test]
async fn each_create_mints_its_own_id_and_arn() {
    let emulator = Emulator::new();
    let mut seen: BTreeMap<String, String> = BTreeMap::new();

    // Requests that differ in nothing but the namespace: the id is minted, so it is
    // never derived from what the client sent.
    for namespace in ["ns-1", "ns-2", "ns-3", "ns-4"] {
        let created = create(
            &emulator,
            REGION,
            json!({
                "Name": "same-name",
                "Namespace": namespace,
                "ApplicationSourceConfig": {
                    "ExternalUrlConfig": { "AccessUrl": "https://example.com/app" }
                },
            }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {namespace:?}: expected HTTP 200, got {} with body {}",
            created.status, created.body
        );
        let id = created.string("Id").unwrap_or_default();
        assert!(
            is_uuid(&id),
            "case {namespace:?}: expected a UUID Id, got {id:?}"
        );
        if let Some(previous) = seen.insert(id.clone(), namespace.to_string()) {
            panic!("case {namespace:?}: id {id} was already minted for {previous}");
        }
        let fetched = get(&emulator, REGION, &id).await;
        assert_eq!(
            fetched.string("Namespace").as_deref(),
            Some(namespace),
            "case {namespace:?}: expected the stored Namespace, got {:?}",
            fetched.string("Namespace")
        );
    }
}

#[tokio::test]
async fn the_arn_carries_the_region_the_request_was_signed_for() {
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let emulator = Emulator::new();
        let created = create(
            &emulator,
            region,
            json!({
                "Name": "regional-application",
                "Namespace": "regional-namespace",
                "ApplicationSourceConfig": {
                    "ExternalUrlConfig": { "AccessUrl": "https://example.com/app" }
                },
            }),
        )
        .await;
        let id = created.string("Id").unwrap_or_default();
        assert_eq!(
            created.string("Arn"),
            Some(expected_arn(region, &id)),
            "case {region:?}: expected the ARN to name the signing region, got {:?}",
            created.string("Arn")
        );

        let fetched = get(&emulator, region, &id).await;
        assert_eq!(
            fetched.string("Arn"),
            created.string("Arn"),
            "case {region:?}: Get must report the ARN create stored, got {:?}",
            fetched.string("Arn")
        );
    }
}

#[tokio::test]
async fn every_modelled_application_type_round_trips() {
    let emulator = Emulator::new();

    for application_type in APPLICATION_TYPES {
        let body = json!({
            "Name": format!("typed-{application_type}"),
            "Namespace": format!("typed-{application_type}"),
            "ApplicationSourceConfig": {
                "ExternalUrlConfig": { "AccessUrl": "https://example.com/typed" }
            },
            "ApplicationType": application_type,
        });
        let created = create(&emulator, REGION, body.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {application_type:?}: expected HTTP 200, got {} with body {}",
            created.status, created.body
        );
        let id = created.string("Id").unwrap_or_default();
        let fetched = get(&emulator, REGION, &id).await;
        assert_reports_the_created_members(application_type, &fetched, &body);
    }

    // A value outside the modelled enum is refused, and nothing is persisted for it.
    let refused = create(
        &emulator,
        REGION,
        json!({
            "Name": "untyped",
            "Namespace": "untyped",
            "ApplicationSourceConfig": {
                "ExternalUrlConfig": { "AccessUrl": "https://example.com/untyped" }
            },
            "ApplicationType": "NOT_A_TYPE",
        }),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "case \"ApplicationType=NOT_A_TYPE\": expected HTTP 400, got {} with body {}",
        refused.status, refused.body
    );
}

#[tokio::test]
async fn initialization_timeout_accepts_each_published_bound_and_refuses_one_past_it() {
    // `range 1..600000`: both bounds are legal, and one step past either is not.
    for (label, timeout, expected_status) in [
        ("one below the minimum", INITIALIZATION_TIMEOUT_MIN - 1, 400),
        ("the minimum", INITIALIZATION_TIMEOUT_MIN, 200),
        ("the maximum", INITIALIZATION_TIMEOUT_MAX, 200),
        ("one above the maximum", INITIALIZATION_TIMEOUT_MAX + 1, 400),
    ] {
        let emulator = Emulator::new();
        let body = json!({
            "Name": "bounded-application",
            "Namespace": "bounded-namespace",
            "ApplicationSourceConfig": {
                "ExternalUrlConfig": { "AccessUrl": "https://example.com/bounded" }
            },
            "InitializationTimeout": timeout,
        });
        let created = create(&emulator, REGION, body.clone()).await;
        assert_eq!(
            created.status, expected_status,
            "case {label:?}: expected HTTP {expected_status} for InitializationTimeout \
             {timeout}, got {} with body {}",
            created.status, created.body
        );

        if expected_status == 200 {
            let id = created.string("Id").unwrap_or_default();
            let fetched = get(&emulator, REGION, &id).await;
            assert_reports_the_created_members(label, &fetched, &body);
        } else {
            // A refused create persists nothing, so the same (Namespace, Name) is still
            // free: were the refused application stored, this create would be refused as
            // a name already in use.
            let mut retried = body.clone();
            retried["InitializationTimeout"] = json!(INITIALIZATION_TIMEOUT_MIN);
            let accepted = create(&emulator, REGION, retried).await;
            assert_eq!(
                accepted.status, 200,
                "case {label:?}: a refused create must persist nothing, but a later \
                 create of the same Namespace and Name answered {} with body {}",
                accepted.status, accepted.body
            );
        }
    }
}

/// How AppIntegrations refuses a create whose `Name` is already in use in the
/// `Namespace` it names: HTTP 400 `InvalidRequestException` with a sentence that is
/// entirely fixed text — it names neither the application nor the namespace, so no two
/// rows of the table below can tell their refusals apart.
const NAME_ALREADY_IN_USE: &str = "Application name is invalid. Name already in use.";

/// Every assertion about a refused duplicate: the status, the error code and the whole
/// message.
fn assert_refused_as_name_in_use(label: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 400,
        "case {label:?}: expected HTTP 400 for a duplicate (Namespace, Name), got {} / {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type, "InvalidRequestException",
        "case {label:?}: expected InvalidRequestException, got {:?}",
        reply.error_type
    );
    assert_eq!(
        reply.message().as_deref(),
        Some(NAME_ALREADY_IN_USE),
        "case {label:?}: expected {NAME_ALREADY_IN_USE:?}, got {:?}",
        reply.message()
    );
}

#[tokio::test]
async fn a_second_create_of_the_same_namespace_and_name_is_refused_as_a_name_in_use() {
    for Case { label, body } in round_trip_cases() {
        let emulator = Emulator::new();
        let first = create(&emulator, REGION, body.clone()).await;
        assert_eq!(
            first.status, 200,
            "case {label:?}: expected the first create to succeed, got {} / {}",
            first.status, first.body
        );

        // Same (Namespace, Name), different everything else: the pair is the identity.
        let mut duplicate = body.clone();
        duplicate["Description"] = json!("a different description");
        duplicate["ApplicationSourceConfig"] =
            json!({ "ExternalUrlConfig": { "AccessUrl": "https://example.com/other" } });
        let second = create(&emulator, REGION, duplicate).await;
        assert_refused_as_name_in_use(label, &second);

        // And the refused create changed nothing.
        let id = first.string("Id").unwrap_or_default();
        let fetched = get(&emulator, REGION, &id).await;
        assert_reports_the_created_members(label, &fetched, &body);

        // Only the pair collides: the same name under another namespace, and another
        // name under the same namespace, are both separate applications.
        let mut other_namespace = body.clone();
        other_namespace["Namespace"] = json!("a-free-namespace");
        let mut other_name = body.clone();
        other_name["Name"] = json!("a free name");
        for (form, variant) in [
            ("same name, other namespace", other_namespace),
            ("same namespace, other name", other_name),
        ] {
            let accepted = create(&emulator, REGION, variant).await;
            assert_eq!(
                accepted.status, 200,
                "case {label:?} / {form:?}: expected HTTP 200, got {} / {}",
                accepted.status, accepted.body
            );
            assert_ne!(
                accepted.string("Id"),
                first.string("Id"),
                "case {label:?} / {form:?}: expected a separate application, got the \
                 id of the first one ({:?})",
                accepted.string("Id")
            );
        }
    }
}

#[tokio::test]
async fn a_create_repeated_with_its_client_token_reports_the_id_it_already_minted() {
    // `ClientToken` is `@idempotencyToken`: replaying the call must neither create a
    // second application nor fail, whatever the token's text (`len 1..2048`).
    for token in ["t", "11111111-2222-3333-4444-555555555555", &"z".repeat(2048)] {
        let emulator = Emulator::new();
        let body = json!({
            "Name": "idempotent-application",
            "Namespace": "idempotent-namespace",
            "ApplicationSourceConfig": {
                "ExternalUrlConfig": { "AccessUrl": "https://example.com/app" }
            },
            "ClientToken": token,
        });

        let first = create(&emulator, REGION, body.clone()).await;
        assert_eq!(
            first.status, 200,
            "case {token:?}: expected the first create to succeed, got {} / {}",
            first.status, first.body
        );

        let replay = create(&emulator, REGION, body.clone()).await;
        assert_eq!(
            replay.status, 200,
            "case {token:?}: expected a replayed create to succeed, got {} / {}",
            replay.status, replay.body
        );
        assert_eq!(
            replay.string("Id"),
            first.string("Id"),
            "case {token:?}: expected the Id of the first create, got {:?}",
            replay.string("Id")
        );
        assert_eq!(
            replay.string("Arn"),
            first.string("Arn"),
            "case {token:?}: expected the ARN of the first create, got {:?}",
            replay.string("Arn")
        );

        let fetched = get(&emulator, REGION, &first.string("Id").unwrap_or_default()).await;
        assert_reports_the_created_members(token, &fetched, &body);
    }

    // Without a token the same second create is a duplicate, not an idempotent replay.
    let emulator = Emulator::new();
    let body = json!({
        "Name": "untokened-application",
        "Namespace": "untokened-namespace",
        "ApplicationSourceConfig": {
            "ExternalUrlConfig": { "AccessUrl": "https://example.com/app" }
        },
    });
    let first = create(&emulator, REGION, body.clone()).await;
    assert_eq!(
        first.status, 200,
        "case \"no ClientToken\": expected the first create to succeed, got {} / {}",
        first.status, first.body
    );
    let second = create(&emulator, REGION, body).await;
    assert_refused_as_name_in_use("no ClientToken", &second);
}

#[tokio::test]
async fn get_of_an_identifier_no_application_carries_is_not_found() {
    let emulator = Emulator::new();
    // One live application, so the refusals below are about the identifier and not
    // about an empty store.
    let created = create(
        &emulator,
        REGION,
        json!({
            "Name": "live-application",
            "Namespace": "live-namespace",
            "ApplicationSourceConfig": {
                "ExternalUrlConfig": { "AccessUrl": "https://example.com/app" }
            },
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "expected the create to succeed, got {} / {}",
        created.status, created.body
    );

    let unknown_id = "00000000-0000-0000-0000-000000000000";
    for (label, identifier) in [
        ("a UUID no application carries", unknown_id.to_string()),
        ("the ARN of that UUID", expected_arn(REGION, unknown_id)),
        (
            "an ARN in another region",
            expected_arn("eu-west-1", unknown_id),
        ),
    ] {
        let reply = get(&emulator, REGION, &identifier).await;
        assert_eq!(
            reply.status, 404,
            "case {label:?}: expected HTTP 404, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type, "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {:?}",
            reply.error_type
        );
        assert_eq!(
            reply.message(),
            Some(format!(
                "Could not find Application with identifier: {identifier}."
            )),
            "case {label:?}: expected the templated not-found message, got body {}",
            reply.body
        );
    }

    // The live application is still readable: a refused read changed nothing.
    let id = created.string("Id").unwrap_or_default();
    let fetched = get(&emulator, REGION, &id).await;
    assert_eq!(
        fetched.status, 200,
        "expected the live application to still be readable, got {} / {}",
        fetched.status, fetched.body
    );
    let _: &Map<String, Value> = fetched
        .body
        .as_object()
        .expect("the reported application is a JSON object");
}
