// SPDX-License-Identifier: CC0-1.0

//! MediaPackage v2 is a REGIONAL service, asserted end-to-end over a SET of Region pairs.
//!
//! The rule under test: every resource of this service belongs to the AWS Region the
//! request that created it was served in, and a request served in another Region neither
//! resolves, lists, mutates, deletes nor tags it — so the same name in two Regions is two
//! independent resources, each with its own ARN, its own children and its own share of
//! the per-Region create ceiling. The model states it for the primary identifier:
//! `CreateChannelGroupRequest$ChannelGroupName` is "the primary identifier for the channel
//! group, and must be unique for your account in the AWS Region"
//! (`smithy_query query="operation" operation="CreateChannelGroup"`).
//!
//! The Region a control-plane request is served in travels in its SigV4 credential scope;
//! the Region a DATA-plane request is served in is named by the published hostname it was
//! addressed to, which the last test covers. Because the Region is therefore the CALLER's
//! choice, the last-but-one test pins that the store stays bounded as a whole however many
//! Regions a caller names. Both sides are driven through the client
//! `channel_lifecycle` owns, extended there with one region-aware send, and the policy
//! writer and URL splitter are `rejection_rule_and_negative_paths`'s.

use serde_json::{json, Value};

use crate::channel_lifecycle::{error_type, Emulator};
use crate::rejection_rule_and_negative_paths::{encoded as pct, grant_policy, split_url};

/// One row of the Region table: the Region a fixture is created in, and another Region
/// the same requests are replayed against.
struct RegionPair {
    /// The Region the resources are created in.
    home: &'static str,
    /// A different Region, where none of them exists.
    other: &'static str,
}

/// The rows. Each pair is two distinct AWS Regions; they differ in partition-internal
/// prefix, in length and in which one is the emulator's own default (`us-east-1`, the
/// Region an unsigned request falls back to), so no case can pass by accident of one
/// Region being special.
const REGION_PAIRS: &[RegionPair] = &[
    RegionPair {
        home: "us-west-2",
        other: "eu-west-1",
    },
    RegionPair {
        home: "us-east-1",
        other: "ap-southeast-2",
    },
    RegionPair {
        home: "eu-central-1",
        other: "us-east-1",
    },
];

/// The names the fixtures use. The rule does not generalise over them — renaming a
/// resource is not a different input — so they are fixed and the Regions vary.
const GROUP: &str = "tg";
const CHANNEL: &str = "tc";
const ENDPOINT: &str = "te";

/// Send one control-plane request to `region`'s endpoint, signed for that Region.
async fn call(
    emulator: &Emulator,
    region: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    emulator
        .send_in_region(
            method,
            path,
            body,
            &format!("mediapackagev2.{region}.amazonaws.com"),
            region,
            &[],
        )
        .await
}

