// SPDX-License-Identifier: CC0-1.0

//! Removing a Lightsail domain recordset — `DeleteDomain` — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: deleting a resource the account holds removes THAT
//! record and everything stored on it from the service's state — for a domain,
//! the modelled documentation's "the specified domain recordset and all of its
//! domain records" — leaving every other record untouched, and logs one terminal
//! operation of the matching modelled `OperationType` against the removed
//! record's own name, resource type and location; a name the account does not
//! hold removes nothing and is refused with the service's `DoesNotExist`
//! envelope, so a second delete of the same name fails rather than reporting a
//! deletion that did not happen. Lightsail serves the domain API from the N.
//! Virginia (`us-east-1`) endpoint alone, so a delete signed for any other
//! Region is refused before anything is removed.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix this service routes on (`OPERATIONS.md`, "Wire
/// protocol").
const TARGET_PREFIX: &str = "Lightsail_20161128";

/// The region a recordset and its operations report in their location: the
/// modelled `RegionName` enum has no `global` member, and the modelled
/// `InvalidInputException` says the domain APIs are available only in us-east-1.
const DOMAIN_REGION: &str = "us-east-1";

/// An Amazon Web Services Region the modelled `RegionName` enum does NOT list,
/// so a case can sit outside that catalog: a scope naming a Region Lightsail
/// does not serve at all is no more the domain Region than any other.
const OFF_ENUM_REGION: &str = "il-central-1";

/// The refusal the modelled `InvalidInputException` documentation states
/// verbatim (`smithy_query(query="doc",shape="InvalidInputException")`), so its
/// text is a requirement and not an example.
const REFUSAL: &str = "Domain and distribution APIs are only available in the N. Virginia \
                       (us-east-1) Amazon Web Services Region. Please set your Amazon Web \
                       Services Region configuration to us-east-1 to create, view, or edit \
                       these resources.";

/// One emulator with one Lightsail service mounted on one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing; routing is by `X-Amz-Target`.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_lightsail_lightsail_20161128::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call `operation` with `body`, answering its HTTP status and parsed body.
    async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        self.call_in(None, operation, body).await
    }

    /// Call `operation` with `body` in `region`, when one is given, answering its
    /// HTTP status and parsed body.
    ///
    /// The suite signs nothing, so the `Authorization` header carries only the
    /// credential SCOPE — which is what names the Region a request is made in.
    async fn call_in(
        &self,
        region: Option<&str>,
        operation: &str,
        body: Value,
    ) -> (u16, Value) {
        let mut builder = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("content-type", "application/x-amz-json-1.1");
        if let Some(region) = region {
            builder = builder.header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261022/{region}/lightsail/\
                     aws4_request, SignedHeaders=host, Signature=not-verified"
                ),
            );
        }
        let request = builder
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Call `operation` and require success, answering the response body.
    async fn ok(&self, operation: &str, body: Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(status, 200, "{operation} must succeed, got {parsed:?}");
        parsed
    }

    /// Create the zone `name` with `tags`, then add `entries` to it, answering
    /// the ids the service minted for those entries.
    async fn zone_with_entries(
        &self,
        name: &str,
        tags: &[(&str, &str)],
        entries: &[Entry],
    ) -> Vec<String> {
        let mut body = json!({"domainName": name});
        if !tags.is_empty() {
            body["tags"] = Value::Array(
                tags.iter()
                    .map(|(key, value)| json!({"key": key, "value": value}))
                    .collect(),
            );
        }
        self.ok("CreateDomain", body).await;

        let mut ids = Vec::new();
        for entry in entries {
            let entry_name = entry.name(name);
            let created = self
                .ok(
                    "CreateDomainEntry",
                    json!({
                        "domainName": name,
                        "domainEntry": {
                            "name": entry_name,
                            "type": entry.entry_type,
                            "target": entry.target,
                        },
                    }),
                )
                .await;
            assert_eq!(
                created["operation"]["resourceName"].as_str(),
                Some(name),
                "case {name}: expected the entry create to log against the zone, got {created:?}"
            );
            let stored = self.ok("GetDomain", json!({"domainName": name})).await;
            let id = stored["domain"]["domainEntries"]
                .as_array()
                .and_then(|list| list.last())
                .and_then(|last| last["id"].as_str())
                .unwrap_or_else(|| {
                    panic!("case {name}: expected the stored entry to carry an id, got {stored:?}")
                })
                .to_string();
            ids.push(id);
        }
        ids
    }
}

