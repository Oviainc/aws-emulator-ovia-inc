// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteProfileObjectType`.
//!
//! The rule under test: a profile object type is identified by the pair (domain, object
//! type name) in the request URI, so the delete removes exactly the record that pair
//! names together with everything stored under it — a following `GetProfileObjectType`
//! of that pair answers `ResourceNotFoundException` (404), every other record is left
//! as it was, and a pair the store does not hold (either member absent, or the same
//! name already deleted) is refused with `ResourceNotFoundException` (404) having
//! removed nothing.
//!
//! Every case goes through the real router, so what is asserted is the wire behaviour a
//! client sees. The HTTP plumbing and the create helpers are this crate's one test
//! harness, in `tests/all/domain_profile_lifecycle.rs`; the put/read rules of the same
//! resource are pinned in `tests/all/profile_object_types.rs` and
//! `tests/all/profile_object_type_identity.rs`.

use serde_json::{json, Value};

use crate::domain_profile_lifecycle::{error_code, Emulator};

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

/// `DeleteProfileObjectType` over the wire:
/// `DELETE /domains/{DomainName}/object-types/{ObjectTypeName}`.
async fn delete(emulator: &Emulator, domain: &str, name: &str) -> (u16, Value) {
    emulator
        .send(
            "DELETE",
            &format!("/domains/{domain}/object-types/{name}"),
            None,
        )
        .await
}

/// The `Message` an error body carries, whichever casing the protocol rendered it in.
fn error_message(body: &Value) -> &str {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// A `Fields` map: `ObjectTypeField`'s members, with a `ContentType` out of the modelled
/// `FieldContentType` enum.
fn fields() -> Value {
    json!({
        "email": {
            "ContentType": "EMAIL_ADDRESS",
            "Source": "_source.email",
            "Target": "_profile.EmailAddress",
        },
    })
}

/// The definitions the tables below span: they differ in the FORM of the definition
/// stored under the name — the bare required `Description`, a template, explicit
/// `Fields`, an assigned object count with tags — because the delete must not depend on
/// which of them the record holds.
fn definitions() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        (
            "the required Description only",
            "Minimal",
            json!({ "Description": "a minimal type" }),
        ),
        (
            "a definition naming a template",
            "CTR_Type",
            json!({ "Description": "from a template", "TemplateId": "CTR-NoInferred" }),
        ),
        (
            "a definition carrying Fields",
            "Email_Records",
            json!({ "Description": "with fields", "Fields": fields() }),
        ),
        (
            "a definition carrying an assigned count and tags",
            "Counted_Type",
            json!({
                "Description": "with a count",
                "MaxProfileObjectCount": 500,
                "ExpirationDays": 30,
                "Tags": { "team": "crm" },
            }),
        ),
        (
            "a name at the leading-underscore edge of the modelled pattern",
            "_Underscore-Type0",
            json!({ "Description": "an underscore-led name" }),
        ),
    ]
}

