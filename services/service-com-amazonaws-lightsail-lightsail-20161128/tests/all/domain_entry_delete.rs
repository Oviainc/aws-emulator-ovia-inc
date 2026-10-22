// SPDX-License-Identifier: CC0-1.0

//! `DeleteDomainEntry` — removing one recordset entry from a stored Lightsail DNS
//! zone — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The harness is the sibling suite's (`instance_lifecycle`), so this
//! file stands up no second emulator of its own.
//!
//! The rule under test: `DeleteDomainEntry` removes from the stored zone the
//! request names the ONE entry its `domainEntry` resolves to. A `domainEntry`
//! carries up to two handles on an entry of a zone — the server-assigned
//! `domainEntry.id`, and the name/type/target triple that identifies a recordset
//! inside a zone — and the removal takes the entry EITHER handle resolves to (the
//! id's when the zone holds that id), leaving every other entry of that zone, in
//! its own order, and every other zone untouched, so a later `GetDomain` reports
//! exactly that; the removal is answered as one terminal `DeleteDomainEntry`
//! operation against the `Domain`; a `domainEntry` resolving to no entry of the
//! zone — one already deleted, say — has nothing to remove, so the delete cannot
//! execute and is refused with the modelled `OperationFailureException`, carrying
//! the service's `DoesNotExist` body code and a message saying there is no such
//! domain record; and a request naming no stored zone, or carrying neither handle,
//! is refused too, each with its own modelled error, and removes nothing.

use serde_json::{json, Value};

use super::domain_entry_recordset::entry_name_of_length;
use super::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The zone every case deletes an entry of.
const ZONE: &str = "example.com";

/// The `operationType` an entry's removal from a zone is logged under, the
/// modelled `OperationType` enum's own `DeleteDomainEntry` member.
const DELETE_OPERATION_TYPE: &str = "DeleteDomainEntry";

/// How a case addresses the entry it deletes: the two handles a request has on an
/// entry already in a zone.
#[derive(Debug, Clone, Copy)]
enum Addressing {
    /// The server-assigned `domainEntry.id` alone.
    Id,
    /// The server-assigned id, with the name, type and target of a DIFFERENT
    /// recordset beside it: the id is a handle the zone holds, so it is what the
    /// request resolves to.
    IdWithStaleValues,
    /// An id the zone holds no entry for, beside the entry's own recordset: the
    /// triple is the handle that resolves, so the entry it identifies is the one
    /// removed.
    StaleIdWithRecordset,
    /// The name, type and target triple alone.
    Recordset,
    /// The same triple with the name and type in the other case — a DNS name and a
    /// record type are case-insensitive (RFC 4343).
    RecordsetOtherCase,
}

/// One `DeleteDomainEntry` request the service must serve: the entries the zone
/// holds, which of them the request deletes, and how it addresses it.
struct Case {
    case: &'static str,
    entries: Vec<Value>,
    /// Index into `entries` of the entry the request deletes.
    deleted: usize,
    addressing: Addressing,
}

