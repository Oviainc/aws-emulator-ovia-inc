// SPDX-License-Identifier: CC0-1.0

//! `CreateDomainEntry` — adding recordset entries to a stored Lightsail DNS zone —
//! driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The harness is the sibling suite's (`instance_lifecycle`), so this
//! file stands up no second emulator of its own.
//!
//! The rule under test: `CreateDomainEntry` appends ONE recordset entry to the
//! stored zone the request names — the entry's name, type, target and alias flag
//! verbatim, under a server-assigned entry id unique in the store — so a later
//! `GetDomain` reports exactly those entries in the order they were added; an
//! entry whose name, type or target is missing, whose type is not a DNS record
//! type the service holds, or whose name is not a DNS name inside that zone is
//! refused and appends nothing; the same entry cannot be added twice; and a zone
//! this account does not hold is refused with the service's `DoesNotExist`
//! envelope.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The zone every table row adds its entry to.
const ZONE: &str = "example.com";

/// The `operationType` a change to a zone's recordsets is logged under.
///
/// The modelled `OperationType` enum has `UpdateDomainEntry` and
/// `DeleteDomainEntry` but no `CreateDomainEntry` member, and
/// `Operation$operationType` targets that enum.
const ENTRY_OPERATION_TYPE: &str = "UpdateDomainEntry";

/// An entry name of exactly `total` characters inside `zone`, built from
/// full-length labels plus one shorter filler label.
///
/// Used to sit a case on a published bound (253 characters for the whole name, 63
/// for one label — RFC 1035 §2.3.4) and one character past it.
fn entry_name_of_length(total: usize, zone: &str) -> String {
    let suffix = format!(".{zone}");
    let budget = total - suffix.len();
    let mut prefix = String::new();
    while prefix.len() + 64 <= budget {
        prefix.push_str(&"a".repeat(63));
        prefix.push('.');
    }
    let name = format!("{prefix}{}{suffix}", "b".repeat(budget - prefix.len()));
    assert_eq!(
        name.len(),
        total,
        "case building a {total}-character entry name: got {} characters",
        name.len()
    );
    name
}

// ---------------------------------------------------------------------------
// The create/read table
// ---------------------------------------------------------------------------

/// One `CreateDomainEntry` request the service must serve.
///
/// The rows differ along everything the rule generalises over: the record type
/// (every type the modelled `DomainEntry$type` documentation lists), the form of
/// the entry name (the zone apex, a subdomain, several labels, the `*` wildcard,
/// underscore-prefixed service and validation labels, and the published length
/// bound), the form of the target, whether the entry is an alias, and whether an
/// entry of the same name and type is already in the zone.
struct Row {
    case: &'static str,
    /// `None` where the name is computed (a bound case), in which case
    /// [`Row::entry_name`] builds it.
    name: Option<&'static str>,
    /// The length the computed name must have.
    computed_length: usize,
    entry_type: &'static str,
    target: &'static str,
    /// `None` leaves `isAlias` out of the request entirely.
    is_alias: Option<bool>,
}

impl Row {
    fn entry_name(&self) -> String {
        match self.name {
            Some(name) => name.to_string(),
            None => entry_name_of_length(self.computed_length, ZONE),
        }
    }

    /// The `domainEntry` member this row sends.
    fn entry(&self) -> Value {
        let mut entry = json!({
            "name": self.entry_name(),
            "type": self.entry_type,
            "target": self.target,
        });
        if let Some(is_alias) = self.is_alias {
            entry["isAlias"] = Value::Bool(is_alias);
        }
        entry
    }

    /// The `isAlias` the stored entry must report: absent means "not an alias".
    fn expected_is_alias(&self) -> bool {
        self.is_alias.unwrap_or(false)
    }
}

