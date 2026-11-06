// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `BatchPutProfileObject`.
//!
//! The rule under test: a batch ingests every item it carries exactly as a single put
//! ingests one object — same derived `ProfileObjectUniqueKey`, same profile tie, same
//! store — and reports one entry per item, correlated by the item's `Id`: the key under
//! `Successful`, or the HTTP status and sentence of the refusal that object would earn on
//! its own under `Failed`. One item's refusal therefore neither stops the batch nor undoes
//! what another item stored, while a domain or object type the store does not hold —
//! neither being a property of one item — refuses the whole call with
//! `ResourceNotFoundException` (404).
//!
//! Every case goes through the real router, so what is asserted is the wire behaviour a
//! client sees. The HTTP plumbing and the create helpers are this crate's one test
//! harness, in `tests/all/domain_profile_lifecycle.rs`; the object type these tests ingest
//! through, and the single-object put they are compared against, are the ones in
//! `tests/all/profile_objects.rs`.

use serde_json::{json, Value};

use crate::domain_profile_lifecycle::{error_code, item_ids, Emulator};
use crate::profile_objects::{contact, ingest, listed, listed_keys, put_contact_type};

/// The object type every case below ingests through.
const OBJECT_TYPE: &str = "Contact_Record";

/// `BatchPutProfileObject` over the wire:
/// `PUT /domains/{DomainName}/profiles/objects/batch-put-profile-object`.
async fn batch_put(
    emulator: &Emulator,
    domain: &str,
    object_type: &str,
    items: Vec<Value>,
) -> (u16, Value) {
    emulator
        .send(
            "PUT",
            &format!("/domains/{domain}/profiles/objects/batch-put-profile-object"),
            Some(json!({ "ObjectTypeName": object_type, "Items": items })),
        )
        .await
}

/// One batch item: the `Id` that correlates it with its answer, and the object as a
/// string serialized from JSON.
fn item(id: &str, object: &Value) -> Value {
    json!({ "Id": id, "Object": object.to_string() })
}

/// One batch item whose `Object` is the given text verbatim — the way a row carries text
/// that is not serialized from a JSON object.
fn raw_item(id: &str, object: &str) -> Value {
    json!({ "Id": id, "Object": object })
}

/// The (`Id`, `ProfileObjectUniqueKey`) pairs of a response's `Successful`, in the order
/// the service reported them.
fn successful(body: &Value) -> Vec<(String, String)> {
    entries(body, "Successful", "ProfileObjectUniqueKey")
}

/// The (`Id`, `Code`) pairs of a response's `Failed`, in the order the service reported
/// them.
fn failed_codes(body: &Value) -> Vec<(String, i64)> {
    body.get("Failed")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|entry| {
                    (
                        text(entry, "Id"),
                        entry.get("Code").and_then(Value::as_i64).unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The (`Id`, `Message`) pairs of a response's `Failed`.
fn failed_messages(body: &Value) -> Vec<(String, String)> {
    entries(body, "Failed", "Message")
}

/// The (`Id`, `member`) pairs of one of the response's lists.
fn entries(body: &Value, list: &str, member: &str) -> Vec<(String, String)> {
    body.get(list)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|entry| (text(entry, "Id"), text(entry, member)))
                .collect()
        })
        .unwrap_or_default()
}

