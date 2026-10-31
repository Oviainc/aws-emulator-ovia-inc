// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests pinning the IDENTITY of a profile object type: which record a
//! `PutProfileObjectType` writes, and which names it accepts at all.
//!
//! The rule under test: a profile object type is identified by the pair (domain, object
//! type name) carried in the request URI, so a put writes exactly the record that pair
//! names — leaving the same name under every other domain untouched — and a read of that
//! pair reports the definition the put stored, while a name outside the modelled
//! `ObjectTypeName` constraint is refused with `BadRequestException` (400) by every
//! operation carrying it.
//!
//! Every case goes through the real router, so what is asserted is the wire behaviour a
//! client sees. The HTTP plumbing and the create helpers are this crate's one test
//! harness, in `tests/all/domain_profile_lifecycle.rs`; the template and constraint rules
//! of the same two operations are pinned in `tests/all/profile_object_types.rs`.

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

/// The same object type name defined under several domains is several independent
/// records: each read reports its own domain's definition, redefining one leaves the
/// others as they were, and a domain that never defined the name does not hold it.
#[tokio::test]
async fn an_object_type_is_identified_by_its_domain_and_its_name() {
    let emulator = Emulator::new();

    // The rows differ in the definition each domain gives the SAME name: a bare
    // description, a description plus an assigned object count, and one naming a
    // template.
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "the first domain's definition",
            "scope-a",
            json!({ "Description": "defined by scope-a" }),
        ),
        (
            "a second domain's definition of the same name",
            "scope-b",
            json!({ "Description": "defined by scope-b", "MaxProfileObjectCount": 7 }),
        ),
        (
            "a third domain's definition of the same name",
            "scope-c",
            json!({ "Description": "defined by scope-c", "TemplateId": "CTR-NoInferred" }),
        ),
    ];
    let shared_name = "Shared_Type";
    let bystander = "scope-d";

    for (case, domain, _) in &rows {
        emulator.create_domain(domain, 30).await;
        let (status, body) = get(&emulator, domain, shared_name).await;
        assert_eq!(
            (status, error_code(&body)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a fresh domain must hold no object type, got {status} {body:?}"
        );
    }
    emulator.create_domain(bystander, 30).await;

    for (case, domain, body) in &rows {
        let (status, response) = put(&emulator, domain, shared_name, body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for {body:?}, got {status} {response:?}"
        );
    }

    // Every domain reports its own definition, not the one written last.
    for (case, domain, body) in &rows {
        let (status, read) = get(&emulator, domain, shared_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetProfileObjectType 200, got {status} {read:?}"
        );
        for (member, expected) in body.as_object().expect("an object request") {
            assert_eq!(
                read.get(member),
                Some(expected),
                "case {case:?}: expected domain {domain:?} to report {member} = {expected:?}, \
                 got {:?}",
                read.get(member)
            );
        }
    }

    // A domain that never defined the name does not hold it, however many other domains
    // did.
    let (status, read) = get(&emulator, bystander, shared_name).await;
    assert_eq!(
        (status, error_code(&read)),
        (404, "ResourceNotFoundException"),
        "case \"a domain that never defined the name\": expected 404, got {status} {read:?}"
    );

    // Redefining the name under ONE domain moves only that domain's record.
    let (redefined_domain, kept) = ("scope-a", "scope-b");
    let (status, response) = put(
        &emulator,
        redefined_domain,
        shared_name,
        json!({ "Description": "redefined by scope-a", "MaxProfileObjectCount": 12 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a redefinition under one domain\": expected 200, got {status} {response:?}"
    );
    let (status, read) = get(&emulator, redefined_domain, shared_name).await;
    assert_eq!(
        (
            status,
            read.get("Description").and_then(Value::as_str),
            read.get("MaxProfileObjectCount").and_then(Value::as_i64)
        ),
        (200, Some("redefined by scope-a"), Some(12)),
        "case \"a redefinition under one domain\": expected the new definition, \
         got {status} {read:?}"
    );
    let (status, read) = get(&emulator, kept, shared_name).await;
    assert_eq!(
        (
            status,
            read.get("Description").and_then(Value::as_str),
            read.get("MaxProfileObjectCount").and_then(Value::as_i64)
        ),
        (200, Some("defined by scope-b"), Some(7)),
        "case \"another domain after the redefinition\": expected its own definition \
         untouched, got {status} {read:?}"
    );
}

/// The `ObjectTypeName` constraint, over a table spanning the forms it allows and the
/// forms it does not. The constraint is the modelled `typeName`
/// (`smithy_query(query="shape",shape="typeName")`): length 1..255, pattern
/// `^[a-zA-Z_][a-zA-Z_0-9-]*$` — so the table carries each published length bound and one
/// past the maximum, and a first character and a later character from outside the
/// pattern's classes.
#[tokio::test]
async fn a_put_refuses_an_object_type_name_outside_its_modelled_constraint() {
    let emulator = Emulator::new();
    emulator.create_domain("namedom", 30).await;

    let max_length_name = "a".repeat(255);
    let past_max_name = "a".repeat(256);
    let rows: Vec<(&str, &str, u16)> = vec![
        ("a name at the minimum length", "a", 200),
        ("a name starting with an underscore", "_internal-type", 200),
        (
            "digits, hyphens and underscores after the first character",
            "A1-b_2",
            200,
        ),
        ("a name at the maximum length", max_length_name.as_str(), 200),
        (
            "a name one past the maximum length",
            past_max_name.as_str(),
            400,
        ),
        ("a name starting with a digit", "9Type", 400),
        ("a name starting with a hyphen", "-Type", 400),
        ("a name carrying a character outside the pattern", "Ty.pe", 400),
    ];

    for (case, name, expected_status) in rows {
        let (status, response) =
            put(&emulator, "namedom", name, json!({ "Description": "a type" })).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for name {name:?}, \
             got {status} {response:?}"
        );

        let (read_status, read) = get(&emulator, "namedom", name).await;
        if expected_status == 200 {
            assert_eq!(
                response.get("ObjectTypeName").and_then(Value::as_str),
                Some(name),
                "case {case:?}: expected the put to echo ObjectTypeName {name:?}, \
                 got {response:?}"
            );
            assert_eq!(
                (read_status, read.get("ObjectTypeName").and_then(Value::as_str)),
                (200, Some(name)),
                "case {case:?}: expected the read to report the stored name, \
                 got {read_status} {read:?}"
            );
        } else {
            assert_eq!(
                error_code(&response),
                "BadRequestException",
                "case {case:?}: expected BadRequestException, got {response:?}"
            );
            // The read carries the same `ObjectTypeName` member, so a name the
            // constraint rejects is refused there too — one wording for one bad value,
            // whichever operation carried it.
            assert_eq!(
                (read_status, error_code(&read)),
                (400, "BadRequestException"),
                "case {case:?}: expected the read of the same name to be refused, \
                 got {read_status} {read:?}"
            );
        }
    }
}
