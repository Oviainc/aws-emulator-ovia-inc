// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `PutProfileObject`, `ListProfileObjects` and
//! `DeleteProfileObject`.
//!
//! The rule under test: an ingested profile object is persisted under the object type it
//! names and the unique key the service derives from the object's own identity, and is
//! tied to exactly one profile — the profile already indexed under any value the object
//! type's `PROFILE` keys select out of the object (its `SECONDARY` keys after all of
//! those), or, when the type allows profile creation and none matches, a profile inferred
//! from the data the type's `Target`s map the object onto. A listing then reports the
//! objects of one (object type, profile) pair from that store, in ascending unique-key
//! order, narrowed by the index values a key carries; a delete removes one of them; and a
//! domain, object type or object the store does not hold is `ResourceNotFoundException`
//! (404), while an `Object` that is not serialized from a JSON object is
//! `BadRequestException` (400).
//!
//! Every case goes through the real router, so what is asserted is the wire behaviour a
//! client sees. The HTTP plumbing and the create helpers are this crate's one test
//! harness, in `tests/all/domain_profile_lifecycle.rs`.

use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_customerprofiles_customerprofiles_20200815::state::{
    service_state, SECONDS_PER_DAY,
};

use crate::domain_profile_lifecycle::{error_code, item_ids, Emulator};
use crate::profile_object_types::error_message;

/// The acknowledgement a completed delete reports: fixed text, interpolating nothing.
const DELETE_ACKNOWLEDGEMENT: &str = "OK";

/// The `Fields` of the object type these tests ingest through: each field says where the
/// data sits in the object (`Source`) and which member of the standard profile model it
/// belongs to (`Target`). The `ContentType` values are the modelled `FieldContentType`
/// enum's (STRING | NUMBER | PHONE_NUMBER | EMAIL_ADDRESS | NAME), and the targets are
/// members of `Profile` as the model declares them.
fn contact_fields() -> Value {
    json!({
        "contactId": {
            "ContentType": "STRING",
            "Source": "_source.contactId",
            "Target": "_profile.Attributes.contactId",
        },
        "phone": {
            "ContentType": "PHONE_NUMBER",
            "Source": "_source.customer.phone",
            "Target": "_profile.PhoneNumber",
        },
        "email": {
            "ContentType": "EMAIL_ADDRESS",
            "Source": "_source.customer.email",
            "Target": "_profile.EmailAddress",
        },
        "account": {
            "ContentType": "STRING",
            "Source": "_source.accountNumber",
            "Target": "_profile.AccountNumber",
        },
        "firstName": {
            "ContentType": "NAME",
            "Source": "_source.customer.firstName",
            "Target": "_profile.FirstName",
        },
        "city": {
            "ContentType": "STRING",
            "Source": "_source.customer.city",
            "Target": "_profile.Address.City",
        },
    })
}

/// The `Keys` of that type, one per `StandardIdentifier` role the ingest reads: `UNIQUE`
/// identifies the object, `PROFILE` ties it to a profile, and `SECONDARY` is searched
/// "after all other PROFILE keys have been searched".
fn contact_keys() -> Value {
    json!({
        "_contactId": [{ "StandardIdentifiers": ["UNIQUE"], "FieldNames": ["contactId"] }],
        "_phone": [{ "StandardIdentifiers": ["PROFILE"], "FieldNames": ["phone"] }],
        "_account": [{ "StandardIdentifiers": ["PROFILE"], "FieldNames": ["account"] }],
        "_email": [{ "StandardIdentifiers": ["SECONDARY"], "FieldNames": ["email"] }],
    })
}

/// `PutProfileObjectType` over the wire, asserting it succeeded.
async fn put_object_type(emulator: &Emulator, domain: &str, name: &str, definition: Value) {
    let (status, body) = emulator
        .send(
            "PUT",
            &format!("/domains/{domain}/object-types/{name}"),
            Some(definition.clone()),
        )
        .await;
    assert_eq!(
        status, 200,
        "seeding object type {name:?} with {definition:?}: expected 200, got {status} {body:?}"
    );
}

/// The contact-record object type: every field and key above, allowed to infer a profile
/// only when `allow_profile_creation` says so.
pub(crate) async fn put_contact_type(
    emulator: &Emulator,
    domain: &str,
    name: &str,
    allow_profile_creation: bool,
) {
    put_object_type(
        emulator,
        domain,
        name,
        json!({
            "Description": "a contact record",
            "AllowProfileCreation": allow_profile_creation,
            "Fields": contact_fields(),
            "Keys": contact_keys(),
        }),
    )
    .await;
}

