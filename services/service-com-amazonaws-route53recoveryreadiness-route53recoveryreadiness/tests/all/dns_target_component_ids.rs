// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the component identifier this service GENERATES for a resource
//! described by a DNS target resource.
//!
//! The rule under test: a resource-set entry that describes a DNS target resource and
//! carries no component identifier is stored, and reported by every read of the set, with
//! one the service generates for it — unique within the set and the same on every read —
//! and that identifier is what addresses the resource in
//! `GetReadinessCheckResourceStatus`; an identifier the caller supplied is kept as given,
//! and an entry that describes no DNS target resource is given none.
//!
//! `Resource$ComponentId` is "the component identifier of the resource, generated when
//! DNS target resource is used" (model: `smithy_query(query="doc", shape="Resource",
//! member="ComponentId")`), and `GetReadinessCheckResourceStatus`'s `ResourceIdentifier`
//! is "the Amazon Resource Name (ARN) or the identifier generated for the resource by
//! Application Recovery Controller (for example, for a DNS target resource)" (model:
//! `smithy_query(query="doc", shape="GetReadinessCheckResourceStatusRequest",
//! member="ResourceIdentifier")`) — so generating it is the service's job, and a DNS
//! target resource (which carries no `ResourceArn`) is addressable by nothing else.
//!
//! The tests are tables. Rows differ along what the rule generalises over: how many
//! entries the set holds, which `Resource` members each carries, how deep the
//! `DnsTargetResource` subtree goes and which target kind it names, whether the caller
//! supplied an identifier, whether two entries are identical, and whether the write was a
//! create or an update.
//!
//! Every assertion goes over the wire through the same [`MultiServiceRouter`] the
//! production emulator mounts (the harness is shared with `readiness_check_lifecycle.rs`
//! rather than copied), so an answer that is not computed from persisted state cannot
//! pass.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::readiness_check_lifecycle::{Emulator, Reply};

/// The resource-set type a DNS target resource belongs to, from the `ResourceSetType`
/// member's own catalog (model: `smithy_query(query="doc",
/// shape="CreateResourceSetRequest", member="ResourceSetType")`).
const DNS_TARGET_TYPE: &str = "AWS::Route53RecoveryReadiness::DNSTargetResource";

/// The constant sentence this service answers a not-found with. Written out here rather
/// than imported, so the test pins the string.
const NOT_FOUND_MESSAGE: &str = "Resource not found";

/// The readiness of a resource nothing has audited, one of the four values the
/// `Readiness` enum declares (model: `smithy_query(query="shape", shape="Readiness")`).
const UNKNOWN: &str = "UNKNOWN";

/// `POST /resourcesets` (model: `CreateResourceSet`).
async fn create_resource_set(emulator: &Emulator, name: &str, resources: &Value) -> Reply {
    emulator
        .send(
            "POST",
            "/resourcesets",
            Some(json!({
                "resourceSetName": name,
                "resourceSetType": DNS_TARGET_TYPE,
                "resources": resources,
            })),
        )
        .await
}

/// `PUT /resourcesets/{ResourceSetName}` (model: `UpdateResourceSet`).
async fn update_resource_set(emulator: &Emulator, name: &str, resources: &Value) -> Reply {
    emulator
        .send(
            "PUT",
            &format!("/resourcesets/{name}"),
            Some(json!({
                "resourceSetType": DNS_TARGET_TYPE,
                "resources": resources,
            })),
        )
        .await
}

/// `GET /resourcesets/{ResourceSetName}` (model: `GetResourceSet`).
async fn get_resource_set(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .send("GET", &format!("/resourcesets/{name}"), None)
        .await
}

/// The entry `ListResourceSets` reports for `name`, or `None` when the listing does not
/// hold it.
async fn listed_entry(emulator: &Emulator, name: &str) -> Option<Value> {
    let listed = emulator
        .send("GET", "/resourcesets?maxResults=1000", None)
        .await;
    listed
        .field("resourceSets")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| item.get("resourceSetName").and_then(Value::as_str) == Some(name))
                .cloned()
        })
}

/// A reply's `resources` member, with an omitted member read as the empty list.
fn resources_of(value: &Value) -> Value {
    match value.get("resources") {
        Some(Value::Array(items)) => Value::Array(items.clone()),
        _ => Value::Array(Vec::new()),
    }
}

/// `entry` without its `componentId`, so the rest of a reported entry can be compared to
/// what was submitted.
fn without_component_id(entry: &Value) -> Value {
    let mut stripped = entry.clone();
    if let Some(members) = stripped.as_object_mut() {
        members.remove("componentId");
    }
    stripped
}

