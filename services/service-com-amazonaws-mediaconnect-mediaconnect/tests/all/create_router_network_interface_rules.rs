// SPDX-License-Identifier: CC0-1.0

//! CreateRouterNetworkInterface's own request rules, end to end over HTTP, read back through
//! BatchGetRouterNetworkInterface.
//!
//! `crate::router_network_interfaces` already pins what the create STORES for each
//! configuration variant and what the batch read reports for it. The three rules below are
//! the ones that suite's rows hold constant, so they are asserted here over the class each
//! one generalises over:
//!
//! 1. **Region.** The interface's `RegionName`, and the Region segment of the ARN the create
//!    mints, come from the request's `RegionName` when it names a Region and from the Region
//!    the request was signed for when it does not ("The Amazon Web Services Region for the
//!    router network interface. Defaults to the current region if not specified",
//!    `smithy_query(query="operation",operation="CreateRouterNetworkInterface")`), and a
//!    `RegionName` that cannot be the Region segment of a minted ARN is refused — the
//!    modelled `RouterNetworkInterfaceArn` pattern constrains that segment to `[a-z0-9-]+`.
//!    The rows vary the Region named (and the partition it belongs to), the Region the
//!    request is SIGNED for, and the way a value fails to be a Region token.
//! 2. **Tags.** A create's `Tags` are stored as given and reported by the read; a request
//!    past the service's tags-per-resource bound, or carrying a pair outside the key and
//!    value lengths the member documents, is refused with the modelled `BadRequestException`
//!    and stores nothing. The rows are the bound and one past it, each published length
//!    bound and one past it, and a key the service stores no tag under.
//! 3. **The store ceiling.** The create refuses once the store holds
//!    [`mc::MAX_ROUTER_NETWORK_INTERFACES`] interfaces, with the modelled
//!    `RouterNetworkInterfaceServiceQuotaExceededException` (HTTP 420) — the only operation
//!    the model declares it on — while a replay of a `ClientToken` the store already holds
//!    still answers, because a replay mints nothing and so needs no slot.
//!
//! The harness is the one `crate::flow_lifecycle` owns, so every case drives the request path
//! a MediaConnect client reaches: routing, restJson1 deserialization, constraint validation,
//! handler, serialization.

