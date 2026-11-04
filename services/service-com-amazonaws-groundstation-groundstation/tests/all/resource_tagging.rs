// SPDX-License-Identifier: CC0-1.0

//! Ground Station RESOURCE TAGGING, end to end over HTTP: what assigning a tag does to
//! the resource an ARN names, and what a tag read reports.
//!
//! The RULE under test: `TagResource` MERGES the entries it carries into the tag map of
//! the resource `resourceArn` resolves to — a key the resource does not hold is added, a
//! key it does hold is overwritten with the value this request carries, and every key the
//! request does not name is left exactly as it was — and that ONE map is what
//! `ListTagsForResource` and the resource's own read report. An ARN that resolves to no
//! resource of this account is refused by both operations with the tagging plane's own
//! not-found refusal — the code `NotFoundException`, HTTP 434 and one fixed sentence — and
//! nothing is written. [`crate::tagging_not_found_refusal`] asserts that refusal over its
//! own table of ARNs that resolve to nothing, across all three operations of the plane;
//! what the suite below adds is that it persists nothing.
//!
//! Each suite asserts that rule over a TABLE rather than one recorded call: every taggable
//! resource this service hands out (a config of each of the eight capability types, a
//! mission profile, a dataflow endpoint group, a contact, an ephemeris and an agent),
//! crossed with
//! writes differing along the dimensions a tag write varies along — one key, several keys
//! at once, a key already held, the same assignment repeated, an empty value, the
//! documented tag character set and sixteen keys in one call. Concurrency is its own
//! dimension: writers that each assign their own key must ALL persist.
//!
//! A tag write is also held to the platform tagging rules every create of this service
//! enforces (`crate::tag_rules`): the suites below run that suite's own legal and illegal
//! tables through `TagResource`, and add the one bound only an assignment can reach — the
//! per-resource tag ceiling judged against the MERGED map.
//!
//! The harness, the signed `restJson1` requests and the response reader are
//! [`crate::scheduling_loop`]'s, reused rather than rebuilt.
//!
//! The table of taggable resources and the tag plumbing below are `pub(crate)`:
//! [`crate::tag_removal`] asserts the REMOVAL half of the same surface against the same
//! resources, through the same front door, rather than building a second copy of them.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    MAX_TAGS_PER_RESOURCE, TAGS_PARAMETER,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::dataflow_endpoint_groups::security_details;
use crate::ephemeris_catalog::tle_inline;
use crate::scheduling_loop::{
    antenna_uplink_config_data, config_data_table, create_config, create_mission_profile, harness,
    harness_serving, harness_with_engine, provision_uplink_approval, send, Reply,
    ABSENT_CONFIG_ARN, ABSENT_MISSION_PROFILE_ARN, ABSENT_UUID, REGION, STATION,
};
use crate::tag_rules::{illegal_tag_maps, legal_tag_maps, tag_map};

/// `arn` as the ONE `{resourceArn}` path segment of `/tags/{resourceArn}`.
///
/// The label is NOT greedy (`smithy_query(query="operation", operation="TagResource")` —
/// `POST /tags/{resourceArn}`), so an SDK percent-encodes the ARN's `/` separators into
/// the single segment and the server percent-decodes the segment back.
pub(crate) fn label(arn: &str) -> String {
    arn.replace('%', "%25").replace('/', "%2F")
}

/// `POST /tags/{resourceArn}` — `TagResource`, assigning `tags`.
pub(crate) async fn tag_resource(
    router: &MultiServiceRouter,
    arn: &str,
    tags: &BTreeMap<String, String>,
) -> Reply {
    send(
        router,
        "POST",
        &format!("/tags/{}", label(arn)),
        Some(json!({"tags": tags})),
    )
    .await
}

/// `POST /tags/{resourceArn}` with `tags` carried verbatim as `body`, for a row that is
/// sending a map the typed helper above cannot spell (or no map at all).
async fn tag_resource_body(
    router: &MultiServiceRouter,
    arn: &str,
    body: Option<&Value>,
) -> Reply {
    let payload = match body {
        Some(tags) => json!({"tags": tags}),
        None => json!({}),
    };
    send(
        router,
        "POST",
        &format!("/tags/{}", label(arn)),
        Some(payload),
    )
    .await
}