/// `PutProfileObject` over the wire: `PUT /domains/{DomainName}/profiles/objects`.
async fn put_object(
    emulator: &Emulator,
    domain: &str,
    object_type: &str,
    object: &Value,
) -> (u16, Value) {
    emulator
        .send(
            "PUT",
            &format!("/domains/{domain}/profiles/objects"),
            Some(json!({
                "ObjectTypeName": object_type,
                "Object": object.to_string(),
            })),
        )
        .await
}

/// `PutProfileObject`, asserting it succeeded, and answering the unique key it derived.
pub(crate) async fn ingest(
    emulator: &Emulator,
    domain: &str,
    object_type: &str,
    object: &Value,
) -> String {
    let (status, body) = put_object(emulator, domain, object_type, object).await;
    assert_eq!(
        status, 200,
        "ingesting {object:?}: expected 200, got {status} {body:?}"
    );
    body.get("ProfileObjectUniqueKey")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("ingesting {object:?}: no ProfileObjectUniqueKey in {body:?}"))
        .to_string()
}

/// `ListProfileObjects` over the wire: `POST /domains/{DomainName}/profiles/objects`.
async fn list(
    emulator: &Emulator,
    domain: &str,
    object_type: &str,
    profile_id: &str,
    extra: Value,
    query: &str,
) -> (u16, Value) {
    let mut body = json!({ "ObjectTypeName": object_type, "ProfileId": profile_id });
    for (member, value) in extra.as_object().expect("an object") {
        body[member] = value.clone();
    }
    emulator
        .send(
            "POST",
            &format!("/domains/{domain}/profiles/objects{query}"),
            Some(body),
        )
        .await
}