/// The table. Its record types are the catalog the modelled `DomainEntry$type`
/// documentation lists ("A, AAAA, CNAME, MX, NS, SOA, SRV, TXT"), each row's
/// target in the form that type carries; the issue's own case is an ordinary
/// subdomain A record.
const ROWS: &[Row] = &[
    Row {
        case: "an address record on a subdomain",
        name: Some("www.example.com"),
        computed_length: 0,
        entry_type: "A",
        target: "192.0.2.10",
        is_alias: Some(false),
    },
    Row {
        case: "an address record at the zone apex, with isAlias left out",
        name: Some("example.com"),
        computed_length: 0,
        entry_type: "A",
        target: "192.0.2.11",
        is_alias: None,
    },
    Row {
        case: "an alias address record pointing at a container service",
        name: Some("app.example.com"),
        computed_length: 0,
        entry_type: "A",
        target: "container-service-1.example23scljs.us-west-2.cs.amazonlightsail.com",
        is_alias: Some(true),
    },
    Row {
        case: "an IPv6 address record",
        name: Some("v6.example.com"),
        computed_length: 0,
        entry_type: "AAAA",
        target: "2001:db8::1",
        is_alias: None,
    },
    Row {
        case: "a canonical name record four labels deep",
        name: Some("cdn.assets.blog.example.com"),
        computed_length: 0,
        entry_type: "CNAME",
        target: "exampled1182ne.cloudfront.net",
        is_alias: None,
    },
    Row {
        case: "a mail exchanger at the apex",
        name: Some("example.com"),
        computed_length: 0,
        entry_type: "MX",
        target: "10 inbound-smtp.us-east-1.amazonaws.com",
        is_alias: None,
    },
    Row {
        case: "a second mail exchanger at the apex: name and type are not unique",
        name: Some("example.com"),
        computed_length: 0,
        entry_type: "MX",
        target: "20 inbound-smtp.us-west-2.amazonaws.com",
        is_alias: None,
    },
    Row {
        case: "a name server record delegating a subdomain",
        name: Some("sub.example.com",),
        computed_length: 0,
        entry_type: "NS",
        target: "ns-111.awsdns-22.com.",
        is_alias: None,
    },
    Row {
        case: "a start of authority record",
        name: Some("example.com"),
        computed_length: 0,
        entry_type: "SOA",
        target: "ns-111.awsdns-22.com. awsdns-hostmaster.amazon.com. 1 7200 900 1209600 86400",
        is_alias: None,
    },
    Row {
        case: "a service locator under underscore-prefixed labels",
        name: Some("_sip._tcp.example.com"),
        computed_length: 0,
        entry_type: "SRV",
        target: "10 5 5060 sipserver.example.com",
        is_alias: None,
    },
    Row {
        case: "a text record under an underscore-prefixed validation label",
        name: Some("_acme-challenge.example.com"),
        computed_length: 0,
        entry_type: "TXT",
        target: "\"gv9J2mZ1QwE-token\"",
        is_alias: None,
    },
    Row {
        case: "a wildcard address record",
        name: Some("*.example.com"),
        computed_length: 0,
        entry_type: "A",
        target: "192.0.2.12",
        is_alias: None,
    },
    Row {
        case: "a 253-character name, the longest the presentation form allows",
        name: None,
        computed_length: 253,
        entry_type: "TXT",
        target: "\"at-the-length-bound\"",
        is_alias: None,
    },
];

/// Create the zone every row adds to.
async fn zone(emulator: &Emulator) {
    emulator.ok("CreateDomain", json!({"domainName": ZONE})).await;
}

