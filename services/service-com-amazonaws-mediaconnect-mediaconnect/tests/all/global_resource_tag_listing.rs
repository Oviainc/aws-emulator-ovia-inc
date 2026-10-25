// SPDX-License-Identifier: CC0-1.0

//! `ListTagsForGlobalResource`, end to end over HTTP.
//!
//! The rule under test: `GET /tags/global/{ResourceArn}` REPORTS the tag map the store holds
//! on the record of the GLOBAL resource that ARN addresses — one of the three the operation
//! covers, "router inputs, router outputs and router network interfaces"
//! (`smithy_query(query="doc",shape="ListTagsForGlobalResource")`) — as a PRESENT, empty map
//! when the resource carries no tags, and changing nothing a later caller sees; an ARN that
//! addresses no such resource is refused with the modelled `NotFoundException` (404), never a
//! 501 and never a success.
//!
//! The table rows differ along what the rule generalises over: WHICH kind of global resource
//! the ARN addresses (all three), how many pairs the resource carries (no `Tags` member at
//! all, an empty map, one, several, the tags-per-resource bound) and the form of each key and
//! value (ASCII, non-ASCII, empty, URI-significant, both documented length maxima). The
//! refusal rows are the ways an ARN can address no global resource, including a flow that
//! really exists and really carries tags, because a flow is not one of the three.
//!
//! The report is also driven against tag maps written STRAIGHT INTO the record and against
//! the map a `TagGlobalResource` write merged, so a handler that answered its create's
//! request rather than reading the record cannot pass.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), and the
//! three creates are the ones the global tag WRITING suite owns, so both suites address the
//! same subjects over the same request path: routing, restJson1 deserialization, constraint
//! validation, handler, serialization.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::{Emulator, REGION};
// The three global-resource creates, the kinds they cover and the global tag write: the
// global tagging suite owns them, so this suite reads back exactly the subjects that one
// writes to.
use crate::global_resource_tags::{create_subject, tag_global, GlobalKind, EVERY_KIND};
// The account id a generated emulator presents, the percent-encoder, the length-bound filler
// and the reply triple the regional tagging suite owns.
use crate::resource_tags::{filler, pct, Reply, ACCOUNT};

/// Longer than [`mc::TRANSITION_SPAN`], so a removal the clock owes is certainly due.
const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// `GET /tags/global/{ResourceArn}`. The ARN is percent-encoded, which is what a client
/// sends for an ARN bound to a NON-greedy `@httpLabel`.
async fn list_global_tags(emulator: &Emulator, arn: &str) -> Reply {
    emulator
        .send("GET", &format!("/tags/global/{}", pct(arn)), None)
        .await
}

/// The tags `ListTagsForGlobalResource` reports for `arn`, asserted to be a 200 carrying a
/// `Tags` map.
async fn reported_tags(emulator: &Emulator, case: &str, arn: &str) -> BTreeMap<String, String> {
    let (status, code, body) = list_global_tags(emulator, arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: listing the tags of {arn:?} expected 200, got {status} {code:?}: {body}"
    );
    // The member is PRESENT even for a resource that carries no tags: an untagged resource
    // has an EMPTY tag map, not an absent one, so an omitted member is a different answer and
    // must not be read here as an empty map.
    let reported = body["tags"]
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: the response must carry a tags map, got {body}"));
    reported
        .iter()
        .map(|(key, value)| {
            let value = value.as_str().unwrap_or_else(|| {
                panic!("case {case:?}: the tag {key:?} must be a string, got {value}")
            });
            (key.clone(), value.to_string())
        })
        .collect()
}

/// Overwrite the stored tag map of the global resource `arn` addresses, whatever kind it is —
/// a write applied to the RECORD, so what the operation reports can only have come from the
/// store.
fn store_tags(emulator: &Emulator, arn: &str, tags: &BTreeMap<String, String>) {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let mut store = state.lock();
    if let Some(router_input) = store.router_inputs.get_mut(arn) {
        router_input.tags = tags.clone();
        return;
    }
    if let Some(router_output) = store.router_outputs.get_mut(arn) {
        router_output.tags = tags.clone();
        return;
    }
    if let Some(interface) = store.router_network_interfaces.get_mut(arn) {
        interface.tags = tags.clone();
        return;
    }
    panic!("the store holds no global resource for {arn:?}");
}

