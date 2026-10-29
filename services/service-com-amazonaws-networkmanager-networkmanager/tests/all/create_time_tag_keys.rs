// SPDX-License-Identifier: CC0-1.0

//! What a create stores for a `Tags` member that names one key more than once, driven over
//! HTTP through the real router (the harness lives in `crate::global_network_lifecycle`).
//! Nothing reaches into `crate::state`: every tag list is read back through an operation —
//! `ListTagsForResource` and, where the kind has one, the resource's own reader.
//!
//! The rule under test: a resource's tags are a key-to-value map, so the list a resource is
//! stored with holds each requested key ONCE, carrying the last value the request gave for
//! it — the same merge `TagResource` performs, and the one the per-resource tag quota (which
//! counts distinct keys) grades a create by. Two consequences are pinned here: a later
//! `TagResource` of that key leaves the resource reporting that key once, and the stored
//! list never holds more entries than the quota the request was graded against.
//!
//! The tables span that class: the shapes a repeat can take (adjacent, separated, three
//! times, two keys at once, an empty value, the same value twice, a non-ASCII key, keys and
//! values at the documented maximum lengths), every create of this service that stores a
//! `Tags` member, the per-entry rules that still judge an entry a repeat later replaces,
//! and both sides of the per-resource tag quota.

use serde_json::{json, Value};

use crate::attachment_lifecycle::policy_document;
use crate::connect_peer_lifecycle::create_connect_peer;
use crate::core_network_lifecycle::create_core_network;
use crate::delete_attachment::{create_attachment_of_kind, Kind};
use crate::device_lifecycle::create_device_ok;
use crate::global_network_lifecycle::{error_code, expected_arn, expected_scoped_arn, Emulator};
use crate::link_lifecycle::create_link_ok;
use crate::network_resources::{create_ok, minted};
use crate::resource_tagging::{
    create_taggable, list_tags, tag, tags_through_resource_reader, tags_uri, untag_uri, DEVICE,
    GLOBAL_NETWORK, LINK, MAX_KEY_CHARS, MAX_TAGS, MAX_VALUE_CHARS, SITE,
};
use crate::transit_gateway_peering::{own_account, transit_gateway_arn};

/// The Region the fixture's core network configures an edge in, which is therefore the
/// Region of every resource it attaches and peers with.
const EDGE_REGION: &str = "us-west-2";

/// A transit gateway route table ARN in `region`, in the documented form of
/// `TransitGatewayRouteTableArn`: "arn:aws:ec2:us-west-2:123456789012:transit-gateway-route-table/tgw-rtb-9876543210123456"
/// (`smithy_query query="doc" shape="CreateTransitGatewayRouteTableAttachmentRequest"
/// member="TransitGatewayRouteTableArn"`).
fn route_table_arn(region: &str, index: usize) -> String {
    format!(
        "arn:aws:ec2:{region}:{}:transit-gateway-route-table/tgw-rtb-{index:016x}",
        own_account()
    )
}