/// The `Items` of a listing, asserting it succeeded.
pub(crate) async fn listed(
    emulator: &Emulator,
    domain: &str,
    object_type: &str,
    profile_id: &str,
) -> Vec<Value> {
    let (status, body) = list(emulator, domain, object_type, profile_id, json!({}), "").await;
    assert_eq!(
        status, 200,
        "listing {object_type} objects of {profile_id}: expected 200, got {status} {body:?}"
    );
    body.get("Items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The unique keys a listing reports, in the order it reported them.
pub(crate) fn listed_keys(items: &[Value]) -> Vec<String> {
    items
        .iter()
        .map(|item| {
            item.get("ProfileObjectUniqueKey")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// `DeleteProfileObject` over the wire:
/// `POST /domains/{DomainName}/profiles/objects/delete`.
async fn delete_object(
    emulator: &Emulator,
    domain: &str,
    object_type: &str,
    profile_id: &str,
    unique_key: &str,
) -> (u16, Value) {
    emulator
        .send(
            "POST",
            &format!("/domains/{domain}/profiles/objects/delete"),
            Some(json!({
                "ObjectTypeName": object_type,
                "ProfileId": profile_id,
                "ProfileObjectUniqueKey": unique_key,
            })),
        )
        .await
}

/// `SearchProfiles` for one predefined key, asserting it succeeded.
async fn search(emulator: &Emulator, domain: &str, key_name: &str, value: &str) -> Value {
    let (status, body) = emulator
        .post(
            &format!("/domains/{domain}/profiles/search"),
            json!({ "KeyName": key_name, "Values": [value] }),
        )
        .await;
    assert_eq!(
        status, 200,
        "searching {key_name}={value}: expected 200, got {status} {body:?}"
    );
    body
}

/// A contact-record object: the shape the fields above read, with the members a row
/// chooses.
pub(crate) fn contact(contact_id: &str, customer: Value, extra: Value) -> Value {
    let mut object = json!({ "contactId": contact_id, "customer": customer });
    for (member, value) in extra.as_object().expect("an object") {
        object[member] = value.clone();
    }
    object
}

/// An ingested object is tied to the profile already indexed under a value its type's
/// keys select out of it — over a table spanning which standard member the matching key
/// maps onto, how deep in the object the value sits, and whether the value arrives as a
/// JSON string or a JSON number. Every row ingests into a domain holding TWO profiles, so
/// a row that matched nothing, or matched the wrong profile, fails.
#[tokio::test]
async fn an_ingested_object_is_tied_to_the_profile_its_keys_identify() {
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "a PROFILE key on a phone number, nested two levels down",
            json!({ "PhoneNumber": "+15551110001" }),
            contact("c-1", json!({ "phone": "+15551110001" }), json!({})),
        ),
        (
            "a PROFILE key on an account number at the top level",
            json!({ "AccountNumber": "A-0002" }),
            contact("c-2", json!({}), json!({ "accountNumber": "A-0002" })),
        ),
        (
            "a PROFILE key whose value arrives as a JSON number",
            json!({ "AccountNumber": "4242" }),
            contact("c-3", json!({}), json!({ "accountNumber": 4242 })),
        ),
        (
            "a SECONDARY key on an email address, searched after the PROFILE keys",
            json!({ "EmailAddress": "ada@example.com" }),
            contact(
                "c-4",
                json!({ "email": "ada@example.com", "phone": "+15559999999" }),
                json!({}),
            ),
        ),
        (
            "a PROFILE key beside members the object type maps onto no key",
            json!({ "PhoneNumber": "+15551110005" }),
            contact(
                "c-5",
                json!({ "phone": "+15551110005", "firstName": "Grace", "city": "Baltimore" }),
                json!({ "notes": "ignored by every field" }),
            ),
        ),
    ];

    for (index, (case, profile, object)) in rows.into_iter().enumerate() {
        let emulator = Emulator::new();
        let domain = format!("matchdom{index}");
        emulator.create_domain(&domain, 90).await;
        put_contact_type(&emulator, &domain, "Contact_Record", true).await;

        let target_id = emulator.create_profile(&domain, profile.clone()).await;
        // A second profile sharing nothing with the object: it must stay empty.
        let other_id = emulator
            .create_profile(
                &domain,
                json!({ "PhoneNumber": "+15550000000", "AccountNumber": "A-OTHER" }),
            )
            .await;

        let unique_key = ingest(&emulator, &domain, "Contact_Record", &object).await;
        assert!(
            !unique_key.is_empty() && unique_key.len() <= 255,
            "case {case:?}: the derived key must be within the modelled 1..255, \
             got {unique_key:?}"
        );

        let items = listed(&emulator, &domain, "Contact_Record", &target_id).await;
        assert_eq!(
            listed_keys(&items),
            vec![unique_key.clone()],
            "case {case:?}: expected the object under profile {target_id}, got {items:?}"
        );
        assert_eq!(
            items[0].get("Object").and_then(Value::as_str),
            Some(object.to_string().as_str()),
            "case {case:?}: expected the object as it was ingested, got {items:?}"
        );
        assert_eq!(
            items[0].get("ObjectTypeName").and_then(Value::as_str),
            Some("Contact_Record"),
            "case {case:?}: expected the type it was ingested under, got {items:?}"
        );

        let others = listed(&emulator, &domain, "Contact_Record", &other_id).await;
        assert!(
            others.is_empty(),
            "case {case:?}: a profile the object's keys do not identify must report no \
             object, got {others:?}"
        );
    }
}

/// The unique key is derived from the object's own identity, so it is stable: the same
/// identity is the same object and replaces it, a different identity is another object.
/// Spanned over a type that defines a UNIQUE key (the identity is that key's value) and
/// one that defines none (the identity is the object itself).
#[tokio::test]
async fn the_unique_key_follows_the_objects_identity() {
    let emulator = Emulator::new();
    emulator.create_domain("keydom", 90).await;
    put_contact_type(&emulator, "keydom", "Contact_Record", true).await;
    // The same fields, with no key marked UNIQUE: nothing identifies an object but the
    // object itself.
    put_object_type(
        &emulator,
        "keydom",
        "Unkeyed_Record",
        json!({
            "Description": "a record with no unique key",
            "AllowProfileCreation": true,
            "Fields": contact_fields(),
            "Keys": {
                "_phone": [{ "StandardIdentifiers": ["PROFILE"], "FieldNames": ["phone"] }],
            },
        }),
    )
    .await;

    let profile_id = emulator
        .create_profile("keydom", json!({ "PhoneNumber": "+15552220001" }))
        .await;
    let customer = json!({ "phone": "+15552220001" });

    let rows: Vec<(&str, &str, Value, Value, bool)> = vec![
        (
            "a UNIQUE key value ingested twice with different data",
            "Contact_Record",
            contact("same-id", customer.clone(), json!({ "notes": "first" })),
            contact("same-id", customer.clone(), json!({ "notes": "second" })),
            true,
        ),
        (
            "two different UNIQUE key values",
            "Contact_Record",
            contact("id-a", customer.clone(), json!({})),
            contact("id-b", customer.clone(), json!({})),
            false,
        ),
        (
            "no UNIQUE key, the same object twice",
            "Unkeyed_Record",
            contact("ignored", customer.clone(), json!({ "notes": "same" })),
            contact("ignored", customer.clone(), json!({ "notes": "same" })),
            true,
        ),
        (
            "no UNIQUE key, two different objects",
            "Unkeyed_Record",
            contact("ignored", customer.clone(), json!({ "notes": "one" })),
            contact("ignored", customer.clone(), json!({ "notes": "two" })),
            false,
        ),
    ];

    for (case, object_type, first, second, same_object) in rows {
        // Each row starts from an empty listing for its own type.
        for item in listed(&emulator, "keydom", object_type, &profile_id).await {
            let key = item
                .get("ProfileObjectUniqueKey")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let (status, body) = delete_object(&emulator, "keydom", object_type, &profile_id, key)
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: clearing {key} failed with {status} {body:?}"
            );
        }

        let first_key = ingest(&emulator, "keydom", object_type, &first).await;
        let second_key = ingest(&emulator, "keydom", object_type, &second).await;
        assert_eq!(
            first_key == second_key,
            same_object,
            "case {case:?}: expected same_object={same_object}, got keys {first_key} and \
             {second_key}"
        );

        let items = listed(&emulator, "keydom", object_type, &profile_id).await;
        let expected = if same_object { 1 } else { 2 };
        assert_eq!(
            items.len(),
            expected,
            "case {case:?}: expected {expected} stored object(s), got {items:?}"
        );
        if same_object {
            assert_eq!(
                items[0].get("Object").and_then(Value::as_str),
                Some(second.to_string().as_str()),
                "case {case:?}: a re-ingest must replace the object it identifies, \
                 got {items:?}"
            );
        }
    }
}

/// With no profile to tie the object to, the type's `AllowProfileCreation` decides: a type
/// that allows it gets an inferred profile carrying the data its `Target`s map, found by
/// the predefined keys that profile is indexed under, and a second object with the same
/// key value maps onto that same profile; a type that does not allow it creates no
/// profile at all.
#[tokio::test]
async fn an_unmatched_object_infers_a_profile_only_where_the_type_allows_one() {
    let emulator = Emulator::new();
    emulator.create_domain("inferdom", 90).await;
    put_contact_type(&emulator, "inferdom", "Inferring", true).await;
    put_contact_type(&emulator, "inferdom", "Not_Inferring", false).await;

    // A type that allows profile creation: the object maps onto a new profile.
    let object = contact(
        "infer-1",
        json!({
            "phone": "+15553330001",
            "email": "inferred@example.com",
            "firstName": "Ada",
            "city": "London",
        }),
        json!({ "accountNumber": "A-INFER" }),
    );
    let unique_key = ingest(&emulator, "inferdom", "Inferring", &object).await;

    let found = search(&emulator, "inferdom", "_phone", "+15553330001").await;
    let ids = item_ids(&found);
    assert_eq!(
        ids.len(),
        1,
        "case \"an inferred profile\": expected exactly one profile indexed under the \
         object's phone number, got {found:?}"
    );
    let inferred = &ids[0];
    let items = found.get("Items").and_then(Value::as_array).expect("Items");
    for (member, expected) in [
        ("PhoneNumber", json!("+15553330001")),
        ("EmailAddress", json!("inferred@example.com")),
        ("FirstName", json!("Ada")),
        ("AccountNumber", json!("A-INFER")),
        ("Address", json!({ "City": "London" })),
        ("Attributes", json!({ "contactId": "infer-1" })),
    ] {
        assert_eq!(
            items[0].get(member),
            Some(&expected),
            "case \"an inferred profile\": expected {member} = {expected:?} mapped from the \
             object, got {:?}",
            items[0].get(member)
        );
    }
    assert_eq!(
        listed_keys(&listed(&emulator, "inferdom", "Inferring", inferred).await),
        vec![unique_key.clone()],
        "case \"an inferred profile\": the object must be listed under it"
    );

    // "Any additional Contact Records with the same phone number will be mapped to the
    // same inferred profile."
    let second = contact("infer-2", json!({ "phone": "+15553330001" }), json!({}));
    let second_key = ingest(&emulator, "inferdom", "Inferring", &second).await;
    let again = search(&emulator, "inferdom", "_phone", "+15553330001").await;
    assert_eq!(
        item_ids(&again),
        ids,
        "case \"a second object with the same key value\": no further profile may be \
         inferred, got {again:?}"
    );
    let items = listed(&emulator, "inferdom", "Inferring", inferred).await;
    assert_eq!(
        listed_keys(&items).into_iter().collect::<BTreeSet<String>>(),
        BTreeSet::from([unique_key, second_key]),
        "case \"a second object with the same key value\": both objects belong to the one \
         profile, got {items:?}"
    );

    // A type that does not allow profile creation: nothing is inferred, and the ingest
    // still answers with the key it derived.
    let unmatched = contact("no-infer", json!({ "phone": "+15554440001" }), json!({}));
    let key = ingest(&emulator, "inferdom", "Not_Inferring", &unmatched).await;
    assert!(
        !key.is_empty(),
        "case \"a type that infers nothing\": the ingest still derives a unique key"
    );
    let found = search(&emulator, "inferdom", "_phone", "+15554440001").await;
    assert!(
        item_ids(&found).is_empty(),
        "case \"a type that infers nothing\": no profile may be created, got {found:?}"
    );

    // An object the type maps nothing out of gives nothing to infer a profile FROM, so
    // even a type that allows one creates none.
    let unmappable = json!({ "nothing": "the type maps no field onto this" });
    let key = ingest(&emulator, "inferdom", "Inferring", &unmappable).await;
    assert!(
        !key.is_empty(),
        "case \"an object that maps nothing\": the ingest still derives a unique key"
    );
    let found = search(&emulator, "inferdom", "_phone", "+15553330001").await;
    assert_eq!(
        item_ids(&found),
        ids,
        "case \"an object that maps nothing\": no further profile may be inferred, \
         got {found:?}"
    );
}

/// The listing is a page of the objects of one (object type, profile) pair, in ascending
/// unique-key order: a table over `MaxResults` on its published bounds walks every page
/// through `NextToken` and sees each object exactly once, in that order.
#[tokio::test]
async fn the_listing_pages_in_unique_key_order() {
    let emulator = Emulator::new();
    emulator.create_domain("pagedom", 90).await;
    put_contact_type(&emulator, "pagedom", "Contact_Record", true).await;
    let profile_id = emulator
        .create_profile("pagedom", json!({ "PhoneNumber": "+15556660001" }))
        .await;

    let mut all: Vec<String> = Vec::new();
    for index in 0..5 {
        all.push(
            ingest(
                &emulator,
                "pagedom",
                "Contact_Record",
                &contact(
                    &format!("page-{index}"),
                    json!({ "phone": "+15556660001" }),
                    json!({}),
                ),
            )
            .await,
        );
    }
    all.sort();

    // `MaxResults` is `maxSize100`, range 1..100: both published bounds, and the sizes
    // either side of the page boundary in between.
    for max_results in [1, 2, 4, 5, 100] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let query = match &token {
                Some(token) => format!("?max-results={max_results}&next-token={token}"),
                None => format!("?max-results={max_results}"),
            };
            let (status, body) = list(
                &emulator,
                "pagedom",
                "Contact_Record",
                &profile_id,
                json!({}),
                &query,
            )
            .await;
            assert_eq!(
                status, 200,
                "case max-results={max_results}: expected 200, got {status} {body:?}"
            );
            let page = body
                .get("Items")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert!(
                page.len() <= max_results,
                "case max-results={max_results}: a page may not exceed it, got {page:?}"
            );
            walked.extend(listed_keys(&page));
            token = body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, all,
            "case max-results={max_results}: paging must report every object once, in \
             unique-key order"
        );
    }

    // The same request twice reports the same page: a listing changes nothing.
    let first = listed(&emulator, "pagedom", "Contact_Record", &profile_id).await;
    let again = listed(&emulator, "pagedom", "Contact_Record", &profile_id).await;
    assert_eq!(
        first, again,
        "two listings of an unchanged profile must be identical"
    );
}

