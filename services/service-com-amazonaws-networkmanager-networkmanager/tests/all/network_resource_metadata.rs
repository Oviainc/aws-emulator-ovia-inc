// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `UpdateNetworkResourceMetadata`, driven over HTTP.
//!
//! Every call goes through the real `MultiServiceRouter`, so the restJson1 route, the
//! `GlobalNetworkId` / `ResourceArn` label bindings and the error envelopes are exercised
//! as a client hits them. Nothing reaches into `crate::state`: the metadata is always read
//! back through `GetNetworkResources`, the inventory reader that reports it.
//!
//! The rule under test: the metadata map a request carries becomes the WHOLE metadata of
//! the network resource its ARN names inside the global network the path addresses — so a
//! later update replaces it, a key the request leaves out is gone, and the inventory
//! reports exactly that map for that one resource, whatever kind it is — while an ARN
//! naming no network resource of that global network is not found and a string that is no
//! ARN of this service is refused on the member.
//! ARN, and a map carrying more entries than one resource's metadata may hold is refused
//! on the member.

use serde_json::{json, Map, Value};

use crate::attachment_lifecycle::{
    attachment_id, core_network_at, create_vpc_attachment, subnet_arn, vpc_arn,
};
use crate::device_lifecycle::create_device_ok;
use crate::global_network_lifecycle::{error_code, expected_arn, expected_scoped_arn, Emulator};
use crate::link_lifecycle::{create_link_ok, plain_link};
use crate::network_resources::{create_ok, listed, minted};
use crate::resource_not_found_message::assert_not_found_answer;
use crate::resource_tagging::escape;

/// The Region the fixture's core network configures an edge in, which is therefore the
/// Region its VPC attachment's VPC and subnet are in.
const EDGE_REGION: &str = "us-west-2";

/// The longest a metadata key or value may be: both target `ConstrainedString`, "len
/// 0..256" (`smithy_query shape=NetworkResourceMetadataMap`).
const MAX_METADATA_CHARS: usize = 256;

/// How many entries one network resource's metadata may carry. The model bounds only each
/// key and value, so this is the emulator's own per-resource bound
/// (`state::MAX_METADATA_ENTRIES`), which is the 50 of AWS's published per-resource tag
/// limit — the closest limit AWS does publish for a resource's key/value side map.
const MAX_METADATA_ENTRIES: usize = 50;

/// A metadata map of `entries` distinct entries, for the rows that drive the bound.
fn metadata_of_size(entries: usize) -> Value {
    let mut map = Map::new();
    for index in 0..entries {
        map.insert(format!("key-{index}"), json!(format!("value-{index}")));
    }
    Value::Object(map)
}

/// One network resource of the fixture's global network: the type the inventory reports it
/// under, and the ARN the metadata is addressed by.
struct Subject {
    resource_type: &'static str,
    arn: String,
}