/// `GET /tags/{resourceArn}` — `ListTagsForResource`.
///
/// `pub(crate)` so a suite asserting what the tag read of a DELETED resource answers, and
/// [`crate::tag_removal`] asserting what is left after a removal, reach the tag reader
/// through this one helper.
pub(crate) async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Reply {
    send(router, "GET", &format!("/tags/{}", label(arn)), None).await
}

/// The `tags` member of a reply, as a map comparable whatever order it serialized in.
pub(crate) fn reported_tags(case: &str, reply: &Reply) -> BTreeMap<String, String> {
    reply
        .field("tags")
        .as_object()
        .unwrap_or_else(|| panic!("case {case}: tags must be a map, got {}", reply.body))
        .iter()
        .map(|(key, value)| {
            let text = value
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: tag {key} must be a string, got {value}")
                })
                .to_string();
            (key.clone(), text)
        })
        .collect()
}

/// A tag map as a request body carries it.
pub(crate) fn tags_of(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// One `TagResource` call of the write table.
struct Write {
    label: &'static str,
    tags: BTreeMap<String, String>,
}

/// The writes every taggable resource is put through, in order, each one asserted against
/// the merge the test computes for itself.
///
/// The rows differ along what the rule generalises over: how many keys a call carries,
/// whether the resource already holds them, and what the key and the value may spell.
fn writes() -> Vec<Write> {
    vec![
        Write {
            label: "one key the resource does not hold",
            tags: tags_of(&[("mission", "aqua")]),
        },
        Write {
            label: "several new keys at once",
            tags: tags_of(&[("stage", "flight"), ("owner", "ops")]),
        },
        Write {
            label: "a key the resource already holds, overwritten",
            tags: tags_of(&[("mission", "terra")]),
        },
        Write {
            label: "the same assignment again",
            tags: tags_of(&[("mission", "terra")]),
        },
        Write {
            label: "an empty value",
            tags: tags_of(&[("note", "")]),
        },
        // AWS documents a tag key and value over letters, digits, spaces and the
        // characters `+ - = . _ : / @`. This model constrains neither
        // (`smithy_query(query="shape", shape="TagsMap")` — a `String` key and a `String`
        // value, no length and no pattern), so the row spans that character set.
        Write {
            label: "the documented tag character set",
            tags: tags_of(&[("cost center:dept/1 +=._-@", "a b+c=d.e_f:g/h@i")]),
        },
        Write {
            label: "sixteen keys in one call",
            tags: (0..16)
                .map(|index| (format!("bulk-{index:02}"), format!("value-{index:02}")))
                .collect(),
        },
    ]
}

/// One taggable resource of this service, as the suites below address it.
pub(crate) struct Taggable {
    pub(crate) label: String,
    /// The ARN the tag operations address it by.
    pub(crate) arn: String,
    /// A read of the RESOURCE that reports its `tags`, where this service models one, so a
    /// tag write is asserted to land in the one map the resource's own read reports.
    /// `None` for an agent, whose `GetAgentConfiguration` answers `agentId` and
    /// `taskingDocument` only (`smithy_query(query="operation",
    /// operation="GetAgentConfiguration")`).
    pub(crate) own_read: Option<String>,
    /// The tags the resource was CREATED with — what a tag read reports before any
    /// `TagResource` call.
    pub(crate) created_with: BTreeMap<String, String>,
}

/// A `RegisterAgent` request: the required members of `AgentDetails` and `DiscoveryData`,
/// and the cpu-core member the service requires one of
/// (`smithy_query(query="shape", shape="AgentDetails,DiscoveryData")`).
fn agent_registration(tags: &BTreeMap<String, String>) -> Value {
    json!({
        "agentDetails": {
            "agentVersion": "1.4.0",
            "instanceId": "i-0123456789abcdef0",
            "instanceType": "m5.large",
            "agentCpuCores": [0, 1],
            "componentVersions": [{"componentType": "Agent", "versions": ["1.4.0"]}],
        },
        "discoveryData": {
            "publicIpAddresses": ["52.0.0.9"],
            "privateIpAddresses": ["10.0.0.9"],
            "capabilityArns": [format!(
                "arn:aws:groundstation:{REGION}:{}:capability/1-antenna",
                engine::emulator_account_id()
            )],
        },
        "tags": tags,
    })
}

/// A `CreateDataflowEndpointGroup` request holding one dataflow endpoint.
fn endpoint_group_request(tags: &BTreeMap<String, String>) -> Value {
    json!({
        "endpointDetails": [{
            "securityDetails": security_details(),
            "endpoint": {
                "name": "tagging-endpoint",
                "address": {"name": "172.31.0.10", "port": 55888},
                "mtu": 1500,
            },
        }],
        "tags": tags,
    })
}

/// A contact's ARN: no response of this service carries one (`ReserveContact` answers
/// `contactId` and `versionId` — `smithy_query(query="operation",
/// operation="ReserveContact")`), so it is composed the way the model addresses the
/// `Contact` resource, `contact/<contactId>`.
pub(crate) fn contact_arn(contact_id: &str) -> String {
    format!(
        "arn:aws:groundstation:{REGION}:{}:contact/{contact_id}",
        engine::emulator_account_id()
    )
}

/// An agent's ARN, `agent/<agentId>`, for the same reason as [`contact_arn`].
pub(crate) fn agent_arn(agent_id: &str) -> String {
    format!(
        "arn:aws:groundstation:{REGION}:{}:agent/{agent_id}",
        engine::emulator_account_id()
    )
}

/// An ephemeris's ARN, `ephemeris/<ephemerisId>`, for the same reason as [`contact_arn`].
fn ephemeris_arn(ephemeris_id: &str) -> String {
    format!(
        "arn:aws:groundstation:{REGION}:{}:ephemeris/{ephemeris_id}",
        engine::emulator_account_id()
    )
}

/// Reserve a contact against a fresh mission profile and answer its id.
async fn reserve_contact(
    router: &MultiServiceRouter,
    name: &str,
    tags: &BTreeMap<String, String>,
) -> String {
    let profile_arn = create_mission_profile(router, name, 180, Some(120), Some(60)).await;
    let start = 1_800_000_000_i64;
    let reserved = send(
        router,
        "POST",
        "/contact",
        Some(json!({
            "missionProfileArn": profile_arn,
            "groundStation": STATION,
            "startTime": start,
            "endTime": start + 600,
            "tags": tags,
        })),
    )
    .await;
    assert_eq!(
        reserved.status, 200,
        "case {name}: expected ReserveContact 200, got {} {}",
        reserved.status, reserved.body
    );
    reserved.string("contactId")
}

/// Every taggable resource this service hands out, created on `router`.
///
/// The config rows span every `ConfigTypeData` member (source:
/// [`crate::scheduling_loop::config_data_table`], built from `smithy_query(query="shape",
/// shape="ConfigTypeData")`), and alternate rows are created WITH tags and with none, so
/// the merge is asserted both onto an existing map and onto an empty one.
pub(crate) async fn taggable_resources(router: &MultiServiceRouter) -> Vec<Taggable> {
    let mut rows: Vec<Taggable> = Vec::new();
    let created = tags_of(&[("stage", "created")]);
    let empty: BTreeMap<String, String> = BTreeMap::new();

    // `uplinkEchoConfig` references an antenna-uplink config, so that one exists first.
    let (_, _, antenna_uplink_arn) = create_config(
        router,
        "tagging echo source",
        json!({"antennaUplinkConfig": antenna_uplink_config_data()}),
        None,
    )
    .await;

    for (index, (config_type, config_data)) in config_data_table(&antenna_uplink_arn)
        .into_iter()
        .enumerate()
    {
        let with_tags = index % 2 == 0;
        let (config_id, _, config_arn) = create_config(
            router,
            &format!("tagging {config_type}"),
            config_data,
            with_tags.then(|| json!({"stage": "created"})),
        )
        .await;
        rows.push(Taggable {
            label: format!("a {config_type} config"),
            arn: config_arn,
            own_read: Some(format!("/config/{config_type}/{config_id}")),
            created_with: if with_tags {
                created.clone()
            } else {
                empty.clone()
            },
        });
    }

    let profile_arn = create_mission_profile(router, "tagging profile", 180, Some(120), None).await;
    let profile_id = profile_arn
        .rsplit('/')
        .next()
        .expect("a mission profile ARN ends in its id")
        .to_string();
    rows.push(Taggable {
        label: "a mission profile".to_string(),
        arn: profile_arn,
        own_read: Some(format!("/missionprofile/{profile_id}")),
        created_with: empty.clone(),
    });

    let group = send(
        router,
        "POST",
        "/dataflowEndpointGroup",
        Some(endpoint_group_request(&created)),
    )
    .await;
    assert_eq!(
        group.status, 200,
        "case a dataflow endpoint group: expected CreateDataflowEndpointGroup 200, got {} {}",
        group.status, group.body
    );
    let group_id = group.string("dataflowEndpointGroupId");
    let group_read = send(
        router,
        "GET",
        &format!("/dataflowEndpointGroup/{group_id}"),
        None,
    )
    .await;
    rows.push(Taggable {
        label: "a dataflow endpoint group".to_string(),
        arn: group_read.string("dataflowEndpointGroupArn"),
        own_read: Some(format!("/dataflowEndpointGroup/{group_id}")),
        created_with: created.clone(),
    });

    let contact_tags = tags_of(&[("mission", "demo")]);
    let contact_id = reserve_contact(router, "tagging contact profile", &contact_tags).await;
    rows.push(Taggable {
        label: "a contact".to_string(),
        arn: contact_arn(&contact_id),
        own_read: Some(format!("/contact/{contact_id}")),
        created_with: contact_tags,
    });

    let ephemeris = send(
        router,
        "POST",
        "/ephemeris",
        Some(json!({
            "name": "tagging ephemeris",
            "ephemeris": tle_inline(&[(1_800_000_000, 1_800_086_400)]),
            "tags": created,
        })),
    )
    .await;
    assert_eq!(
        ephemeris.status, 200,
        "case an ephemeris: expected CreateEphemeris 200, got {} {}",
        ephemeris.status, ephemeris.body
    );
    let ephemeris_id = ephemeris.string("ephemerisId");
    rows.push(Taggable {
        label: "an ephemeris".to_string(),
        arn: ephemeris_arn(&ephemeris_id),
        own_read: Some(format!("/ephemeris/{ephemeris_id}")),
        created_with: created.clone(),
    });

    let agent = send(router, "POST", "/agent", Some(agent_registration(&created))).await;
    assert_eq!(
        agent.status, 200,
        "case an agent: expected RegisterAgent 200, got {} {}",
        agent.status, agent.body
    );
    let agent_id = agent.string("agentId");
    rows.push(Taggable {
        label: "an agent".to_string(),
        arn: agent_arn(&agent_id),
        own_read: None,
        created_with: created,
    });

    rows
}

#[tokio::test]
async fn tag_resource_merges_its_entries_into_the_tags_of_the_resource_its_arn_names() {
    // The taggable resources include a reserved contact, so the account is served by the
    // station that reservation is held at.
    let router = harness_serving(STATION);

    for resource in taggable_resources(&router).await {
        let Taggable {
            label,
            arn,
            own_read,
            created_with,
        } = resource;

        // Before any tag write, a tag read reports exactly what the create stored — the
        // empty map where the create carried no tag, never an omitted member.
        let listed = list_tags(&router, &arn).await;
        assert_eq!(
            listed.status, 200,
            "case {label} (created): expected ListTagsForResource 200, got {} {}",
            listed.status, listed.body
        );
        let mut expected = created_with;
        assert_eq!(
            reported_tags(&format!("{label} (created)"), &listed),
            expected,
            "case {label} (created): a tag read must report the tags the create stored, got {}",
            listed.body
        );

        for write in writes() {
            let case = format!("{label}: {}", write.label);
            let tagged = tag_resource(&router, &arn, &write.tags).await;
            assert_eq!(
                tagged.status, 200,
                "case {case}: expected TagResource 200, got {} {}",
                tagged.status, tagged.body
            );

            // The merge the rule describes, computed here rather than read back from the
            // emulator: the request's entries over the ones the resource already held.
            expected.extend(write.tags.clone());

            let listed = list_tags(&router, &arn).await;
            assert_eq!(
                listed.status, 200,
                "case {case}: expected ListTagsForResource 200, got {} {}",
                listed.status, listed.body
            );
            assert_eq!(
                reported_tags(&case, &listed),
                expected,
                "case {case}: the assigned keys must be merged into the resource's tags, got {}",
                listed.body
            );

            // The tags a tag read reports are the resource's own: the SAME map its own
            // read answers, not a second copy beside it.
            if let Some(route) = &own_read {
                let read = send(&router, "GET", route, None).await;
                assert_eq!(
                    read.status, 200,
                    "case {case}: expected {route} 200, got {} {}",
                    read.status, read.body
                );
                assert_eq!(
                    reported_tags(&case, &read),
                    expected,
                    "case {case}: the resource's own read must report the merged tags, got {}",
                    read.body
                );
            }
        }
    }
}

#[tokio::test]
async fn a_tag_write_touches_only_the_resource_its_arn_names() {
    let router = harness_serving(STATION);
    let resources = taggable_resources(&router).await;

    // What every resource holds before the one write below.
    let mut before: Vec<(String, String, BTreeMap<String, String>)> = Vec::new();
    for resource in &resources {
        let listed = list_tags(&router, &resource.arn).await;
        let case = format!("{} (before)", resource.label);
        assert_eq!(
            listed.status, 200,
            "case {case}: expected ListTagsForResource 200, got {} {}",
            listed.status, listed.body
        );
        before.push((
            resource.label.clone(),
            resource.arn.clone(),
            reported_tags(&case, &listed),
        ));
    }

    // Each resource in turn is the ONE that gets the write; every other must be untouched.
    for (index, (target_label, target_arn, _)) in before.iter().enumerate() {
        let written = tags_of(&[("touched", "yes")]);
        let tagged = tag_resource(&router, target_arn, &written).await;
        assert_eq!(
            tagged.status, 200,
            "case {target_label}: expected TagResource 200, got {} {}",
            tagged.status, tagged.body
        );

        for (other_index, (other_label, other_arn, other_tags)) in before.iter().enumerate() {
            let case = format!("{target_label} tagged, reading {other_label}");
            let listed = list_tags(&router, other_arn).await;
            let mut expected = other_tags.clone();
            if other_index <= index {
                // Already had its own turn (or is the target): it carries the write.
                expected.extend(written.clone());
            }
            assert_eq!(
                reported_tags(&case, &listed),
                expected,
                "case {case}: a tag write must change only the resource its ARN names, got {}",
                listed.body
            );
        }
    }
}

/// How many writers the concurrency suite runs at once, and how many keys each assigns.
const CONCURRENT_WRITERS: usize = 16;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_tag_writes_all_persist() {
    let (router, engine) = harness_with_engine();
    provision_uplink_approval(&engine);
    let (_, _, config_arn) = create_config(
        &router,
        "concurrently tagged",
        json!({"trackingConfig": {"autotrack": "REQUIRED"}}),
        Some(json!({"stage": "created"})),
    )
    .await;

    // Writers that each assign their OWN key, in parallel on a multi-thread runtime: a
    // tag write is a read-modify-write of the resource's one map, so a writer that read
    // the map before another's write must not drop that write. Each also assigns the
    // SHARED key, whose surviving value is whichever writer wrote last — any of them.
    let shared = Arc::new(router.clone());
    let mut writers = Vec::with_capacity(CONCURRENT_WRITERS);
    for index in 0..CONCURRENT_WRITERS {
        let router = Arc::clone(&shared);
        let arn = config_arn.clone();
        writers.push(tokio::spawn(async move {
            let mut tags: BTreeMap<String, String> = BTreeMap::new();
            tags.insert(format!("writer-{index:02}"), format!("value-{index:02}"));
            tags.insert("shared".to_string(), format!("writer-{index:02}"));
            let tagged = tag_resource(&router, &arn, &tags).await;
            (index, tagged.status)
        }));
    }
    for writer in writers {
        let (index, status) = writer.await.expect("a tag writer task");
        assert_eq!(
            status, 200,
            "case writer-{index:02}: expected TagResource 200, got {status}"
        );
    }

    let listed = list_tags(&router, &config_arn).await;
    assert_eq!(
        listed.status, 200,
        "case concurrent: expected ListTagsForResource 200, got {} {}",
        listed.status, listed.body
    );
    let reported = reported_tags("concurrent", &listed);

    // Every writer's own key persisted, beside the tag the create stored.
    let mut expected = tags_of(&[("stage", "created")]);
    for index in 0..CONCURRENT_WRITERS {
        expected.insert(format!("writer-{index:02}"), format!("value-{index:02}"));
    }
    let shared_value = reported.get("shared").cloned();
    let mut without_shared = reported.clone();
    without_shared.remove("shared");
    assert_eq!(
        without_shared, expected,
        "case concurrent: every concurrent writer's key must persist, got {}",
        listed.body
    );
    // The contested key holds one writer's value — whichever wrote last.
    let candidates: Vec<String> = (0..CONCURRENT_WRITERS)
        .map(|index| format!("writer-{index:02}"))
        .collect();
    assert!(
        shared_value
            .as_ref()
            .is_some_and(|value| candidates.contains(value)),
        "case concurrent: the contested key must hold one writer's value, got {shared_value:?}"
    );
}

#[tokio::test]
async fn an_arn_that_names_no_resource_is_refused_by_both_tag_operations() {
    let router = harness();

    // A config created only to be DELETED: an ARN whose resource is gone resolves to
    // nothing, exactly as one that never existed does.
    let (config_id, config_type, deleted_arn) = create_config(
        &router,
        "deleted before tagging",
        json!({"trackingConfig": {"autotrack": "PREFERRED"}}),
        Some(json!({"stage": "created"})),
    )
    .await;
    let deleted = send(
        &router,
        "DELETE",
        &format!("/config/{config_type}/{config_id}"),
        None,
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "case delete first: expected DeleteConfig 200, got {} {}",
        deleted.status, deleted.body
    );

    // A live resource, so the suite also shows the refusals persisted nothing anywhere.
    let (_, _, live_arn) = create_config(
        &router,
        "untouched by the refusals",
        json!({"trackingConfig": {"autotrack": "REQUIRED"}}),
        Some(json!({"stage": "created"})),
    )
    .await;

    let account = engine::emulator_account_id();
    let absent: Vec<(&str, String)> = vec![
        ("a config that was never created", ABSENT_CONFIG_ARN.to_string()),
        (
            "a mission profile that was never created",
            ABSENT_MISSION_PROFILE_ARN.to_string(),
        ),
        ("a config that was deleted", deleted_arn),
        (
            "a contact that was never reserved",
            contact_arn(ABSENT_UUID),
        ),
        ("an agent that was never registered", agent_arn(ABSENT_UUID)),
        (
            "a dataflow endpoint group that was never created",
            format!(
                "arn:aws:groundstation:{REGION}:{account}:dataflow-endpoint-group/{ABSENT_UUID}"
            ),
        ),
        (
            "a resource of another service",
            "arn:aws:s3:::ground-station-downlink".to_string(),
        ),
    ];

    for (case, arn) in absent {
        let tagged = tag_resource(&router, &arn, &tags_of(&[("mission", "aqua")])).await;
        assert_eq!(
            tagged.status, 434,
            "case {case}: TagResource must answer 434, got {} {}",
            tagged.status, tagged.body
        );
        assert_eq!(
            tagged.error_code(),
            "NotFoundException",
            "case {case}: TagResource must answer NotFoundException, got {}",
            tagged.error_code()
        );
        let message = tagged.field("message").as_str().unwrap_or_default();
        assert_eq!(
            message, "Resource was not found",
            "case {case}: the refusal states one fixed sentence, got {message:?}"
        );

        let listed = list_tags(&router, &arn).await;
        assert_eq!(
            listed.status, 434,
            "case {case}: ListTagsForResource must answer 434, got {} {}",
            listed.status, listed.body
        );
        assert_eq!(
            listed.error_code(),
            "NotFoundException",
            "case {case}: ListTagsForResource must answer NotFoundException, got {}",
            listed.error_code()
        );
    }

    // Nothing the refusals touched: the live resource still holds only its own tag.
    let listed = list_tags(&router, &live_arn).await;
    assert_eq!(
        reported_tags("the live resource", &listed),
        tags_of(&[("stage", "created")]),
        "case the live resource: a refused tag write must persist nothing, got {}",
        listed.body
    );
}

/// A config carrying no tag, for a row that wants a resource of its own, and its ARN.
pub(crate) async fn fresh_config(router: &MultiServiceRouter, name: &str) -> String {
    let (_, _, arn) = create_config(
        router,
        name,
        json!({"trackingConfig": {"autotrack": "REQUIRED"}}),
        None,
    )
    .await;
    arn
}

#[tokio::test]
async fn an_assignment_is_held_to_the_platform_tag_rules_every_create_enforces() {
    let router = harness();

    // The rules are a property of the TAG MAP, so the tables are `crate::tag_rules`'
    // own — every published bound, one past each, both spellings of the reserved prefix
    // and a length counted in characters rather than bytes — run here through
    // `TagResource` instead of through a create.
    let target = fresh_config(&router, "tag rules target").await;
    for (row, tags) in illegal_tag_maps() {
        let case = format!("TagResource / {row}");
        let before = list_tags(&router, &target).await;
        let held = reported_tags(&case, &before);

        let refused = tag_resource_body(&router, &target, Some(&tags)).await;
        assert_eq!(
            refused.status, 431,
            "case {case:?}: expected 431, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {}",
            refused.body
        );
        assert_eq!(
            refused.field("parameterName"),
            &json!(TAGS_PARAMETER),
            "case {case:?}: the refusal must name the tags member, got {}",
            refused.body
        );

        let after = list_tags(&router, &target).await;
        assert_eq!(
            reported_tags(&case, &after),
            held,
            "case {case:?}: a refused assignment must write nothing, got {}",
            after.body
        );
    }

    // Each legal row gets its OWN resource: the ceiling row assigns as many tags as a
    // resource may carry, which only lands on a resource that holds none yet.
    for (index, (row, tags)) in legal_tag_maps().into_iter().enumerate() {
        let case = format!("TagResource / {row}");
        let arn = fresh_config(&router, &format!("tag rules accepted {index}")).await;

        // `tags` is a REQUIRED member of this operation (`smithy_query(query="operation",
        // operation="TagResource")`), so the row that omits it is refused by the model's
        // own rules before the handler, as this service words that refusal.
        let Some(tags) = tags else {
            let refused = tag_resource_body(&router, &arn, None).await;
            assert_eq!(
                refused.status, 400,
                "case {case:?}: a request without the required tags member must be refused \
                 400, got {} {}",
                refused.status, refused.body
            );
            continue;
        };

        let assigned = tag_resource_body(&router, &arn, Some(&tags)).await;
        assert_eq!(
            assigned.status, 200,
            "case {case:?}: expected TagResource 200, got {} {}",
            assigned.status, assigned.body
        );
        let listed = list_tags(&router, &arn).await;
        assert_eq!(
            listed.field("tags"),
            &tags,
            "case {case:?}: the assigned map must read back verbatim, got {}",
            listed.field("tags")
        );
    }
}

#[tokio::test]
async fn the_per_resource_tag_ceiling_is_judged_against_the_merged_map() {
    let router = harness();
    let ceiling = MAX_TAGS_PER_RESOURCE;

    // At the ceiling: a resource holding none may be assigned exactly as many tags as it
    // may carry (the bound itself, from `state::MAX_TAGS_PER_RESOURCE`).
    let full = fresh_config(&router, "at the tag ceiling").await;
    let at_ceiling = tag_map(ceiling);
    let assigned = tag_resource_body(&router, &full, Some(&at_ceiling)).await;
    assert_eq!(
        assigned.status, 200,
        "case at the ceiling: expected TagResource 200, got {} {}",
        assigned.status, assigned.body
    );
    let listed = list_tags(&router, &full).await;
    let held = reported_tags("at the ceiling", &listed);
    assert_eq!(
        held.len(),
        ceiling,
        "case at the ceiling: the resource must carry {ceiling} tags, got {}",
        held.len()
    );

    // One past it, reached only by the MERGE: the request map is legal on its own (one
    // entry), and the resource already holds the ceiling, so the assignment is refused
    // and nothing is written.
    let mut one_more = Map::new();
    one_more.insert("one-too-many".to_string(), json!("v"));
    let refused = tag_resource_body(&router, &full, Some(&Value::Object(one_more))).await;
    assert_eq!(
        refused.status, 431,
        "case one past the ceiling: expected 431, got {} {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "InvalidParameterException",
        "case one past the ceiling: expected InvalidParameterException, got {}",
        refused.body
    );
    assert_eq!(
        refused.field("parameterName"),
        &json!(TAGS_PARAMETER),
        "case one past the ceiling: the refusal must name the tags member, got {}",
        refused.body
    );
    let after = list_tags(&router, &full).await;
    assert_eq!(
        reported_tags("one past the ceiling", &after),
        held,
        "case one past the ceiling: a refused assignment must write nothing, got {}",
        after.body
    );

    // An OVERWRITE of a key the resource already holds adds nothing to the count, so a
    // resource at the ceiling still accepts it.
    let overwritten = tags_of(&[("tag-000", "rewritten")]);
    let accepted = tag_resource(&router, &full, &overwritten).await;
    assert_eq!(
        accepted.status, 200,
        "case overwrite at the ceiling: expected TagResource 200, got {} {}",
        accepted.status, accepted.body
    );
    let after = list_tags(&router, &full).await;
    let now_held = reported_tags("overwrite at the ceiling", &after);
    assert_eq!(
        now_held.len(),
        ceiling,
        "case overwrite at the ceiling: the count must not grow, got {}",
        now_held.len()
    );
    assert_eq!(
        now_held.get("tag-000").map(String::as_str),
        Some("rewritten"),
        "case overwrite at the ceiling: the value must be the one just assigned, got {:?}",
        now_held.get("tag-000")
    );

    // Reaching the ceiling exactly through a mixed call: one key already held (free) and
    // one new (the last slot).
    let nearly = fresh_config(&router, "one short of the tag ceiling").await;
    let short_of = tag_map(ceiling - 1);
    let filled = tag_resource_body(&router, &nearly, Some(&short_of)).await;
    assert_eq!(
        filled.status, 200,
        "case one short of the ceiling: expected TagResource 200, got {} {}",
        filled.status, filled.body
    );
    let mixed = tags_of(&[("tag-000", "rewritten"), ("the-last-slot", "v")]);
    let accepted = tag_resource(&router, &nearly, &mixed).await;
    assert_eq!(
        accepted.status, 200,
        "case the last slot: expected TagResource 200, got {} {}",
        accepted.status, accepted.body
    );
    let after = list_tags(&router, &nearly).await;
    assert_eq!(
        reported_tags("the last slot", &after).len(),
        ceiling,
        "case the last slot: the resource must end at the ceiling, got {}",
        after.body
    );

    // And one key past it from the same starting point: refused.
    let two_new = tags_of(&[("over-one", "v"), ("over-two", "v")]);
    let refused = tag_resource(&router, &nearly, &two_new).await;
    assert_eq!(
        refused.status, 431,
        "case two new keys past the ceiling: expected 431, got {} {}",
        refused.status, refused.body
    );
}