/// An `ObjectFilter` keeps the objects carrying one of its values for the key it names —
/// over a table of one value, several, a value nothing carries, a key the type does not
/// define, and a key marked `LOOKUP_ONLY`, which "is not persisted to be used for
/// searching" and so indexes nothing.
#[tokio::test]
async fn an_object_filter_selects_by_the_index_values_a_key_carries() {
    let emulator = Emulator::new();
    emulator.create_domain("filterdom", 90).await;
    put_object_type(
        &emulator,
        "filterdom",
        "Filtered",
        json!({
            "Description": "a record with a lookup-only key",
            "AllowProfileCreation": true,
            "Fields": contact_fields(),
            "Keys": {
                "_contactId": [{ "StandardIdentifiers": ["UNIQUE"], "FieldNames": ["contactId"] }],
                "_phone": [{ "StandardIdentifiers": ["PROFILE"], "FieldNames": ["phone"] }],
                "lookupCity": [{
                    "StandardIdentifiers": ["LOOKUP_ONLY"],
                    "FieldNames": ["city"],
                }],
            },
        }),
    )
    .await;
    let profile_id = emulator
        .create_profile("filterdom", json!({ "PhoneNumber": "+15557770001" }))
        .await;

    let mut keys: Vec<String> = Vec::new();
    for contact_id in ["f-1", "f-2", "f-3"] {
        keys.push(
            ingest(
                &emulator,
                "filterdom",
                "Filtered",
                &contact(
                    contact_id,
                    json!({ "phone": "+15557770001", "city": "Oslo" }),
                    json!({}),
                ),
            )
            .await,
        );
    }

    let rows: Vec<(&str, Value, usize)> = vec![
        (
            "one value of the UNIQUE key",
            json!({ "KeyName": "_contactId", "Values": ["f-2"] }),
            1,
        ),
        (
            "two values of the UNIQUE key",
            json!({ "KeyName": "_contactId", "Values": ["f-1", "f-3"] }),
            2,
        ),
        (
            "the PROFILE key every object carries",
            json!({ "KeyName": "_phone", "Values": ["+15557770001"] }),
            3,
        ),
        (
            "a value no object carries",
            json!({ "KeyName": "_contactId", "Values": ["f-9"] }),
            0,
        ),
        (
            "a key the object type does not define",
            json!({ "KeyName": "_orderId", "Values": ["f-1"] }),
            0,
        ),
        (
            "a LOOKUP_ONLY key, which indexes nothing",
            json!({ "KeyName": "lookupCity", "Values": ["Oslo"] }),
            0,
        ),
    ];

    for (case, filter, expected) in rows {
        let (status, body) = list(
            &emulator,
            "filterdom",
            "Filtered",
            &profile_id,
            json!({ "ObjectFilter": filter }),
            "",
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body:?}"
        );
        let items = body
            .get("Items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            items.len(),
            expected,
            "case {case:?}: expected {expected} object(s), got {items:?}"
        );
    }

    // Unfiltered, every object is still there: the filter narrows the answer and stores
    // nothing.
    assert_eq!(
        listed(&emulator, "filterdom", "Filtered", &profile_id)
            .await
            .len(),
        keys.len(),
        "an unfiltered listing must still report every object"
    );
}