/// Assert `reported` is `submitted` with the rule applied, and answer the identifier each
/// entry carries, in order.
///
/// Per entry: an identifier the caller supplied comes back unchanged; an entry describing
/// a DNS target resource and submitting none carries a generated identifier, which has to
/// be usable as ONE URI path segment because that is where
/// `GetReadinessCheckResourceStatus` takes it (`@httpLabel`); an entry describing no DNS
/// target resource is given none. Every other member is reported as submitted.
fn reported_component_ids(case: &str, reported: &Value, submitted: &Value) -> Vec<Option<String>> {
    let reported = reported.as_array().cloned().unwrap_or_default();
    let submitted = submitted.as_array().cloned().unwrap_or_default();
    assert_eq!(
        reported.len(),
        submitted.len(),
        "case {case:?}: expected {} resources back, got {reported:?}",
        submitted.len()
    );

    let mut ids = Vec::new();
    for (index, (got, sent)) in reported.iter().zip(submitted.iter()).enumerate() {
        let id = got
            .get("componentId")
            .and_then(Value::as_str)
            .map(str::to_string);
        let submitted_id = sent.get("componentId").and_then(Value::as_str);
        let describes_dns_target = sent.get("dnsTargetResource").is_some();
        match submitted_id {
            Some(supplied) => assert_eq!(
                id.as_deref(),
                Some(supplied),
                "case {case:?}: resource {index}: expected the submitted componentId kept, \
                 got {id:?}"
            ),
            None if describes_dns_target => {
                let minted = id.clone().unwrap_or_default();
                assert!(
                    !minted.is_empty(),
                    "case {case:?}: resource {index} describes a DNS target resource and \
                     submitted no componentId, so the service generates one, got {got:?}"
                );
                assert!(
                    minted
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~')),
                    "case {case:?}: resource {index}: a generated componentId addresses the \
                     resource as one URI path segment, so it carries no character a client \
                     has to escape, got {minted:?}"
                );
            }
            None => assert!(
                id.is_none(),
                "case {case:?}: resource {index} describes no DNS target resource, so no \
                 componentId is generated for it, got {id:?}"
            ),
        }
        assert_eq!(
            without_component_id(got),
            without_component_id(sent),
            "case {case:?}: resource {index}: expected every other submitted member \
             preserved, got {got:?}"
        );
        ids.push(id);
    }
    ids
}