use serde_json::{json, Map, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::{Emulator, REGION};
use crate::router_network_interfaces::{
    batch_get_network_interfaces, create_network_interface, public_configuration, stored_count,
};

/// The `routerNetworkInterface` the create answered, or a panic naming the case.
fn created_interface(case: &str, created: &Value) -> Value {
    created
        .get("routerNetworkInterface")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the create answers a routerNetworkInterface: {created}"))
}

/// The Region segment of a router network interface ARN — segment 4 of
/// `arn:<partition>:mediaconnect:<region>:<account>:routerNetworkInterface:<id>`.
fn arn_region(case: &str, arn: &str) -> String {
    arn.split(':')
        .nth(3)
        .unwrap_or_else(|| panic!("case {case:?}: an ARN has a Region segment, got {arn:?}"))
        .to_string()
}

/// One create, and the Region the stored interface must report for it.
struct RegionCase {
    case: &'static str,
    /// The `RegionName` the request names, or `None` when it names none.
    requested: Option<&'static str>,
    expected: &'static str,
}

/// The rows span Region names of different forms — two-part and three-part, with and without
/// a digit, including a Region name AWS serves in another partition — and the two ways a
/// request can leave the Region to the service: naming the signed Region explicitly, or
/// naming nothing. Only the Region segment varies: `engine::emulator_arn` mints every ARN in
/// the `aws` partition, which is why these rows assert the Region segment and not the
/// partition.
///
/// Every value is a Region token as [`mc::is_region_token`] (the modelled ARN Region segment
/// `[a-z0-9-]+`) defines one; none of them is the Region of [`REGION`] except the two rows
/// that are about that Region.
fn region_cases() -> Vec<RegionCase> {
    vec![
        RegionCase {
            case: "no RegionName: the Region the request was signed for (the distinguishing \
                   case, where the signed Region is not the fallback, is \
                   `a_create_that_names_no_region_takes_the_signed_region`)",
            requested: None,
            expected: REGION,
        },
        RegionCase {
            case: "RegionName naming the signed Region explicitly",
            requested: Some(REGION),
            expected: REGION,
        },
        RegionCase {
            case: "RegionName naming another Region of the same partition",
            requested: Some("us-west-2"),
            expected: "us-west-2",
        },
        RegionCase {
            case: "RegionName naming a Region on another continent",
            requested: Some("eu-central-1"),
            expected: "eu-central-1",
        },
        RegionCase {
            case: "RegionName naming a four-part Region name",
            requested: Some("ap-southeast-4"),
            expected: "ap-southeast-4",
        },
        RegionCase {
            case: "RegionName naming a GovCloud Region",
            requested: Some("us-gov-west-1"),
            expected: "us-gov-west-1",
        },
    ]
}

/// A Region no row of [`region_cases`] names and that is not [`REGION`], used to tell the
/// Region a request was SIGNED for apart from the fallback the crate uses when a request
/// carries no credential scope (`crate::state::DEFAULT_REGION`, which equals [`REGION`]).
const SIGNED_ELSEWHERE: &str = "eu-west-1";

#[tokio::test]
async fn a_create_that_names_no_region_takes_the_signed_region() {
    let emulator = Emulator::new();

    let (status, code, created) = emulator
        .send_signed_for(
            SIGNED_ELSEWHERE,
            "POST",
            "/v1/routerNetworkInterface",
            Some(json!({
                "name": "rni-signed-elsewhere",
                "configuration": public_configuration(1),
            })),
        )
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "a create signed for {SIGNED_ELSEWHERE} expected 201, got {status} {code:?}: {created}"
    );
    let interface = created_interface("signed for another Region, naming none", &created);
    let arn = interface["arn"]
        .as_str()
        .expect("the create answers an ARN")
        .to_string();

    assert_eq!(
        interface["regionName"],
        json!(SIGNED_ELSEWHERE),
        "a create that names no RegionName takes the Region it was signed for, got {:?}",
        interface["regionName"]
    );
    assert_eq!(
        arn_region("signed for another Region, naming none", &arn),
        SIGNED_ELSEWHERE,
        "the minted ARN's Region segment is the signed Region, got {arn:?}"
    );
    assert!(
        mc::is_mintable_router_network_interface_arn(&arn),
        "the minted ARN must satisfy the modelled RouterNetworkInterfaceArn pattern, got {arn:?}"
    );

    // An explicit RegionName still wins over the Region the request was signed for.
    let (status, code, created) = emulator
        .send_signed_for(
            SIGNED_ELSEWHERE,
            "POST",
            "/v1/routerNetworkInterface",
            Some(json!({
                "name": "rni-signed-elsewhere-named",
                "configuration": public_configuration(1),
                "regionName": "ap-south-1",
            })),
        )
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "a create naming a Region expected 201, got {status} {code:?}: {created}"
    );
    let interface = created_interface("signed for one Region, naming another", &created);
    assert_eq!(
        interface["regionName"],
        json!("ap-south-1"),
        "an explicit RegionName wins over the signed Region, got {:?}",
        interface["regionName"]
    );
    assert_eq!(
        arn_region("signed for one Region, naming another", interface["arn"].as_str().unwrap_or_default()),
        "ap-south-1",
        "the minted ARN's Region segment is the named Region, got {:?}",
        interface["arn"]
    );
}