/// The key of the first entry of a tag list, which is the position a repeated key keeps.
fn first_key(list: &Value) -> String {
    list[0]["Key"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a first tag with a Key in {list}"))
        .to_string()
}

/// `list` with `key` taking `value`: the key keeps its position when the list holds it, and
/// is appended when it does not — the merge a `TagResource` performs.
fn with_value(list: &Value, key: &str, value: &str) -> Value {
    let mut entries: Vec<Value> = list
        .as_array()
        .unwrap_or_else(|| panic!("expected a tag list, got {list}"))
        .clone();
    match entries.iter().position(|entry| entry["Key"] == json!(key)) {
        Some(index) => entries[index] = tag(key, value),
        None => entries.push(tag(key, value)),
    }
    json!(entries)
}

/// `list` without the entry `key` names.
fn without_key(list: &Value, key: &str) -> Value {
    let entries: Vec<Value> = list
        .as_array()
        .unwrap_or_else(|| panic!("expected a tag list, got {list}"))
        .iter()
        .filter(|entry| entry["Key"] != json!(key))
        .cloned()
        .collect();
    json!(entries)
}

/// One row of the repeated-key table: the `Tags` a create carries and the list the resource
/// must then be stored with.
struct RepeatCase {
    name: &'static str,
    sent: Value,
    stored: Value,
}

/// The rows. They differ along everything the rule generalises over: whether a key is
/// repeated at all, how the repeats sit in the list (adjacent, separated by another key,
/// interleaved with a second repeated key), how many times a key is named, what the losing
/// and winning values are (including an empty value and the same value twice), the character
/// set of the key, and the documented maximum key and value lengths.
fn repeat_cases() -> Vec<RepeatCase> {
    vec![
        RepeatCase {
            name: "distinct keys only",
            sent: json!([tag("env", "test"), tag("tier", "primary")]),
            stored: json!([tag("env", "test"), tag("tier", "primary")]),
        },
        RepeatCase {
            name: "one key named twice",
            sent: json!([tag("env", "first"), tag("env", "last")]),
            stored: json!([tag("env", "last")]),
        },
        RepeatCase {
            name: "one key named twice with another key between",
            sent: json!([tag("env", "first"), tag("tier", "primary"), tag("env", "last")]),
            stored: json!([tag("env", "last"), tag("tier", "primary")]),
        },
        RepeatCase {
            name: "one key named three times",
            sent: json!([tag("env", "a"), tag("env", "b"), tag("env", "c")]),
            stored: json!([tag("env", "c")]),
        },
        RepeatCase {
            name: "two keys each named twice, interleaved",
            sent: json!([
                tag("env", "first"),
                tag("tier", "primary"),
                tag("env", "last"),
                tag("tier", "secondary"),
            ]),
            stored: json!([tag("env", "last"), tag("tier", "secondary")]),
        },
        RepeatCase {
            name: "the repeat carries an empty value",
            sent: json!([tag("env", "first"), tag("env", "")]),
            stored: json!([tag("env", "")]),
        },
        RepeatCase {
            name: "the repeat carries the same value",
            sent: json!([tag("env", "test"), tag("env", "test")]),
            stored: json!([tag("env", "test")]),
        },
        RepeatCase {
            name: "a non-ASCII key named twice",
            sent: json!([tag("ключ", "один"), tag("ключ", "два")]),
            stored: json!([tag("ключ", "два")]),
        },
        RepeatCase {
            name: "a key at the documented maximum length, named twice",
            sent: json!([
                tag(&"k".repeat(MAX_KEY_CHARS), "first"),
                tag(&"k".repeat(MAX_KEY_CHARS), "last"),
            ]),
            stored: json!([tag(&"k".repeat(MAX_KEY_CHARS), "last")]),
        },
        RepeatCase {
            name: "the winning value is at the documented maximum length",
            sent: json!([
                tag("env", "first"),
                tag("env", &"v".repeat(MAX_VALUE_CHARS)),
            ]),
            stored: json!([tag("env", &"v".repeat(MAX_VALUE_CHARS))]),
        },
    ]
}

#[tokio::test]
async fn a_key_a_create_names_more_than_once_is_stored_once_with_the_last_value() {
    for case in repeat_cases() {
        let name = case.name;
        let emulator = Emulator::new();
        let resource = create_taggable(&emulator, GLOBAL_NETWORK, &case.sent).await;

        // Both readers of the stored list answer the merged list, so the repeat was
        // collapsed before anything was stored rather than on the way out.
        let (status, listed) = list_tags(&emulator, &resource.arn).await;
        assert_eq!(
            status, 200,
            "case {name:?}: ListTagsForResource expected 200, got {status} with {listed}"
        );
        assert_eq!(
            listed, case.stored,
            "case {name:?}: expected {} stored, got {listed}",
            case.stored
        );
        let through_reader = tags_through_resource_reader(&emulator, &resource).await;
        assert_eq!(
            through_reader, case.stored,
            "case {name:?}: the resource's own reader must answer the same stored tags, got \
             {through_reader}"
        );

        // A later write of the repeated key reaches the one entry that key has, so the
        // resource reports it once with the new value — a second copy would have survived.
        let key = first_key(&case.stored);
        let (status, tagged) = emulator
            .post(&tags_uri(&resource.arn), json!({ "Tags": [tag(&key, "retagged")] }))
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: TagResource({key:?}) expected 200, got {status} with {tagged}"
        );
        let expected = with_value(&case.stored, &key, "retagged");
        let (_, listed) = list_tags(&emulator, &resource.arn).await;
        assert_eq!(
            listed, expected,
            "case {name:?}: expected {expected} after re-tagging {key:?}, got {listed}"
        );

        // And a removal of that key leaves none of it behind.
        let (status, untagged) = emulator
            .delete(&untag_uri(&resource.arn, &[key.as_str()]))
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: UntagResource({key:?}) expected 200, got {status} with {untagged}"
        );
        let expected = without_key(&case.stored, &key);
        let (_, listed) = list_tags(&emulator, &resource.arn).await;
        assert_eq!(
            listed, expected,
            "case {name:?}: expected {expected} after removing {key:?}, got {listed}"
        );
    }
}