/// Assert no generated identifier repeats another identifier of the same set — neither
/// one the service generated nor one the caller wrote. The identifier addresses ONE
/// resource of the set, so a status request carrying it must not be ambiguous.
fn assert_generated_ids_are_distinct(case: &str, ids: &[Option<String>], submitted: &Value) {
    let submitted = submitted.as_array().cloned().unwrap_or_default();
    let mut seen: BTreeSet<String> = submitted
        .iter()
        .filter_map(|sent| {
            sent.get("componentId")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    for (id, sent) in ids.iter().zip(submitted.iter()) {
        if sent.get("componentId").is_some() {
            continue;
        }
        let Some(id) = id else {
            continue;
        };
        assert!(
            seen.insert(id.clone()),
            "case {case:?}: expected a generated componentId no other resource of the set \
             carries, got {id:?} among {seen:?}"
        );
    }
}

/// One DNS target resource naming a network load balancer behind a Route 53 record.
fn nlb_entry(index: usize) -> Value {
    json!({
        "dnsTargetResource": {
            "domainName": format!("app-{index}.example.com"),
            "hostedZoneArn": "arn:aws:route53:::hostedzone/Z1EXAMPLE1",
            "recordSetId": format!("record-{index}"),
            "recordType": "A",
            "targetResource": {
                "nLBResource": {
                    "arn": format!(
                        "arn:aws:elasticloadbalancing:us-east-1:111122223333:loadbalancer/net/nlb-{index}/0123456789abcdef"
                    ),
                },
            },
        },
    })
}

/// Rows differing along the entry count, which `Resource` members each entry carries, how
/// deep the `DnsTargetResource` subtree goes and which target kind it names, whether the
/// caller supplied an identifier, and whether two entries are identical.
fn rows() -> Vec<(&'static str, Value)> {
    vec![
        ("an empty set", json!([])),
        (
            "one DNS entry carrying the whole subtree",
            json!([nlb_entry(1)]),
        ),
        (
            "a DNS entry carrying only a domain name",
            json!([{"dnsTargetResource": {"domainName": "bare.example.com"}}]),
        ),
        (
            "a DNS entry pointing at a Route 53 record",
            json!([{
                "dnsTargetResource": {
                    "domainName": "api.example.com",
                    "recordType": "CNAME",
                    "targetResource": {
                        "r53Resource": {
                            "domainName": "api.example.com",
                            "recordSetId": "record-r53",
                        },
                    },
                },
                "readinessScopes": [
                    "arn:aws:route53-recovery-readiness::111122223333:cell/cell_a",
                ],
            }]),
        ),
        (
            "a DNS entry whose component id the caller supplied",
            json!([{
                "componentId": "written_by_the_caller",
                "dnsTargetResource": {"domainName": "supplied.example.com"},
            }]),
        ),
        (
            "an entry carrying only an ARN",
            json!([{"resourceArn": "arn:aws:sqs:us-east-1:111122223333:queue_1"}]),
        ),
        (
            "an ARN entry beside two DNS entries",
            json!([
                {"resourceArn": "arn:aws:dynamodb:us-east-1:111122223333:table/one"},
                nlb_entry(2),
                {"dnsTargetResource": {"domainName": "second.example.com", "recordType": "AAAA"}},
            ]),
        ),
        (
            "two entries whose DNS target resources are identical",
            json!([nlb_entry(3), nlb_entry(3)]),
        ),
        (
            "one supplied id repeated on two entries, beside a DNS entry with none",
            json!([
                {"componentId": "shared_id", "dnsTargetResource": {"domainName": "a.example.com"}},
                {"componentId": "shared_id", "resourceArn": "arn:aws:sns:us-east-1:111122223333:topic"},
                nlb_entry(4),
            ]),
        ),
        (
            "eight DNS entries",
            Value::Array((10..18).map(nlb_entry).collect()),
        ),
    ]
}

/// Every row: the create reports the submitted list with the generated identifiers added,
/// and `GetResourceSet` and `ListResourceSets` report that identical list afterwards.
#[tokio::test]
async fn a_dns_target_entry_is_stored_with_a_generated_component_id() {
    let emulator = Emulator::new();

    for (index, (case, submitted)) in rows().into_iter().enumerate() {
        let name = format!("dns_set_{index}");
        let created = create_resource_set(&emulator, &name, &submitted).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200, got {} / {}",
            created.status, created.body
        );
        let stored = resources_of(&created.body);
        let ids = reported_component_ids(&format!("{case} (create)"), &stored, &submitted);
        assert_generated_ids_are_distinct(case, &ids, &submitted);

        let fetched = get_resource_set(&emulator, &name).await;
        assert_eq!(
            resources_of(&fetched.body),
            stored,
            "case {case:?}: expected GetResourceSet to report the identical list the create \
             reported, got {}",
            fetched.body
        );

        let listed = listed_entry(&emulator, &name)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: expected the set to be listed"));
        assert_eq!(
            resources_of(&listed),
            stored,
            "case {case:?}: expected ListResourceSets to report the identical list the create \
             reported, got {listed}"
        );
    }
}

/// Every row: an update is held to the same rule as a create — a replacement entry that
/// describes a DNS target resource and carries no identifier is stored with a generated
/// one, and the list the update reports is what the reader sees.
#[tokio::test]
async fn an_update_generates_component_ids_for_its_dns_entries_too() {
    let emulator = Emulator::new();

    for (index, (case, replacement)) in rows().into_iter().enumerate() {
        let name = format!("updated_dns_set_{index}");
        // The record starts out holding an entry of the OTHER kind, so nothing of the
        // replacement's identifiers can have come from what was stored before.
        let seeded = create_resource_set(
            &emulator,
            &name,
            &json!([{"resourceArn": "arn:aws:sqs:us-east-1:111122223333:seed"}]),
        )
        .await;
        assert_eq!(
            seeded.status, 200,
            "case {case:?}: expected the seed create to answer HTTP 200, got {} / {}",
            seeded.status, seeded.body
        );

        let updated = update_resource_set(&emulator, &name, &replacement).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200, got {} / {}",
            updated.status, updated.body
        );
        let stored = resources_of(&updated.body);
        let ids = reported_component_ids(&format!("{case} (update)"), &stored, &replacement);
        assert_generated_ids_are_distinct(case, &ids, &replacement);

        let fetched = get_resource_set(&emulator, &name).await;
        assert_eq!(
            resources_of(&fetched.body),
            stored,
            "case {case:?}: expected the update to be persisted, got {}",
            fetched.body
        );

        // Re-submitting the list the service reported changes nothing: the identifiers it
        // generated are now the caller's, and are kept as given.
        let again = update_resource_set(&emulator, &name, &stored).await;
        assert_eq!(
            resources_of(&again.body),
            stored,
            "case {case:?}: expected re-submitting the reported list to report it back \
             unchanged, got {}",
            again.body
        );

        // And re-submitting the ORIGINAL list, without identifiers, yields the same ones:
        // the identifier is a function of the resource, not of when it was written.
        let resubmitted = update_resource_set(&emulator, &name, &replacement).await;
        assert_eq!(
            resources_of(&resubmitted.body),
            stored,
            "case {case:?}: expected the same identifiers for the same resources, got {}",
            resubmitted.body
        );
    }
}