#[tokio::test]
async fn the_created_interface_region_is_the_requested_region() {
    let emulator = Emulator::new();

    for RegionCase {
        case,
        requested,
        expected,
    } in region_cases()
    {
        let mut body = json!({
            "name": "rni-region",
            "configuration": public_configuration(1),
        });
        if let Some(region) = requested {
            body["regionName"] = json!(region);
        }

        let (status, code, created) = create_network_interface(&emulator, body).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: create expected 201, got {status} {code:?}: {created}"
        );
        let interface = created_interface(case, &created);
        let arn = interface["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create answers an ARN: {created}"))
            .to_string();

        assert_eq!(
            interface["regionName"],
            json!(expected),
            "case {case:?}: the stored interface's regionName must be {expected:?}, got {:?}",
            interface["regionName"]
        );
        assert_eq!(
            arn_region(case, &arn),
            expected,
            "case {case:?}: the minted ARN's Region segment must be {expected:?}, got {arn:?}"
        );
        assert!(
            mc::is_mintable_router_network_interface_arn(&arn),
            "case {case:?}: the minted ARN must satisfy the modelled RouterNetworkInterfaceArn \
             pattern, got {arn:?}"
        );

        // And the read answers from the stored record, not from the request that made it.
        let (status, code, read) = batch_get_network_interfaces(&emulator, &[&arn]).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: batch get expected 200, got {status} {code:?}: {read}"
        );
        assert_eq!(
            read["routerNetworkInterfaces"][0]["regionName"],
            json!(expected),
            "case {case:?}: the read interface's regionName must be {expected:?}, got {read}"
        );
    }
}

#[tokio::test]
async fn a_region_name_that_is_not_a_region_token_is_refused() {
    let emulator = Emulator::new();

    // Each row fails the modelled ARN Region segment `[a-z0-9-]+` in a DIFFERENT way: an
    // uppercase letter, an underscore, a space, a path separator, a dot.
    for (case, region) in [
        ("an uppercase Region name", "US-WEST-2"),
        ("an underscore instead of a hyphen", "us_west_2"),
        ("a space inside the Region name", "us west 2"),
        ("a path separator inside the Region name", "us/west-2"),
        ("a dotted Region name", "us.west.2"),
    ] {
        let before = stored_count(&emulator);
        let (status, code, answer) = create_network_interface(
            &emulator,
            json!({
                "name": "rni-bad-region",
                "configuration": public_configuration(1),
                "regionName": region,
            }),
        )
        .await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: expected 400 BadRequestException for {region:?}, got {status} \
             {code:?}: {answer}"
        );
        assert!(
            answer["message"]
                .as_str()
                .unwrap_or_default()
                .starts_with(mc::VALIDATION_FAILED_PREFIX),
            "case {case:?}: the refusal carries this service's validation envelope, got {answer}"
        );
        assert_eq!(
            stored_count(&emulator),
            before,
            "case {case:?}: a refused create stores nothing, got {} stored",
            stored_count(&emulator)
        );
    }
}

/// A tag map of `count` distinct pairs. (Named apart from `crate::resource_tags::tag_map`,
/// which builds a map from pairs rather than from a count.)
fn distinct_tags(count: usize) -> Value {
    let mut tags = Map::new();
    for index in 0..count {
        tags.insert(format!("key-{index}"), json!(format!("value-{index}")));
    }
    Value::Object(tags)
}

/// A tag map of the single pair `key` / `value` — spelled out rather than through `json!`,
/// whose object keys are literals.
fn one_tag(key: String, value: String) -> Value {
    let mut tags = Map::new();
    tags.insert(key, json!(value));
    Value::Object(tags)
}

