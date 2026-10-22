// SPDX-License-Identifier: CC0-1.0

//! `UpdateDomainEntry` — changing one recordset entry of a stored Lightsail DNS
//! zone — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The harness is the sibling suite's (`instance_lifecycle`), so this
//! file stands up no second emulator of its own.
//!
//! The rule under test: `UpdateDomainEntry` replaces, in the stored zone the
//! request names, the ONE entry whose server-assigned id `domainEntry.id` carries
//! — its name, type, target and alias flag become the request's verbatim, while
//! the entry keeps that id and its place in the zone and every other entry of
//! every zone is left alone — so a later `GetDomain` reports exactly that; the
//! change is logged as one terminal `UpdateDomainEntry` operation against the
//! `Domain`, answered in `operations`; and a request that addresses no entry,
//! names no zone, omits one of the values a recordset is made of, or would make
//! the entry a duplicate of another entry of the same zone is refused and changes
//! nothing.

use serde_json::{json, Value};

use super::domain_entry_recordset::entry_name_of_length;
use super::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The zone every case updates an entry of.
const ZONE: &str = "example.com";

/// The `operationType` a change to a zone's recordsets is logged under, from the
/// modelled `OperationType` enum.
const ENTRY_OPERATION_TYPE: &str = "UpdateDomainEntry";

/// The entries sitting on either side of the one each case updates, so a case
/// shows what the update left alone as well as what it changed.
fn decoys() -> [Value; 2] {
    [
        json!({"name": "first.example.com", "type": "TXT", "target": "\"first-decoy\""}),
        json!({"name": "last.example.com", "type": "AAAA", "target": "2001:db8::9"}),
    ]
}

/// One `UpdateDomainEntry` request the service must serve: the entry as it was
/// created, and the `domainEntry` the update carries (without the id, which only
/// the store can supply).
struct Case {
    case: &'static str,
    created: Value,
    update: Value,
}

impl Case {
    /// The `isAlias` the updated entry must report: absent from the request means
    /// "not an alias", exactly as it does on a create.
    fn expected_is_alias(&self) -> bool {
        self.update["isAlias"].as_bool().unwrap_or(false)
    }
}