/// One string member of a response entry, empty when it carries none.
fn text(entry: &Value, member: &str) -> String {
    entry
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `ProfileId` of the single profile indexed under one predefined search key, which
/// is how a case finds the profile a batch tied its object to.
async fn profile_by_key(emulator: &Emulator, domain: &str, key_name: &str, value: &str) -> String {
    let (status, body) = emulator
        .search(domain, json!({ "KeyName": key_name, "Values": [value] }), "")
        .await;
    assert_eq!(
        status, 200,
        "searching {key_name}={value}: expected 200, got {status} {body:?}"
    );
    let ids = item_ids(&body);
    assert_eq!(
        ids.len(),
        1,
        "searching {key_name}={value}: expected exactly one profile, got {ids:?}"
    );
    ids[0].clone()
}

/// A domain holding the contact-record type and one profile reachable by phone number and
/// by account number: the profile an item's keys can tie an object to.
///
/// Answers the domain name and that profile's id.
async fn seeded(emulator: &Emulator, domain: &str) -> String {
    emulator.create_domain(domain, 90).await;
    put_contact_type(emulator, domain, OBJECT_TYPE, true).await;
    emulator
        .create_profile(
            domain,
            json!({ "PhoneNumber": SEEDED_PHONE, "AccountNumber": SEEDED_ACCOUNT }),
        )
        .await
}

/// The phone number of the profile [`seeded`] creates.
const SEEDED_PHONE: &str = "+15551110001";

/// The account number of that same profile.
const SEEDED_ACCOUNT: &str = "A-SEED";

/// Every item of one batch is ingested as a single put ingests one object: it lands under
/// the key the service derives from its own identity and is tied to the profile its type's
/// keys identify, and its answer is correlated by the `Id` the request gave it.
///
/// The table spans which standard member the matching key maps onto, how deep in the
/// object the value sits, whether the value arrives as a JSON string or a JSON number,
/// whether the object matches a stored profile or has one inferred for it, and whether it
/// carries members the type maps onto nothing.
#[tokio::test]
async fn a_batch_ingests_every_item_as_a_single_put_does() {
    // (case, the object, the key the profile that must own it is indexed under)
    let rows: Vec<(&str, Value, (&str, &str))> = vec![
        (
            "a PROFILE key on a phone number, nested two levels down",
            contact("b-1", json!({ "phone": SEEDED_PHONE }), json!({})),
            ("_phone", SEEDED_PHONE),
        ),
        (
            "a PROFILE key on an account number at the top level",
            contact("b-2", json!({}), json!({ "accountNumber": SEEDED_ACCOUNT })),
            ("_account", SEEDED_ACCOUNT),
        ),
        (
            "a PROFILE key whose value arrives as a JSON number",
            contact("b-3", json!({}), json!({ "accountNumber": 4242 })),
            ("_account", "4242"),
        ),
        (
            "an object no stored profile claims, which infers one",
            contact("b-4", json!({ "phone": "+15551110444" }), json!({})),
            ("_phone", "+15551110444"),
        ),
        (
            "a SECONDARY key on an email address, searched after the PROFILE keys",
            contact("b-5", json!({ "email": "ada@example.com" }), json!({})),
            ("_email", "ada@example.com"),
        ),
        (
            "a matching key beside members the type maps onto nothing",
            contact(
                "b-6",
                json!({ "phone": SEEDED_PHONE, "firstName": "Grace", "city": "Baltimore" }),
                json!({ "notes": "mapped by no field" }),
            ),
            ("_phone", SEEDED_PHONE),
        ),
    ];

    let emulator = Emulator::new();
    let domain = "batchdom";
    seeded(&emulator, domain).await;

    let ids: Vec<String> = (0..rows.len()).map(|index| format!("item-{index}")).collect();
    let items: Vec<Value> = rows
        .iter()
        .zip(&ids)
        .map(|((_, object, _), id)| item(id, object))
        .collect();

    let (status, body) = batch_put(&emulator, domain, OBJECT_TYPE, items).await;
    assert_eq!(status, 200, "expected 200, got {status} {body:?}");
    assert_eq!(
        failed_codes(&body),
        Vec::new(),
        "no item of this batch may fail, got {body:?}"
    );

    let answers = successful(&body);
    assert_eq!(
        answers.iter().map(|(id, _)| id.clone()).collect::<Vec<String>>(),
        ids,
        "expected one answer per item, correlated by Id in request order, got {body:?}"
    );

    for ((case, object, (key_name, value)), (id, unique_key)) in rows.iter().zip(&answers) {
        assert!(
            !unique_key.is_empty() && unique_key.len() <= 255,
            "case {case:?}: the derived key must be within the modelled 1..255, \
             got {unique_key:?}"
        );

        // The profile the object's keys identify now reports it, with the object exactly
        // as the item carried it.
        let owner = profile_by_key(&emulator, domain, key_name, value).await;
        let items = listed(&emulator, domain, OBJECT_TYPE, &owner).await;
        assert!(
            listed_keys(&items).contains(unique_key),
            "case {case:?}: expected {unique_key:?} under profile {owner}, got {items:?}"
        );
        let stored = items
            .iter()
            .find(|entry| text(entry, "ProfileObjectUniqueKey") == *unique_key)
            .unwrap_or_else(|| panic!("case {case:?}: no entry for {unique_key:?} in {items:?}"));
        assert_eq!(
            text(stored, "Object"),
            object.to_string(),
            "case {case:?}: expected the object as item {id} carried it, got {stored:?}"
        );
        assert_eq!(
            text(stored, "ObjectTypeName"),
            OBJECT_TYPE,
            "case {case:?}: expected the type it was ingested under, got {stored:?}"
        );

        // The single put derives the same key for the same object, which is the batch
        // going through the one ingest path rather than a second one of its own.
        let single = ingest(&emulator, domain, OBJECT_TYPE, object).await;
        assert_eq!(
            &single, unique_key,
            "case {case:?}: expected the single put to derive the batch's key"
        );
    }
}

/// An item whose `Object` is not serialized from a JSON object fails on its own: it is
/// reported in `Failed` with the status that refusal carries and a sentence describing it,
/// the items beside it still land, and nothing is persisted for the failed one.
///
/// The table spans the JSON kinds that are not an object, and text that is not JSON at
/// all, each mixed into a batch with a valid item.
#[tokio::test]
async fn a_failed_item_is_reported_beside_the_items_that_landed() {
    // (case, the Object text that is not a serialized JSON object)
    let rows: Vec<(&str, &str)> = vec![
        ("text that is not JSON at all", "not json"),
        ("a JSON array", "[{\"contactId\":\"x\"}]"),
        ("a JSON string", "\"contactId\""),
        ("a JSON number", "42"),
        ("a JSON boolean", "true"),
        ("JSON null", "null"),
        ("a JSON object that was never closed", "{\"contactId\":\"x\""),
    ];

    for (index, (case, object_text)) in rows.into_iter().enumerate() {
        let emulator = Emulator::new();
        let domain = format!("faileddom{index}");
        let owner = seeded(&emulator, &domain).await;

        let good = contact("good", json!({ "phone": SEEDED_PHONE }), json!({}));
        let (status, body) = batch_put(
            &emulator,
            &domain,
            OBJECT_TYPE,
            vec![
                raw_item("bad-item", object_text),
                item("good-item", &good),
                raw_item("bad-item-2", object_text),
            ],
        )
        .await;

        assert_eq!(
            status, 200,
            "case {case:?}: one item's refusal must not refuse the call, \
             got {status} {body:?}"
        );
        assert_eq!(
            failed_codes(&body),
            vec![
                ("bad-item".to_string(), 400),
                ("bad-item-2".to_string(), 400),
            ],
            "case {case:?}: expected both invalid items reported with HTTP 400, \
             got {body:?}"
        );
        for (id, message) in failed_messages(&body) {
            assert!(
                !message.is_empty(),
                "case {case:?}: item {id} must carry a message describing the error, \
                 got {body:?}"
            );
        }

        let answers = successful(&body);
        assert_eq!(
            answers.iter().map(|(id, _)| id.clone()).collect::<Vec<String>>(),
            vec!["good-item".to_string()],
            "case {case:?}: expected the valid item to land, got {body:?}"
        );

        // Exactly the valid item is persisted: a failed item writes nothing.
        let items = listed(&emulator, &domain, OBJECT_TYPE, &owner).await;
        assert_eq!(
            listed_keys(&items),
            vec![answers[0].1.clone()],
            "case {case:?}: expected only the valid item's object to be held, \
             got {items:?}"
        );
    }
}

/// A batch whose every item is refused reports them all and persists nothing, and the
/// call still succeeds: `Successful` and `Failed` are both always reported, so a caller
/// reads either outcome off an empty list rather than off an absent member.
#[tokio::test]
async fn a_batch_whose_every_item_fails_still_answers_both_lists() {
    let emulator = Emulator::new();
    let domain = "allfaildom";
    let owner = seeded(&emulator, domain).await;

    let (status, body) = batch_put(
        &emulator,
        domain,
        OBJECT_TYPE,
        vec![raw_item("one", "not json"), raw_item("two", "[1,2,3]")],
    )
    .await;

    assert_eq!(status, 200, "expected 200, got {status} {body:?}");
    assert_eq!(
        failed_codes(&body),
        vec![("one".to_string(), 400), ("two".to_string(), 400)],
        "expected every item reported as failed, got {body:?}"
    );
    assert_eq!(
        successful(&body),
        Vec::new(),
        "expected no item to land, got {body:?}"
    );
    assert!(
        body.get("Successful").and_then(Value::as_array).is_some(),
        "expected Successful to be reported as an empty list, got {body:?}"
    );
    let items = listed(&emulator, domain, OBJECT_TYPE, &owner).await;
    assert!(
        items.is_empty(),
        "expected nothing persisted, got {items:?}"
    );
}

/// The identity of an object, not its place in the request, is what identifies it: the
/// same object carried twice earns the same key, in one batch and across batches, and the
/// domain holds one object for it. A different identity is a different object.
#[tokio::test]
async fn the_same_object_in_a_batch_is_the_same_object() {
    let emulator = Emulator::new();
    let domain = "idempotentdom";
    let owner = seeded(&emulator, domain).await;

    let object = contact("c-1", json!({ "phone": SEEDED_PHONE }), json!({}));
    let other = contact("c-2", json!({ "phone": SEEDED_PHONE }), json!({}));

    let (status, body) = batch_put(
        &emulator,
        domain,
        OBJECT_TYPE,
        vec![
            item("first", &object),
            item("second", &object),
            item("third", &other),
        ],
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} {body:?}");

    let answers = successful(&body);
    assert_eq!(
        answers.len(),
        3,
        "expected one answer per item, got {body:?}"
    );
    assert_eq!(
        answers[0].1, answers[1].1,
        "two items carrying the same object must earn the same key, got {body:?}"
    );
    assert_ne!(
        answers[0].1, answers[2].1,
        "items carrying different objects must earn different keys, got {body:?}"
    );

    let mut held = listed_keys(&listed(&emulator, domain, OBJECT_TYPE, &owner).await);
    held.sort();
    let mut expected = vec![answers[0].1.clone(), answers[2].1.clone()];
    expected.sort();
    assert_eq!(
        held, expected,
        "expected one held object per distinct identity, got {held:?}"
    );

    // A later batch re-ingesting the same object replaces it rather than storing a
    // second copy of it.
    let (status, body) = batch_put(
        &emulator,
        domain,
        OBJECT_TYPE,
        vec![item("again", &object)],
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} {body:?}");
    assert_eq!(
        successful(&body),
        vec![("again".to_string(), answers[0].1.clone())],
        "re-ingesting the same object must earn its key again, got {body:?}"
    );
    let mut after = listed_keys(&listed(&emulator, domain, OBJECT_TYPE, &owner).await);
    after.sort();
    assert_eq!(
        after, expected,
        "re-ingesting the same object must not add a second copy, got {after:?}"
    );
}

/// A domain or object type the store does not hold is not a property of one item, so it
/// refuses the whole call with `ResourceNotFoundException` (404) and persists nothing.
#[tokio::test]
async fn an_absent_domain_or_object_type_refuses_the_whole_batch() {
    let emulator = Emulator::new();
    let domain = "absentdom";
    let owner = seeded(&emulator, domain).await;
    let object = contact("d-1", json!({ "phone": SEEDED_PHONE }), json!({}));

    // (case, domain, object type)
    let rows: Vec<(&str, &str, &str)> = vec![
        ("a domain that is not stored", "nosuchdomain", OBJECT_TYPE),
        (
            "an object type the domain does not hold",
            domain,
            "Never_Defined",
        ),
    ];

    for (case, domain_name, object_type) in rows {
        let (status, body) = batch_put(
            &emulator,
            domain_name,
            object_type,
            vec![item("first", &object)],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body:?}"
        );
    }

    let items = listed(&emulator, domain, OBJECT_TYPE, &owner).await;
    assert!(
        items.is_empty(),
        "a refused batch must persist nothing, got {items:?}"
    );
}