/// A global network holding one resource of every kind this service keeps in a global
/// network's network-resource inventory, so the table below spans the KINDS the rule covers
/// — the ones held under the global network itself (a site, a link, a device, a connection)
/// and the ones held under its core network (the core network, an attachment).
async fn inventory(emulator: &Emulator) -> (String, Vec<Subject>) {
    let (network, core_network) =
        core_network_at(emulator, "network resource metadata", &[EDGE_REGION]).await;
    let site = emulator.create_site(&network, "hq").await;
    let link = create_link_ok(emulator, &network, &site, &plain_link()).await;
    let device = create_device_ok(emulator, &network, json!({ "SiteId": site })).await;
    let peer_device = create_device_ok(emulator, &network, json!({})).await;
    let connection = minted(
        &create_ok(
            emulator,
            &format!("/global-networks/{network}/connections"),
            json!({ "DeviceId": device, "ConnectedDeviceId": peer_device }),
        )
        .await,
        "Connection",
        "ConnectionId",
    );
    let (status, attached) = create_vpc_attachment(
        emulator,
        json!({
            "CoreNetworkId": core_network,
            "VpcArn": vpc_arn(EDGE_REGION, 1),
            "SubnetArns": [subnet_arn(EDGE_REGION, 2)],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateVpcAttachment: expected 200, got {status} with {attached}"
    );
    let attachment = attachment_id("vpc attachment", &attached["VpcAttachment"]);

    let subjects = vec![
        Subject {
            resource_type: "site",
            arn: expected_scoped_arn("site", &network, &site),
        },
        Subject {
            resource_type: "link",
            arn: expected_scoped_arn("link", &network, &link),
        },
        Subject {
            resource_type: "device",
            arn: expected_scoped_arn("device", &network, &device),
        },
        Subject {
            resource_type: "connection",
            arn: expected_scoped_arn("connection", &network, &connection),
        },
        Subject {
            resource_type: "core-network",
            arn: expected_arn("core-network", &core_network),
        },
        Subject {
            resource_type: "attachment",
            arn: expected_arn("attachment", &attachment),
        },
    ];
    (network, subjects)
}

/// One metadata map a row stores, named for the assertion messages.
struct MetadataCase {
    name: &'static str,
    metadata: Value,
}

/// The maps the table stores, differing along what the rule generalises over: how many
/// entries the map carries (none, one, several) and what the keys and values inside it are
/// — each a `ConstrainedString` of 0 to [`MAX_METADATA_CHARS`] characters, so the empty
/// string and a string at that bound are both in the class.
fn metadata_cases() -> Vec<MetadataCase> {
    let mut longest = Map::new();
    longest.insert(
        "k".repeat(MAX_METADATA_CHARS),
        json!("v".repeat(MAX_METADATA_CHARS)),
    );
    vec![
        MetadataCase {
            name: "one entry",
            metadata: json!({ "owner": "net-eng" }),
        },
        MetadataCase {
            name: "several entries",
            metadata: json!({ "owner": "net-eng", "tier": "gold", "zone": "a" }),
        },
        MetadataCase {
            name: "a key and a value at the modelled maximum length",
            metadata: Value::Object(longest),
        },
        MetadataCase {
            name: "an empty key and an empty value",
            metadata: json!({ "": "" }),
        },
        MetadataCase {
            name: "a value outside ASCII",
            metadata: json!({ "site": "Zürich HQ" }),
        },
        MetadataCase {
            name: "no entries at all",
            metadata: json!({}),
        },
    ]
}

/// The `UpdateNetworkResourceMetadata` URI of one resource.
///
/// `/global-networks/{GlobalNetworkId}/network-resources/{ResourceArn}/metadata` binds a
/// NON-greedy `@httpLabel` for the ARN (`smithy_query query="operation"
/// operation="UpdateNetworkResourceMetadata"`), so a client sends its `:` and `/` encoded.
fn metadata_uri(network: &str, arn: &str) -> String {
    format!(
        "/global-networks/{network}/network-resources/{}/metadata",
        escape(arn)
    )
}

/// `UpdateNetworkResourceMetadata` with `metadata`, answering the status and the body.
async fn update_metadata(
    emulator: &Emulator,
    network: &str,
    arn: &str,
    metadata: &Value,
) -> (u16, Value) {
    emulator
        .patch(
            &metadata_uri(network, arn),
            json!({ "Metadata": metadata }),
        )
        .await
}

/// `UpdateNetworkResourceMetadata` that must succeed, answering the body.
async fn update_ok(emulator: &Emulator, network: &str, case: &str, arn: &str, metadata: &Value) -> Value {
    let (status, body) = update_metadata(emulator, network, arn, metadata).await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200, got {status} with {body}"
    );
    body
}

/// The `Metadata` the inventory reports for the one resource `arn` names, or `Value::Null`
/// when it reports the member absent.
async fn reported_metadata(emulator: &Emulator, network: &str, case: &str, arn: &str) -> Value {
    let query = format!("resourceArn={}", escape(arn));
    let (entries, _) = listed(emulator, network, &query).await;
    assert_eq!(
        entries.len(),
        1,
        "case {case}: expected exactly one inventory entry for {arn}, got {entries:?}"
    );
    entries[0]["Metadata"].clone()
}

#[tokio::test]
async fn every_metadata_map_round_trips_through_the_inventory_for_every_resource_kind() {
    let emulator = Emulator::new();
    let (network, subjects) = inventory(&emulator).await;

    for subject in &subjects {
        // Before anything is stored, the resource carries no metadata at all — the member
        // is absent rather than an empty map.
        let before = reported_metadata(
            &emulator,
            &network,
            &format!("{} before any update", subject.resource_type),
            &subject.arn,
        )
        .await;
        assert_eq!(
            before,
            Value::Null,
            "case {} before any update: expected no Metadata, got {before}",
            subject.resource_type
        );

        for case in metadata_cases() {
            let name = format!("{} with {}", subject.resource_type, case.name);
            let answer = update_ok(&emulator, &network, &name, &subject.arn, &case.metadata).await;
            assert_eq!(
                answer["ResourceArn"],
                json!(subject.arn),
                "case {name}: expected the addressed ResourceArn, got {answer}"
            );
            assert_eq!(
                answer["Metadata"], case.metadata,
                "case {name}: expected the stored map, got {answer}"
            );

            let reported = reported_metadata(&emulator, &network, &name, &subject.arn).await;
            assert_eq!(
                reported, case.metadata,
                "case {name}: GetNetworkResources must report the stored map, got {reported}"
            );
        }
    }
}

#[tokio::test]
async fn metadata_belongs_to_one_resource_and_a_later_update_replaces_the_whole_map() {
    let emulator = Emulator::new();
    let (network, subjects) = inventory(&emulator).await;

    // Each resource is given a map naming its own kind, so a resource reporting another's
    // metadata — or one write reaching two resources — shows up here.
    let own: Vec<Value> = subjects
        .iter()
        .enumerate()
        .map(|(index, subject)| {
            json!({ "kind": subject.resource_type, "slot": index.to_string() })
        })
        .collect();
    for (subject, metadata) in subjects.iter().zip(&own) {
        update_ok(
            &emulator,
            &network,
            subject.resource_type,
            &subject.arn,
            metadata,
        )
        .await;
    }
    for (subject, metadata) in subjects.iter().zip(&own) {
        let reported =
            reported_metadata(&emulator, &network, subject.resource_type, &subject.arn).await;
        assert_eq!(
            &reported, metadata,
            "case {}: expected its own metadata, got {reported}",
            subject.resource_type
        );
    }

    // The map is the WHOLE metadata, so a narrower map replaces the wider one rather than
    // merging into it, and an empty map leaves the resource with no entries.
    let subject = &subjects[0];
    for (case, stored, expected) in [
        (
            "one key replaces the stored pair",
            json!({ "kind": "replaced" }),
            json!({ "kind": "replaced" }),
        ),
        (
            "a key the request leaves out is gone",
            json!({ "slot": "only" }),
            json!({ "slot": "only" }),
        ),
        (
            "an empty map clears every entry",
            json!({}),
            json!({}),
        ),
    ] {
        let answer = update_ok(&emulator, &network, case, &subject.arn, &stored).await;
        assert_eq!(
            answer["Metadata"], expected,
            "case {case}: expected {expected}, got {answer}"
        );
        let reported = reported_metadata(&emulator, &network, case, &subject.arn).await;
        assert_eq!(
            reported, expected,
            "case {case}: GetNetworkResources must report {expected}, got {reported}"
        );
    }

    // Replacing one resource's metadata changed no other resource's.
    for (subject, metadata) in subjects.iter().zip(&own).skip(1) {
        let reported =
            reported_metadata(&emulator, &network, subject.resource_type, &subject.arn).await;
        assert_eq!(
            &reported, metadata,
            "case {} after another resource was updated: expected its own metadata, got {reported}",
            subject.resource_type
        );
    }
}

#[tokio::test]
async fn an_arn_that_names_no_network_resource_of_the_global_network_is_refused_and_stores_nothing()
{
    let emulator = Emulator::new();
    let (network, subjects) = inventory(&emulator).await;
    let subject = &subjects[0];
    let kept = json!({ "owner": "net-eng" });
    update_ok(&emulator, &network, "the resource under test", &subject.arn, &kept).await;

    // A site of ANOTHER global network: a well-formed ARN of this service naming a stored
    // resource that this global network does not hold.
    let other_network = emulator.create_global_network("another network").await;
    let other_site = emulator.create_site(&other_network, "elsewhere").await;
    let other_site_arn = expected_scoped_arn("site", &other_network, &other_site);
    // A well-formed ARN of this service that names nothing at all.
    let unknown_site = "site-00000000000000099";
    let unknown_site_arn = expected_scoped_arn("site", &network, unknown_site);
    // A global network the store does not hold, addressed by the path.
    let unknown_network = "global-network-00000000000000099";

    for (case, uri, resource_id, resource_type) in [
        (
            "another global network's site",
            metadata_uri(&network, &other_site_arn),
            other_site.clone(),
            "site",
        ),
        (
            "an ARN of this service naming nothing",
            metadata_uri(&network, &unknown_site_arn),
            unknown_site.to_string(),
            "site",
        ),
        (
            "a global network the store does not hold",
            metadata_uri(unknown_network, &subject.arn),
            unknown_network.to_string(),
            "global-network",
        ),
    ] {
        assert_not_found_answer(
            &emulator,
            case,
            "PATCH",
            &uri,
            Some(json!({ "Metadata": { "owner": "intruder" } })),
            &resource_id,
            resource_type,
        )
        .await;
    }

    // A string that is no ARN of this service is a FIELD failure on the member that
    // carried it, not a missing resource.
    for (case, arn) in [
        ("not an ARN at all", "not-an-arn".to_string()),
        ("an ARN of another service", vpc_arn(EDGE_REGION, 3)),
        (
            "an ARN of this service with no resource id",
            format!("arn:aws:networkmanager::{}:site", engine::emulator_account_id()),
        ),
    ] {
        let (status, body) = update_metadata(
            &emulator,
            &network,
            &arn,
            &json!({ "owner": "intruder" }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: wrong error code in {body}"
        );
        assert_eq!(
            body["Fields"][0]["Name"].as_str(),
            Some("ResourceArn"),
            "case {case}: the refusal must name the member at fault, got {body}"
        );
    }

    // Every refusal stored nothing: the resource still carries the map it was given.
    let reported = reported_metadata(&emulator, &network, "after the refusals", &subject.arn).await;
    assert_eq!(
        reported, kept,
        "case after the refusals: expected the metadata stored before them, got {reported}"
    );
}

#[tokio::test]
async fn a_metadata_map_is_accepted_up_to_the_per_resource_bound_and_refused_past_it() {
    let emulator = Emulator::new();
    let (network, subjects) = inventory(&emulator).await;
    let subject = &subjects[0];

    // At the bound, and one inside it, the map is stored and reported in full.
    for entries in [MAX_METADATA_ENTRIES - 1, MAX_METADATA_ENTRIES] {
        let case = format!("{entries} entries");
        let expected = metadata_of_size(entries);
        let answer = update_ok(&emulator, &network, &case, &subject.arn, &expected).await;
        assert_eq!(
            answer["Metadata"], expected,
            "case {case}: expected the stored map, got {answer}"
        );
        let reported = reported_metadata(&emulator, &network, &case, &subject.arn).await;
        assert_eq!(
            reported, expected,
            "case {case}: GetNetworkResources must report the stored map, got {reported}"
        );
    }

    // One past it is refused on the member, and the resource keeps the map it had.
    let kept = metadata_of_size(MAX_METADATA_ENTRIES);
    let case = format!("{} entries", MAX_METADATA_ENTRIES + 1);
    let (status, body) = update_metadata(
        &emulator,
        &network,
        &subject.arn,
        &metadata_of_size(MAX_METADATA_ENTRIES + 1),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case}: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case {case}: wrong error code in {body}"
    );
    assert_eq!(
        body["Fields"][0]["Name"].as_str(),
        Some("Metadata"),
        "case {case}: the refusal must name the member at fault, got {body}"
    );
    let reported = reported_metadata(&emulator, &network, &case, &subject.arn).await;
    assert_eq!(
        reported, kept,
        "case {case}: the refusal must store nothing, got {reported}"
    );
}