#[tokio::test]
async fn created_tags_are_stored_up_to_the_documented_bounds() {
    let emulator = Emulator::new();

    // The accepted rows: no tags at all, one pair, the most pairs one resource carries
    // (`mc::MAX_TAGS_PER_RESOURCE`), a key at the documented maximum key length and a value
    // at the documented maximum value length ("Tag keys can have a maximum character length
    // of 128 characters" / "tag values can have a maximum length of 256 characters",
    // `smithy_query(query="doc",shape="TagResourceRequest",member="Tags")`), and an empty
    // VALUE, which those bounds allow (a tag is addressed by its key, not its value).
    let accepted: Vec<(&str, Value)> = vec![
        ("no Tags member at all", Value::Null),
        ("an empty tag map", json!({})),
        ("one tag pair", distinct_tags(1)),
        (
            "the most tag pairs one resource carries",
            distinct_tags(mc::MAX_TAGS_PER_RESOURCE),
        ),
        (
            "a key at the documented maximum length",
            one_tag(
                "k".repeat(mc::MAX_TAG_KEY_LENGTH),
                "at-the-key-bound".to_string(),
            ),
        ),
        (
            "a value at the documented maximum length",
            one_tag(
                "at-the-value-bound".to_string(),
                "v".repeat(mc::MAX_TAG_VALUE_LENGTH),
            ),
        ),
        (
            "an empty tag value",
            one_tag("empty-value".to_string(), String::new()),
        ),
    ];

    for (case, tags) in accepted {
        let mut body = json!({
            "name": "rni-tags",
            "configuration": public_configuration(1),
        });
        if !tags.is_null() {
            body["tags"] = tags.clone();
        }
        let expected_tags = if tags.is_null() { json!({}) } else { tags };

        let (status, code, created) = create_network_interface(&emulator, body).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: create expected 201, got {status} {code:?}: {created}"
        );
        let interface = created_interface(case, &created);
        assert_eq!(
            interface["tags"], expected_tags,
            "case {case:?}: the created interface reports the tags the request gave, got {:?}",
            interface["tags"]
        );

        let arn = interface["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create answers an ARN: {created}"))
            .to_string();
        let (status, _, read) = batch_get_network_interfaces(&emulator, &[&arn]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: batch get expected 200, got {status}: {read}"
        );
        assert_eq!(
            read["routerNetworkInterfaces"][0]["tags"], expected_tags,
            "case {case:?}: the read interface reports the stored tags, got {read}"
        );
    }
}

#[tokio::test]
async fn a_tag_the_service_will_not_store_is_refused() {
    let emulator = Emulator::new();

    // One row per way a tag map can be outside what this service stores: one pair past the
    // tags-per-resource bound, one character past each documented length, and a key no tag
    // can be addressed by.
    let refused: Vec<(&str, Value)> = vec![
        (
            "one pair past the tags-per-resource bound",
            distinct_tags(mc::MAX_TAGS_PER_RESOURCE + 1),
        ),
        (
            "a key one character past the documented maximum",
            one_tag(
                "k".repeat(mc::MAX_TAG_KEY_LENGTH + 1),
                "past-the-key-bound".to_string(),
            ),
        ),
        (
            "a value one character past the documented maximum",
            one_tag(
                "past-the-value-bound".to_string(),
                "v".repeat(mc::MAX_TAG_VALUE_LENGTH + 1),
            ),
        ),
        (
            "an empty tag key",
            one_tag(String::new(), "no-key".to_string()),
        ),
    ];

    for (case, tags) in refused {
        let before = stored_count(&emulator);
        let (status, code, answer) = create_network_interface(
            &emulator,
            json!({
                "name": "rni-bad-tags",
                "configuration": public_configuration(1),
                "tags": tags,
            }),
        )
        .await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: expected 400 BadRequestException, got {status} {code:?}: {answer}"
        );
        assert!(
            answer["message"]
                .as_str()
                .unwrap_or_default()
                .starts_with(mc::VALIDATION_FAILED_PREFIX),
            "case {case:?}: the refusal carries this service's validation envelope, got {answer}"
        );
        assert_eq!(
            stored_count(&emulator),
            before,
            "case {case:?}: a refused create stores nothing, got {} stored",
            stored_count(&emulator)
        );
    }
}

/// Fill the store up to `count` router network interfaces by cloning the record `template_arn`
/// names, each clone under its own freshly minted ARN and id.
///
/// The ceiling is a bound on what is STORED, so the suite that asserts it needs a full store
/// rather than a thousand round trips — the same shaping as `crate::flow_lifecycle`'s flow
/// seeding, and the calls that matter (the one that fills the last slot, the one past the
/// bound, the replay) still go over the wire.
fn seed_interfaces_to(emulator: &Emulator, template_arn: &str, count: usize) {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let mut store = state.lock();
    let template = store
        .router_network_interfaces
        .get(template_arn)
        .cloned()
        .expect("the seeded store already holds the interface the create minted");
    while store.router_network_interfaces.len() < count {
        let suffix = store.next_suffix();
        let id = mc::router_resource_id(suffix);
        let arn = engine::emulator_arn(
            mc::SERVICE_NAMESPACE,
            REGION,
            &mc::router_network_interface_resource_fragment(&id),
        );
        let mut seeded = template.clone();
        seeded.arn = arn.clone();
        seeded.id = id;
        // A seeded record replays nobody's idempotency token: only the interface the create
        // below minted carries one.
        seeded.client_token = None;
        store.router_network_interfaces.insert(arn, seeded);
    }
}

