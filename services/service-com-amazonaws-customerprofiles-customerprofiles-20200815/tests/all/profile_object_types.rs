// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `PutProfileObjectType` and `GetProfileObjectType`.
//!
//! The rule under test: when a request names a configuration TEMPLATE, the template
//! supplies the members `PutProfileObjectTypeRequest$TemplateId` names
//! (`AllowProfileCreation`, `SourceLastUpdatedTimestampFormat`, `Fields` and `Keys`), so
//! a request carrying the template AND any of them states the same configuration twice
//! and is refused with `BadRequestException` (400) before anything is persisted — a
//! following `GetProfileObjectType` for that name answers `ResourceNotFoundException`
//! (404). A request that does not state the configuration twice is persisted as given,
//! and the read reports the definition that was stored.
//!
//! Every case goes through the real router, so what is asserted is the wire behaviour a
//! client sees. The HTTP plumbing and the create helpers are this crate's one test
//! harness, in `tests/all/domain_profile_lifecycle.rs`.

use serde_json::{json, Value};

use crate::domain_profile_lifecycle::{error_code, Emulator};

/// The members the `TemplateId` documentation names as supplied by the template, in the
/// order it lists them: "AllowProfileCreation, SourceLastUpdatedTimestampFormat, Fields,
/// and Keys".
const TEMPLATE_SUPPLIED_MEMBERS: [&str; 4] = [
    "AllowProfileCreation",
    "SourceLastUpdatedTimestampFormat",
    "Fields",
    "Keys",
];

/// The sentence the service refuses a request that states its configuration twice with:
/// one fixed text for the whole conflict class, naming no member of the request.
const TEMPLATE_CONFLICT_MESSAGE: &str = "Can't specify fields or keys together with a templateId";

/// A `Fields` map: `ObjectTypeField`'s three members, with a `ContentType` out of the
/// modelled `FieldContentType` enum (STRING | NUMBER | PHONE_NUMBER | EMAIL_ADDRESS |
/// NAME).
fn fields() -> Value {
    json!({
        "phone": {
            "ContentType": "PHONE_NUMBER",
            "Source": "_source.phone",
            "Target": "_profile.PhoneNumber",
        },
    })
}

/// A `Keys` map: one key whose `StandardIdentifiers` comes from the modelled
/// `StandardIdentifier` enum.
fn keys() -> Value {
    json!({
        "phoneKey": [{ "StandardIdentifiers": ["PROFILE"], "FieldNames": ["phone"] }],
    })
}

/// `PutProfileObjectType` over the wire:
/// `PUT /domains/{DomainName}/object-types/{ObjectTypeName}`.
async fn put(emulator: &Emulator, domain: &str, name: &str, body: Value) -> (u16, Value) {
    emulator
        .send(
            "PUT",
            &format!("/domains/{domain}/object-types/{name}"),
            Some(body),
        )
        .await
}

/// `GetProfileObjectType` over the wire:
/// `GET /domains/{DomainName}/object-types/{ObjectTypeName}`.
async fn get(emulator: &Emulator, domain: &str, name: &str) -> (u16, Value) {
    emulator
        .send("GET", &format!("/domains/{domain}/object-types/{name}"), None)
        .await
}

