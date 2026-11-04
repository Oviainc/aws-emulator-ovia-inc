// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteCalculatedAttributeDefinition`.
//!
//! The rule under test: the (domain, name) pair identifies exactly one calculated
//! attribute definition, so a delete removes that one record — the readers stop serving
//! it, every other definition and every other domain is untouched, and the name is free
//! for a later create — while a pair the store does not hold is nothing to delete and is
//! refused `ResourceNotFoundException` (404).
//!
//! Every case goes through the real router, so what is asserted is the wire behaviour a
//! client sees. The HTTP plumbing lives in this crate's one test harness,
//! `tests/all/domain_profile_lifecycle.rs`; the calculated-attribute request helpers in
//! `tests/all/calculated_attribute_definitions.rs`.

use serde_json::{json, Value};

use crate::calculated_attribute_definitions::{
    attribute_details, create, definition, exists_message, get, item_names, list,
};
use crate::domain_profile_lifecycle::{error_code, Emulator};
use crate::profile_object_types::error_message;

/// `DeleteCalculatedAttributeDefinition` over the wire:
/// `DELETE /domains/{DomainName}/calculated-attributes/{CalculatedAttributeName}`.
async fn delete(emulator: &Emulator, domain: &str, name: &str) -> (u16, Value) {
    emulator
        .send(
            "DELETE",
            &format!("/domains/{domain}/calculated-attributes/{name}"),
            None,
        )
        .await
}

/// The sentence this service refuses a reference to a calculated attribute definition a
/// domain does not hold with: fixed text naming the resource kind and "not found", whose
/// only variable part is the name the request's `{CalculatedAttributeName}` label carried
/// — the sibling of the already-exists sentence a create of a held name is refused with.
fn absent_message(name: &str) -> String {
    format!("Calculated attribute definition with name {name} not found")
}

/// A definition's STORED members: what the create persisted, with the two members the
/// readers compute from the clock on every read (`Status` and `Readiness`, which report
/// how far the indexing of the definition's historical data has got) dropped, so
/// comparing two reads compares what a delete could have disturbed.
fn stored_members(body: &Value) -> Value {
    let mut members = body.clone();
    if let Some(object) = members.as_object_mut() {
        object.remove("Status");
        object.remove("Readiness");
    }
    members
}

/// Whether a response body carries no members — what a success of an operation whose
/// model declares no output members answers with.
fn is_empty_body(body: &Value) -> bool {
    match body {
        Value::Null => true,
        Value::Object(members) => members.is_empty(),
        _ => false,
    }
}

/// The definitions the delete table removes, spanning what the rule generalises over: the
/// form of the name, the statistic, and how much optional configuration the definition was
/// created with (the delete addresses it by name alone, so none of that may change the
/// outcome).
fn rows() -> Vec<(&'static str, String, Value)> {
    vec![
        (
            "a definition created from the required members only",
            "Minimal".to_string(),
            definition("SUM"),
        ),
        (
            "a name that starts with the underscore the pattern allows",
            "_leading_underscore".to_string(),
            definition("COUNT"),
        ),
        (
            "a name carrying the digits and hyphens the pattern allows",
            "Order-Total-90d".to_string(),
            definition("FIRST_OCCURRENCE"),
        ),
        (
            "a name at the modelled minimum length",
            "A".to_string(),
            definition("MINIMUM"),
        ),
        (
            "a name at the modelled maximum length",
            format!("L{}", "o".repeat(254)),
            definition("MAXIMUM"),
        ),
        (
            "a definition over two attributes",
            "Two_Attributes".to_string(),
            json!({
                "AttributeDetails": attribute_details("Orders", &["Amount", "Discount"]),
                "Statistic": "AVERAGE",
            }),
        ),
        (
            "a definition carrying every optional member",
            "Fully_Configured".to_string(),
            json!({
                "AttributeDetails": attribute_details("Orders", &["Amount"]),
                "Statistic": "SUM",
                "DisplayName": "Order total",
                "Description": "the total of every order",
                "Conditions": {
                    "Range": { "Value": 30, "Unit": "DAYS" },
                    "ObjectCount": 10,
                    "Threshold": { "Value": "100", "Operator": "GREATER_THAN" },
                },
                "UseHistoricalData": false,
                "Tags": { "team": "crm" },
            }),
        ),
        (
            "a definition that asked for historical data",
            "Historical".to_string(),
            json!({
                "AttributeDetails": attribute_details("Orders", &["Amount"]),
                "Statistic": "COUNT",
                "UseHistoricalData": true,
            }),
        ),
    ]
}