/// Create one resource through EVERY operation of this service that stores a `Tags` member,
/// each carrying `tags`, and answer `(the operation's name, the ARN its stored tag list is
/// read back through)` for each.
async fn every_tagged_create(emulator: &Emulator, tags: &Value) -> Vec<(&'static str, String)> {
    let mut rows: Vec<(&'static str, String)> = Vec::new();

    // The kinds the shared taggable harness drives: a global network and the site, link and
    // device named inside one.
    for (operation, kind) in [
        ("CreateGlobalNetwork", GLOBAL_NETWORK),
        ("CreateSite", SITE),
        ("CreateLink", LINK),
        ("CreateDevice", DEVICE),
    ] {
        let resource = create_taggable(emulator, kind, tags).await;
        rows.push((operation, resource.arn));
    }

    // A connection ties two devices of one global network together.
    let network = emulator.create_global_network("create-time tag keys").await;
    let site = emulator.create_site(&network, "create-time tag keys").await;
    let link = create_link_ok(
        emulator,
        &network,
        &site,
        &json!({ "Bandwidth": { "UploadSpeed": 10, "DownloadSpeed": 20 } }),
    )
    .await;
    assert!(
        !link.is_empty(),
        "the fixture's link must carry an id, got {link:?}"
    );
    let device = create_device_ok(emulator, &network, json!({ "SiteId": site })).await;
    let peer_device = create_device_ok(emulator, &network, json!({})).await;
    let mut connection_body = json!({ "DeviceId": device, "ConnectedDeviceId": peer_device });
    if let Some(object) = connection_body.as_object_mut() {
        object.insert("Tags".to_string(), tags.clone());
    }
    let connection = minted(
        &create_ok(
            emulator,
            &format!("/global-networks/{network}/connections"),
            connection_body,
        )
        .await,
        "Connection",
        "ConnectionId",
    );
    rows.push((
        "CreateConnection",
        expected_scoped_arn("connection", &network, &connection),
    ));

    // The core network every attachment and peering below hangs off, created with a policy
    // that configures one edge.
    let (status, created) = create_core_network(
        emulator,
        json!({
            "GlobalNetworkId": network,
            "Description": "create-time tag keys",
            "PolicyDocument": policy_document(&[EDGE_REGION]),
            "Tags": tags,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateCoreNetwork expected 200, got {status} with {created}"
    );
    let core_network = minted(&created, "CoreNetwork", "CoreNetworkId");
    rows.push((
        "CreateCoreNetwork",
        expected_arn("core-network", &core_network),
    ));

    // One attachment of every kind this service mints, each on that core network.
    let mut connect_attachment = String::new();
    for (index, (operation, kind)) in [
        ("CreateVpcAttachment", Kind::Vpc),
        ("CreateConnectAttachment", Kind::Connect),
        ("CreateSiteToSiteVpnAttachment", Kind::SiteToSiteVpn),
        ("CreateDirectConnectGatewayAttachment", Kind::DirectConnectGateway),
    ]
    .into_iter()
    .enumerate()
    {
        let (id, _teardown) = create_attachment_of_kind(
            emulator,
            operation,
            kind,
            &core_network,
            EDGE_REGION,
            index,
            tags,
        )
        .await;
        if kind == Kind::Connect {
            connect_attachment = id.clone();
        }
        rows.push((operation, expected_arn("attachment", &id)));
    }

    // A Connect peer runs inside the Connect attachment's tunnel.
    let (status, peer) = create_connect_peer(
        emulator,
        json!({
            "ConnectAttachmentId": connect_attachment,
            "PeerAddress": "203.0.113.1",
            "CoreNetworkAddress": "203.0.113.2",
            "InsideCidrBlocks": ["169.254.10.0/29"],
            "BgpOptions": { "PeerAsn": 65000 },
            "Tags": tags,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateConnectPeer expected 200, got {status} with {peer}"
    );
    let connect_peer = minted(&peer, "ConnectPeer", "ConnectPeerId");
    rows.push((
        "CreateConnectPeer",
        expected_arn("connect-peer", &connect_peer),
    ));

    // A peering of a transit gateway at that edge, and the route table attachment carried
    // over it.
    let (status, peered) = emulator
        .post(
            "/transit-gateway-peerings",
            json!({
                "CoreNetworkId": core_network,
                "TransitGatewayArn": transit_gateway_arn(
                    "aws",
                    EDGE_REGION,
                    &own_account(),
                    7,
                    17,
                ),
                "Tags": tags,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateTransitGatewayPeering expected 200, got {status} with {peered}"
    );
    let peering = minted(&peered["TransitGatewayPeering"], "Peering", "PeeringId");
    rows.push((
        "CreateTransitGatewayPeering",
        expected_arn("peering", &peering),
    ));

    let (status, attached) = emulator
        .post(
            "/transit-gateway-route-table-attachments",
            json!({
                "PeeringId": peering,
                "TransitGatewayRouteTableArn": route_table_arn(EDGE_REGION, 7),
                "Tags": tags,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateTransitGatewayRouteTableAttachment expected 200, got {status} with {attached}"
    );
    let route_table_attachment = minted(
        &attached["TransitGatewayRouteTableAttachment"],
        "Attachment",
        "AttachmentId",
    );
    rows.push((
        "CreateTransitGatewayRouteTableAttachment",
        expected_arn("attachment", &route_table_attachment),
    ));

    rows
}

#[tokio::test]
async fn every_create_that_stores_tags_collapses_a_repeated_key_to_one_entry() {
    let emulator = Emulator::new();
    // One repeated key and one key named once, so a create that stored the list verbatim
    // would report three entries where the map holds two.
    let sent = json!([tag("env", "first"), tag("team", "net"), tag("env", "last")]);
    let expected = json!([tag("env", "last"), tag("team", "net")]);

    let rows = every_tagged_create(&emulator, &sent).await;
    assert_eq!(
        rows.len(),
        13,
        "every create that stores tags must be in the table, got {rows:?}"
    );

    for (operation, arn) in &rows {
        let (status, listed) = list_tags(&emulator, arn).await;
        assert_eq!(
            status, 200,
            "case {operation}: ListTagsForResource({arn}) expected 200, got {status} with {listed}"
        );
        assert_eq!(
            listed, expected,
            "case {operation}: expected {expected} stored for {arn}, got {listed}"
        );
    }
}

/// One row of the per-entry validation table: the `Tags` a create carries and the status it
/// must answer.
struct ValidationCase {
    name: &'static str,
    sent: Value,
    status: u16,
}

/// The rows: a repeat does not excuse an entry from the per-tag rules, so an entry a later
/// repeat would replace is judged too — at the documented maximum key and value lengths
/// (`smithy_query query="doc" shape="Tag" member="Key"` / `"Value"`) and one past each.
fn validation_cases() -> Vec<ValidationCase> {
    vec![
        ValidationCase {
            name: "both values within the documented maximum",
            sent: json!([tag("env", &"v".repeat(MAX_VALUE_CHARS)), tag("env", "ok")]),
            status: 200,
        },
        ValidationCase {
            name: "the replaced value is one past the documented maximum",
            sent: json!([tag("env", &"v".repeat(MAX_VALUE_CHARS + 1)), tag("env", "ok")]),
            status: 400,
        },
        ValidationCase {
            name: "the winning value is one past the documented maximum",
            sent: json!([tag("env", "ok"), tag("env", &"v".repeat(MAX_VALUE_CHARS + 1))]),
            status: 400,
        },
        ValidationCase {
            name: "a repeated key one past the documented maximum",
            sent: json!([
                tag(&"k".repeat(MAX_KEY_CHARS + 1), "first"),
                tag(&"k".repeat(MAX_KEY_CHARS + 1), "last"),
            ]),
            status: 400,
        },
        ValidationCase {
            name: "a repeated key AWS reserves",
            sent: json!([tag("aws:env", "first"), tag("aws:env", "last")]),
            status: 400,
        },
        ValidationCase {
            name: "an empty key named twice",
            sent: json!([tag("", "first"), tag("", "last")]),
            status: 400,
        },
    ]
}

#[tokio::test]
async fn a_repeated_key_does_not_excuse_an_entry_from_the_per_tag_rules() {
    for case in validation_cases() {
        let name = case.name;
        let emulator = Emulator::new();
        let (status, body) = emulator
            .post(
                "/global-networks",
                json!({ "Description": "create-time tag keys", "Tags": case.sent }),
            )
            .await;
        assert_eq!(
            status, case.status,
            "case {name:?}: expected {}, got {status} with {body}",
            case.status
        );

        let (_, networks) = emulator.get("/global-networks").await;
        let stored = networks["GlobalNetworks"].as_array().map(Vec::len);
        if case.status == 200 {
            assert_eq!(
                stored,
                Some(1),
                "case {name:?}: the accepted create must store the global network, got {networks}"
            );
        } else {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {name:?}: wrong error code in {body}"
            );
            assert_eq!(
                stored,
                Some(0),
                "case {name:?}: a refused create must store nothing, got {networks}"
            );
        }
    }
}

#[tokio::test]
async fn the_tag_quota_a_create_is_graded_by_counts_the_keys_it_is_stored_with() {
    // Each key named TWICE: the quota counts distinct keys, so a list of twice the ceiling
    // in entries is within it — and the resource is stored with exactly the keys it was
    // graded on, never with the entries.
    let doubled: Vec<Value> = (0..MAX_TAGS)
        .flat_map(|index| {
            let key = format!("key-{index:02}");
            [tag(&key, "first"), tag(&key, "last")]
        })
        .collect();
    let emulator = Emulator::new();
    let resource = create_taggable(&emulator, GLOBAL_NETWORK, &json!(doubled)).await;
    let (status, listed) = list_tags(&emulator, &resource.arn).await;
    assert_eq!(
        status, 200,
        "a create at the quota in distinct keys: expected 200, got {status} with {listed}"
    );
    assert_eq!(
        listed.as_array().map(Vec::len),
        Some(MAX_TAGS),
        "the stored list must hold one entry per distinct key, got {listed}"
    );
    assert_eq!(
        listed[0],
        tag("key-00", "last"),
        "each stored key must carry the last value the request gave it, got {listed}"
    );

    // One distinct key past the ceiling is refused with the modelled quota error, however
    // the entries are spread.
    let past: Vec<Value> = (0..=MAX_TAGS)
        .flat_map(|index| {
            let key = format!("key-{index:02}");
            [tag(&key, "first"), tag(&key, "last")]
        })
        .collect();
    let (status, refused) = emulator
        .post(
            "/global-networks",
            json!({ "Description": "past the tag quota", "Tags": past }),
        )
        .await;
    assert_eq!(
        status, 402,
        "a create past the quota in distinct keys: expected 402, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ServiceQuotaExceededException",
        "a create past the quota: wrong error code in {refused}"
    );

    // And a list one ENTRY past the ceiling whose extra entry repeats a key it already
    // carries is within the quota, because the resource it would leave carries the ceiling.
    let mut one_repeat: Vec<Value> = (0..MAX_TAGS)
        .map(|index| tag(&format!("key-{index:02}"), "value"))
        .collect();
    one_repeat.push(tag("key-00", "last"));
    let resource = create_taggable(&emulator, GLOBAL_NETWORK, &json!(one_repeat)).await;
    let (status, listed) = list_tags(&emulator, &resource.arn).await;
    assert_eq!(
        status, 200,
        "a create one entry past the quota but at it in keys: expected 200, got {status} with \
         {listed}"
    );
    assert_eq!(
        listed.as_array().map(Vec::len),
        Some(MAX_TAGS),
        "the stored list must hold one entry per distinct key, got {listed}"
    );
    assert_eq!(
        listed[0],
        tag("key-00", "last"),
        "the repeated key must carry the last value the request gave it, got {listed}"
    );
}
