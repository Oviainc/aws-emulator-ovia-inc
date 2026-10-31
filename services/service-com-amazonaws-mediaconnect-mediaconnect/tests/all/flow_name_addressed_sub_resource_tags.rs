// SPDX-License-Identifier: CC0-1.0

//! The tag members of the flow sub-resources addressed BY NAME, end to end over HTTP.
//!
//! A media stream and a VPC interface are the two sub-resources a flow holds that have no
//! ARN of their own (each is addressed by its name within its flow), and each create
//! request that adds one carries a tag member: `AddMediaStreamRequest$MediaStreamTags`
//! ("the key-value pairs that can be used to tag and organize the media stream") and
//! `VpcInterfaceRequest$VpcInterfaceTags`. The rule under test: a tag member of a create
//! request writes the same kind of tag map a tag write writes, so it is bounded by exactly
//! the two rules a tag write is bounded by — every pair within the documented key and value
//! lengths with a non-empty key, and no more pairs than a resource keeps — and a request
//! outside them is refused with the modelled `BadRequestException` (400) and stores
//! nothing.
//!
//! The rows come from the one table that states those bounds
//! (`crate::flow_sub_resource_tags::tag_member_cases`, whose values are read from
//! `mc::MAX_TAG_KEY_LENGTH`, `mc::MAX_TAG_VALUE_LENGTH` and `mc::MAX_TAGS_PER_RESOURCE`,
//! each at the bound and one past it), so these members are judged by the same rows the
//! ARN-bearing sub-resources' tag members are. Each row is driven through BOTH operations
//! that add such a sub-resource to a flow — the create path (`CreateFlow`) and the add path
//! (`AddFlowMediaStreams`, `AddFlowVpcInterfaces`) — because the bound belongs to the tag
//! member, not to one operation.
//!
//! The cases drive the real request path (routing, restJson1 deserialization, constraint
//! validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`], so what they assert is what a MediaConnect client
//! sees on the wire.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::Emulator;
use crate::flow_media_streams::add_media_streams;
use crate::flow_sub_resource_tags::{expected_tags, tag_member_cases, TagMemberCase};
use crate::flow_vpc_interfaces::{add_vpc_interfaces, vpc_interface_request, SUBNET_ID};
use crate::remove_flow_vpc_interface::described_vpc_interfaces;

/// The name every sub-resource below takes: one per flow, so what a flow holds afterwards
/// is only what the call under test stored.
const ENTRY: &str = "under-test";

/// A security group the VPC interface requests below name — `SecurityGroupIds` is required.
const SECURITY_GROUP: &str = "sg-0123456789abcdef0";

/// An `AddMediaStreamRequest` named [`ENTRY`], carrying `tags` as its `MediaStreamTags`
/// when there are any (the member is optional, so a null row carries none at all).
fn media_stream_request(tags: &Value) -> Value {
    let mut request = json!({
        "mediaStreamId": 1,
        "mediaStreamName": ENTRY,
        "mediaStreamType": "video"
    });
    if !tags.is_null() {
        request["mediaStreamTags"] = tags.clone();
    }
    request
}

/// A `VpcInterfaceRequest` named [`ENTRY`], carrying `tags` as its `VpcInterfaceTags` when
/// there are any — built through this suite's one builder of the shape.
fn vpc_interface(tags: &Value) -> Value {
    vpc_interface_request(
        ENTRY,
        None,
        &[SECURITY_GROUP],
        SUBNET_ID,
        (!tags.is_null()).then(|| tags.clone()),
    )
}

/// `POST /v1/flows` for a flow named `name` whose `member` list is the single `entry`.
async fn create_flow_holding(
    emulator: &Emulator,
    name: &str,
    member: &str,
    entry: Value,
) -> (u16, Option<String>, Value) {
    let mut body = json!({
        "name": name,
        "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }
    });
    body[member] = json!([entry]);
    emulator.create_flow(body).await
}