/// The table. Its rows differ along everything the rule generalises over: WHICH of
/// the four members the update changes (the target alone, the type, the name, all
/// of them at once, the alias flag alone, or nothing at all), the record type
/// before and after (the catalog the modelled `DomainEntry$type` documentation
/// lists: "A, AAAA, CNAME, MX, NS, SOA, SRV, TXT"), the form of the name (a
/// subdomain, the zone apex, several labels, the `*` wildcard, underscore-prefixed
/// service and validation labels, and the published 253-character bound) and the
/// form of the target.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "a new target on the same name and type",
            created: json!({"name": "www.example.com", "type": "A", "target": "192.0.2.10"}),
            update: json!({"name": "www.example.com", "type": "A", "target": "192.0.2.99"}),
        },
        Case {
            case: "a new type and target on the same name",
            created: json!({"name": "www.example.com", "type": "A", "target": "192.0.2.10"}),
            update: json!({
                "name": "www.example.com",
                "type": "CNAME",
                "target": "exampled1182ne.cloudfront.net",
            }),
        },
        Case {
            case: "a new name, keeping the type and target",
            created: json!({"name": "www.example.com", "type": "A", "target": "192.0.2.10"}),
            update: json!({"name": "mail.example.com", "type": "A", "target": "192.0.2.10"}),
        },
        Case {
            case: "a new name four labels deep",
            created: json!({"name": "cdn.example.com", "type": "CNAME", "target": "a.cloudfront.net"}),
            update: json!({
                "name": "cdn.assets.blog.example.com",
                "type": "CNAME",
                "target": "b.cloudfront.net",
            }),
        },
        Case {
            case: "name, type and target all at once, onto the wildcard label",
            created: json!({"name": "a.example.com", "type": "A", "target": "192.0.2.1"}),
            update: json!({"name": "*.example.com", "type": "TXT", "target": "\"wildcarded\""}),
        },
        Case {
            case: "a plain address record turned into an alias",
            created: json!({"name": "app.example.com", "type": "A", "target": "192.0.2.5", "isAlias": false}),
            update: json!({
                "name": "app.example.com",
                "type": "A",
                "target": "container-service-1.example23scljs.us-west-2.cs.amazonlightsail.com",
                "isAlias": true,
            }),
        },
        Case {
            case: "an alias record turned back into a plain one",
            created: json!({
                "name": "app.example.com",
                "type": "A",
                "target": "container-service-1.example23scljs.us-west-2.cs.amazonlightsail.com",
                "isAlias": true,
            }),
            update: json!({"name": "app.example.com", "type": "A", "target": "192.0.2.6", "isAlias": false}),
        },
        Case {
            case: "an alias record updated with isAlias left out, which is not an alias",
            created: json!({
                "name": "app.example.com",
                "type": "A",
                "target": "container-service-1.example23scljs.us-west-2.cs.amazonlightsail.com",
                "isAlias": true,
            }),
            update: json!({"name": "app.example.com", "type": "A", "target": "192.0.2.7"}),
        },
        Case {
            case: "an IPv6 address record's target",
            created: json!({"name": "v6.example.com", "type": "AAAA", "target": "2001:db8::1"}),
            update: json!({"name": "v6.example.com", "type": "AAAA", "target": "2001:db8::2"}),
        },
        Case {
            case: "a mail exchanger's priority and host, at the apex",
            created: json!({
                "name": "example.com",
                "type": "MX",
                "target": "10 inbound-smtp.us-east-1.amazonaws.com",
            }),
            update: json!({
                "name": "example.com",
                "type": "MX",
                "target": "20 inbound-smtp.us-west-2.amazonaws.com",
            }),
        },
        Case {
            case: "a name server record delegating a different subdomain",
            created: json!({"name": "sub.example.com", "type": "NS", "target": "ns-111.awsdns-22.com."}),
            update: json!({"name": "other.example.com", "type": "NS", "target": "ns-222.awsdns-33.com."}),
        },
        Case {
            case: "a start of authority record's serial",
            created: json!({
                "name": "example.com",
                "type": "SOA",
                "target": "ns-111.awsdns-22.com. awsdns-hostmaster.amazon.com. 1 7200 900 1209600 86400",
            }),
            update: json!({
                "name": "example.com",
                "type": "SOA",
                "target": "ns-111.awsdns-22.com. awsdns-hostmaster.amazon.com. 2 7200 900 1209600 86400",
            }),
        },
        Case {
            case: "a service locator under underscore-prefixed labels",
            created: json!({"name": "_sip._tcp.example.com", "type": "SRV", "target": "10 5 5060 sip.example.com"}),
            update: json!({"name": "_sip._udp.example.com", "type": "SRV", "target": "20 5 5060 sip.example.com"}),
        },
        Case {
            case: "a text record under an underscore-prefixed validation label",
            created: json!({"name": "_acme-challenge.example.com", "type": "TXT", "target": "\"old-token\""}),
            update: json!({"name": "_acme-challenge.example.com", "type": "TXT", "target": "\"new-token\""}),
        },
        Case {
            case: "the apex record, which an update may also move off the apex",
            created: json!({"name": "example.com", "type": "A", "target": "192.0.2.11"}),
            update: json!({"name": "apex.example.com", "type": "A", "target": "192.0.2.12"}),
        },
        Case {
            // 253 characters is the longest a presentation-form domain name may be
            // (RFC 1035 §2.3.4), the same bound the create suite sits a row on.
            case: "a 253-character name, the longest the presentation form allows",
            created: json!({"name": "short.example.com", "type": "TXT", "target": "\"short\""}),
            update: json!({
                "name": entry_name_of_length(253, ZONE),
                "type": "TXT",
                "target": "\"at-the-length-bound\"",
            }),
        },
        Case {
            case: "an update to the values the entry already carries",
            created: json!({"name": "idem.example.com", "type": "A", "target": "192.0.2.13"}),
            update: json!({"name": "idem.example.com", "type": "A", "target": "192.0.2.13"}),
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

/// A zone holding the two decoys with `subject` between them, answering the stored
/// entries and the id of the operation the subject's own create logged. The
/// subject is therefore at index 1, where it must stay.
async fn zone_with(emulator: &Emulator, subject: &Value) -> (Vec<Value>, String) {
    create_zone(emulator, ZONE).await;
    let [first, last] = decoys();
    create_entry(emulator, ZONE, &first).await;
    let created = create_entry(emulator, ZONE, subject).await;
    create_entry(emulator, ZONE, &last).await;
    (
        entries(emulator, ZONE).await,
        created["id"].as_str().unwrap_or_default().to_string(),
    )
}

/// Assert `operations` is the single terminal `UpdateDomainEntry` log entry the
/// change wrote against the zone.
fn assert_update_operation(case: &str, answered: &Value, excluded_id: &str) {
    let operations = answered["operations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected an operations array, got {answered:?}"));
    assert_eq!(
        operations.len(),
        1,
        "case {case}: expected the one operation the change wrote, got {operations:?}"
    );
    let operation = &operations[0];
    assert_eq!(
        operation["operationType"].as_str(),
        Some(ENTRY_OPERATION_TYPE),
        "case {case}: expected the {ENTRY_OPERATION_TYPE} operation type, got {operation:?}"
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
// The update table
// ---------------------------------------------------------------------------

/// RULE: an update replaces the name, type, target and alias flag of the ONE entry
/// its `domainEntry.id` addresses — keeping that id and the entry's place in the
/// zone, and leaving every other entry alone — so the next `GetDomain` reports the
/// values the update carried, and the change is answered as one terminal
/// `UpdateDomainEntry` operation against the zone.
#[tokio::test]
async fn an_entry_reads_back_as_it_was_updated() {
    for case in cases() {
        let label = case.case;
        let emulator = Emulator::new();
        let (before, create_operation_id) = zone_with(&emulator, &case.created).await;
        let entry_id = before[1]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label}: the created entry must carry an id"))
            .to_string();

        let mut requested = case.update.clone();
        requested["id"] = Value::String(entry_id.clone());
        let answered = emulator
            .ok(
                "UpdateDomainEntry",
                json!({"domainName": ZONE, "domainEntry": requested}),
            )
            .await;
        assert_update_operation(label, &answered, &create_operation_id);

        let after = entries(&emulator, ZONE).await;
        assert_eq!(
            after.len(),
            before.len(),
            "case {label}: an update adds no entry, so the zone must still hold the \
             entries it was given, got {after:?}"
        );
        let updated = after[1].clone();
        assert_eq!(
            updated["id"].as_str(),
            Some(entry_id.as_str()),
            "case {label}: expected the entry to keep its server-assigned id at its own \
             place in the zone, got {after:?}"
        );
        assert_eq!(
            updated["name"], requested["name"],
            "case {label}: expected the name the update carried, got {updated:?}"
        );
        assert_eq!(
            updated["type"], requested["type"],
            "case {label}: expected the type the update carried, got {updated:?}"
        );
        assert_eq!(
            updated["target"], requested["target"],
            "case {label}: expected the target the update carried, got {updated:?}"
        );
        assert_eq!(
            updated["isAlias"].as_bool(),
            Some(case.expected_is_alias()),
            "case {label}: expected isAlias {}, got {updated:?}",
            case.expected_is_alias()
        );
        assert!(
            updated["options"].is_null(),
            "case {label}: the modelled options member is discontinued, so no entry \
             reports it, got {updated:?}"
        );

        // The entries on either side of the subject are untouched.
        assert_eq!(
            after[0], before[0],
            "case {label}: expected the entry before the updated one to be untouched"
        );
        assert_eq!(
            after[2], before[2],
            "case {label}: expected the entry after the updated one to be untouched"
        );

        // Reading changes nothing a later caller sees.
        let again = entries(&emulator, ZONE).await;
        assert_eq!(
            again, after,
            "case {label}: expected the second read to report the same entries, got {again:?}"
        );
    }
}

/// RULE: an entry belongs to the ONE zone the request names, so an id is addressed
/// inside that zone only: another zone's entries are neither changed nor reachable.
#[tokio::test]
async fn an_update_reaches_only_the_zone_it_names() {
    let emulator = Emulator::new();
    let other = "other.example";
    let (before, _) = zone_with(
        &emulator,
        &json!({"name": "www.example.com", "type": "A", "target": "192.0.2.10"}),
    )
    .await;
    create_zone(&emulator, other).await;
    let other_entry = json!({"name": "www.other.example", "type": "A", "target": "192.0.2.20"});
    create_entry(&emulator, other, &other_entry).await;
    let other_before = entries(&emulator, other).await;
    let entry_id = before[1]["id"].as_str().unwrap_or_default().to_string();

    emulator
        .ok(
            "UpdateDomainEntry",
            json!({
                "domainName": ZONE,
                "domainEntry": {
                    "id": entry_id,
                    "name": "www.example.com",
                    "type": "A",
                    "target": "192.0.2.77",
                },
            }),
        )
        .await;
    assert_eq!(
        entries(&emulator, other).await,
        other_before,
        "case two zones: expected '{other}' to be untouched by an update to '{ZONE}'"
    );

    // The same id, addressed in the other zone, belongs to no entry of it.
    let (status, body) = emulator
        .call(
            "UpdateDomainEntry",
            json!({
                "domainName": other,
                "domainEntry": {
                    "id": entry_id,
                    "name": "www.other.example",
                    "type": "A",
                    "target": "192.0.2.21",
                },
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case an id from another zone: expected a 400 refusal, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "NotFoundException",
        "case an id from another zone: expected the modelled NotFoundException, got {body:?}"
    );
    assert_eq!(
        body["code"].as_str(),
        Some("DoesNotExist"),
        "case an id from another zone: expected code DoesNotExist, got {body:?}"
    );
    assert_eq!(
        entries(&emulator, other).await,
        other_before,
        "case an id from another zone: expected a refused update to change nothing"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// RULE: a `domainEntry` that does not describe a recordset this zone can hold is
/// refused — with the modelled `InvalidInputException` (400) whose message names
/// the member, or, where a modelled constraint already covers it, with the
/// boundary walk's own `ValidationException` (400) — and changes nothing.
#[tokio::test]
async fn an_update_the_zone_cannot_hold_is_refused_and_changes_nothing() {
    // Each case is one way the request is not an update of a recordset of this
    // zone: no entry addressed; a missing name, type or target; a type outside the
    // catalog the modelled `DomainEntry$type` documentation lists; a name that is
    // not inside the zone; and a name that is not a DNS name at all — including
    // one character past each published length bound (RFC 1035 §2.3.4: 63 per
    // label, 253 for the whole presentation form).
    let cases: Vec<(&str, Value, &str, &str)> = vec![
        (
            "no id, so no entry is addressed",
            json!({"name": "www.example.com", "type": "A", "target": "192.0.2.60"}),
            "domainEntry.id",
            "InvalidInputException",
        ),
        (
            // `DomainEntry$id` targets `NonEmptyString`, whose `\S` pattern the
            // centralized walk enforces BEFORE the handler runs, so this one is
            // the boundary's own `ValidationException`.
            "a blank id",
            json!({"id": "   ", "name": "www.example.com", "type": "A", "target": "192.0.2.60"}),
            "domainEntry.id",
            "ValidationException",
        ),
        (
            "no name",
            json!({"type": "A", "target": "192.0.2.60"}),
            "domainEntry.name",
            "InvalidInputException",
        ),
        (
            "a blank name",
            json!({"name": "  ", "type": "A", "target": "192.0.2.60"}),
            "domainEntry.name",
            "InvalidInputException",
        ),
        (
            "no type",
            json!({"name": "www.example.com", "target": "192.0.2.60"}),
            "domainEntry.type",
            "InvalidInputException",
        ),
        (
            "no target",
            json!({"name": "www.example.com", "type": "A"}),
            "domainEntry.target",
            "InvalidInputException",
        ),
        (
            "a blank target",
            json!({"name": "www.example.com", "type": "A", "target": ""}),
            "domainEntry.target",
            "InvalidInputException",
        ),
        (
            "a record type the service does not hold",
            json!({"name": "www.example.com", "type": "SPF", "target": "v=spf1 -all"}),
            "domainEntry.type",
            "InvalidInputException",
        ),
        (
            "a record type differing only in case",
            json!({"name": "www.example.com", "type": "cname", "target": "other.example.com"}),
            "domainEntry.type",
            "InvalidInputException",
        ),
        (
            "a name in another zone",
            json!({"name": "www.other.example", "type": "A", "target": "192.0.2.60"}),
            "domainEntry.name",
            "InvalidInputException",
        ),
        (
            "a name that merely contains the zone",
            json!({"name": "www.example.com.attacker.test", "type": "A", "target": "192.0.2.60"}),
            "domainEntry.name",
            "InvalidInputException",
        ),
        (
            "a single-label name",
            json!({"name": "localhost", "type": "A", "target": "192.0.2.60"}),
            "domainEntry.name",
            "InvalidInputException",
        ),
        (
            "a trailing dot",
            json!({"name": "www.example.com.", "type": "A", "target": "192.0.2.60"}),
            "domainEntry.name",
            "InvalidInputException",
        ),
        (
            "a wildcard that is not the leftmost label",
            json!({"name": "a.*.example.com", "type": "A", "target": "192.0.2.60"}),
            "domainEntry.name",
            "InvalidInputException",
        ),
        (
            "a leading hyphen in a label",
            json!({"name": "-www.example.com", "type": "A", "target": "192.0.2.60"}),
            "domainEntry.name",
            "InvalidInputException",
        ),
        (
            "a 64-character label",
            json!({
                "name": format!("{}.example.com", "a".repeat(64)),
                "type": "A",
                "target": "192.0.2.60",
            }),
            "domainEntry.name",
            "InvalidInputException",
        ),
        (
            "a 254-character name",
            json!({
                "name": entry_name_of_length(254, ZONE),
                "type": "TXT",
                "target": "\"past-the-bound\"",
            }),
            "domainEntry.name",
            "InvalidInputException",
        ),
    ];

    for (case, entry, member, code) in cases {
        let emulator = Emulator::new();
        let (before, _) = zone_with(
            &emulator,
            &json!({"name": "www.example.com", "type": "A", "target": "192.0.2.10"}),
        )
        .await;
        // Every case that reaches the zone at all addresses the entry that is in
        // it; the two id cases are what they are.
        let mut requested = entry.clone();
        if requested["id"].is_null() && member != "domainEntry.id" {
            requested["id"] = before[1]["id"].clone();
        }

        let (status, body) = emulator
            .call(
                "UpdateDomainEntry",
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
            body["message"].as_str().unwrap_or_default().contains(member),
            "case {case}: expected the refusal to name {member}, got {body:?}"
        );
        assert_eq!(
            entries(&emulator, ZONE).await,
            before,
            "case {case}: a refused update changes nothing, so the zone's entries must \
             still be what they were"
        );
    }
}

/// RULE: an entry is identified inside its zone by its name, type and target, so
/// an update that would make the addressed entry a duplicate of a DIFFERENT entry
/// of the same zone is refused and changes nothing — while changing any one of the
/// three to a triple no other entry holds is served.
#[tokio::test]
async fn an_update_onto_another_entry_of_the_zone_is_refused() {
    let emulator = Emulator::new();
    let (before, _) = zone_with(
        &emulator,
        &json!({"name": "www.example.com", "type": "A", "target": "192.0.2.10"}),
    )
    .await;
    let entry_id = before[1]["id"].as_str().unwrap_or_default().to_string();
    let twin = before[0].clone();

    let (status, body) = emulator
        .call(
            "UpdateDomainEntry",
            json!({
                "domainName": ZONE,
                "domainEntry": {
                    "id": entry_id,
                    "name": twin["name"],
                    "type": twin["type"],
                    "target": twin["target"],
                },
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case onto another entry: expected a 400 refusal, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInputException",
        "case onto another entry: expected the modelled InvalidInputException, got {body:?}"
    );
    assert_eq!(
        entries(&emulator, ZONE).await,
        before,
        "case onto another entry: expected the zone's entries to be untouched"
    );

    // Each of the three identifying values, varied one at a time off that twin, is
    // a triple no other entry holds, so the update is served.
    let variants = [
        (
            "a different name",
            json!({"name": "second.example.com", "type": twin["type"], "target": twin["target"]}),
        ),
        (
            "a different type",
            json!({"name": twin["name"], "type": "SRV", "target": twin["target"]}),
        ),
        (
            "a different target",
            json!({"name": twin["name"], "type": twin["type"], "target": "\"another-decoy\""}),
        ),
    ];
    for (case, variant) in &variants {
        let mut requested = variant.clone();
        requested["id"] = Value::String(entry_id.clone());
        emulator
            .ok(
                "UpdateDomainEntry",
                json!({"domainName": ZONE, "domainEntry": requested}),
            )
            .await;
        let stored = entries(&emulator, ZONE).await;
        assert_eq!(
            stored.len(),
            before.len(),
            "case {case}: an update adds no entry, got {stored:?}"
        );
        assert_eq!(
            stored[1]["name"], variant["name"],
            "case {case}: expected the varied name to be stored, got {stored:?}"
        );
        assert_eq!(
            stored[1]["type"], variant["type"],
            "case {case}: expected the varied type to be stored, got {stored:?}"
        );
        assert_eq!(
            stored[1]["target"], variant["target"],
            "case {case}: expected the varied target to be stored, got {stored:?}"
        );
    }
}

/// RULE: an id the zone holds no entry for addresses nothing to update, so the
/// request is refused with the service's `DoesNotExist` envelope — HTTP 400,
/// `NotFoundException`, body code `DoesNotExist` and a message naming the id —
/// never a 501, never a hollow 200 and never a newly appended entry.
#[tokio::test]
async fn an_id_the_zone_holds_no_entry_for_is_refused() {
    let emulator = Emulator::new();
    let (before, _) = zone_with(
        &emulator,
        &json!({"name": "www.example.com", "type": "A", "target": "192.0.2.10"}),
    )
    .await;
    let known = before[1]["id"].as_str().unwrap_or_default().to_string();
    // The stored id with its first character changed: a uuid-shaped id this store
    // did not mint, so it addresses no entry however close it looks.
    let near_miss: String = known
        .chars()
        .enumerate()
        .map(|(index, character)| match (index, character) {
            (0, '0') => '1',
            (0, _) => '0',
            _ => character,
        })
        .collect();

    for id in [
        "00000000-0000-0000-0000-000000000000".to_string(),
        "not-an-id".to_string(),
        format!("{known}x"),
        near_miss,
    ] {
        let (status, body) = emulator
            .call(
                "UpdateDomainEntry",
                json!({
                    "domainName": ZONE,
                    "domainEntry": {
                        "id": id,
                        "name": "www.example.com",
                        "type": "A",
                        "target": "192.0.2.80",
                    },
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case the id {id}: expected a 400 refusal, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case the id {id}: expected the modelled NotFoundException, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case the id {id}: expected code DoesNotExist, got {body:?}"
        );
        assert!(
            body["message"].as_str().unwrap_or_default().contains(&id),
            "case the id {id}: expected the refusal to name the id, got {body:?}"
        );
        assert_eq!(
            entries(&emulator, ZONE).await,
            before,
            "case the id {id}: expected the zone's entries to be untouched"
        );
    }
}

/// RULE: an entry is held by a zone, so a `domainName` this account holds no
/// recordset for is refused with the service's `DoesNotExist` envelope and a
/// message naming the resource type and the requested name.
#[tokio::test]
async fn an_update_to_a_zone_that_does_not_exist_is_refused() {
    let emulator = Emulator::new();
    let (before, _) = zone_with(
        &emulator,
        &json!({"name": "www.example.com", "type": "A", "target": "192.0.2.10"}),
    )
    .await;
    let entry_id = before[1]["id"].as_str().unwrap_or_default().to_string();

    for name in ["absent.example", "example.com.extra", "other.test"] {
        let (status, body) = emulator
            .call(
                "UpdateDomainEntry",
                json!({
                    "domainName": name,
                    "domainEntry": {
                        "id": entry_id,
                        "name": format!("www.{name}"),
                        "type": "A",
                        "target": "192.0.2.90",
                    },
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case updating in {name}: expected a 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case updating in {name}: expected the modelled NotFoundException, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case updating in {name}: expected code DoesNotExist, got {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(format!("The Domain does not exist: {name}").as_str()),
            "case updating in {name}: expected the resource-type/name message, got {body:?}"
        );
    }

    assert_eq!(
        entries(&emulator, ZONE).await,
        before,
        "case updating a zone that does not exist: expected the stored zone to be untouched"
    );
}