/// The wire error code of a failure body: awsJson carries it in `__type`, either
/// bare or namespaced.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// Whether `value` has the shape of the UUID Lightsail's resource and operation
/// ids take: five hex groups of 8-4-4-4-12.
fn is_uuid_shaped(value: &str) -> bool {
    let lengths: Vec<usize> = value.split('-').map(str::len).collect();
    lengths == vec![8, 4, 4, 4, 12]
        && value.chars().all(|c| c == '-' || c.is_ascii_hexdigit())
}

/// One recordset entry to put in a zone before deleting it.
struct Entry {
    /// The label prefixed to the zone's own name, or `""` for the zone apex.
    label: &'static str,
    /// A record type from the modelled `DomainEntry$type` documentation's list
    /// ("address for IPv4 (A), address for IPv6 (AAAA), canonical name (CNAME),
    /// mail exchanger (MX), name server (NS), start of authority (SOA), service
    /// locator (SRV), or text (TXT)").
    entry_type: &'static str,
    target: &'static str,
}

impl Entry {
    fn name(&self, domain_name: &str) -> String {
        if self.label.is_empty() {
            domain_name.to_string()
        } else {
            format!("{}.{domain_name}", self.label)
        }
    }
}

/// One zone the suite creates and then deletes.
///
/// The rows differ along everything the rule generalises over: how many
/// recordsets the zone holds when it is deleted (none, one, several), which
/// record types and names those are, the number of dot-separated labels and the
/// characters in the zone's own name, and how many tags it carries.
struct Row {
    case: &'static str,
    domain_name: &'static str,
    tags: &'static [(&'static str, &'static str)],
    entries: &'static [Entry],
}

const ROWS: &[Row] = &[
    Row {
        case: "an empty two-label zone with no tags",
        domain_name: "example.com",
        tags: &[],
        entries: &[],
    },
    Row {
        case: "a zone holding one apex A record, one tag",
        domain_name: "single.example.net",
        tags: &[("tier", "edge")],
        entries: &[Entry {
            label: "",
            entry_type: "A",
            target: "192.0.2.10",
        }],
    },
    Row {
        case: "a four-label zone holding five records of five types, two tags",
        domain_name: "dns.zone.example.co.uk",
        tags: &[("stack", "prod"), ("owner", "dns-team")],
        entries: &[
            Entry {
                label: "www",
                entry_type: "CNAME",
                target: "cdn.example.org",
            },
            Entry {
                label: "",
                entry_type: "AAAA",
                target: "2001:db8::1",
            },
            Entry {
                label: "mail",
                entry_type: "MX",
                target: "10 mx.example.org",
            },
            Entry {
                label: "_sip._tcp",
                entry_type: "SRV",
                target: "0 5 5060 sip.example.org",
            },
            Entry {
                label: "verify",
                entry_type: "TXT",
                target: "\"token=abc\"",
            },
        ],
    },
    Row {
        case: "digits and hyphens in every label, one record, no tags",
        domain_name: "3rd-zone-7.my-example-9.net",
        tags: &[],
        entries: &[Entry {
            label: "ns1",
            entry_type: "NS",
            target: "ns-1.example.org",
        }],
    },
];

// ---------------------------------------------------------------------------
// The delete table
// ---------------------------------------------------------------------------