/// RULE: a delete removes exactly the definition the (domain, name) pair identifies. Each
/// row is created, deleted, and then checked three ways — the reader reports it absent,
/// the listing no longer carries it, and the name is free for a new create — while every
/// row not yet deleted is still listed and still reads back unchanged.
#[tokio::test]
async fn a_delete_removes_exactly_the_named_definition() {
    let emulator = Emulator::new();
    emulator.create_domain("deldom", 30).await;

    let rows = rows();
    for (case, name, body) in &rows {
        let (status, created) = create(&emulator, "deldom", name, body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateCalculatedAttributeDefinition 200, got {status} \
             {created:?}"
        );
    }

    // What every surviving row reads back as, captured before any delete, so a later
    // comparison catches a delete that disturbed a definition it does not address.
    let mut survivors: Vec<(&str, String, Value)> = Vec::new();
    for (case, name, _) in &rows {
        let (status, read) = get(&emulator, "deldom", name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the created definition to be readable, got {status} \
             {read:?}"
        );
        survivors.push((case, name.clone(), stored_members(&read)));
    }

    while !survivors.is_empty() {
        let (case, name, _) = survivors.remove(0);

        let (status, deleted) = delete(&emulator, "deldom", &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteCalculatedAttributeDefinition 200, got {status} \
             {deleted:?}"
        );
        assert!(
            is_empty_body(&deleted),
            "case {case:?}: the operation models no output members, so the response must \
             carry none, got {deleted:?}"
        );

        // The reader no longer serves it, and says so the way this service words an
        // absent per-domain resource.
        let (status, read) = get(&emulator, "deldom", &name).await;
        assert_eq!(
            (status, error_code(&read)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException after the delete, got \
             {status} {read:?}"
        );
        assert_eq!(
            error_message(&read),
            absent_message(&name),
            "case {case:?}: expected the absent-definition sentence, got {read:?}"
        );

        // The listing reports exactly the definitions not yet deleted, and each of them
        // still reads back byte for byte as it did before any delete ran.
        let (status, listed) = list(&emulator, "deldom", "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListCalculatedAttributeDefinitions 200, got {status} \
             {listed:?}"
        );
        let listed_names = item_names(&listed);
        assert!(
            !listed_names.contains(&name),
            "case {case:?}: the deleted name must not be listed, got {listed_names:?}"
        );
        for (survivor_case, survivor_name, before) in &survivors {
            assert!(
                listed_names.contains(survivor_name),
                "case {case:?}: deleting it must leave {survivor_case:?} listed, got \
                 {listed_names:?}"
            );
            let (status, read) = get(&emulator, "deldom", survivor_name).await;
            assert_eq!(
                (status, &stored_members(&read)),
                (200, before),
                "case {case:?}: deleting it must not change {survivor_case:?}, got {status} \
                 {read:?}"
            );
        }

        // The name is free again: a create that would have been refused as already
        // existing is admitted, and the definition it stores is the new one.
        let (status, recreated) = create(&emulator, "deldom", &name, definition("SUM")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the freed name to be creatable again, got {status} \
             {recreated:?}"
        );
        assert_eq!(
            recreated.get("Statistic").and_then(Value::as_str),
            Some("SUM"),
            "case {case:?}: the re-create must store its own configuration, got \
             {recreated:?}"
        );

        // ...and the re-created definition is itself subject to the identity rule, so
        // another create of that name is refused again.
        let (status, refused) = create(&emulator, "deldom", &name, definition("COUNT")).await;
        assert_eq!(
            (status, error_code(&refused)),
            (400, "BadRequestException"),
            "case {case:?}: expected the re-created name to be held again, got {status} \
             {refused:?}"
        );
        assert_eq!(
            error_message(&refused),
            exists_message(&name),
            "case {case:?}: expected the already-exists sentence, got {refused:?}"
        );

        // Leave the store as the loop expects it: the row is deleted and gone.
        let (status, deleted) = delete(&emulator, "deldom", &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the re-created definition to be deletable, got \
             {status} {deleted:?}"
        );
    }

    let (status, listed) = list(&emulator, "deldom", "").await;
    assert_eq!(
        (status, item_names(&listed)),
        (200, Vec::<String>::new()),
        "case \"every row deleted\": the domain must hold no definition, got {status} \
         {listed:?}"
    );
}

/// RULE, identity half: the pair identifies the definition, so the same name in another
/// domain is another definition and a delete in one domain leaves it alone. The rows
/// differ in how many domains hold the name and which of them the delete addresses.
#[tokio::test]
async fn a_delete_is_scoped_to_the_domain_it_names() {
    let emulator = Emulator::new();
    let domains = ["alpha", "beta", "gamma"];
    for domain in domains {
        emulator.create_domain(domain, 30).await;
    }

    let shared = "Shared_Name";
    for domain in domains {
        let (status, created) = create(&emulator, domain, shared, definition("SUM")).await;
        assert_eq!(
            status, 200,
            "case {domain:?}: expected the same name to be creatable per domain, got \
             {status} {created:?}"
        );
    }

    for (index, domain) in domains.iter().enumerate() {
        let (status, deleted) = delete(&emulator, domain, shared).await;
        assert_eq!(
            status, 200,
            "case {domain:?}: expected DeleteCalculatedAttributeDefinition 200, got \
             {status} {deleted:?}"
        );

        for other in &domains[index + 1..] {
            let (status, read) = get(&emulator, other, shared).await;
            assert_eq!(
                status, 200,
                "case {domain:?}: the definition in {other:?} is a different resource and \
                 must survive, got {status} {read:?}"
            );
        }
        for already in &domains[..=index] {
            let (status, read) = get(&emulator, already, shared).await;
            assert_eq!(
                (status, error_code(&read)),
                (404, "ResourceNotFoundException"),
                "case {domain:?}: the definition in {already:?} is deleted, got {status} \
                 {read:?}"
            );
        }
    }
}

/// RULE, refusal half: a (domain, name) pair the store does not hold is nothing to
/// delete, so it is refused `ResourceNotFoundException` (404) and the store is left as it
/// was. The rows span the ways a pair can be absent: an unknown domain, an unknown name,
/// a name only another domain holds, and a name this domain held until a moment ago.
#[tokio::test]
async fn a_delete_of_an_absent_pair_is_resource_not_found() {
    let emulator = Emulator::new();
    emulator.create_domain("holder", 30).await;
    emulator.create_domain("empty", 30).await;
    create(&emulator, "holder", "Held", definition("SUM")).await;
    create(&emulator, "holder", "Transient", definition("COUNT")).await;

    let (status, _) = delete(&emulator, "holder", "Transient").await;
    assert_eq!(
        status, 200,
        "case \"the delete the later cases build on\": expected 200, got {status}"
    );

    // (case, domain, name, the sentence the refusal must carry)
    let cases: Vec<(&str, &str, &str, String)> = vec![
        (
            "a domain that was never created",
            "nosuchdomain",
            "Held",
            "Domain nosuchdomain is not found".to_string(),
        ),
        (
            "a name the domain never held",
            "holder",
            "NeverDefined",
            absent_message("NeverDefined"),
        ),
        (
            "a name only another domain holds",
            "empty",
            "Held",
            absent_message("Held"),
        ),
        (
            "a name this domain held until it was deleted",
            "holder",
            "Transient",
            absent_message("Transient"),
        ),
    ];

    for (case, domain, name, message) in cases {
        let (status, refused) = delete(&emulator, domain, name).await;
        assert_eq!(
            (status, error_code(&refused)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException, got {status} {refused:?}"
        );
        assert_eq!(
            error_message(&refused),
            message,
            "case {case:?}: expected {message:?}, got {refused:?}"
        );
    }

    // Every refusal left the store exactly as it was: the one definition the domain holds
    // is still there, and the empty domain still holds none.
    let (status, listed) = list(&emulator, "holder", "").await;
    assert_eq!(
        (status, item_names(&listed)),
        (200, vec!["Held".to_string()]),
        "case \"after every refusal\": the held definition must be untouched, got {status} \
         {listed:?}"
    );
    let (status, listed) = list(&emulator, "empty", "").await;
    assert_eq!(
        (status, item_names(&listed)),
        (200, Vec::<String>::new()),
        "case \"after every refusal\": the empty domain must hold none, got {status} \
         {listed:?}"
    );
}