/// The `Message` an error body carries, whichever casing the protocol rendered it in.
pub(crate) fn error_message(body: &Value) -> &str {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// A request body with `Description` plus the members of `extra`.
fn definition(extra: Value) -> Value {
    let mut body = json!({ "Description": "an object type" });
    for (member, value) in extra.as_object().expect("an object") {
        body[member] = value.clone();
    }
    body
}

/// The refusal, over a table spanning every member the template supplies: each one on its
/// own beside `TemplateId`, two together, and all four at once. Every row must leave the
/// object type uncreated.
#[tokio::test]
async fn a_put_that_names_a_template_and_what_the_template_supplies_is_refused() {
    let emulator = Emulator::new();
    emulator.create_domain("tmpldom", 30).await;

    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "a template and Fields",
            definition(json!({ "TemplateId": "CTR-NoInferred", "Fields": fields() })),
            vec!["Fields"],
        ),
        (
            "a template and Keys",
            definition(json!({ "TemplateId": "CTR-NoInferred", "Keys": keys() })),
            vec!["Keys"],
        ),
        (
            "a template and both Fields and Keys",
            definition(json!({
                "TemplateId": "CTR-NoInferred",
                "Fields": fields(),
                "Keys": keys(),
            })),
            vec!["Fields", "Keys"],
        ),
        (
            "a template and AllowProfileCreation",
            definition(json!({ "TemplateId": "Salesforce-Account", "AllowProfileCreation": true })),
            vec!["AllowProfileCreation"],
        ),
        (
            "a template and SourceLastUpdatedTimestampFormat",
            definition(json!({
                "TemplateId": "Zendesk-User",
                "SourceLastUpdatedTimestampFormat": "yyyy-MM-dd'T'HH:mm:ssX",
            })),
            vec!["SourceLastUpdatedTimestampFormat"],
        ),
        (
            "a template and every member it supplies",
            definition(json!({
                "TemplateId": "Marketo-Lead",
                "AllowProfileCreation": true,
                "SourceLastUpdatedTimestampFormat": "yyyy-MM-dd",
                "Fields": fields(),
                "Keys": keys(),
            })),
            TEMPLATE_SUPPLIED_MEMBERS.to_vec(),
        ),
        (
            "a template, what it supplies, and members it does not",
            definition(json!({
                "TemplateId": "CTR-NoInferred",
                "ExpirationDays": 30,
                "Keys": keys(),
            })),
            vec!["Keys"],
        ),
    ];

    for (index, (case, body, conflicting)) in rows.into_iter().enumerate() {
        let name = format!("Conflict{index}");

        let (status, response) = put(&emulator, "tmpldom", &name, body.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {body:?}, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {response:?}"
        );
        // The sentence is fixed for the class: whichever of the members collided, the
        // refusal reads the same and names none of them.
        let message = error_message(&response);
        assert_eq!(
            message, TEMPLATE_CONFLICT_MESSAGE,
            "case {case:?} (conflicting {conflicting:?}): expected the fixed conflict \
             sentence, got {message:?}"
        );
        for member in TEMPLATE_SUPPLIED_MEMBERS {
            assert!(
                !message.contains(member),
                "case {case:?}: the refusal must name no request member, got {message:?}"
            );
        }

        // Nothing was created: the read answers 404, not a partial definition.
        let (status, read) = get(&emulator, "tmpldom", &name).await;
        assert_eq!(
            (status, error_code(&read)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: the refused put must create nothing, got {status} {read:?}"
        );
    }
}