/// A delete removes exactly the object it names, and reports the fixed acknowledgement;
/// everything else it could name is `ResourceNotFoundException` (404).
#[tokio::test]
async fn a_delete_removes_one_object_and_refuses_anything_it_does_not_hold() {
    let emulator = Emulator::new();
    emulator.create_domain("deldom", 90).await;
    put_contact_type(&emulator, "deldom", "Contact_Record", true).await;
    let profile_id = emulator
        .create_profile("deldom", json!({ "PhoneNumber": "+15558880001" }))
        .await;

    let mut keys: Vec<String> = Vec::new();
    for contact_id in ["d-1", "d-2"] {
        keys.push(
            ingest(
                &emulator,
                "deldom",
                "Contact_Record",
                &contact(contact_id, json!({ "phone": "+15558880001" }), json!({})),
            )
            .await,
        );
    }

    let (status, body) = delete_object(&emulator, "deldom", "Contact_Record", &profile_id, &keys[0])
        .await;
    assert_eq!(
        status, 200,
        "case \"a held object\": expected 200, got {status} {body:?}"
    );
    assert_eq!(
        body.get("Message").and_then(Value::as_str),
        Some(DELETE_ACKNOWLEDGEMENT),
        "case \"a held object\": expected the fixed acknowledgement, got {body:?}"
    );
    assert_eq!(
        listed_keys(&listed(&emulator, "deldom", "Contact_Record", &profile_id).await),
        vec![keys[1].clone()],
        "case \"a held object\": only the named object may go"
    );

    let refusals: Vec<(&str, &str, &str, &str, String)> = vec![
        (
            "the same object again",
            "deldom",
            "Contact_Record",
            profile_id.as_str(),
            keys[0].clone(),
        ),
        (
            "a unique key the profile never held",
            "deldom",
            "Contact_Record",
            profile_id.as_str(),
            "f".repeat(64),
        ),
        (
            "an object type the domain does not hold",
            "deldom",
            "Never_Defined",
            profile_id.as_str(),
            keys[1].clone(),
        ),
        (
            "a profile the object is not tied to",
            "deldom",
            "Contact_Record",
            "00000000000000000000000000000000",
            keys[1].clone(),
        ),
        (
            "a domain that is not stored",
            "nosuchdomain",
            "Contact_Record",
            profile_id.as_str(),
            keys[1].clone(),
        ),
    ];

    for (case, domain, object_type, profile, unique_key) in refusals {
        let (status, body) = delete_object(&emulator, domain, object_type, profile, &unique_key)
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException, got {status} {body:?}"
        );
    }

    // The object the refusals named is still there.
    assert_eq!(
        listed_keys(&listed(&emulator, "deldom", "Contact_Record", &profile_id).await),
        vec![keys[1].clone()],
        "a refused delete must remove nothing"
    );
}