/// One tag set a global resource can carry: the `Tags` member of its create, and the map the
/// report must answer for it.
struct TagRow {
    case: &'static str,
    /// The `Tags` member of the create request, or `None` when the create names none at all.
    requested: Option<Value>,
    expected: BTreeMap<String, String>,
}

fn pairs(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

fn as_json(tags: &BTreeMap<String, String>) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect(),
    )
}

/// The rows span the class the rule generalises over: the pair count (no member at all, an
/// empty map, one, several, the tags-per-resource bound) and the form of a key and a value —
/// including the two lengths the `Tags` member documentation publishes
/// (`mc::MAX_TAG_KEY_LENGTH` / `mc::MAX_TAG_VALUE_LENGTH`, from
/// `smithy_query(query="doc",shape="TagResourceRequest",member="Tags")`: "Tag keys can have a
/// maximum character length of 128 characters, and tag values can have a maximum length of
/// 256 characters").
fn tag_rows() -> Vec<TagRow> {
    let maximum_count: BTreeMap<String, String> = (0..mc::MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("key-{index:03}"), format!("value-{index:03}")))
        .collect();
    let longest: BTreeMap<String, String> = BTreeMap::from([(
        filler(mc::MAX_TAG_KEY_LENGTH, 3),
        filler(mc::MAX_TAG_VALUE_LENGTH, 7),
    )]);

    vec![
        TagRow {
            case: "no tags member at all",
            requested: None,
            expected: BTreeMap::new(),
        },
        TagRow {
            case: "an empty tags map",
            requested: Some(json!({})),
            expected: BTreeMap::new(),
        },
        TagRow {
            case: "one pair",
            requested: Some(json!({ "env": "prod" })),
            expected: pairs(&[("env", "prod")]),
        },
        TagRow {
            case: "three pairs",
            requested: Some(json!({ "env": "prod", "team": "video", "cost-center": "cc-42" })),
            expected: pairs(&[("env", "prod"), ("team", "video"), ("cost-center", "cc-42")]),
        },
        TagRow {
            case: "a non-ASCII key and value",
            requested: Some(json!({ "проект": "видео-поток" })),
            expected: pairs(&[("проект", "видео-поток")]),
        },
        TagRow {
            case: "an empty value",
            requested: Some(json!({ "blank": "" })),
            expected: pairs(&[("blank", "")]),
        },
        TagRow {
            case: "URI-significant characters in the key and the value",
            requested: Some(json!({ "path/key": "a:b c&d=e" })),
            expected: pairs(&[("path/key", "a:b c&d=e")]),
        },
        TagRow {
            case: "a key and a value at the documented maximum character lengths",
            requested: Some(as_json(&longest)),
            expected: longest,
        },
        TagRow {
            case: "the maximum number of tags one resource carries",
            requested: Some(as_json(&maximum_count)),
            expected: maximum_count,
        },
    ]
}

/// Every global resource reports the tag map it carries, whatever kind of resource it is and
/// whatever that map holds — and a report leaves it where it was, so a second read, and every
/// other resource's read, answer the same thing.
#[tokio::test]
async fn listing_reports_each_global_resources_stored_tags() {
    let emulator = Emulator::new();

    // Every resource created below, with the map its own report must answer, so the last pass
    // can assert that no read and no other resource's tags disturbed it.
    let mut created: Vec<(String, String, BTreeMap<String, String>)> = Vec::new();

    let mut index = 0_usize;
    for row in tag_rows() {
        for kind in EVERY_KIND {
            index += 1;
            let case = format!("{kind:?}: {}", row.case);
            let arn = create_subject(
                &emulator,
                kind,
                &format!("gtl-{index}"),
                row.requested.clone(),
            )
            .await;

            let reported = reported_tags(&emulator, &case, &arn).await;
            assert_eq!(
                reported, row.expected,
                "case {case:?}: the report must be the stored tag map, got {reported:?}"
            );
            // A read changes nothing a later caller sees.
            let again = reported_tags(&emulator, &case, &arn).await;
            assert_eq!(
                again, row.expected,
                "case {case:?}: a second report must answer the same map, got {again:?}"
            );

            created.push((case, arn, row.expected.clone()));
        }
    }

    for (case, arn, expected) in &created {
        let reported = reported_tags(&emulator, case, arn).await;
        assert_eq!(
            &reported, expected,
            "case {case:?}: every resource still reports its OWN tags, got {reported:?}"
        );
    }
}