/// The generated identifier is what addresses the resource: a readiness check over the set
/// reports a per-resource status for it, and reports the modelled not-found for an
/// identifier the set does not hold.
#[tokio::test]
async fn a_generated_component_id_addresses_the_resource_in_a_readiness_check_status() {
    let emulator = Emulator::new();

    let submitted = json!([
        nlb_entry(5),
        {"dnsTargetResource": {"domainName": "other.example.com"}},
        {"resourceArn": "arn:aws:sqs:us-east-1:111122223333:queue_9"},
    ]);
    let created = create_resource_set(&emulator, "addressed_set", &submitted).await;
    assert_eq!(
        created.status, 200,
        "expected the create to answer HTTP 200, got {} / {}",
        created.status, created.body
    );
    let stored = resources_of(&created.body);
    let ids = reported_component_ids("addressed set", &stored, &submitted);

    let check = emulator
        .send(
            "POST",
            "/readinesschecks",
            Some(json!({
                "readinessCheckName": "addressed_check",
                "resourceSetName": "addressed_set",
            })),
        )
        .await;
    assert_eq!(
        check.status, 200,
        "expected the readiness check to be created, got {} / {}",
        check.status, check.body
    );

    for (index, identifier) in ids.iter().enumerate() {
        let Some(identifier) = identifier else {
            continue;
        };
        let case = format!("resource {index} by its generated componentId");
        let reply = emulator
            .send(
                "GET",
                &format!("/readinesschecks/addressed_check/resource/{identifier}/status"),
                None,
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected the generated identifier to address the resource with \
             HTTP 200, got {} / {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.string("readiness").as_deref(),
            Some(UNKNOWN),
            "case {case:?}: expected readiness {UNKNOWN}, got {}",
            reply.body
        );
    }

    // An identifier of the right FORM that the set does not hold is still not found: the
    // status is reported for a resource the check watches, not for any well-formed id.
    let unknown_id = ids
        .iter()
        .flatten()
        .next()
        .map(|id| format!("{id}zz"))
        .expect("the set holds a generated identifier");
    let reply = emulator
        .send(
            "GET",
            &format!("/readinesschecks/addressed_check/resource/{unknown_id}/status"),
            None,
        )
        .await;
    assert_eq!(
        reply.status, 404,
        "expected HTTP 404 for an identifier the set does not hold, got {} / {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        "ResourceNotFoundException",
        "expected the modelled not-found, got {:?} / {}",
        reply.error_code(),
        reply.body
    );
    assert_eq!(
        reply.string("message").unwrap_or_default(),
        NOT_FOUND_MESSAGE,
        "expected the service's constant not-found sentence, got {}",
        reply.body
    );
}

/// The identifier is scoped to the set: the same DNS target resource in two different sets
/// is two different resources, so each is addressable without the other's check answering.
#[tokio::test]
async fn the_same_dns_target_resource_in_two_sets_gets_two_identifiers() {
    let emulator = Emulator::new();

    let submitted = json!([nlb_entry(6)]);
    let mut minted = Vec::new();
    for name in ["scoped_set_a", "scoped_set_b"] {
        let created = create_resource_set(&emulator, name, &submitted).await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: expected HTTP 200, got {} / {}",
            created.status, created.body
        );
        let stored = resources_of(&created.body);
        let ids = reported_component_ids(name, &stored, &submitted);
        minted.push(
            ids.into_iter()
                .next()
                .flatten()
                .unwrap_or_else(|| panic!("case {name:?}: expected a generated identifier")),
        );
    }

    assert_ne!(
        minted[0], minted[1],
        "expected one identifier per set, got {minted:?}"
    );
}