/// RULE: `DeleteDomain` removes the named zone and every recordset stored on it,
/// and logs ONE terminal `DeleteDomain` operation naming that zone, its resource
/// type and its location.
#[tokio::test]
async fn deleting_a_zone_removes_it_and_all_of_its_records() {
    // ONE emulator for every row: the operation ids below are minted by one
    // store's own allocator, so they are comparable with each other.
    let emulator = Emulator::new();
    let mut operation_ids: BTreeSet<String> = BTreeSet::new();

    for row in ROWS {
        let case = row.case;
        let entry_ids = emulator
            .zone_with_entries(row.domain_name, row.tags, row.entries)
            .await;

        // The zone really did hold those records before the delete, so the
        // assertions below are about a removal and not about an empty zone.
        let before = emulator
            .ok("GetDomain", json!({"domainName": row.domain_name}))
            .await;
        assert_eq!(
            before["domain"]["domainEntries"].as_array().map(Vec::len),
            Some(row.entries.len()),
            "case {case}: expected the zone to hold its records before the delete, got {:?}",
            before["domain"]["domainEntries"]
        );

        let deleted = emulator
            .ok("DeleteDomain", json!({"domainName": row.domain_name}))
            .await;
        let operation = deleted["operation"].clone();
        assert_eq!(
            operation["operationType"].as_str(),
            Some("DeleteDomain"),
            "case {case}: expected the DeleteDomain operation type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceType"].as_str(),
            Some("Domain"),
            "case {case}: expected the Domain resource type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceName"].as_str(),
            Some(row.domain_name),
            "case {case}: expected the operation to name the deleted zone, got {operation:?}"
        );
        assert_eq!(
            operation["location"]["regionName"].as_str(),
            Some(DOMAIN_REGION),
            "case {case}: expected the zone's own region on the log entry, got {:?}",
            operation["location"]
        );
        assert!(
            operation["location"]["availabilityZone"].is_null(),
            "case {case}: a DNS zone is not zonal, so no availability zone may be \
             reported, got {:?}",
            operation["location"]
        );
        assert_eq!(
            operation["status"].as_str(),
            Some("Succeeded"),
            "case {case}: the zone is gone as the call returns, so expected a Succeeded \
             operation, got {operation:?}"
        );
        assert_eq!(
            operation["isTerminal"].as_bool(),
            Some(true),
            "case {case}: expected a terminal operation, got {operation:?}"
        );
        assert!(
            operation["createdAt"].as_f64().unwrap_or_default() > 0.0,
            "case {case}: expected a creation timestamp, got {operation:?}"
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

        // The log entry is in the store, not just in the response.
        let logged = emulator
            .ok("GetOperation", json!({"operationId": operation_id}))
            .await;
        assert_eq!(
            logged["operation"], operation,
            "case {case}: expected the log to hold the operation the delete answered, got \
             {logged:?}"
        );

        // The zone is gone from the store.
        let (read_status, read_body) = emulator
            .call("GetDomain", json!({"domainName": row.domain_name}))
            .await;
        assert_eq!(
            read_status, 400,
            "case {case}: expected the deleted zone to be unreadable, got {read_status} \
             {read_body:?}"
        );
        assert_eq!(
            read_body["code"].as_str(),
            Some("DoesNotExist"),
            "case {case}: expected code DoesNotExist for the deleted zone, got {read_body:?}"
        );

        // Every record went with it: an entry the zone held can no longer be
        // addressed, and the refusal is about the missing ZONE.
        for (entry, id) in row.entries.iter().zip(&entry_ids) {
            let (status, body) = emulator
                .call(
                    "DeleteDomainEntry",
                    json!({
                        "domainName": row.domain_name,
                        "domainEntry": {"id": id},
                    }),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case}: expected the {} record to be gone with its zone, got {status} \
                 {body:?}",
                entry.entry_type
            );
            assert_eq!(
                body["message"].as_str(),
                Some(format!("The Domain does not exist: {}", row.domain_name).as_str()),
                "case {case}: expected the refusal to be about the missing zone, got {body:?}"
            );
        }
    }
}