/// The report follows the STORED tag map: a sequence of maps written straight into the record
/// — which no request ever carried — is what the operation answers, for every kind of global
/// resource.
#[tokio::test]
async fn the_report_follows_the_stored_tag_map() {
    let emulator = Emulator::new();

    // The maps are written in sequence onto the same record, so the report tracks the store
    // rather than the first thing that was ever put there.
    let writes = [
        pairs(&[("env", "prod")]),
        pairs(&[("env", "staging"), ("owner", "ops")]),
        BTreeMap::new(),
        pairs(&[("re-tagged", "after empty")]),
    ];

    for (index, kind) in EVERY_KIND.into_iter().enumerate() {
        let case = format!("{kind:?}: a tag map written into the record");
        let arn = create_subject(
            &emulator,
            kind,
            &format!("gtl-stored-{index}"),
            Some(json!({ "created-with": "a pair no write below keeps" })),
        )
        .await;

        for stored in &writes {
            store_tags(&emulator, &arn, stored);
            let reported = reported_tags(&emulator, &case, &arn).await;
            assert_eq!(
                &reported, stored,
                "case {case:?}: the report must answer the stored map {stored:?}, got \
                 {reported:?}"
            );
        }
    }
}

/// The report and the global tag WRITE are two views of the one stored map: whatever a
/// `TagGlobalResource` call merges into a resource's tags is what the report answers next,
/// for every kind of global resource.
#[tokio::test]
async fn the_report_answers_what_a_global_tag_write_merged() {
    let emulator = Emulator::new();

    for (index, kind) in EVERY_KIND.into_iter().enumerate() {
        let arn = create_subject(
            &emulator,
            kind,
            &format!("gtl-written-{index}"),
            Some(json!({ "env": "prod" })),
        )
        .await;

        let mut expected = pairs(&[("env", "prod")]);
        for (row, written) in [
            ("one new key", json!({ "owner": "ops" })),
            ("overwrite a key it already carries", json!({ "env": "staging" })),
            (
                "two pairs at once",
                json!({ "cost-center": "cc-42", "проект": "видео-поток" }),
            ),
            ("no pairs at all", json!({})),
        ] {
            let case = format!("{kind:?}: {row}");
            let (status, code, body) = tag_global(&emulator, &arn, written.clone()).await;
            assert_eq!(
                (status, code.as_deref()),
                (204, None),
                "case {case:?}: the tag write expected 204, got {status} {code:?}: {body}"
            );
            for (key, value) in written.as_object().expect("the row names a tag map") {
                expected.insert(
                    key.clone(),
                    value.as_str().expect("a tag value is a string").to_string(),
                );
            }

            let reported = reported_tags(&emulator, &case, &arn).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: the report must answer the merged map, got {reported:?}"
            );
        }
    }
}

