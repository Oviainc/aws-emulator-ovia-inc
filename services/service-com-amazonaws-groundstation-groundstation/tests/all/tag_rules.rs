// SPDX-License-Identifier: CC0-1.0

//! The platform tagging rules every create of this service enforces, end to end over
//! HTTP through the same front door as the rest of this target's suites (the harness,
//! signed `restJson1` requests and the response reader come from
//! [`crate::scheduling_loop`]).
//!
//! One RULE, asserted over a table rather than one recorded request: a create carrying a
//! `tags` map that breaks the AWS tagging rules is refused with the modelled
//! `InvalidParameterException` (HTTP 431) naming the `tags` member, and NOTHING is
//! stored — a key in the reserved `aws:` namespace (in any letter case), a key outside
//! 1–128 characters, a value past 256 characters, or more than 50 tags on one resource.
//! Its mirror: a map inside every one of those bounds is created and reads back verbatim
//! through the resource's own Get/Describe.
//!
//! The rule is a property of the TAG MAP, not of the resource, so both tables run
//! against every create path of this service that takes tags — `CreateConfig`,
//! `CreateMissionProfile`, `CreateDataflowEndpointGroup` and `ReserveContact` — and the
//! rows vary what the rule generalises over: which reserved spelling, key length, value
//! length, tag count, and whether a length is counted in characters or bytes.
//!
//! The bounds themselves are the constants in `state` (each citing its source);
//! nothing here hardcodes a number the rule is stated with.

use serde_json::{json, Map, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    service_state, MAX_TAGS_PER_RESOURCE, MAX_TAG_VALUE_CHARS, RESERVED_TAG_KEY_PREFIX,
    TAGS_PARAMETER, TAG_KEY_CHARS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::dataflow_endpoint_groups::security_details;
use crate::scheduling_loop::{create_tracking_config, harness_with_engine, is_uuid, send, Reply};

/// A multi-byte character, for the rows that assert a length bound is counted in
/// CHARACTERS and not in the bytes a UTF-8 encoding of them takes.
const WIDE_CHAR: char = 'é';

/// Which create path a row is sent to. The rule is the tag map's, so every row runs
/// against all of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    Config,
    MissionProfile,
    DataflowEndpointGroup,
    Contact,
}

impl Target {
    /// Every create of this service that accepts a `tags` map and is implemented
    /// (`smithy_query(query="operation", operation="CreateConfig,CreateMissionProfile,\
    /// CreateDataflowEndpointGroup,ReserveContact")` — `TagResource`, `CreateEphemeris`
    /// and `RegisterAgent` also take one but are still scaffolded 501s, each with its
    /// own open issue).
    const ALL: [Target; 4] = [
        Target::Config,
        Target::MissionProfile,
        Target::DataflowEndpointGroup,
        Target::Contact,
    ];

    /// Names the create path in every assertion.
    fn label(self) -> &'static str {
        match self {
            Target::Config => "CreateConfig",
            Target::MissionProfile => "CreateMissionProfile",
            Target::DataflowEndpointGroup => "CreateDataflowEndpointGroup",
            Target::Contact => "ReserveContact",
        }
    }
}

/// A create against `target`, carrying `tags` when the row names one, with whatever
/// prerequisite resources that path needs created first.
async fn create_tagged(
    router: &MultiServiceRouter,
    target: Target,
    name: &str,
    tags: Option<&Value>,
) -> Reply {
    let (path, mut body) = match target {
        Target::Config => (
            "/config",
            json!({
                "name": name,
                "configData": {"trackingConfig": {"autotrack": "PREFERRED"}},
            }),
        ),
        Target::MissionProfile => {
            let tracking_arn = create_tracking_config(router, &format!("{name} tracking")).await;
            (
                "/missionprofile",
                json!({
                    "name": name,
                    "minimumViableContactDurationSeconds": 180,
                    "dataflowEdges": [],
                    "trackingConfigArn": tracking_arn,
                }),
            )
        }
        Target::DataflowEndpointGroup => (
            "/dataflowEndpointGroup",
            json!({
                "endpointDetails": [{
                    "securityDetails": security_details(),
                    "endpoint": {
                        "name": "downlink",
                        "address": {"name": "172.31.0.10", "port": 55888},
                        "mtu": 1500,
                    },
                }],
            }),
        ),
        Target::Contact => {
            let profile_arn =
                crate::scheduling_loop::create_mission_profile(router, name, 180, None, None).await;
            (
                "/contact",
                json!({
                    "missionProfileArn": profile_arn,
                    "groundStation": "Ohio-1",
                    "startTime": 1_800_000_000_i64,
                    "endTime": 1_800_000_600_i64,
                }),
            )
        }
    };
    if let Some(tags) = tags {
        body["tags"] = tags.clone();
    }
    send(router, "POST", path, Some(body)).await
}