/// RULE: the records go with the zone, so a zone re-created under the same name
/// after a delete starts empty rather than inheriting what the deleted one held.
#[tokio::test]
async fn a_zone_recreated_after_a_delete_holds_no_records() {
    for row in ROWS {
        let case = row.case;
        let emulator = Emulator::new();
        emulator
            .zone_with_entries(row.domain_name, row.tags, row.entries)
            .await;
        emulator
            .ok("DeleteDomain", json!({"domainName": row.domain_name}))
            .await;

        // The name is free again: creating it is no longer the "already holds
        // this domain" refusal.
        emulator
            .ok("CreateDomain", json!({"domainName": row.domain_name}))
            .await;
        let fresh = emulator
            .ok("GetDomain", json!({"domainName": row.domain_name}))
            .await;
        assert_eq!(
            fresh["domain"]["domainEntries"].as_array().map(Vec::len),
            Some(0),
            "case {case}: expected the re-created zone to hold no records, got {:?}",
            fresh["domain"]["domainEntries"]
        );
        assert_eq!(
            fresh["domain"]["tags"].as_array().map(Vec::len),
            Some(0),
            "case {case}: expected the re-created zone to carry only the new request's \
             tags, got {:?}",
            fresh["domain"]["tags"]
        );
        assert_ne!(
            fresh["domain"]["arn"].as_str(),
            None,
            "case {case}: expected the re-created zone to carry an ARN, got {fresh:?}"
        );
    }
}