/// An ARN that addresses no global resource is refused with the modelled `NotFoundException`
/// (404) — never a 501 and never a success. The rows are the ways an ARN can address none:
/// each of the three global ARN forms with an id no create minted, a resource this service
/// really holds that is not a global one (a flow, whose tags `ListTagsForResource` does
/// report), another MediaConnect resource type, another service's ARN, and a string that is
/// no ARN at all.
#[tokio::test]
async fn listing_an_arn_that_addresses_no_global_resource_is_not_found() {
    let emulator = Emulator::new();

    // A flow that really exists and really carries tags: it is not one of the three global
    // resources, so the GLOBAL report does not reach it, while the regional one does.
    let (status, code, flow) = emulator
        .create_flow(json!({
            "name": "global-tag-listing-flow",
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" },
            "flowTags": { "env": "prod" },
        }))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "creating the fixture flow expected 201, got {status} {code:?}: {flow}"
    );
    let flow_arn = flow["flow"]["flowArn"]
        .as_str()
        .unwrap_or_else(|| panic!("the create answers a FlowArn, got {flow}"))
        .to_string();
    let (status, code, listed) = emulator
        .send("GET", &format!("/tags/{}", pct(&flow_arn)), None)
        .await;
    assert_eq!(
        (status, code.as_deref(), listed["tags"]["env"].as_str()),
        (200, None, Some("prod")),
        "the flow's own tags are reported by ListTagsForResource, got {status} {code:?}: \
         {listed}"
    );

    let rows = [
        (
            "a router input ARN no create minted",
            format!("arn:aws:mediaconnect:{REGION}:{ACCOUNT}:routerInput:ffffffffffff"),
        ),
        (
            "a router output ARN no create minted",
            format!("arn:aws:mediaconnect:{REGION}:{ACCOUNT}:routerOutput:eeeeeeeeeeee"),
        ),
        (
            "a router network interface ARN no create minted",
            format!("arn:aws:mediaconnect:{REGION}:{ACCOUNT}:routerNetworkInterface:dddddddddddd"),
        ),
        (
            "a flow that exists and carries tags — a flow is not a global resource",
            flow_arn,
        ),
        (
            "a gateway ARN",
            format!("arn:aws:mediaconnect:{REGION}:{ACCOUNT}:gateway:1-0000beef:some-gateway"),
        ),
        (
            "another service's ARN",
            format!("arn:aws:medialive:{REGION}:{ACCOUNT}:channel:1234567"),
        ),
        ("a string that is no ARN at all", "not-an-arn".to_string()),
    ];

    for (case, arn) in rows {
        let (status, code, body) = list_global_tags(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (404, Some("NotFoundException")),
            "case {case:?}: expected 404 NotFoundException, got {status} {code:?}: {body}"
        );
    }
}

/// A global resource the clock has taken away stops reporting tags: a router input's and a
/// router output's deletion completes one transition span after the delete, and the report
/// answers that resource's tags right up to that instant and `NotFoundException` afterwards.
#[tokio::test]
async fn a_resource_the_clock_has_removed_reports_no_tags() {
    let emulator = Emulator::new();

    // An untouched subject of the third kind, so the removals are shown to take away only
    // the resources they addressed.
    let interface = create_subject(
        &emulator,
        GlobalKind::NetworkInterface,
        "gtl-outlives",
        None,
    )
    .await;

    for (index, (kind, path)) in [
        (GlobalKind::Input, "/v1/routerInput"),
        (GlobalKind::Output, "/v1/routerOutput"),
    ]
    .into_iter()
    .enumerate()
    {
        let case = format!("{kind:?}: deleted, then settled by the clock");
        let expected = pairs(&[("env", "prod"), ("team", "video")]);
        let arn = create_subject(
            &emulator,
            kind,
            &format!("gtl-deleted-{index}"),
            Some(as_json(&expected)),
        )
        .await;

        let (status, code, body) = emulator
            .send("DELETE", &format!("{path}/{}", pct(&arn)), None)
            .await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: the delete is served, got {status} {code:?}: {body}"
        );

        // The record is still there until the removal comes due, so its tags are still
        // reported.
        let reported = reported_tags(&emulator, &case, &arn).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: a resource whose deletion is in flight still reports its tags, got \
             {reported:?}"
        );

        tokio::time::sleep(PAST_THE_TRANSITION).await;
        let (status, code, body) = list_global_tags(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (404, Some("NotFoundException")),
            "case {case:?}: once the clock has removed the resource the report is a 404, got \
             {status} {code:?}: {body}"
        );
    }

    let reported = reported_tags(&emulator, "the interface outlives them", &interface).await;
    assert_eq!(
        reported,
        BTreeMap::new(),
        "the untouched interface still reports its own (empty) tag map, got {reported:?}"
    );
}

/// The wait the case above sits out really covers the crate's transition span, rather than a
/// number this suite repeats.
#[test]
fn the_wait_covers_the_transition_span() {
    assert!(
        PAST_THE_TRANSITION > mc::TRANSITION_SPAN,
        "the wait must be longer than the transition span, got {PAST_THE_TRANSITION:?} against \
         {:?}",
        mc::TRANSITION_SPAN
    );
}