/// The objects of a profile are data pertaining to it: deleting the profile takes them,
/// merging the profile moves them onto the profile that absorbed it, and deleting the
/// object type takes everything ingested through it.
#[tokio::test]
async fn an_objects_lifetime_follows_the_resources_it_hangs_off() {
    let emulator = Emulator::new();
    emulator.create_domain("cascadedom", 90).await;
    put_contact_type(&emulator, "cascadedom", "Contact_Record", true).await;

    // Deleting the profile.
    let gone = emulator
        .create_profile("cascadedom", json!({ "PhoneNumber": "+15551112222" }))
        .await;
    ingest(
        &emulator,
        "cascadedom",
        "Contact_Record",
        &contact("x-1", json!({ "phone": "+15551112222" }), json!({})),
    )
    .await;
    let (status, body) = emulator.post(
        "/domains/cascadedom/profiles/delete",
        json!({ "ProfileId": gone }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"DeleteProfile\": expected 200, got {status} {body:?}"
    );
    assert!(
        listed(&emulator, "cascadedom", "Contact_Record", &gone)
            .await
            .is_empty(),
        "case \"DeleteProfile\": the deleted profile's objects must go with it"
    );

    // Merging the profile.
    let main = emulator
        .create_profile("cascadedom", json!({ "PhoneNumber": "+15553334444" }))
        .await;
    let merged = emulator
        .create_profile("cascadedom", json!({ "PhoneNumber": "+15555556666" }))
        .await;
    let moved = ingest(
        &emulator,
        "cascadedom",
        "Contact_Record",
        &contact("x-2", json!({ "phone": "+15555556666" }), json!({})),
    )
    .await;
    let (status, body) = emulator
        .post(
            "/domains/cascadedom/profiles/objects/merge",
            json!({ "MainProfileId": main, "ProfileIdsToBeMerged": [merged] }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"MergeProfiles\": expected 200, got {status} {body:?}"
    );
    assert_eq!(
        listed_keys(&listed(&emulator, "cascadedom", "Contact_Record", &main).await),
        vec![moved],
        "case \"MergeProfiles\": the merged profile's objects belong to the main profile"
    );
    assert!(
        listed(&emulator, "cascadedom", "Contact_Record", &merged)
            .await
            .is_empty(),
        "case \"MergeProfiles\": the merged profile holds nothing afterwards"
    );

    // Deleting the object type.
    let (status, body) = emulator
        .send("DELETE", "/domains/cascadedom/object-types/Contact_Record", None)
        .await;
    assert_eq!(
        status, 200,
        "case \"DeleteProfileObjectType\": expected 200, got {status} {body:?}"
    );
    put_contact_type(&emulator, "cascadedom", "Contact_Record", true).await;
    assert!(
        listed(&emulator, "cascadedom", "Contact_Record", &main)
            .await
            .is_empty(),
        "case \"DeleteProfileObjectType\": a redefined type starts with no object"
    );
}

/// An object is kept for the retention its object type sets, falling back to the domain's
/// own default, and is reported for no longer than that: the store is aged directly,
/// because the retention is measured in days. The count the object type reports as
/// available follows the objects that are still held.
#[tokio::test]
async fn an_object_is_reported_only_until_its_retention_runs_out() {
    let emulator = Emulator::new();
    // The domain's `DefaultExpirationDays` is the fallback retention; the object type
    // below sets its own, so the two are told apart.
    emulator.create_domain("expiredom", 30).await;
    put_object_type(
        &emulator,
        "expiredom",
        "Short_Lived",
        json!({
            "Description": "a record with its own retention",
            "AllowProfileCreation": true,
            "ExpirationDays": 2,
            "MaxProfileObjectCount": 5,
            "Fields": contact_fields(),
            "Keys": contact_keys(),
        }),
    )
    .await;
    let profile_id = emulator
        .create_profile("expiredom", json!({ "PhoneNumber": "+15559990001" }))
        .await;

    let two_days = Duration::from_secs(2 * SECONDS_PER_DAY);
    let rows: Vec<(&str, Duration, usize)> = vec![
        ("ingested just now", Duration::ZERO, 1),
        ("one second short of the retention", two_days - Duration::from_secs(1), 1),
        ("exactly at the retention", two_days, 0),
        ("long past the retention", two_days * 10, 0),
    ];

    for (case, age, expected) in rows {
        let unique_key = ingest(
            &emulator,
            "expiredom",
            "Short_Lived",
            &contact("e-1", json!({ "phone": "+15559990001" }), json!({})),
        )
        .await;

        // Age the stored object through the SAME shared engine the router handed the
        // service: the retention is days long, so no test can wait it out.
        {
            let state = service_state(&*emulator.engine).expect("the service's store");
            let mut store = state.lock();
            let domain = store
                .domains
                .get_mut("expiredom")
                .expect("the domain created above");
            let stored = domain
                .profile_objects
                .get_mut(&(
                    "Short_Lived".to_string(),
                    profile_id.clone(),
                    unique_key.clone(),
                ))
                .unwrap_or_else(|| panic!("case {case:?}: the ingested object is stored"));
            stored.created_at = SystemTime::now() - age;
        }

        let items = listed(&emulator, "expiredom", "Short_Lived", &profile_id).await;
        assert_eq!(
            items.len(),
            expected,
            "case {case:?}: expected {expected} object(s) still reported, got {items:?}"
        );

        // `MaxAvailableProfileObjectCount` is the assigned count less what is held, so it
        // follows the same reckoning.
        let (status, object_type) = emulator
            .send("GET", "/domains/expiredom/object-types/Short_Lived", None)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetProfileObjectType 200, got {status} {object_type:?}"
        );
        assert_eq!(
            object_type
                .get("MaxAvailableProfileObjectCount")
                .and_then(Value::as_i64),
            Some(5 - expected as i64),
            "case {case:?}: expected the available count to follow the objects held, \
             got {object_type:?}"
        );
    }
}

/// The refusals: an `Object` that is not serialized from a JSON object, and a domain or
/// object type the store does not hold. Nothing is persisted for any of them.
#[tokio::test]
async fn the_object_operations_refuse_invalid_input_and_absent_resources() {
    let emulator = Emulator::new();
    emulator.create_domain("baddom", 90).await;
    put_contact_type(&emulator, "baddom", "Contact_Record", true).await;
    let profile_id = emulator
        .create_profile("baddom", json!({ "PhoneNumber": "+15551230001" }))
        .await;

    // `Object` is `stringifiedJson`, len 1..256000: both published bounds, one past the
    // maximum, and the JSON values that are not an object. The envelope `{"pad":"…"}` is
    // ten characters, so the padding makes up the rest of each bound.
    let at_the_maximum = format!(r#"{{"pad":"{}"}}"#, "p".repeat(256_000 - 10));
    let past_the_maximum = format!(r#"{{"pad":"{}"}}"#, "p".repeat(256_000 - 9));
    assert_eq!(
        (at_the_maximum.len(), past_the_maximum.len()),
        (256_000, 256_001),
        "the bound rows must sit exactly on the modelled maximum and one past it"
    );
    let rows: Vec<(&str, String, u16)> = vec![
        ("an empty object", "{}".to_string(), 200),
        ("at the maximum length", at_the_maximum, 200),
        ("one past the maximum length", past_the_maximum, 400),
        ("below the minimum length", String::new(), 400),
        ("text that is not JSON at all", "not json".to_string(), 400),
        ("a truncated JSON object", r#"{"a":"#.to_string(), 400),
        ("a JSON array", r#"[{"a":1}]"#.to_string(), 400),
        ("a JSON string", r#""a contact""#.to_string(), 400),
        ("a JSON number", "42".to_string(), 400),
        ("a JSON null", "null".to_string(), 400),
    ];

    for (case, object, expected_status) in rows {
        let (status, body) = emulator
            .send(
                "PUT",
                "/domains/baddom/profiles/objects",
                Some(json!({ "ObjectTypeName": "Contact_Record", "Object": object })),
            )
            .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {body:?}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "BadRequestException",
                "case {case:?}: expected BadRequestException, got {body:?}"
            );
        }
    }

    // A domain, and an object type within it, the store does not hold.
    let absent: Vec<(&str, &str, &str)> = vec![
        ("a domain that is not stored", "nosuchdomain", "Contact_Record"),
        ("an object type the domain does not hold", "baddom", "Never_Defined"),
    ];
    for (case, domain, object_type) in absent {
        let (status, body) = put_object(&emulator, domain, object_type, &json!({ "a": 1 })).await;
        assert_eq!(
            (status, error_code(&body)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 on the ingest, got {status} {body:?}"
        );
        let named = if domain == "baddom" { object_type } else { domain };
        assert!(
            error_message(&body).contains(named),
            "case {case:?}: the refusal must name {named}, got {body:?}"
        );

        let (status, body) = list(&emulator, domain, object_type, &profile_id, json!({}), "").await;
        assert_eq!(
            (status, error_code(&body)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 on the listing, got {status} {body:?}"
        );
    }

    // A profile that holds no object of the type is an empty page, not a refusal: the
    // listing reports the objects of a pair, and that pair holds none.
    let (status, body) = list(
        &emulator,
        "baddom",
        "Contact_Record",
        "ffffffffffffffffffffffffffffffff",
        json!({}),
        "",
    )
    .await;
    assert_eq!(
        (status, body.get("Items")),
        (200, Some(&json!([]))),
        "case \"a profile holding no object\": expected an empty page, got {status} {body:?}"
    );

    // Every refused ingest above persisted nothing: a valid one is the only object the
    // profile holds afterwards.
    ingest(
        &emulator,
        "baddom",
        "Contact_Record",
        &contact("ok-1", json!({ "phone": "+15551230001" }), json!({})),
    )
    .await;
    let items = listed(&emulator, "baddom", "Contact_Record", &profile_id).await;
    assert_eq!(
        items.len(),
        1,
        "only the accepted ingest may have been stored for this profile, got {items:?}"
    );
}