/// The resource a create answered with, read back through its own implemented reader:
/// `GetConfig`, `GetMissionProfile`, `GetDataflowEndpointGroup` and `DescribeContact`.
async fn read_back(router: &MultiServiceRouter, target: Target, created: &Reply) -> Reply {
    let route = match target {
        Target::Config => format!(
            "/config/{}/{}",
            created.string("configType"),
            created.string("configId")
        ),
        Target::MissionProfile => {
            format!("/missionprofile/{}", created.string("missionProfileId"))
        }
        Target::DataflowEndpointGroup => format!(
            "/dataflowEndpointGroup/{}",
            created.string("dataflowEndpointGroupId")
        ),
        Target::Contact => format!("/contact/{}", created.string("contactId")),
    };
    send(router, "GET", &route, None).await
}

/// The id member a create answers with on this path.
fn id_member(target: Target) -> &'static str {
    match target {
        Target::Config => "configId",
        Target::MissionProfile => "missionProfileId",
        Target::DataflowEndpointGroup => "dataflowEndpointGroupId",
        Target::Contact => "contactId",
    }
}

/// How many resources of `target`'s kind the store holds.
///
/// Read from the store rather than through a listing, because what has to be observable
/// for a refused create is that it wrote NOTHING, and the store is where that is visible
/// whatever the listing's own state.
fn stored_count(engine: &engine::InMemoryEngine, target: Target) -> usize {
    let state = service_state(engine).expect("this service's state store");
    let guard = state.lock();
    match target {
        Target::Config => guard.configs.len(),
        Target::MissionProfile => guard.mission_profiles.len(),
        Target::DataflowEndpointGroup => guard.dataflow_endpoint_groups.len(),
        Target::Contact => guard.contacts.len(),
    }
}

/// `count` distinct legal tags: `tag-00` … , each with a short value.
fn tag_map(count: usize) -> Value {
    let mut map = Map::new();
    for index in 0..count {
        map.insert(format!("tag-{index:03}"), json!("v"));
    }
    Value::Object(map)
}

/// A single-entry tag map.
fn one_tag(key: &str, value: &str) -> Value {
    json!({key: value})
}

/// A key of `chars` repeats of `fill`.
fn sized(fill: char, chars: usize) -> String {
    std::iter::repeat_n(fill, chars).collect()
}

/// The tag maps the rules refuse, each row differing in WHAT it breaks.
fn illegal_tag_maps() -> Vec<(String, Value)> {
    let (min_key, max_key) = TAG_KEY_CHARS;
    vec![
        (
            "a key in the reserved namespace".to_string(),
            one_tag(
                &format!("{RESERVED_TAG_KEY_PREFIX}cloudformation:stack-name"),
                "demo",
            ),
        ),
        (
            "the reserved prefix as the whole key".to_string(),
            one_tag(RESERVED_TAG_KEY_PREFIX, "x"),
        ),
        (
            "the reserved prefix spelled in upper case".to_string(),
            one_tag(
                &format!("{}owner", RESERVED_TAG_KEY_PREFIX.to_uppercase()),
                "team",
            ),
        ),
        (
            "the reserved prefix spelled in mixed case".to_string(),
            one_tag("Aws:createdBy", "cloudformation"),
        ),
        (
            "a reserved key beside a legal one".to_string(),
            json!({"mission": "demo", "aws:createdBy": "cloudformation"}),
        ),
        (
            format!("an empty key, below the floor of {min_key}"),
            one_tag("", "value"),
        ),
        (
            format!("a key of {} characters, one past the ceiling", max_key + 1),
            one_tag(&sized('k', max_key + 1), "value"),
        ),
        (
            "a key far past the ceiling".to_string(),
            one_tag(&sized('k', 300), "value"),
        ),
        (
            format!(
                "a key of {} multi-byte characters, one past the ceiling",
                max_key + 1
            ),
            one_tag(&sized(WIDE_CHAR, max_key + 1), "value"),
        ),
        (
            format!(
                "a value of {} characters, one past the ceiling",
                MAX_TAG_VALUE_CHARS + 1
            ),
            one_tag("size", &sized('v', MAX_TAG_VALUE_CHARS + 1)),
        ),
        (
            "a value far past the ceiling".to_string(),
            one_tag("size", &sized('v', 400)),
        ),
        (
            format!(
                "{} tags, one past the ceiling",
                MAX_TAGS_PER_RESOURCE + 1
            ),
            tag_map(MAX_TAGS_PER_RESOURCE + 1),
        ),
        (
            "far more tags than the ceiling".to_string(),
            tag_map(MAX_TAGS_PER_RESOURCE + 10),
        ),
    ]
}