/// The entries `GetDomain` reports for `ZONE`.
async fn entries(emulator: &Emulator) -> Vec<Value> {
    let domain = emulator.ok("GetDomain", json!({"domainName": ZONE})).await["domain"].clone();
    domain["domainEntries"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a domainEntries array, got {domain:?}"))
        .clone()
}

/// RULE: every entry `GetDomain` reports comes from the one `CreateDomainEntry`
/// that appended it — the caller's name, type, target and alias flag verbatim,
/// under a server-assigned id — in the order the entries were added.
#[tokio::test]
async fn an_entry_reads_back_as_it_was_created() {
    let emulator = Emulator::new();
    zone(&emulator).await;
    let mut operation_ids: BTreeSet<String> = BTreeSet::new();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let created = emulator
            .ok(
                "CreateDomainEntry",
                json!({"domainName": ZONE, "domainEntry": row.entry()}),
            )
            .await;

        // The create answers ONE operation-log entry against the zone: a
        // `DomainEntry` carries no status member, so the entry is in the zone as
        // the call returns and the entry is terminal.
        let operation = created["operation"].clone();
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
        let operation_id = operation["id"].as_str().unwrap_or_default().to_string();
        assert!(
            is_uuid_shaped(&operation_id),
            "case {case}: expected a uuid-shaped operation id, got {operation_id:?}"
        );
        assert!(
            operation_ids.insert(operation_id.clone()),
            "case {case}: expected a fresh operation id, got the already-used {operation_id:?}"
        );

        // The append is observable through the implemented reader, at the end of
        // the zone's entry list.
        let stored = entries(&emulator).await;
        assert_eq!(
            stored.len(),
            index + 1,
            "case {case}: expected the zone to hold exactly the {} entries added so far, \
             got {stored:?}",
            index + 1
        );
        let entry = stored[index].clone();
        assert_eq!(
            entry["name"].as_str(),
            Some(row.entry_name().as_str()),
            "case {case}: expected the entry name the request carried, got {entry:?}"
        );
        assert_eq!(
            entry["type"].as_str(),
            Some(row.entry_type),
            "case {case}: expected the entry type the request carried, got {entry:?}"
        );
        assert_eq!(
            entry["target"].as_str(),
            Some(row.target),
            "case {case}: expected the target the request carried, got {entry:?}"
        );
        assert_eq!(
            entry["isAlias"].as_bool(),
            Some(row.expected_is_alias()),
            "case {case}: expected isAlias {}, got {entry:?}",
            row.expected_is_alias()
        );
        assert!(
            entry["options"].is_null(),
            "case {case}: the modelled options member is discontinued, so no entry \
             reports it, got {entry:?}"
        );

        // Reading changes nothing a later caller sees.
        let again = entries(&emulator).await;
        assert_eq!(
            again, stored,
            "case {case}: expected the second read to report the same entries, got {again:?}"
        );
    }

    // Every entry carries its own server-assigned id — the handle
    // `UpdateDomainEntry` and `DeleteDomainEntry` address it by.
    let stored = entries(&emulator).await;
    let mut entry_ids: BTreeSet<String> = BTreeSet::new();
    for (row, entry) in ROWS.iter().zip(&stored) {
        let id = entry["id"].as_str().unwrap_or_default().to_string();
        assert!(
            is_uuid_shaped(&id),
            "case {}: expected a uuid-shaped entry id, got {id:?}",
            row.case
        );
        assert!(
            entry_ids.insert(id.clone()),
            "case {}: expected a fresh entry id, got the already-used {id:?}",
            row.case
        );
    }
    assert!(
        entry_ids.is_disjoint(&operation_ids),
        "expected entry ids and operation ids to be distinct values, got {entry_ids:?} and \
         {operation_ids:?}"
    );
}