/// RULE: a delete removes ONE record, so every other zone the account holds —
/// and every record in it — is exactly as it was afterwards.
#[tokio::test]
async fn deleting_one_zone_leaves_every_other_untouched() {
    let emulator = Emulator::new();
    for row in ROWS {
        emulator
            .zone_with_entries(row.domain_name, row.tags, row.entries)
            .await;
    }

    let mut remaining: Vec<&Row> = ROWS.iter().collect();
    while let Some(target) = remaining.pop() {
        let before: Vec<Value> = {
            let mut snapshot = Vec::new();
            for row in &remaining {
                snapshot.push(
                    emulator
                        .ok("GetDomain", json!({"domainName": row.domain_name}))
                        .await,
                );
            }
            snapshot
        };

        emulator
            .ok("DeleteDomain", json!({"domainName": target.domain_name}))
            .await;

        for (row, snapshot) in remaining.iter().zip(&before) {
            let after = emulator
                .ok("GetDomain", json!({"domainName": row.domain_name}))
                .await;
            assert_eq!(
                &after, snapshot,
                "case deleting {}: expected '{}' to be untouched, got {after:?}",
                target.case, row.domain_name
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// RULE: a name the account does not hold removes nothing and is refused with
/// the service's `DoesNotExist` envelope — HTTP 400, `NotFoundException`, body
/// code `DoesNotExist` and a message naming the resource type and the requested
/// name — never a 501, never a hollow 200.
#[tokio::test]
async fn deleting_a_zone_the_account_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    let kept = "present.example";
    emulator.zone_with_entries(kept, &[], &[]).await;
    let before = emulator.ok("GetDomain", json!({"domainName": kept})).await;

    // Each case is a name no zone of this account answers to: a name never
    // created, a longer name under one that was, a name differing only in its
    // last label, and a name that is not a DNS domain name at all.
    for name in [
        "absent.example",
        "present.example.extra",
        "present.test",
        "localhost",
        "example..com",
        "",
    ] {
        let (status, body) = emulator
            .call("DeleteDomain", json!({"domainName": name}))
            .await;
        assert_eq!(
            status, 400,
            "case deleting {name:?}: expected a 400 refusal, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case deleting {name:?}: expected the modelled NotFoundException, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case deleting {name:?}: expected code DoesNotExist, got {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(format!("The Domain does not exist: {name}").as_str()),
            "case deleting {name:?}: expected the resource-type/name message, got {body:?}"
        );

        let after = emulator.ok("GetDomain", json!({"domainName": kept})).await;
        assert_eq!(
            after, before,
            "case deleting {name:?}: expected the refusal to remove nothing, got {after:?}"
        );
    }
}

/// The first table row whose zone holds more than one record, so a case about a
/// refusal can show that neither the zone nor its records were touched.
fn a_zone_with_records() -> &'static Row {
    ROWS.iter()
        .find(|row| row.entries.len() > 1)
        .expect("the table holds a zone with more than one record")
}

/// RULE: Lightsail serves the domain API from the N. Virginia (`us-east-1`)
/// endpoint ALONE, so a delete whose credential scope names any other Region is
/// refused with the modelled `InvalidInputException` (400) carrying the
/// documented restriction, and the zone is still there afterwards.
///
/// The dedicated `domain_region_restriction` suite walks the whole modelled
/// `RegionName` catalog for every domain operation; these rows pin the same rule
/// on this operation, including a Region the modelled enum does not list.
#[tokio::test]
async fn deleting_a_zone_outside_the_domain_region_is_refused() {
    for region in ["us-west-2", "eu-central-1", "ap-southeast-1", OFF_ENUM_REGION] {
        let emulator = Emulator::new();
        let row = a_zone_with_records();
        emulator
            .zone_with_entries(row.domain_name, row.tags, row.entries)
            .await;
        let before = emulator
            .ok("GetDomain", json!({"domainName": row.domain_name}))
            .await;

        let (status, body) = emulator
            .call_in(
                Some(region),
                "DeleteDomain",
                json!({"domainName": row.domain_name}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case deleting in {region}: expected a 400 refusal, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case deleting in {region}: expected the modelled InvalidInputException, got \
             {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(REFUSAL),
            "case deleting in {region}: expected the documented restriction, got {:?}",
            body["message"]
        );

        let after = emulator
            .ok("GetDomain", json!({"domainName": row.domain_name}))
            .await;
        assert_eq!(
            after, before,
            "case deleting in {region}: expected the refused delete to leave the zone and \
             its records untouched, got {after:?}"
        );
    }

    // The same delete IS served when the scope names the domain Region.
    let emulator = Emulator::new();
    let row = a_zone_with_records();
    emulator
        .zone_with_entries(row.domain_name, row.tags, row.entries)
        .await;
    let (status, body) = emulator
        .call_in(
            Some(DOMAIN_REGION),
            "DeleteDomain",
            json!({"domainName": row.domain_name}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case deleting in {DOMAIN_REGION}: the delete must be served, got {status} {body:?}"
    );
    assert_eq!(
        body["operation"]["operationType"].as_str(),
        Some("DeleteDomain"),
        "case deleting in {DOMAIN_REGION}: expected the DeleteDomain operation, got {body:?}"
    );
}

/// RULE: the delete is not idempotent — the record is gone after the first one,
/// so a second delete of the same name has nothing to remove and fails rather
/// than reporting a deletion that did not happen.
#[tokio::test]
async fn deleting_a_zone_twice_fails_the_second_time() {
    for row in ROWS {
        let case = row.case;
        let emulator = Emulator::new();
        emulator
            .zone_with_entries(row.domain_name, row.tags, row.entries)
            .await;
        emulator
            .ok("DeleteDomain", json!({"domainName": row.domain_name}))
            .await;

        let (status, body) = emulator
            .call("DeleteDomain", json!({"domainName": row.domain_name}))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the second delete to be refused, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case}: expected the modelled NotFoundException, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case {case}: expected code DoesNotExist, got {body:?}"
        );
        assert!(
            body["operation"].is_null(),
            "case {case}: a refused delete logs no operation, got {body:?}"
        );
    }
}