/// `Items` is modelled `@length(min: 1, max: 10)`, so a batch is held to that count at
/// the boundary: each published bound is served and one step past either is refused as
/// `BadRequestException` (400).
#[tokio::test]
async fn the_batch_holds_its_modelled_item_count() {
    let emulator = Emulator::new();
    let domain = "countdom";
    seeded(&emulator, domain).await;

    // (case, how many items the batch carries, the status it earns)
    let rows: Vec<(&str, usize, u16)> = vec![
        ("below the minimum", 0, 400),
        ("at the minimum", 1, 200),
        ("at the maximum", 10, 200),
        ("above the maximum", 11, 400),
    ];

    for (case, count, expected) in rows {
        let items: Vec<Value> = (0..count)
            .map(|index| {
                item(
                    &format!("count-{count}-{index}"),
                    &contact(
                        &format!("k-{count}-{index}"),
                        json!({ "phone": SEEDED_PHONE }),
                        json!({}),
                    ),
                )
            })
            .collect();
        let (status, body) = batch_put(&emulator, domain, OBJECT_TYPE, items).await;
        assert_eq!(
            status, expected,
            "case {case:?} ({count} items): expected HTTP {expected}, got {status} {body:?}"
        );
        if expected == 200 {
            assert_eq!(
                successful(&body).len(),
                count,
                "case {case:?}: expected one answer per item, got {body:?}"
            );
        } else {
            assert_eq!(
                error_code(&body),
                "BadRequestException",
                "case {case:?}: expected BadRequestException, got {body:?}"
            );
        }
    }
}