#[tokio::test]
async fn create_refuses_past_the_store_ceiling_but_still_answers_a_replay() {
    const CEILING: usize = mc::MAX_ROUTER_NETWORK_INTERFACES;
    const TOKEN: &str = "ceiling-token";

    let emulator = Emulator::new();
    let (status, code, created) = create_network_interface(
        &emulator,
        json!({
            "name": "rni-ceiling-first",
            "configuration": public_configuration(1),
            "clientToken": TOKEN,
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "the first create expected 201, got {status} {code:?}: {created}"
    );
    let first_arn = created["routerNetworkInterface"]["arn"]
        .as_str()
        .expect("the create answers an ARN")
        .to_string();

    // One slot short of the ceiling, set up directly.
    seed_interfaces_to(&emulator, &first_arn, CEILING - 1);
    assert_eq!(
        stored_count(&emulator),
        CEILING - 1,
        "the store is one slot short of the ceiling, got {} stored",
        stored_count(&emulator)
    );

    let (status, code, created) = create_network_interface(
        &emulator,
        json!({
            "name": "rni-ceiling-last",
            "configuration": public_configuration(1),
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "the create that fills the last slot expected 201, got {status} {code:?}: {created}"
    );
    let last_arn = created["routerNetworkInterface"]["arn"]
        .as_str()
        .expect("the create answers an ARN")
        .to_string();

    // One past the ceiling: refused with the modelled quota failure, and nothing stored.
    let (status, code, answer) = create_network_interface(
        &emulator,
        json!({
            "name": "rni-ceiling-overflow",
            "configuration": public_configuration(1),
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (
            420,
            Some("RouterNetworkInterfaceServiceQuotaExceededException")
        ),
        "at the ceiling, create expected 420 \
         RouterNetworkInterfaceServiceQuotaExceededException, got {status} {code:?}: {answer}"
    );
    assert!(
        answer["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "at the ceiling, the refusal carries a message, got {answer}"
    );
    assert_eq!(
        stored_count(&emulator),
        CEILING,
        "a refused create stores nothing, got {} stored",
        stored_count(&emulator)
    );

    // What was created is still readable, and the refusal did not disturb it.
    let (status, code, read) =
        batch_get_network_interfaces(&emulator, &[&first_arn, &last_arn]).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "after the refusal the stored interfaces are still readable, got {status} {code:?}: \
         {read}"
    );
    assert_eq!(
        read["routerNetworkInterfaces"]
            .as_array()
            .map(|interfaces| interfaces.len()),
        Some(2),
        "both created interfaces are reported after the refusal, got {read}"
    );

    // A replayed ClientToken mints nothing, so it needs no slot: it answers with the
    // interface that token already minted even though the store is full.
    let (status, code, replay) = create_network_interface(
        &emulator,
        json!({
            "name": "rni-ceiling-first",
            "configuration": public_configuration(1),
            "clientToken": TOKEN,
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "at the ceiling, a replayed ClientToken expected 201, got {status} {code:?}: {replay}"
    );
    assert_eq!(
        replay["routerNetworkInterface"]["arn"].as_str(),
        Some(first_arn.as_str()),
        "at the ceiling, a replayed ClientToken answers with the interface it minted, got \
         {replay}"
    );
    assert_eq!(
        stored_count(&emulator),
        CEILING,
        "a replayed ClientToken mints no interface, got {} stored",
        stored_count(&emulator)
    );
}