/// Create the group -> channel -> origin-endpoint chain in `region`, returning the
/// `CreateChannelGroup` and `CreateOriginEndpoint` bodies.
async fn create_chain(emulator: &Emulator, region: &str) -> (Value, Value) {
    let (status, group) = call(
        emulator,
        region,
        "POST",
        "/channelGroup",
        Some(json!({"ChannelGroupName": GROUP, "Description": format!("in {region}")})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {region:?}: CreateChannelGroup expected 200, got {status} {group}"
    );
    let (status, channel) = call(
        emulator,
        region,
        "POST",
        &format!("/channelGroup/{GROUP}/channel"),
        Some(json!({"ChannelName": CHANNEL})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {region:?}: CreateChannel expected 200, got {status} {channel}"
    );
    let (status, endpoint) = call(
        emulator,
        region,
        "POST",
        &format!("/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint"),
        Some(json!({
            "OriginEndpointName": ENDPOINT,
            "ContainerType": "TS",
            "HlsManifests": [{"ManifestName": "index"}],
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {region:?}: CreateOriginEndpoint expected 200, got {status} {endpoint}"
    );
    (group, endpoint)
}

/// Every reporting operation of the hierarchy, as the path it is bound to. Each resolves
/// in the Region the chain was created in and nowhere else.
fn reader_paths() -> Vec<(&'static str, String)> {
    vec![
        ("GetChannelGroup", format!("/channelGroup/{GROUP}")),
        ("ListChannels", format!("/channelGroup/{GROUP}/channel")),
        ("GetChannel", format!("/channelGroup/{GROUP}/channel/{CHANNEL}/")),
        (
            "ListOriginEndpoints",
            format!("/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint"),
        ),
        (
            "GetOriginEndpoint",
            format!("/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint/{ENDPOINT}"),
        ),
        (
            "ListHarvestJobs",
            format!(
                "/channelGroup/{GROUP}/harvestJob?channelName={CHANNEL}&\
                 originEndpointName={ENDPOINT}"
            ),
        ),
    ]
}

/// A resource created in one Region resolves there and is absent from every other: the
/// read served elsewhere is `ResourceNotFoundException` (404), at every level of the
/// hierarchy.
#[tokio::test]
async fn a_resource_resolves_only_in_the_region_it_was_created_in() {
    for pair in REGION_PAIRS {
        let RegionPair { home, other } = pair;
        let emulator = Emulator::new();
        let (group, _endpoint) = create_chain(&emulator, home).await;

        // The ARN the create minted names the Region it was served in.
        let arn = group["Arn"].as_str().unwrap_or_default();
        assert!(
            arn.contains(&format!(":{home}:")),
            "case {home:?}: the group ARN must name its own Region, got {arn}"
        );

        for (operation, path) in reader_paths() {
            let case = format!("{operation} created in {home}, read in {other}");
            let (status, body) = call(&emulator, home, "GET", &path, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the read in {home} expected 200, got {status} {body}"
            );

            let (status, body) = call(&emulator, other, "GET", &path, None).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404, got {status} {body}"
            );
            assert_eq!(
                error_type(&body),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {body}"
            );
        }
    }
}

/// A listing reports exactly the Region it is served in: the groups created elsewhere are
/// not in it, whatever their number.
#[tokio::test]
async fn a_listing_reports_only_the_region_it_is_served_in() {
    // The counts differ per Region so an empty answer cannot be mistaken for a correct
    // one, and the names overlap so the slice cannot be keyed to a name.
    let cases: &[(&str, &[&str])] = &[
        ("us-west-2", &["shared", "west-only", "zzz"]),
        ("eu-west-1", &["shared"]),
        ("ap-northeast-1", &[]),
    ];

    let emulator = Emulator::new();
    for (region, names) in cases {
        for name in *names {
            let (status, body) = call(
                &emulator,
                region,
                "POST",
                "/channelGroup",
                Some(json!({"ChannelGroupName": name})),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {region:?}/{name:?}: CreateChannelGroup expected 200, got {status} {body}"
            );
        }
    }

    for (region, names) in cases {
        let (status, body) = call(&emulator, region, "GET", "/channelGroup?maxResults=100", None)
            .await;
        assert_eq!(
            status, 200,
            "case {region:?}: ListChannelGroups expected 200, got {status} {body}"
        );
        let listed: Vec<String> = body["Items"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|item| {
                item["ChannelGroupName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        let mut expected: Vec<String> = names.iter().map(|name| (*name).to_string()).collect();
        expected.sort();
        assert_eq!(
            listed, expected,
            "case {region:?}: the listing must report this Region's groups in name order, got \
             {body}"
        );
    }
}

/// The same name in two Regions is two independent resources: both creates succeed, each
/// gets its own ARN and egress domain, and a write to one leaves the other alone.
#[tokio::test]
async fn the_same_name_in_two_regions_is_two_independent_resources() {
    for pair in REGION_PAIRS {
        let RegionPair { home, other } = pair;
        let emulator = Emulator::new();
        let (home_group, _) = create_chain(&emulator, home).await;
        // The name is taken in `home`; in `other` it is free, so this is a create and not
        // the `ConflictException` a repeat in the SAME Region gets.
        let (status, other_group) = call(
            &emulator,
            other,
            "POST",
            "/channelGroup",
            Some(json!({"ChannelGroupName": GROUP, "Description": format!("in {other}")})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {other:?}: a name used in {home} must still be free here, got {status} \
             {other_group}"
        );

        for member in ["Arn", "EgressDomain"] {
            assert_ne!(
                home_group[member], other_group[member],
                "case {home:?}/{other:?}: the two groups must not share {member}: {home_group} \
                 vs {other_group}"
            );
        }
        for (region, group) in [(home, &home_group), (other, &other_group)] {
            let arn = group["Arn"].as_str().unwrap_or_default();
            let domain = group["EgressDomain"].as_str().unwrap_or_default();
            assert!(
                arn.contains(&format!(":{region}:")),
                "case {region:?}: the ARN must name this Region, got {arn}"
            );
            assert!(
                domain.contains(&format!(".{region}.")),
                "case {region:?}: the egress domain must name this Region, got {domain}"
            );
        }

        // A repeat create in the SAME Region is still a conflict: the rule widens the
        // namespace per Region, it does not make a name reusable within one.
        let (status, body) = call(
            &emulator,
            other,
            "POST",
            "/channelGroup",
            Some(json!({"ChannelGroupName": GROUP})),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {other:?}: a repeat create in the same Region expected 409, got {status} {body}"
        );
        assert_eq!(
            error_type(&body),
            "ConflictException",
            "case {other:?}: expected ConflictException, got {body}"
        );

        // `other`'s group holds no channel: the children belong to the Region's own group.
        let (status, body) = call(
            &emulator,
            other,
            "GET",
            &format!("/channelGroup/{GROUP}/channel"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {other:?}: ListChannels expected 200, got {status} {body}"
        );
        assert_eq!(
            body["Items"],
            json!([]),
            "case {other:?}: the group of this Region has no channels, got {body}"
        );

        // Deleting the group in `other` leaves `home`'s group exactly as it was.
        let (status, body) = call(&emulator, other, "DELETE", &format!("/channelGroup/{GROUP}"), None)
            .await;
        assert_eq!(
            status, 200,
            "case {other:?}: DeleteChannelGroup expected 200, got {status} {body}"
        );
        let (status, read) = call(&emulator, home, "GET", &format!("/channelGroup/{GROUP}"), None)
            .await;
        assert_eq!(
            status, 200,
            "case {home:?}: the group must survive a delete served in {other}, got {status} {read}"
        );
        assert_eq!(
            read["ETag"], home_group["ETag"],
            "case {home:?}: a delete served in {other} must not touch this group, got {read}"
        );
    }
}

/// One row of the mutation table: the operation, how it is addressed, and the status it
/// answers when it is served in a Region that holds none of the named resources.
///
/// An `@idempotent` delete promises only that the resource is absent when it returns, and
/// it IS absent in that Region, so it succeeds there; every other mutation resolves a
/// resource first and so refuses with the modelled 404. Either way the resource in the
/// Region that does hold it must be untouched, which is what the test asserts.
struct MutationRow {
    label: &'static str,
    method: &'static str,
    path: fn() -> String,
    body: fn() -> Option<Value>,
    expected: u16,
}

/// The rows: one per implemented operation that writes somewhere in the hierarchy.
fn mutation_rows() -> Vec<MutationRow> {
    vec![
        MutationRow {
            label: "UpdateChannelGroup",
            method: "PUT",
            path: || format!("/channelGroup/{GROUP}"),
            body: || Some(json!({"Description": "written from the wrong Region"})),
            expected: 404,
        },
        MutationRow {
            label: "UpdateChannel",
            method: "PUT",
            path: || format!("/channelGroup/{GROUP}/channel/{CHANNEL}/"),
            body: || Some(json!({"Description": "written from the wrong Region"})),
            expected: 404,
        },
        MutationRow {
            label: "UpdateOriginEndpoint",
            method: "PUT",
            path: || format!("/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint/{ENDPOINT}"),
            body: || {
                Some(json!({
                    "ContainerType": "TS",
                    "HlsManifests": [{"ManifestName": "index"}],
                }))
            },
            expected: 404,
        },
        MutationRow {
            label: "PutChannelPolicy",
            method: "PUT",
            path: || format!("/channelGroup/{GROUP}/channel/{CHANNEL}/policy"),
            body: || Some(json!({"Policy": grant_policy("mediapackagev2:GetObject")})),
            expected: 404,
        },
        MutationRow {
            label: "PutOriginEndpointPolicy",
            method: "POST",
            path: || {
                format!(
                    "/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint/{ENDPOINT}/policy"
                )
            },
            body: || Some(json!({"Policy": grant_policy("mediapackagev2:GetObject")})),
            expected: 404,
        },
        MutationRow {
            label: "ResetChannelState",
            method: "POST",
            path: || format!("/channelGroup/{GROUP}/channel/{CHANNEL}/reset"),
            body: || None,
            expected: 404,
        },
        MutationRow {
            label: "ResetOriginEndpointState",
            method: "POST",
            path: || {
                format!("/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint/{ENDPOINT}/reset")
            },
            body: || None,
            expected: 404,
        },
        MutationRow {
            label: "DeleteOriginEndpoint",
            method: "DELETE",
            path: || format!("/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint/{ENDPOINT}"),
            body: || None,
            expected: 200,
        },
        MutationRow {
            label: "DeleteChannel",
            method: "DELETE",
            path: || format!("/channelGroup/{GROUP}/channel/{CHANNEL}/"),
            body: || None,
            expected: 200,
        },
        MutationRow {
            label: "DeleteChannelGroup",
            method: "DELETE",
            path: || format!("/channelGroup/{GROUP}"),
            body: || None,
            expected: 200,
        },
    ]
}

/// A mutation served in another Region changes nothing: it either refuses with the
/// modelled 404 or succeeds as the no-op an idempotent delete is there, and the resources
/// of the Region that holds them read back byte for byte as before.
#[tokio::test]
async fn a_mutation_served_in_another_region_changes_nothing() {
    for pair in REGION_PAIRS {
        let RegionPair { home, other } = pair;
        for row in mutation_rows() {
            let case = format!("{} served in {other} against {home}", row.label);
            let emulator = Emulator::new();
            create_chain(&emulator, home).await;

            let mut before: Vec<(String, Value)> = Vec::new();
            for (operation, path) in reader_paths() {
                let (status, body) = call(&emulator, home, "GET", &path, None).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: {operation} in {home} expected 200, got {status} {body}"
                );
                before.push((operation.to_string(), body));
            }

            let (status, body) =
                call(&emulator, other, row.method, &(row.path)(), (row.body)()).await;
            assert_eq!(
                status, row.expected,
                "case {case:?}: expected {}, got {status} {body}",
                row.expected
            );
            if row.expected == 404 {
                assert_eq!(
                    error_type(&body),
                    "ResourceNotFoundException",
                    "case {case:?}: expected ResourceNotFoundException, got {body}"
                );
            }

            for (operation, expected) in before {
                let path = reader_paths()
                    .into_iter()
                    .find(|(name, _)| *name == operation)
                    .map(|(_, path)| path)
                    .expect("every recorded read has its path");
                let (status, body) = call(&emulator, home, "GET", &path, None).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: {operation} in {home} must still answer 200, got {status} \
                     {body}"
                );
                assert_eq!(
                    body, expected,
                    "case {case:?}: {operation} in {home} must report what it did before the \
                     request served in {other}"
                );
            }
        }
    }
}

/// A resource ARN reaches only the Region that holds the resource it names: the tagging
/// operations resolve it in the Region they are served in, so an ARN of another Region is
/// absent — and tagging through the right ARN touches only that Region's copy.
#[tokio::test]
async fn a_resource_arn_reaches_only_the_region_that_holds_it() {
    for pair in REGION_PAIRS {
        let RegionPair { home, other } = pair;
        let emulator = Emulator::new();
        let (home_group, _) = create_chain(&emulator, home).await;
        let home_arn = home_group["Arn"].as_str().unwrap_or_default().to_string();

        // The same name in `other`, so a lookup that ignored the Region would find
        // SOMETHING for the home ARN rather than nothing.
        let (status, other_group) = call(
            &emulator,
            other,
            "POST",
            "/channelGroup",
            Some(json!({"ChannelGroupName": GROUP})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {other:?}: CreateChannelGroup expected 200, got {status} {other_group}"
        );
        let other_arn = other_group["Arn"].as_str().unwrap_or_default().to_string();

        // Tagging and reading tags through the home ARN, served in `other`.
        let probes: Vec<(&str, &str, &str, Option<Value>)> = vec![
            ("ListTagsForResource", "GET", "", None),
            (
                "TagResource",
                "POST",
                "",
                Some(json!({"tags": {"k": "from the wrong Region"}})),
            ),
            ("UntagResource", "DELETE", "?tagKeys=k", None),
        ];
        for (operation, method, query, body) in probes {
            let case = format!("{operation} on {home}'s ARN served in {other}");
            let path = format!("/tags/{}{query}", pct(&home_arn));
            let (status, answer) = call(&emulator, other, method, &path, body).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404, got {status} {answer}"
            );
        }

        // Each Region's own ARN reaches its own copy, and only it.
        for (region, arn) in [(home, &home_arn), (other, &other_arn)] {
            let (status, body) = call(
                &emulator,
                region,
                "POST",
                &format!("/tags/{}", pct(arn)),
                Some(json!({"tags": {"owner": region}})),
            )
            .await;
            assert!(
                status == 200 || status == 204,
                "case {region:?}: TagResource expected success, got {status} {body}"
            );
        }
        for (region, arn) in [(home, &home_arn), (other, &other_arn)] {
            let (status, body) =
                call(&emulator, region, "GET", &format!("/tags/{}", pct(arn)), None).await;
            assert_eq!(
                status, 200,
                "case {region:?}: ListTagsForResource expected 200, got {status} {body}"
            );
            assert_eq!(
                body["tags"]["owner"],
                json!(region),
                "case {region:?}: the tag must be the one written through this Region's ARN, got \
                 {body}"
            );
            assert_eq!(
                body["tags"]["k"],
                Value::Null,
                "case {region:?}: the tag attempted from the wrong Region must not be stored, got \
                 {body}"
            );
        }
    }
}

/// The channel-group ceiling is per Region: a Region filled to the emulator's bound
/// refuses the next create with `ServiceQuotaExceededException` (402) while another Region
/// still has its whole allowance.
#[tokio::test]
async fn the_channel_group_ceiling_is_applied_per_region() {
    use service_com_amazonaws_mediapackagev2_mediapackagev2::state::MAX_CHANNEL_GROUPS;

    let home = "us-west-2";
    let other = "eu-west-1";
    let emulator = Emulator::new();
    for index in 0..MAX_CHANNEL_GROUPS {
        let (status, body) = call(
            &emulator,
            home,
            "POST",
            "/channelGroup",
            Some(json!({"ChannelGroupName": format!("g-{index}")})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case \"group {index} of {MAX_CHANNEL_GROUPS} in {home}\": expected 200, got {status} \
             {body}"
        );
    }

    let (status, body) = call(
        &emulator,
        home,
        "POST",
        "/channelGroup",
        Some(json!({"ChannelGroupName": "one-too-many"})),
    )
    .await;
    assert_eq!(
        status, 402,
        "case \"past the ceiling in {home}\": expected 402, got {status} {body}"
    );
    assert_eq!(
        error_type(&body),
        "ServiceQuotaExceededException",
        "case \"past the ceiling in {home}\": expected ServiceQuotaExceededException, got {body}"
    );

    let (status, body) = call(
        &emulator,
        other,
        "POST",
        "/channelGroup",
        Some(json!({"ChannelGroupName": "g-0"})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"first group in {other}\": a Region's ceiling is its own, got {status} {body}"
    );
}

/// The store as a WHOLE is still bounded: the Region a create is served in comes from the
/// request's own credential scope, so partitioning by Region must not let a caller grow
/// the store a record per request by naming a fresh Region each time. Once the store-wide
/// ceiling is reached the create is refused with the same
/// `ServiceQuotaExceededException`, in a Region that holds nothing at all.
#[tokio::test]
async fn the_store_wide_channel_group_ceiling_bounds_every_region_together() {
    use service_com_amazonaws_mediapackagev2_mediapackagev2::state::{
        MAX_CHANNEL_GROUPS, MAX_CHANNEL_GROUPS_TOTAL,
    };

    let emulator = Emulator::new();
    // Fill Region after Region, each to its own ceiling, until the store-wide ceiling is
    // reached. Every Region here is empty before its own block, so nothing below is the
    // per-Region refusal.
    let mut stored = 0usize;
    let mut partition = 0usize;
    while stored < MAX_CHANNEL_GROUPS_TOTAL {
        let region = format!("eu-south-{partition}");
        let room = MAX_CHANNEL_GROUPS.min(MAX_CHANNEL_GROUPS_TOTAL - stored);
        for index in 0..room {
            let (status, body) = call(
                &emulator,
                &region,
                "POST",
                "/channelGroup",
                Some(json!({"ChannelGroupName": format!("g-{index}")})),
            )
            .await;
            assert_eq!(
                status, 200,
                "case \"group {index} of {room} in {region}\": expected 200, got {status} {body}"
            );
        }
        stored += room;
        partition += 1;
    }

    // A Region the store holds nothing in: only the store-wide ceiling can refuse this.
    let fresh = format!("eu-south-{partition}");
    let (status, body) = call(
        &emulator,
        &fresh,
        "POST",
        "/channelGroup",
        Some(json!({"ChannelGroupName": "g-0"})),
    )
    .await;
    assert_eq!(
        status, 402,
        "case \"a fresh Region past the store-wide ceiling of {MAX_CHANNEL_GROUPS_TOTAL}\": \
         expected 402, got {status} {body}"
    );
    assert_eq!(
        error_type(&body),
        "ServiceQuotaExceededException",
        "case \"a fresh Region past the store-wide ceiling\": expected \
         ServiceQuotaExceededException, got {body}"
    );
    let (status, body) = call(&emulator, &fresh, "GET", "/channelGroup?maxResults=100", None).await;
    assert_eq!(
        status, 200,
        "case \"a fresh Region past the store-wide ceiling\": ListChannelGroups expected 200, got \
         {status} {body}"
    );
    assert_eq!(
        body["Items"],
        json!([]),
        "case \"a fresh Region past the store-wide ceiling\": the refused create must store \
         nothing, got {body}"
    );
}

/// A published data-plane hostname serves only its own Region's resources.
///
/// Both Regions hold a group, channel and origin endpoint of the same names, and only
/// `home`'s endpoint carries a policy granting the fetch. The SAME object path is fetched
/// on each Region's published egress host: the grant is honoured on `home`'s host (so the
/// fetch falls through to the object lookup, which this emulator answers 404 since it
/// packages nothing) and is NOT honoured on `other`'s, whose endpoint has no policy.
#[tokio::test]
async fn a_published_data_plane_host_serves_only_its_own_region() {
    for pair in REGION_PAIRS {
        let RegionPair { home, other } = pair;
        let emulator = Emulator::new();
        let (home_group, home_endpoint) = create_chain(&emulator, home).await;
        let (other_group, _) = create_chain(&emulator, other).await;

        let (status, body) = call(
            &emulator,
            home,
            "POST",
            &format!("/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint/{ENDPOINT}/policy"),
            Some(json!({"Policy": grant_policy("mediapackagev2:GetObject")})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {home:?}: PutOriginEndpointPolicy expected 200, got {status} {body}"
        );

        let url = home_endpoint["HlsManifests"][0]["Url"]
            .as_str()
            .unwrap_or_else(|| panic!("case {home:?}: the endpoint must publish a manifest URL"));
        let (home_host, object) = split_url(url);
        assert_eq!(
            home_host,
            home_group["EgressDomain"].as_str().unwrap_or_default(),
            "case {home:?}: the manifest URL must be served from the group's egress domain"
        );
        let other_host = other_group["EgressDomain"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let (status, body) = emulator
            .send_in_region("GET", &object, None, &home_host, home, &[])
            .await;
        assert_eq!(
            status, 404,
            "case {home:?}: the granted fetch on this Region's host must reach the object \
             lookup, got {status} {body}"
        );

        let (status, body) = emulator
            .send_in_region("GET", &object, None, &other_host, other, &[])
            .await;
        assert_eq!(
            status, 403,
            "case {other:?}: the same path on another Region's host must be judged against that \
             Region's endpoint, which grants nothing, got {status} {body}"
        );
    }
}