/// A flow with nothing but its source, for the add path, asserted created.
async fn plain_flow(emulator: &Emulator, case: &str, name: &str) -> String {
    let (status, code, created) = emulator
        .create_flow(json!({
            "name": name,
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }
        }))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: the fixture flow must be created, got {status} {code:?}: {created}"
    );
    created["flow"]["flowArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The media streams a flow holds, each with the tag map the request that added it left on
/// it.
///
/// `MediaStream` models no tag member and a media stream has no ARN of its own, so no
/// response reports these pairs: the store is the only place that states what was written.
fn stored_media_streams(
    emulator: &Emulator,
    flow_arn: &str,
) -> Vec<(String, BTreeMap<String, String>)> {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = state.lock();
    store
        .flows
        .get(flow_arn)
        .map(|flow| {
            flow.media_streams
                .iter()
                .map(|stream| (stream.stream.media_stream_name.clone(), stream.tags.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// The names of the flows this emulator holds.
async fn flow_names(emulator: &Emulator, case: &str) -> Vec<String> {
    let (status, code, listed) = emulator.list_flows("").await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: listing flows expected 200, got {status} {code:?}: {listed}"
    );
    listed["flows"]
        .as_array()
        .map(|flows| {
            flows
                .iter()
                .filter_map(|flow| flow["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// `MediaStreamTags` is bounded exactly as a tag write is, on the create path and the add
/// path alike: a map inside the bounds is stored verbatim on the media stream's record, and
/// one outside them is 400 `BadRequestException` that stores nothing — no media stream on
/// the add path, and no flow at all on the create path.
#[tokio::test]
async fn media_stream_tag_members_are_bounded_like_a_tag_write() {
    let emulator = Emulator::new();

    for (index, row) in tag_member_cases().into_iter().enumerate() {
        let TagMemberCase {
            case,
            tags,
            status: want_status,
            code: want_code,
        } = row;

        // The create path: CreateFlow's `MediaStreams` adds media streams to the flow it
        // creates.
        let name = format!("ms-tags-create-{index}");
        let (status, code, created) =
            create_flow_holding(&emulator, &name, "mediaStreams", media_stream_request(&tags)).await;
        assert_eq!(
            (status, code.as_deref()),
            (want_status, want_code),
            "case {case:?} / CreateFlow MediaStreamTags: expected {want_status} {want_code:?}, \
             got {status} {code:?}: {created}"
        );
        if want_status == 201 {
            let flow_arn = created["flow"]["flowArn"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let stored = stored_media_streams(&emulator, &flow_arn);
            assert_eq!(
                stored,
                vec![(ENTRY.to_string(), expected_tags(&tags))],
                "case {case:?} / CreateFlow MediaStreamTags: the accepted pairs are what the \
                 media stream carries, got {stored:?}"
            );
        } else {
            let names = flow_names(&emulator, case).await;
            assert!(
                !names.contains(&name),
                "case {case:?} / CreateFlow MediaStreamTags: a refused create stores no flow, \
                 got {names:?}"
            );
        }

        // The add path: the same map on the same member of the same shape, this time
        // against a flow that already exists.
        let name = format!("ms-tags-add-{index}");
        let flow_arn = plain_flow(&emulator, case, &name).await;
        let (status, code, added) =
            add_media_streams(&emulator, &flow_arn, json!([media_stream_request(&tags)])).await;
        assert_eq!(
            (status, code.as_deref()),
            (want_status, want_code),
            "case {case:?} / AddFlowMediaStreams MediaStreamTags: expected {want_status} \
             {want_code:?}, got {status} {code:?}: {added}"
        );
        let stored = stored_media_streams(&emulator, &flow_arn);
        let expected = if want_status == 201 {
            vec![(ENTRY.to_string(), expected_tags(&tags))]
        } else {
            Vec::new()
        };
        assert_eq!(
            stored, expected,
            "case {case:?} / AddFlowMediaStreams MediaStreamTags: expected the flow to hold \
             {expected:?}, got {stored:?}"
        );
    }
}

/// `VpcInterfaceTags` is the same kind of member on the other name-addressed sub-resource,
/// so it is bounded by the same rules: a map inside the bounds leaves the VPC interface on
/// the flow, and one outside them is 400 `BadRequestException` that adds no VPC interface —
/// and no flow at all on the create path.
#[tokio::test]
async fn vpc_interface_tag_members_are_bounded_like_a_tag_write() {
    let emulator = Emulator::new();

    for (index, row) in tag_member_cases().into_iter().enumerate() {
        let TagMemberCase {
            case,
            tags,
            status: want_status,
            code: want_code,
        } = row;

        // The create path: CreateFlow's `VpcInterfaces`.
        let name = format!("vpc-tags-create-{index}");
        let (status, code, created) =
            create_flow_holding(&emulator, &name, "vpcInterfaces", vpc_interface(&tags)).await;
        assert_eq!(
            (status, code.as_deref()),
            (want_status, want_code),
            "case {case:?} / CreateFlow VpcInterfaceTags: expected {want_status} {want_code:?}, \
             got {status} {code:?}: {created}"
        );
        if want_status == 201 {
            let flow_arn = created["flow"]["flowArn"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let held = described_vpc_interfaces(&emulator, case, &flow_arn).await;
            let names: Vec<&str> = held
                .iter()
                .filter_map(|interface| interface["name"].as_str())
                .collect();
            assert_eq!(
                names,
                vec![ENTRY],
                "case {case:?} / CreateFlow VpcInterfaceTags: an accepted map leaves the VPC \
                 interface on the flow, got {held:?}"
            );
        } else {
            let names = flow_names(&emulator, case).await;
            assert!(
                !names.contains(&name),
                "case {case:?} / CreateFlow VpcInterfaceTags: a refused create stores no flow, \
                 got {names:?}"
            );
        }

        // The add path: AddFlowVpcInterfaces.
        let name = format!("vpc-tags-add-{index}");
        let flow_arn = plain_flow(&emulator, case, &name).await;
        let (status, code, added) =
            add_vpc_interfaces(&emulator, &flow_arn, json!([vpc_interface(&tags)])).await;
        assert_eq!(
            (status, code.as_deref()),
            (want_status, want_code),
            "case {case:?} / AddFlowVpcInterfaces VpcInterfaceTags: expected {want_status} \
             {want_code:?}, got {status} {code:?}: {added}"
        );
        let held = described_vpc_interfaces(&emulator, case, &flow_arn).await;
        let names: Vec<&str> = held
            .iter()
            .filter_map(|interface| interface["name"].as_str())
            .collect();
        let expected: Vec<&str> = if want_status == 201 {
            vec![ENTRY]
        } else {
            Vec::new()
        };
        assert_eq!(
            names, expected,
            "case {case:?} / AddFlowVpcInterfaces VpcInterfaceTags: expected the flow to hold \
             {expected:?}, got {held:?}"
        );
    }
}