/// RULE: an entry belongs to the ONE zone the request named, so adding entries to
/// a zone leaves every other zone's entries alone.
#[tokio::test]
async fn an_entry_lands_only_in_the_zone_it_names() {
    let emulator = Emulator::new();
    zone(&emulator).await;
    let other = "other.example";
    emulator
        .ok("CreateDomain", json!({"domainName": other}))
        .await;

    for row in ROWS {
        emulator
            .ok(
                "CreateDomainEntry",
                json!({"domainName": ZONE, "domainEntry": row.entry()}),
            )
            .await;
    }
    emulator
        .ok(
            "CreateDomainEntry",
            json!({
                "domainName": other,
                "domainEntry": {"name": "www.other.example", "type": "A", "target": "192.0.2.20"},
            }),
        )
        .await;

    assert_eq!(
        entries(&emulator).await.len(),
        ROWS.len(),
        "case two zones: expected '{ZONE}' to hold only its own {} entries",
        ROWS.len()
    );
    let other_domain = emulator
        .ok("GetDomain", json!({"domainName": other}))
        .await["domain"]
        .clone();
    let other_entries = other_domain["domainEntries"]
        .as_array()
        .unwrap_or_else(|| panic!("case two zones: expected a domainEntries array, got {other_domain:?}"))
        .clone();
    assert_eq!(
        other_entries.len(),
        1,
        "case two zones: expected '{other}' to hold only the one entry it was given, got \
         {other_entries:?}"
    );
    assert_eq!(
        other_entries[0]["name"].as_str(),
        Some("www.other.example"),
        "case two zones: expected its own entry, got {other_entries:?}"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// RULE: a `domainEntry` the service cannot hold as a recordset of the named zone
/// is refused with the modelled `InvalidInputException` (400) whose message names
/// the member, and appends nothing.
#[tokio::test]
async fn an_entry_the_zone_cannot_hold_is_refused_and_appends_nothing() {
    // Each case is one way the entry is not a recordset of this zone: a missing
    // name, type or target; a type outside the catalog the modelled
    // `DomainEntry$type` documentation lists (including one that differs only in
    // case, and a record type DNS has but Lightsail does not list); a name that
    // is not inside the zone; and a name that is not a DNS name at all —
    // including one character past each published length bound (RFC 1035 §2.3.4:
    // 63 per label, 253 for the whole presentation form).
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "no name",
            json!({"type": "A", "target": "192.0.2.30"}),
            "domainEntry.name",
        ),
        (
            "a blank name",
            json!({"name": "   ", "type": "A", "target": "192.0.2.30"}),
            "domainEntry.name",
        ),
        (
            "no type",
            json!({"name": "www.example.com", "target": "192.0.2.30"}),
            "domainEntry.type",
        ),
        (
            "no target",
            json!({"name": "www.example.com", "type": "A"}),
            "domainEntry.target",
        ),
        (
            "a blank target",
            json!({"name": "www.example.com", "type": "A", "target": ""}),
            "domainEntry.target",
        ),
        (
            "a record type the service does not hold",
            json!({"name": "www.example.com", "type": "SPF", "target": "v=spf1 -all"}),
            "domainEntry.type",
        ),
        (
            "a record type differing only in case",
            json!({"name": "www.example.com", "type": "cname", "target": "other.example.com"}),
            "domainEntry.type",
        ),
        (
            "a name in another zone",
            json!({"name": "www.other.example", "type": "A", "target": "192.0.2.30"}),
            "domainEntry.name",
        ),
        (
            "a name that merely contains the zone",
            json!({"name": "www.example.com.attacker.test", "type": "A", "target": "192.0.2.30"}),
            "domainEntry.name",
        ),
        (
            "a name that extends the zone's last label",
            json!({"name": "wwwexample.com", "type": "A", "target": "192.0.2.30"}),
            "domainEntry.name",
        ),
        (
            "a single-label name",
            json!({"name": "localhost", "type": "A", "target": "192.0.2.30"}),
            "domainEntry.name",
        ),
        (
            "an empty inner label",
            json!({"name": "www..example.com", "type": "A", "target": "192.0.2.30"}),
            "domainEntry.name",
        ),
        (
            "a trailing dot",
            json!({"name": "www.example.com.", "type": "A", "target": "192.0.2.30"}),
            "domainEntry.name",
        ),
        (
            "a wildcard that is not the leftmost label",
            json!({"name": "a.*.example.com", "type": "A", "target": "192.0.2.30"}),
            "domainEntry.name",
        ),
        (
            "a space in a label",
            json!({"name": "w ww.example.com", "type": "A", "target": "192.0.2.30"}),
            "domainEntry.name",
        ),
        (
            "a leading hyphen in a label",
            json!({"name": "-www.example.com", "type": "A", "target": "192.0.2.30"}),
            "domainEntry.name",
        ),
        (
            "a 64-character label",
            json!({
                "name": format!("{}.example.com", "a".repeat(64)),
                "type": "A",
                "target": "192.0.2.30",
            }),
            "domainEntry.name",
        ),
        (
            "a 254-character name",
            json!({
                "name": entry_name_of_length(254, ZONE),
                "type": "TXT",
                "target": "\"past-the-bound\"",
            }),
            "domainEntry.name",
        ),
    ];

    for (case, entry, member) in cases {
        let emulator = Emulator::new();
        zone(&emulator).await;
        let (status, body) = emulator
            .call(
                "CreateDomainEntry",
                json!({"domainName": ZONE, "domainEntry": entry}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected a 400 refusal, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {case}: expected the modelled InvalidInputException, got {body:?}"
        );
        assert!(
            body["message"].as_str().unwrap_or_default().contains(member),
            "case {case}: expected the refusal to name {member}, got {body:?}"
        );
        assert!(
            entries(&emulator).await.is_empty(),
            "case {case}: a refused create appends nothing, so the zone must still be \
             empty"
        );
    }
}

/// RULE: an entry is identified inside its zone by its name, type and target, so
/// creating the same entry twice is refused and the zone is left as it was —
/// while an entry differing in any one of the three is a new entry.
#[tokio::test]
async fn the_same_entry_cannot_be_created_twice() {
    let emulator = Emulator::new();
    zone(&emulator).await;
    let entry = json!({"name": "www.example.com", "type": "A", "target": "192.0.2.40"});
    emulator
        .ok(
            "CreateDomainEntry",
            json!({"domainName": ZONE, "domainEntry": entry}),
        )
        .await;
    let before = entries(&emulator).await;

    let (status, body) = emulator
        .call(
            "CreateDomainEntry",
            json!({"domainName": ZONE, "domainEntry": entry}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case the same entry twice: expected a 400 refusal, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInputException",
        "case the same entry twice: expected the modelled InvalidInputException, got {body:?}"
    );
    assert_eq!(
        entries(&emulator).await,
        before,
        "case the same entry twice: expected the zone's entries to be untouched"
    );

    // Each of the three identifying values, varied one at a time, is a new entry.
    let variants = [
        (
            "a different name",
            json!({"name": "mail.example.com", "type": "A", "target": "192.0.2.40"}),
        ),
        (
            "a different type",
            json!({"name": "www.example.com", "type": "TXT", "target": "192.0.2.40"}),
        ),
        (
            "a different target",
            json!({"name": "www.example.com", "type": "A", "target": "192.0.2.41"}),
        ),
    ];
    for (case, variant) in &variants {
        emulator
            .ok(
                "CreateDomainEntry",
                json!({"domainName": ZONE, "domainEntry": variant}),
            )
            .await;
        assert!(
            entries(&emulator)
                .await
                .iter()
                .any(|stored| stored["name"] == variant["name"]
                    && stored["type"] == variant["type"]
                    && stored["target"] == variant["target"]),
            "case {case}: expected the varied entry to be stored, got {:?}",
            entries(&emulator).await
        );
    }
    assert_eq!(
        entries(&emulator).await.len(),
        1 + variants.len(),
        "case varying one value at a time: expected one entry per distinct triple"
    );
}

/// RULE: an entry is held by a zone, so a `domainName` this account holds no
/// recordset for is refused with the service's `DoesNotExist` envelope — HTTP
/// 400, `NotFoundException`, body code `DoesNotExist` and a message naming the
/// resource type and the requested name — never a 501 and never a hollow 200.
#[tokio::test]
async fn an_entry_cannot_be_added_to_a_zone_that_does_not_exist() {
    let emulator = Emulator::new();
    zone(&emulator).await;

    for name in ["absent.example", "example.com.extra", "other.test"] {
        let (status, body) = emulator
            .call(
                "CreateDomainEntry",
                json!({
                    "domainName": name,
                    "domainEntry": {
                        "name": format!("www.{name}"),
                        "type": "A",
                        "target": "192.0.2.50",
                    },
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case adding to {name}: expected a 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case adding to {name}: expected the modelled NotFoundException, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case adding to {name}: expected code DoesNotExist, got {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(format!("The Domain does not exist: {name}").as_str()),
            "case adding to {name}: expected the resource-type/name message, got {body:?}"
        );
    }

    assert!(
        entries(&emulator).await.is_empty(),
        "case adding to a zone that does not exist: expected the stored zone to be \
         untouched"
    );
}