/// Deleting one name of a domain that holds several removes exactly that record: the
/// read of it answers 404, every other name of the domain still reports the definition
/// its put stored, and deleting the same name again is refused.
#[tokio::test]
async fn a_delete_removes_exactly_the_named_object_type() {
    let emulator = Emulator::new();
    emulator.create_domain("deldom", 30).await;

    let rows = definitions();
    for (case, name, body) in &rows {
        let (status, response) = put(&emulator, "deldom", name, body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutProfileObjectType 200, got {status} {response:?}"
        );
    }

    // The reads every surviving row must keep answering with, taken before any delete.
    let mut remaining: Vec<(&str, &str, Value)> = Vec::new();
    for (case, name, _) in &rows {
        let (status, read) = get(&emulator, "deldom", name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetProfileObjectType 200 before any delete, \
             got {status} {read:?}"
        );
        remaining.push((*case, *name, read));
    }

    // Delete the rows from the MIDDLE outward, so no row is only ever the first or the
    // last entry of the domain's map.
    let order = [2usize, 0, 4, 1, 3];
    for index in order {
        let (case, name, _) = rows[index].clone();

        let (status, deleted) = delete(&emulator, "deldom", name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteProfileObjectType 200, got {status} {deleted:?}"
        );
        let message = deleted.get("Message").and_then(Value::as_str);
        assert!(
            message.is_some_and(|text| text.contains(name)),
            "case {case:?}: expected a Message naming {name:?}, got {deleted:?}"
        );

        // The deleted pair is gone, and the refusal names the absent object type the way
        // the service words that refusal.
        let (status, read) = get(&emulator, "deldom", name).await;
        assert_eq!(
            (status, error_code(&read)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected the read of a deleted object type to answer 404, \
             got {status} {read:?}"
        );
        assert_eq!(
            error_message(&read),
            format!("ObjectType {name} not found"),
            "case {case:?}: unexpected refusal message, got {read:?}"
        );

        // Deleting it again is refused: the record is no longer held.
        let (status, again) = delete(&emulator, "deldom", name).await;
        assert_eq!(
            (status, error_code(&again)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected a second delete to answer 404, got {status} {again:?}"
        );
        assert_eq!(
            error_message(&again),
            format!("ObjectType {name} not found"),
            "case {case:?}: unexpected refusal message from the second delete, got {again:?}"
        );

        // Every name not yet deleted still reports exactly what its put stored.
        remaining.retain(|(_, held, _)| *held != name);
        for (held_case, held, expected) in &remaining {
            let (status, read) = get(&emulator, "deldom", held).await;
            assert_eq!(
                (status, &read),
                (200, expected),
                "case {case:?}: deleting it must leave {held_case:?} unchanged, \
                 got {status} {read:?}"
            );
        }
    }
}

/// The delete is scoped to the domain in its URI: the same object type name defined
/// under several domains is several independent records, so deleting it under one leaves
/// the others readable.
#[tokio::test]
async fn a_delete_removes_the_name_only_under_the_domain_it_names() {
    let emulator = Emulator::new();

    // The rows differ in the definition each domain gives the SAME name.
    let domains = ["scope-one", "scope-two", "scope-three"];
    for (index, domain) in domains.iter().enumerate() {
        emulator.create_domain(domain, 30).await;
        let (status, response) = put(
            &emulator,
            domain,
            "Shared_Type",
            json!({ "Description": format!("defined by {domain}"), "SourcePriority": index + 1 }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {domain:?}: expected PutProfileObjectType 200, got {status} {response:?}"
        );
    }

    let (status, deleted) = delete(&emulator, "scope-two", "Shared_Type").await;
    assert_eq!(
        status, 200,
        "case \"the middle domain\": expected 200, got {status} {deleted:?}"
    );

    for domain in domains {
        let expected_status = if domain == "scope-two" { 404 } else { 200 };
        let (status, read) = get(&emulator, domain, "Shared_Type").await;
        assert_eq!(
            status, expected_status,
            "case {domain:?}: expected HTTP {expected_status} after deleting the name \
             under scope-two, got {status} {read:?}"
        );
        if expected_status == 200 {
            assert_eq!(
                read.get("Description").and_then(Value::as_str),
                Some(format!("defined by {domain}").as_str()),
                "case {domain:?}: expected the domain's own definition, got {read:?}"
            );
        }
    }
}

/// The refusals, over a table of pairs the store does not hold: an absent domain, an
/// absent name, a name that was never defined, and a name whose domain was deleted. None
/// of them removes anything.
#[tokio::test]
async fn a_delete_of_a_pair_the_store_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    emulator.create_domain("heldom", 30).await;
    emulator.create_domain("goneDomain", 30).await;
    let (status, response) = put(
        &emulator,
        "heldom",
        "Held_Type",
        json!({ "Description": "held" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "seeding Held_Type: expected 200, got {status} {response:?}"
    );
    let (status, response) = put(
        &emulator,
        "goneDomain",
        "Held_Type",
        json!({ "Description": "held in a domain about to go" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "seeding goneDomain's Held_Type: expected 200, got {status} {response:?}"
    );
    let (status, response) = emulator.send("DELETE", "/domains/goneDomain", None).await;
    assert_eq!(
        status, 200,
        "seeding: expected DeleteDomain 200, got {status} {response:?}"
    );

    let rows: Vec<(&str, &str, &str)> = vec![
        ("a domain that was never created", "nosuchdomain", "Held_Type"),
        ("a domain that was deleted", "goneDomain", "Held_Type"),
        ("a name the domain never defined", "heldom", "Never_Defined"),
        ("a name differing only in case", "heldom", "held_type"),
    ];

    for (case, domain, name) in rows {
        let (status, body) = delete(&emulator, domain, name).await;
        assert_eq!(
            (status, error_code(&body)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException, got {status} {body:?}"
        );
        assert!(
            !error_message(&body).is_empty(),
            "case {case:?}: the refusal must carry a message, got {body:?}"
        );
    }

    // Nothing above removed the record that IS held.
    let (status, read) = get(&emulator, "heldom", "Held_Type").await;
    assert_eq!(
        status, 200,
        "case \"the held record\": a refused delete must remove nothing, got {status} {read:?}"
    );
}

/// A name outside the modelled `ObjectTypeName` constraint (len 1..255, pattern
/// `^[a-zA-Z_][a-zA-Z_0-9-]*$`) is refused as a bad request, on each published bound and
/// one past it, rather than reaching the store.
#[tokio::test]
async fn a_delete_refuses_a_name_outside_the_modelled_constraint() {
    let emulator = Emulator::new();
    emulator.create_domain("bounddom", 30).await;

    let at_max = format!("N{}", "a".repeat(254));
    let past_max = format!("N{}", "a".repeat(255));
    let rows: Vec<(&str, String, u16)> = vec![
        ("a name at the minimum length", "N".to_string(), 404),
        ("a name at the maximum length", at_max, 404),
        ("a name one past the maximum length", past_max, 400),
        (
            "a name whose first character is outside the pattern",
            "0Type".to_string(),
            400,
        ),
        (
            "a name carrying a character outside the pattern",
            "Ty.pe".to_string(),
            400,
        ),
    ];

    for (case, name, expected_status) in rows {
        let (status, body) = delete(&emulator, "bounddom", &name).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for {name:?}, got {status} {body:?}"
        );
    }
}