/// The success path, over a table of definitions that do NOT state the configuration
/// twice: a template alone, a template beside members it does not supply, `Fields` and
/// `Keys` with no template at all, and the bare required `Description`. Each definition
/// is echoed by the put and reported by the read.
#[tokio::test]
async fn a_put_persists_the_definition_and_the_read_reports_it() {
    let emulator = Emulator::new();
    emulator.create_domain("objdom", 30).await;

    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a template alone",
            "CTR",
            definition(json!({ "TemplateId": "CTR-NoInferred" })),
        ),
        (
            "a template beside members it does not supply",
            "CTR_with_extras",
            definition(json!({
                "TemplateId": "CTR-NoInferred",
                "ExpirationDays": 90,
                "EncryptionKey": "arn:aws:kms:us-east-1:111122223333:key/abcd",
                "MaxProfileObjectCount": 500,
                "SourcePriority": 2,
                "Tags": { "team": "crm" },
            })),
        ),
        (
            "a template beside AllowProfileCreation left at its modelled default",
            "CTR_default_flag",
            definition(json!({ "TemplateId": "CTR-NoInferred", "AllowProfileCreation": false })),
        ),
        (
            "Fields and Keys with no template",
            "Phone-Records",
            definition(json!({ "Fields": fields(), "Keys": keys() })),
        ),
        (
            "every member a request may carry without a template",
            "Full_Type",
            definition(json!({
                "AllowProfileCreation": true,
                "SourceLastUpdatedTimestampFormat": "yyyy-MM-dd'T'HH:mm:ssX",
                "Fields": fields(),
                "Keys": keys(),
                "ExpirationDays": 1098,
                "MaxProfileObjectCount": 1,
                "SourcePriority": 1,
                "Tags": { "team": "crm", "env": "test" },
            })),
        ),
        (
            "the required Description only",
            "Minimal",
            definition(json!({})),
        ),
    ];

    for (case, name, body) in rows {
        let (status, response) = put(&emulator, "objdom", name, body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for {body:?}, got {status} {response:?}"
        );
        assert_eq!(
            response.get("ObjectTypeName").and_then(Value::as_str),
            Some(name),
            "case {case:?}: expected ObjectTypeName {name:?}, got {response:?}"
        );

        // Every member the request carried comes back, from the put and from the read.
        let (status, read) = get(&emulator, "objdom", name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetProfileObjectType 200, got {status} {read:?}"
        );
        for (member, expected) in body.as_object().expect("an object request") {
            assert_eq!(
                response.get(member),
                Some(expected),
                "case {case:?}: expected the put to echo {member} = {expected:?}, got {:?}",
                response.get(member)
            );
            assert_eq!(
                read.get(member),
                Some(expected),
                "case {case:?}: expected the read to report {member} = {expected:?} as the \
                 put stored it, got {:?}",
                read.get(member)
            );
        }
        assert_eq!(
            read.get("ObjectTypeName").and_then(Value::as_str),
            Some(name),
            "case {case:?}: expected the read to report ObjectTypeName {name:?}, got {read:?}"
        );
        assert!(
            read.get("CreatedAt").is_some() && read.get("LastUpdatedAt").is_some(),
            "case {case:?}: expected CreatedAt and LastUpdatedAt, got {read:?}"
        );
        // `MaxAvailableProfileObjectCount` is the unused part of the count assigned to
        // the type, so a type with no assigned count reports neither member.
        assert_eq!(
            read.get("MaxAvailableProfileObjectCount"),
            body.get("MaxProfileObjectCount"),
            "case {case:?}: expected the available count to follow the assigned one, \
             got {read:?}"
        );

        // The read changed nothing a later caller sees.
        let (status, again) = get(&emulator, "objdom", name).await;
        assert_eq!(
            (status, &again),
            (200, &read),
            "case {case:?}: two reads of an unchanged object type must be identical, \
             got {read:?} then {again:?}"
        );
    }
}