/// The tag maps the rules allow, including every published bound and the near misses of
/// the reserved prefix. `None` is a request with no `tags` member at all.
fn legal_tag_maps() -> Vec<(String, Option<Value>)> {
    let (min_key, max_key) = TAG_KEY_CHARS;
    vec![
        ("no tags member at all".to_string(), None),
        (
            "an empty tag map".to_string(),
            Some(Value::Object(Map::new())),
        ),
        (
            format!("a key of {min_key} character, the floor"),
            Some(one_tag(&sized('k', min_key), "v")),
        ),
        (
            format!("a key of {max_key} characters, the ceiling"),
            Some(one_tag(&sized('k', max_key), "v")),
        ),
        (
            format!("a key of {max_key} multi-byte characters, the ceiling"),
            Some(one_tag(&sized(WIDE_CHAR, max_key), "v")),
        ),
        (
            format!("a value of {MAX_TAG_VALUE_CHARS} characters, the ceiling"),
            Some(one_tag("size", &sized('v', MAX_TAG_VALUE_CHARS))),
        ),
        (
            "an empty value".to_string(),
            Some(one_tag("empty-value", "")),
        ),
        (
            format!("{MAX_TAGS_PER_RESOURCE} tags, the ceiling"),
            Some(tag_map(MAX_TAGS_PER_RESOURCE)),
        ),
        (
            "a key that CONTAINS the reserved prefix without starting with it".to_string(),
            Some(json!({"x-aws:owner": "team", "awsome": "yes", "not-aws:either": "1"})),
        ),
    ]
}

#[tokio::test]
async fn an_illegal_tag_map_is_refused_on_every_create_path_and_stores_nothing() {
    let (router, engine) = harness_with_engine();

    for target in Target::ALL {
        for (index, (row, tags)) in illegal_tag_maps().into_iter().enumerate() {
            let case = format!("{} / {row}", target.label());
            let before = stored_count(&engine, target);
            let name = format!("tag-refusal-{index}");
            let reply = create_tagged(&router, target, &name, Some(&tags)).await;

            assert_eq!(
                reply.status, 431,
                "case {case:?}: expected 431, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                "InvalidParameterException",
                "case {case:?}: expected InvalidParameterException, got {}",
                reply.body
            );
            assert_eq!(
                reply.field("parameterName"),
                &json!(TAGS_PARAMETER),
                "case {case:?}: the refusal must name the tags member, got {}",
                reply.body
            );
            assert!(
                reply
                    .body
                    .get("message")
                    .and_then(Value::as_str)
                    .is_some_and(|message| !message.is_empty()),
                "case {case:?}: the refusal must carry a message, got {}",
                reply.body
            );
            assert!(
                reply.body.get(id_member(target)).is_none(),
                "case {case:?}: a refused create must mint no id, got {}",
                reply.body
            );
            assert_eq!(
                stored_count(&engine, target),
                before,
                "case {case:?}: a refused create must store nothing"
            );
        }
    }
}

#[tokio::test]
async fn a_legal_tag_map_is_created_and_reads_back_verbatim_on_every_create_path() {
    let (router, engine) = harness_with_engine();

    for target in Target::ALL {
        for (index, (row, tags)) in legal_tag_maps().into_iter().enumerate() {
            let case = format!("{} / {row}", target.label());
            let before = stored_count(&engine, target);
            let name = format!("tag-accepted-{index}");
            let created = create_tagged(&router, target, &name, tags.as_ref()).await;

            assert_eq!(
                created.status, 200,
                "case {case:?}: expected 200, got {} {}",
                created.status, created.body
            );
            let id = created.string(id_member(target));
            assert!(
                is_uuid(&id),
                "case {case:?}: the minted id must be a lowercase uuid, got {id:?}"
            );
            assert_eq!(
                stored_count(&engine, target),
                before + 1,
                "case {case:?}: an accepted create must store the resource"
            );

            let read = read_back(&router, target, &created).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: expected the read-back to be 200, got {} {}",
                read.status, read.body
            );
            // A resource carrying no tag reads back as the empty map, never an omitted
            // member (`state::response_tags`), so an omitted `tags` and an empty one
            // read back alike.
            let expected = tags.unwrap_or_else(|| Value::Object(Map::new()));
            assert_eq!(
                read.field("tags"),
                &expected,
                "case {case:?}: the resource must read back the tags it was created \
                 with, got {}",
                read.field("tags")
            );
        }
    }
}