/// The table. Its rows differ along everything the rule generalises over: WHICH
/// handle resolves the entry (id alone, id beside another recordset's values, the
/// recordset triple alone, that triple in another case, the triple beside an id
/// the zone holds no entry for), the record type (the catalog the modelled
/// `DomainEntry$type` documentation lists: "A, AAAA, CNAME, MX, NS, SOA, SRV,
/// TXT"), the form of the name (a subdomain, the zone apex, several labels, the
/// `*` wildcard, underscore-prefixed service and validation labels, and the
/// published 253-character bound), how many entries the zone holds (one, two,
/// several) and WHERE in the zone the deleted entry sits (first, middle, last).
fn cases() -> Vec<Case> {
    let many = || {
        vec![
            json!({"name": "first.example.com", "type": "TXT", "target": "\"first-decoy\""}),
            json!({"name": "www.example.com", "type": "A", "target": "192.0.2.10"}),
            json!({"name": "last.example.com", "type": "AAAA", "target": "2001:db8::9"}),
        ]
    };
    vec![
        Case {
            case: "the only entry of the zone, addressed by its id",
            entries: vec![json!({"name": "www.example.com", "type": "A", "target": "192.0.2.10"})],
            deleted: 0,
            addressing: Addressing::Id,
        },
        Case {
            case: "the only entry of the zone, addressed by its recordset",
            entries: vec![json!({"name": "www.example.com", "type": "A", "target": "192.0.2.10"})],
            deleted: 0,
            addressing: Addressing::Recordset,
        },
        Case {
            case: "the first of three entries, addressed by its id",
            entries: many(),
            deleted: 0,
            addressing: Addressing::Id,
        },
        Case {
            case: "the middle of three entries, addressed by its id",
            entries: many(),
            deleted: 1,
            addressing: Addressing::Id,
        },
        Case {
            case: "the last of three entries, addressed by its id",
            entries: many(),
            deleted: 2,
            addressing: Addressing::Id,
        },
        Case {
            case: "the middle of three entries, addressed by its recordset",
            entries: many(),
            deleted: 1,
            addressing: Addressing::Recordset,
        },
        Case {
            case: "the last of three entries, addressed by its recordset",
            entries: many(),
            deleted: 2,
            addressing: Addressing::Recordset,
        },
        Case {
            case: "an entry addressed by its recordset in the other case",
            entries: many(),
            deleted: 1,
            addressing: Addressing::RecordsetOtherCase,
        },
        Case {
            case: "an entry addressed by its id, beside values of another recordset",
            entries: many(),
            deleted: 1,
            addressing: Addressing::IdWithStaleValues,
        },
        Case {
            case: "an entry addressed by its recordset, beside an id the zone holds no entry for",
            entries: many(),
            deleted: 1,
            addressing: Addressing::StaleIdWithRecordset,
        },
        Case {
            case: "a canonical name record",
            entries: vec![
                json!({"name": "cdn.example.com", "type": "CNAME", "target": "exampled1182ne.cloudfront.net"}),
                json!({"name": "www.example.com", "type": "A", "target": "192.0.2.10"}),
            ],
            deleted: 0,
            addressing: Addressing::Recordset,
        },
        Case {
            case: "a mail exchanger at the zone apex",
            entries: vec![
                json!({"name": "example.com", "type": "MX", "target": "10 inbound-smtp.us-east-1.amazonaws.com"}),
                json!({"name": "example.com", "type": "A", "target": "192.0.2.11"}),
            ],
            deleted: 0,
            addressing: Addressing::Recordset,
        },
        Case {
            case: "a name server record delegating a subdomain",
            entries: vec![
                json!({"name": "sub.example.com", "type": "NS", "target": "ns-111.awsdns-22.com."}),
            ],
            deleted: 0,
            addressing: Addressing::Id,
        },
        Case {
            case: "a start of authority record",
            entries: vec![json!({
                "name": "example.com",
                "type": "SOA",
                "target": "ns-111.awsdns-22.com. awsdns-hostmaster.amazon.com. 1 7200 900 1209600 86400",
            })],
            deleted: 0,
            addressing: Addressing::Recordset,
        },
        Case {
            case: "a service locator under underscore-prefixed labels",
            entries: vec![
                json!({"name": "_sip._tcp.example.com", "type": "SRV", "target": "10 5 5060 sip.example.com"}),
                json!({"name": "www.example.com", "type": "A", "target": "192.0.2.10"}),
            ],
            deleted: 0,
            addressing: Addressing::Recordset,
        },
        Case {
            case: "a text record under an underscore-prefixed validation label",
            entries: vec![
                json!({"name": "_acme-challenge.example.com", "type": "TXT", "target": "\"token\""}),
            ],
            deleted: 0,
            addressing: Addressing::Id,
        },
        Case {
            case: "an entry on the wildcard label",
            entries: vec![
                json!({"name": "*.example.com", "type": "TXT", "target": "\"wildcarded\""}),
                json!({"name": "www.example.com", "type": "A", "target": "192.0.2.10"}),
            ],
            deleted: 0,
            addressing: Addressing::Recordset,
        },
        Case {
            case: "an entry four labels deep",
            entries: vec![
                json!({"name": "cdn.assets.blog.example.com", "type": "CNAME", "target": "b.cloudfront.net"}),
            ],
            deleted: 0,
            addressing: Addressing::Id,
        },
        Case {
            case: "an alias record pointing at another Amazon Web Services resource",
            entries: vec![json!({
                "name": "app.example.com",
                "type": "A",
                "target": "container-service-1.example23scljs.us-west-2.cs.amazonlightsail.com",
                "isAlias": true,
            })],
            deleted: 0,
            addressing: Addressing::Recordset,
        },
        Case {
            // 253 characters is the longest a presentation-form domain name may be
            // (RFC 1035 §2.3.4), the same bound the create suite sits a row on.
            case: "an entry with a 253-character name, the longest the presentation form allows",
            entries: vec![json!({
                "name": entry_name_of_length(253, ZONE),
                "type": "TXT",
                "target": "\"at-the-length-bound\"",
            })],
            deleted: 0,
            addressing: Addressing::Recordset,
        },
    ]
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create `zone`.
async fn create_zone(emulator: &Emulator, zone: &str) {
    emulator
        .ok("CreateDomain", json!({"domainName": zone}))
        .await;
}

/// Append `entry` to `zone`, answering the operation the create logged.
async fn create_entry(emulator: &Emulator, zone: &str, entry: &Value) -> Value {
    emulator
        .ok(
            "CreateDomainEntry",
            json!({"domainName": zone, "domainEntry": entry}),
        )
        .await["operation"]
        .clone()
}

/// The entries `GetDomain` reports for `zone`.
async fn entries(emulator: &Emulator, zone: &str) -> Vec<Value> {
    let domain = emulator
        .ok("GetDomain", json!({"domainName": zone}))
        .await["domain"]
        .clone();
    domain["domainEntries"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a domainEntries array, got {domain:?}"))
        .clone()
}

/// A zone holding `requested` in order, answering the stored entries and the id
/// of the operation the last create logged.
async fn zone_with(emulator: &Emulator, requested: &[Value]) -> (Vec<Value>, String) {
    create_zone(emulator, ZONE).await;
    let mut last = Value::Null;
    for entry in requested {
        last = create_entry(emulator, ZONE, entry).await;
    }
    (
        entries(emulator, ZONE).await,
        last["id"].as_str().unwrap_or_default().to_string(),
    )
}

/// The `domainEntry` a case sends: the handle it addresses `stored` by.
fn addressed(addressing: Addressing, stored: &Value, other: Option<&Value>) -> Value {
    match addressing {
        Addressing::Id => json!({"id": stored["id"]}),
        Addressing::IdWithStaleValues => {
            let other = other.expect("a case addressing stale values needs another entry");
            json!({
                "id": stored["id"],
                "name": other["name"],
                "type": other["type"],
                "target": other["target"],
            })
        }
        Addressing::StaleIdWithRecordset => json!({
            // A uuid-shaped id of all zeroes: this store mints ids from its own
            // counter, so no entry of any zone carries it.
            "id": "00000000-0000-0000-0000-000000000000",
            "name": stored["name"],
            "type": stored["type"],
            "target": stored["target"],
        }),
        Addressing::Recordset => json!({
            "name": stored["name"],
            "type": stored["type"],
            "target": stored["target"],
        }),
        Addressing::RecordsetOtherCase => json!({
            "name": stored["name"].as_str().unwrap_or_default().to_uppercase(),
            "type": stored["type"].as_str().unwrap_or_default().to_lowercase(),
            "target": stored["target"],
        }),
    }
}

/// Assert `answered` is the single terminal `DeleteDomainEntry` log entry the
/// removal wrote against the zone.
fn assert_delete_operation(case: &str, answered: &Value, excluded_id: &str) {
    let operation = &answered["operation"];
    assert!(
        operation.is_object(),
        "case {case}: expected the operation the removal wrote, got {answered:?}"
    );
    assert_eq!(
        operation["operationType"].as_str(),
        Some(DELETE_OPERATION_TYPE),
        "case {case}: expected the {DELETE_OPERATION_TYPE} operation type, got {operation:?}"
    );
    assert_eq!(
        operation["resourceType"].as_str(),
        Some("Domain"),
        "case {case}: expected the Domain resource type, got {operation:?}"
    );
    assert_eq!(
        operation["resourceName"].as_str(),
        Some(ZONE),
        "case {case}: expected the operation to name the zone, got {operation:?}"
    );
    assert_eq!(
        operation["status"].as_str(),
        Some("Succeeded"),
        "case {case}: expected a Succeeded operation, got {operation:?}"
    );
    assert_eq!(
        operation["isTerminal"].as_bool(),
        Some(true),
        "case {case}: expected a terminal operation, got {operation:?}"
    );
    assert_eq!(
        operation["location"]["regionName"].as_str(),
        Some("us-east-1"),
        "case {case}: expected the zone's own region, got {operation:?}"
    );
    let id = operation["id"].as_str().unwrap_or_default();
    assert!(
        is_uuid_shaped(id),
        "case {case}: expected a uuid-shaped operation id, got {id:?}"
    );
    assert_ne!(
        id, excluded_id,
        "case {case}: expected a fresh operation id, got the create's own {id:?}"
    );
}

// ---------------------------------------------------------------------------
// The delete table
// ---------------------------------------------------------------------------

/// RULE: a delete removes the ONE entry its `domainEntry` addresses — by the
/// server-assigned id when the request carries one, otherwise by the
/// name/type/target triple — leaving every other entry of the zone in its own
/// order, so the next `GetDomain` reports the zone without that entry and with
/// nothing else changed, and the removal is answered as one terminal
/// `DeleteDomainEntry` operation against the zone.
#[tokio::test]
async fn an_addressed_entry_is_gone_from_the_zone_it_was_deleted_from() {
    for case in cases() {
        let label = case.case;
        let emulator = Emulator::new();
        let (before, create_operation_id) = zone_with(&emulator, &case.entries).await;
        let stored = before[case.deleted].clone();
        let other = before.iter().enumerate().find_map(|(index, entry)| {
            (index != case.deleted).then(|| entry.clone())
        });
        let requested = addressed(case.addressing, &stored, other.as_ref());

        let answered = emulator
            .ok(
                "DeleteDomainEntry",
                json!({"domainName": ZONE, "domainEntry": requested}),
            )
            .await;
        assert_delete_operation(label, &answered, &create_operation_id);

        let after = entries(&emulator, ZONE).await;
        let expected: Vec<Value> = before
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != case.deleted)
            .map(|(_, entry)| entry.clone())
            .collect();
        assert_eq!(
            after, expected,
            "case {label}: expected the zone to hold its other entries, in order, got {after:?}"
        );
        assert!(
            !after
                .iter()
                .any(|entry| entry["id"] == stored["id"]),
            "case {label}: expected the addressed entry to be gone, got {after:?}"
        );

        // Reading changes nothing a later caller sees.
        let again = entries(&emulator, ZONE).await;
        assert_eq!(
            again, after,
            "case {label}: expected the second read to report the same entries, got {again:?}"
        );
    }
}

/// RULE: an entry is in the zone once, so only the first delete of it can execute:
/// the second resolves to no entry of the zone and is refused with the modelled
/// `OperationFailureException` (400), leaving the zone's other entries alone.
#[tokio::test]
async fn deleting_the_same_entry_twice_is_refused_the_second_time() {
    for addressing in [
        Addressing::Id,
        Addressing::Recordset,
        Addressing::StaleIdWithRecordset,
    ] {
        let case = format!("{addressing:?}");
        let emulator = Emulator::new();
        let (before, _) = zone_with(
            &emulator,
            &[
                json!({"name": "www.example.com", "type": "A", "target": "192.0.2.10"}),
                json!({"name": "mail.example.com", "type": "A", "target": "192.0.2.20"}),
            ],
        )
        .await;
        let requested = addressed(addressing, &before[0], None);

        emulator
            .ok(
                "DeleteDomainEntry",
                json!({"domainName": ZONE, "domainEntry": requested.clone()}),
            )
            .await;
        let after = entries(&emulator, ZONE).await;
        assert_eq!(
            after,
            vec![before[1].clone()],
            "case a second delete addressed by {case}: expected the first delete to leave \
             the other entry, got {after:?}"
        );

        let (status, body) = emulator
            .call(
                "DeleteDomainEntry",
                json!({"domainName": ZONE, "domainEntry": requested}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case a second delete addressed by {case}: expected a 400 refusal, got \
             {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "OperationFailureException",
            "case a second delete addressed by {case}: expected the modelled \
             OperationFailureException, got {body:?}"
        );
        assert_eq!(
            entries(&emulator, ZONE).await,
            after,
            "case a second delete addressed by {case}: expected the zone to be unchanged \
             by a refused delete"
        );
    }
}

/// RULE: an entry belongs to the ONE zone the request names, so it is addressed
/// inside that zone only: a zone holding the very same recordset keeps its own
/// entry, and an id minted for another zone's entry addresses nothing here.
#[tokio::test]
async fn a_delete_reaches_only_the_zone_it_names() {
    let emulator = Emulator::new();
    let other = "other.example";
    let twin = json!({"name": "www.example.com", "type": "A", "target": "192.0.2.10"});
    let (before, _) = zone_with(&emulator, std::slice::from_ref(&twin)).await;
    assert_eq!(
        before.len(),
        1,
        "case two zones: expected '{ZONE}' to hold the one entry it was given, got {before:?}"
    );
    create_zone(&emulator, other).await;
    // The same recordset values, inside the other zone's own name.
    let other_entry = json!({"name": "www.other.example", "type": "A", "target": "192.0.2.10"});
    create_entry(&emulator, other, &other_entry).await;
    let other_before = entries(&emulator, other).await;

    emulator
        .ok(
            "DeleteDomainEntry",
            json!({"domainName": ZONE, "domainEntry": {
                "name": twin["name"],
                "type": twin["type"],
                "target": twin["target"],
            }}),
        )
        .await;
    assert_eq!(
        entries(&emulator, other).await,
        other_before,
        "case two zones: expected '{other}' to be untouched by a delete in '{ZONE}'"
    );
    assert!(
        entries(&emulator, ZONE).await.is_empty(),
        "case two zones: expected '{ZONE}' to have lost its only entry"
    );

    // The other zone's entry id, addressed in the emptied zone, belongs to no
    // entry of it, so the delete cannot execute and removes nothing from either
    // zone.
    let foreign_id = other_before[0]["id"].clone();
    let (status, body) = emulator
        .call(
            "DeleteDomainEntry",
            json!({"domainName": ZONE, "domainEntry": {"id": foreign_id}}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case an id from another zone: expected a 400 refusal, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "OperationFailureException",
        "case an id from another zone: expected the modelled OperationFailureException, \
         got {body:?}"
    );
    assert_eq!(
        entries(&emulator, other).await,
        other_before,
        "case an id from another zone: expected the other zone to keep its entry"
    );
    assert!(
        entries(&emulator, ZONE).await.is_empty(),
        "case an id from another zone: expected '{ZONE}' to stay empty"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// RULE: a `domainEntry` neither of whose handles resolves to an entry of the zone
/// gives the delete no recordset to remove, so the operation cannot execute: the
/// modelled `OperationFailureException` (400), whose body carries the service's
/// `DoesNotExist` code and a message, built from the request, saying there is no
/// such domain record — and nothing removed.
#[tokio::test]
async fn a_domain_entry_the_zone_does_not_hold_is_refused() {
    // Each case is one way the request addresses no entry of this zone: an id this
    // store never minted, and a triple differing from the stored one in exactly
    // one of the three values that identify an entry (including a record type
    // outside the catalog the modelled `DomainEntry$type` documentation lists).
    // The zone holds an address record and a canonical name record whose target
    // carries letters, so a case can vary the target's case alone.
    let held = [
        json!({"name": "www.example.com", "type": "A", "target": "192.0.2.10"}),
        json!({"name": "cdn.example.com", "type": "CNAME", "target": "Exampled1182ne.CloudFront.net"}),
    ];
    let cases: Vec<(&str, Value)> = vec![
        ("an id no entry carries", json!({"id": "00000000-0000-0000-0000-000000000000"})),
        ("an id that is not an id at all", json!({"id": "not-an-id"})),
        (
            "an entry's name sent as its id",
            json!({"id": "www.example.com"}),
        ),
        (
            "another name, with the stored type and target",
            json!({"name": "mail.example.com", "type": "A", "target": "192.0.2.10"}),
        ),
        (
            "another type, with the stored name and target",
            json!({"name": "www.example.com", "type": "TXT", "target": "192.0.2.10"}),
        ),
        (
            "another target, with the stored name and type",
            json!({"name": "www.example.com", "type": "A", "target": "192.0.2.99"}),
        ),
        (
            "a record type the service does not hold",
            json!({"name": "www.example.com", "type": "SPF", "target": "192.0.2.10"}),
        ),
        (
            // A name and a record type are case-insensitive (RFC 4343), but a
            // target is the recordset's data: the same rule the create's own
            // duplicate check applies.
            "a target differing only in case, which is another target",
            json!({
                "name": "cdn.example.com",
                "type": "CNAME",
                "target": "exampled1182ne.cloudfront.net",
            }),
        ),
        (
            "both handles resolving nothing at once",
            json!({
                "id": "00000000-0000-0000-0000-000000000000",
                "name": "mail.example.com",
                "type": "MX",
                "target": "10 inbound-smtp.us-east-1.amazonaws.com",
            }),
        ),
    ];

    for (case, requested) in cases {
        let emulator = Emulator::new();
        let (before, _) = zone_with(&emulator, &held).await;

        let (status, body) = emulator
            .call(
                "DeleteDomainEntry",
                json!({"domainName": ZONE, "domainEntry": requested}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected a 400 refusal, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "OperationFailureException",
            "case {case}: expected the modelled OperationFailureException, got {body:?}"
        );
        // The refusal says there is no such domain record, and is built from the
        // request: it names the zone it looked in and every handle the request
        // carried.
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.to_lowercase().contains("no such domain record"),
            "case {case}: expected the refusal to say there is no such domain record, \
             got {body:?}"
        );
        assert!(
            message.contains(ZONE),
            "case {case}: expected the refusal to name the zone, got {body:?}"
        );
        for handle in ["id", "name", "type", "target"] {
            if let Some(value) = requested[handle].as_str() {
                assert!(
                    message.contains(value),
                    "case {case}: expected the refusal to name the {handle} the request \
                     carried ({value}), got {body:?}"
                );
            }
        }
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case {case}: expected the body code that tells a client the record is \
             already gone, got {body:?}"
        );
        assert_eq!(
            entries(&emulator, ZONE).await,
            before,
            "case {case}: a refused delete removes nothing, so the zone's entries must \
             still be what they were"
        );
    }
}

/// RULE: a `domainEntry` carrying neither an id nor a whole name/type/target
/// triple addresses no entry, so it is refused — with the modelled
/// `InvalidInputException` (400) whose message names the members that address one,
/// or, where a modelled constraint already covers it, with the boundary walk's own
/// `ValidationException` (400) — and removes nothing.
#[tokio::test]
async fn a_domain_entry_that_addresses_nothing_is_refused() {
    let cases: Vec<(&str, Value, &str)> = vec![
        ("an empty domainEntry", json!({}), "InvalidInputException"),
        (
            "a name alone",
            json!({"name": "www.example.com"}),
            "InvalidInputException",
        ),
        (
            "a name and a type, with no target",
            json!({"name": "www.example.com", "type": "A"}),
            "InvalidInputException",
        ),
        (
            "a name and a target, with no type",
            json!({"name": "www.example.com", "target": "192.0.2.10"}),
            "InvalidInputException",
        ),
        (
            "a type and a target, with no name",
            json!({"type": "A", "target": "192.0.2.10"}),
            "InvalidInputException",
        ),
        (
            "a blank name beside the stored type and target",
            json!({"name": "   ", "type": "A", "target": "192.0.2.10"}),
            "InvalidInputException",
        ),
        (
            "a blank target beside the stored name and type",
            json!({"name": "www.example.com", "type": "A", "target": ""}),
            "InvalidInputException",
        ),
        (
            // `DomainEntry$id` targets `NonEmptyString`, whose `\S` pattern the
            // centralized walk enforces BEFORE the handler runs, so this one is
            // the boundary's own `ValidationException`.
            "a blank id",
            json!({"id": "   ", "name": "www.example.com", "type": "A", "target": "192.0.2.10"}),
            "ValidationException",
        ),
    ];

    let stored = json!({"name": "www.example.com", "type": "A", "target": "192.0.2.10"});
    for (case, requested, code) in cases {
        let emulator = Emulator::new();
        let (before, _) = zone_with(&emulator, std::slice::from_ref(&stored)).await;

        let (status, body) = emulator
            .call(
                "DeleteDomainEntry",
                json!({"domainName": ZONE, "domainEntry": requested}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected a 400 refusal, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case}: expected {code}, got {body:?}"
        );
        assert!(
            body["message"]
                .as_str()
                .unwrap_or_default()
                .contains("domainEntry"),
            "case {case}: expected the refusal to name the member, got {body:?}"
        );
        assert_eq!(
            entries(&emulator, ZONE).await,
            before,
            "case {case}: a refused delete removes nothing"
        );
    }
}

/// RULE: an entry is held by a zone, so a `domainName` this account holds no
/// recordset for is refused with the service's `DoesNotExist` envelope and a
/// message naming the resource type and the requested name.
#[tokio::test]
async fn a_delete_in_a_zone_that_does_not_exist_is_refused() {
    let emulator = Emulator::new();
    let stored = json!({"name": "www.example.com", "type": "A", "target": "192.0.2.10"});
    let (before, _) = zone_with(&emulator, std::slice::from_ref(&stored)).await;
    let entry_id = before[0]["id"].clone();

    for name in ["absent.example", "example.com.extra", "other.test"] {
        for requested in [
            json!({"id": entry_id.clone()}),
            json!({"name": format!("www.{name}"), "type": "A", "target": "192.0.2.10"}),
        ] {
            let (status, body) = emulator
                .call(
                    "DeleteDomainEntry",
                    json!({"domainName": name, "domainEntry": requested}),
                )
                .await;
            assert_eq!(
                status, 400,
                "case deleting in {name}: expected a 400, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "NotFoundException",
                "case deleting in {name}: expected the modelled NotFoundException, got {body:?}"
            );
            assert_eq!(
                body["code"].as_str(),
                Some("DoesNotExist"),
                "case deleting in {name}: expected code DoesNotExist, got {body:?}"
            );
            assert_eq!(
                body["message"].as_str(),
                Some(format!("The Domain does not exist: {name}").as_str()),
                "case deleting in {name}: expected the resource-type/name message, got {body:?}"
            );
        }
    }

    assert_eq!(
        entries(&emulator, ZONE).await,
        before,
        "case deleting in a zone that does not exist: expected the stored zone to be untouched"
    );
}

/// RULE: a delete leaves the zone able to hold the same recordset again, because
/// nothing but that one entry left the store — so the triple it freed can be
/// created afresh, and it comes back with an id of its own.
#[tokio::test]
async fn a_deleted_recordset_can_be_created_again() {
    let emulator = Emulator::new();
    let stored = json!({"name": "www.example.com", "type": "A", "target": "192.0.2.10"});
    let (before, _) = zone_with(&emulator, std::slice::from_ref(&stored)).await;
    let first_id = before[0]["id"].as_str().unwrap_or_default().to_string();

    emulator
        .ok(
            "DeleteDomainEntry",
            json!({"domainName": ZONE, "domainEntry": {"id": first_id.clone()}}),
        )
        .await;
    create_entry(&emulator, ZONE, &stored).await;

    let after = entries(&emulator, ZONE).await;
    assert_eq!(
        after.len(),
        1,
        "case recreating a deleted recordset: expected the zone to hold it once, got {after:?}"
    );
    assert_eq!(
        after[0]["name"], stored["name"],
        "case recreating a deleted recordset: expected the recreated name, got {after:?}"
    );
    let second_id = after[0]["id"].as_str().unwrap_or_default();
    assert!(
        is_uuid_shaped(second_id),
        "case recreating a deleted recordset: expected a uuid-shaped id, got {second_id:?}"
    );
    assert_ne!(
        second_id, first_id,
        "case recreating a deleted recordset: expected an id of its own, got the deleted \
         entry's {second_id:?}"
    );
}