/// A put over a name the domain already holds REDEFINES it: the definition is replaced
/// wholesale (a member the second request leaves out stops being reported) and the
/// record keeps the instant it was first created at.
#[tokio::test]
async fn a_put_over_a_held_name_redefines_it() {
    let emulator = Emulator::new();
    emulator.create_domain("redefdom", 30).await;

    let (status, first) = put(
        &emulator,
        "redefdom",
        "Redefined",
        definition(json!({ "Fields": fields(), "ExpirationDays": 30 })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the first definition\": expected 200, got {status} {first:?}"
    );

    let (status, second) = put(
        &emulator,
        "redefdom",
        "Redefined",
        json!({ "Description": "redefined from a template", "TemplateId": "CTR-NoInferred" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the redefinition\": expected 200, got {status} {second:?}"
    );

    let (status, read) = get(&emulator, "redefdom", "Redefined").await;
    assert_eq!(
        status, 200,
        "case \"after the redefinition\": expected 200, got {status} {read:?}"
    );
    assert_eq!(
        read.get("TemplateId").and_then(Value::as_str),
        Some("CTR-NoInferred"),
        "case \"after the redefinition\": expected the new TemplateId, got {read:?}"
    );
    assert_eq!(
        read.get("Description").and_then(Value::as_str),
        Some("redefined from a template"),
        "case \"after the redefinition\": expected the new Description, got {read:?}"
    );
    assert_eq!(
        read.get("Fields"),
        None,
        "case \"after the redefinition\": the replaced definition must drop Fields, \
         got {read:?}"
    );
    assert_eq!(
        read.get("ExpirationDays"),
        None,
        "case \"after the redefinition\": the replaced definition must drop ExpirationDays, \
         got {read:?}"
    );
    assert_eq!(
        read.get("CreatedAt"),
        first.get("CreatedAt"),
        "case \"after the redefinition\": CreatedAt must not move, got {read:?}"
    );
}

/// The other refusals: a domain that is not stored, a name the domain does not hold, and
/// input outside a modelled constraint — `Description` on its published bounds and one
/// past each.
#[tokio::test]
async fn the_object_type_operations_refuse_an_absent_resource_and_invalid_input() {
    let emulator = Emulator::new();
    emulator.create_domain("boundsdom", 30).await;

    let (status, body) = put(
        &emulator,
        "nosuchdomain",
        "Anything",
        definition(json!({})),
    )
    .await;
    assert_eq!(
        (status, error_code(&body)),
        (404, "ResourceNotFoundException"),
        "case \"a put into a domain that is not stored\": expected 404, got {status} {body:?}"
    );

    let (status, body) = get(&emulator, "nosuchdomain", "Anything").await;
    assert_eq!(
        (status, error_code(&body)),
        (404, "ResourceNotFoundException"),
        "case \"a read in a domain that is not stored\": expected 404, got {status} {body:?}"
    );

    let (status, body) = get(&emulator, "boundsdom", "NeverDefined").await;
    assert_eq!(
        (status, error_code(&body)),
        (404, "ResourceNotFoundException"),
        "case \"a read of a name the domain does not hold\": expected 404, got {status} {body:?}"
    );

    // `Description` is `sensitiveText`, len 1..1000; `ObjectTypeName` is `typeName`,
    // len 1..255, pattern ^[a-zA-Z_][a-zA-Z_0-9-]*$.
    let constraints: Vec<(&str, &str, Value, u16)> = vec![
        (
            "a Description at the minimum length",
            "Min_Description",
            json!({ "Description": "d" }),
            200,
        ),
        (
            "a Description at the maximum length",
            "Max_Description",
            json!({ "Description": "d".repeat(1000) }),
            200,
        ),
        (
            "a Description one past the maximum length",
            "Past_Max_Description",
            json!({ "Description": "d".repeat(1001) }),
            400,
        ),
        (
            "a Description below the minimum length",
            "Empty_Description",
            json!({ "Description": "" }),
            400,
        ),
        ("no Description at all", "No_Description", json!({}), 400),
        (
            "an ExpirationDays one past the modelled maximum",
            "Past_Expiration",
            definition(json!({ "ExpirationDays": 1099 })),
            400,
        ),
        (
            "an ExpirationDays below the modelled minimum",
            "Zero_Expiration",
            definition(json!({ "ExpirationDays": 0 })),
            400,
        ),
        (
            "a MaxProfileObjectCount below the modelled minimum",
            "Zero_Max_Count",
            definition(json!({ "MaxProfileObjectCount": 0 })),
            400,
        ),
        (
            "a ContentType outside the modelled enum",
            "Bad_Content_Type",
            definition(json!({
                "Fields": { "phone": { "ContentType": "TELEPHONE" } },
            })),
            400,
        ),
    ];

    for (case, name, body, expected_status) in constraints {
        let (status, response) = put(&emulator, "boundsdom", name, body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {response:?}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&response),
                "BadRequestException",
                "case {case:?}: expected BadRequestException, got {response:?}"
            );
            let (status, read) = get(&emulator, "boundsdom", name).await;
            assert_eq!(
                (status, error_code(&read)),
                (404, "ResourceNotFoundException"),
                "case {case:?}: the refused put must create nothing, got {status} {read:?}"
            );
        }
    }
}
