// SPDX-License-Identifier: CC0-1.0

//! What Config reports about the resources it has discovered, end to end over the
//! wire, and what recording one puts into state.
//!
//! Four rules are pinned here, each over a TABLE of inputs rather than one recorded
//! case:
//!
//! 1. `GetDiscoveredResourceCounts` groups the discovered-resource inventory by
//!    resource type and reports one count per type, ordered by the number of resources
//!    descending, with `totalDiscoveredResources` the number of resources it counted
//!    over every reported type — narrowed to the `resourceTypes` the request named;
//! 2. the page it serves holds at most `limit` counts (100 when the request specifies
//!    0), and the `nextToken` it returns walks the remaining counts exactly once;
//! 3. a resource type contributes a count only while Config is RECORDING it: nothing
//!    at all while no configuration recorder is recording, and otherwise only what the
//!    recording recorder's strategy covers;
//! 4. `PutResourceConfig` records the configuration state of a resource type the
//!    CloudFormation registry holds a registration for while a recorder is running,
//!    refusing anything else with the modelled error.
//!
//! The harness (`Emulator`, `with_recorder_and_channel`) is the one
//! `config_setup_and_rule_loop` already builds; every case drives the emulator the way
//! a client does, through the `MultiServiceRouter`.

use serde_json::{json, Value};

use crate::config_connectors::refusal;
use crate::config_setup_and_rule_loop::{recorder_request, with_recorder_and_channel, Emulator};

/// The name Config assigns a configuration recorder created without one
/// (`ConfigurationRecorder$name`: "Config automatically assigns the name of
/// \"default\" ... if you do not specify a name at creation time").
const RECORDER: &str = "default";

/// Resource types whose configuration state Config records: entries of the
/// CloudFormation registry, as `smithy_query(query="shape", shape="ResourceType")`
/// enumerates the public ones — `PutResourceConfig` records a type the registry holds a
/// registration for and refuses anything else. Spelled in ascending order, because the
/// counts of two types holding equally many resources are reported in resource-type
/// order.
const RECORDED_TYPES: [&str; 5] = [
    "AWS::DynamoDB::Table",
    "AWS::EC2::Volume",
    "AWS::IAM::Role",
    "AWS::KMS::Key",
    "AWS::Lambda::Function",
];

/// Another resource type of the same registry, which no row records: used inside a
/// recording group, to pin what a recorder does and does not record.
const NATIVE_TYPE: &str = "AWS::S3::Bucket";

/// The largest `limit` the operation accepts
/// (`GetDiscoveredResourceCountsRequest$limit`, `@range(min: 0, max: 100)`: "You
/// cannot specify a number greater than 100").
const MAX_LIMIT: i64 = 100;

/// The page size the operation uses when the request specifies none
/// (`GetDiscoveredResourceCounts`: "By default, Config lists 100 ResourceCount objects
/// on each page").
const DEFAULT_PAGE_SIZE: usize = 100;

/// The counts a row of a table expects, as `(resourceType, count)` pairs in the order
/// the operation must report them.
type ExpectedCounts = &'static [(&'static str, i64)];

/// One row of the grouping table: the case name, the inventory to record (a resource
/// type and how many resources of it), and the counts expected for it.
type InventoryCase = (
    &'static str,
    &'static [(&'static str, usize)],
    ExpectedCounts,
);

/// One row of the `resourceTypes` table: the case name, the resource types the request
/// asks for, and the counts expected for them.
type RequestedTypesCase = (&'static str, &'static [&'static str], ExpectedCounts);

/// What a row of the recording table changes about the account's configuration
/// recorder before the counts are read.
enum RecorderChange {
    /// Stop the configuration recorder.
    Stop,
    /// Replace the recorder's recording group.
    Group(Value),
}

/// One row of the recording table: the case name, the change to the recorder, and the
/// counts expected afterwards.
type RecorderCase = (&'static str, RecorderChange, ExpectedCounts);

/// One row of the identity table: the case name and the resources to record, each a
/// resource id and the custom name it is recorded with, if any.
type RecordedResourcesCase = (
    &'static str,
    &'static [(&'static str, Option<&'static str>)],
);

/// The `PutResourceConfig` body recording `configuration` for one resource.
fn resource_config(resource_type: &str, resource_id: &str, configuration: &str) -> Value {
    json!({
        "ResourceType": resource_type,
        "SchemaVersionId": "00000000-0000-0000-0000-000000000001",
        "ResourceId": resource_id,
        "ResourceName": resource_id,
        "Configuration": configuration,
    })
}

/// Turn the account's configuration recorder on: Config discovers resources only while
/// a recorder is recording.
async fn start_recording(emulator: &Emulator) {
    emulator
        .ok(
            "StartConfigurationRecorder",
            &json!({ "ConfigurationRecorderName": RECORDER }),
        )
        .await;
}

/// Record one resource of `resource_type` into Config's inventory.
async fn record(emulator: &Emulator, resource_type: &str, resource_id: &str) {
    emulator
        .ok(
            "PutResourceConfig",
            &resource_config(
                resource_type,
                resource_id,
                &format!("{{\"name\":\"{resource_id}\"}}"),
            ),
        )
        .await;
}

/// An emulator with a recording configuration recorder and `inventory` recorded: each
/// entry is a resource type and how many resources of that type to record.
async fn with_inventory(inventory: &[(&str, usize)]) -> Emulator {
    let emulator = Emulator::new();
    with_recorder_and_channel(&emulator).await;
    start_recording(&emulator).await;
    for (resource_type, resources) in inventory {
        for index in 0..*resources {
            record(&emulator, resource_type, &format!("{resource_type}-{index:03}")).await;
        }
    }
    emulator
}

/// The `(resourceType, count)` pairs a response reports, in the order it reported
/// them. `pub(crate)` so the suites that drive the discovered-resource inventory
/// through other operations read a count the one way.
pub(crate) fn counts_of(case: &str, body: &Value) -> Vec<(String, i64)> {
    body["resourceCounts"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected resourceCounts, got {body}"))
        .iter()
        .map(|entry| {
            let resource_type = entry["resourceType"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a resourceType, got {entry}"))
                .to_string();
            let count = entry["count"]
                .as_i64()
                .unwrap_or_else(|| panic!("case {case:?}: expected a count, got {entry}"));
            (resource_type, count)
        })
        .collect()
}

/// `totalDiscoveredResources` of a response.
fn total_of(case: &str, body: &Value) -> i64 {
    body["totalDiscoveredResources"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case:?}: expected totalDiscoveredResources, got {body}"))
}

/// Every count the operation reports for `resource_types` (all of them when empty), in
/// one call that asks for no `limit`.
async fn counts(
    case: &str,
    emulator: &Emulator,
    resource_types: &[&str],
) -> (Vec<(String, i64)>, i64) {
    let mut request = json!({});
    if !resource_types.is_empty() {
        request["resourceTypes"] = json!(resource_types);
    }
    let body = emulator.ok("GetDiscoveredResourceCounts", &request).await;
    (counts_of(case, &body), total_of(case, &body))
}

/// What paging with one `limit` produced: every entry in the order the pages reported
/// them, the size of each page, and the total each page carried.
struct Walk {
    entries: Vec<(String, i64)>,
    page_sizes: Vec<usize>,
    totals: Vec<i64>,
}

/// Page through `GetDiscoveredResourceCounts` with `limit`, following every
/// `nextToken` it hands back.
async fn walk(case: &str, emulator: &Emulator, limit: i64) -> Walk {
    let mut walked = Walk {
        entries: Vec::new(),
        page_sizes: Vec::new(),
        totals: Vec::new(),
    };
    let mut token: Option<String> = None;
    // A bound, so a token that failed to advance fails the case instead of hanging.
    for _ in 0..32 {
        let mut request = json!({ "limit": limit });
        if let Some(token) = token.as_deref() {
            request["nextToken"] = json!(token);
        }
        let body = emulator.ok("GetDiscoveredResourceCounts", &request).await;
        let page = counts_of(case, &body);
        walked.page_sizes.push(page.len());
        walked.totals.push(total_of(case, &body));
        walked.entries.extend(page);
        token = body["nextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return walked;
        }
    }
    panic!("case {case:?}: paging with limit {limit} did not terminate within 32 pages");
}

#[tokio::test]
async fn the_counts_group_the_recorded_inventory_by_resource_type() {
    // Rows differ along what the rule generalises over: how many resource types the
    // inventory holds, how many resources each type has, and whether two types hold
    // equally many.
    let cases: [InventoryCase; 6] = [
        ("nothing recorded at all", &[], &[]),
        (
            "one resource of one type",
            &[(RECORDED_TYPES[0], 1)],
            &[(RECORDED_TYPES[0], 1)],
        ),
        (
            "three types with distinct counts",
            &[(RECORDED_TYPES[0], 3), (RECORDED_TYPES[1], 1), (RECORDED_TYPES[2], 2)],
            &[
                (RECORDED_TYPES[0], 3),
                (RECORDED_TYPES[2], 2),
                (RECORDED_TYPES[1], 1),
            ],
        ),
        (
            "types with equally many resources, ordered by resource type",
            &[(RECORDED_TYPES[2], 2), (RECORDED_TYPES[0], 2), (RECORDED_TYPES[1], 2)],
            &[
                (RECORDED_TYPES[0], 2),
                (RECORDED_TYPES[1], 2),
                (RECORDED_TYPES[2], 2),
            ],
        ),
        (
            "one busy type beside one with a single resource",
            &[(RECORDED_TYPES[4], 7), (RECORDED_TYPES[3], 1)],
            &[(RECORDED_TYPES[4], 7), (RECORDED_TYPES[3], 1)],
        ),
        (
            "five types, one resource more each",
            &[
                (RECORDED_TYPES[0], 1),
                (RECORDED_TYPES[1], 2),
                (RECORDED_TYPES[2], 3),
                (RECORDED_TYPES[3], 4),
                (RECORDED_TYPES[4], 5),
            ],
            &[
                (RECORDED_TYPES[4], 5),
                (RECORDED_TYPES[3], 4),
                (RECORDED_TYPES[2], 3),
                (RECORDED_TYPES[1], 2),
                (RECORDED_TYPES[0], 1),
            ],
        ),
    ];

    for (case, inventory, expected) in cases {
        let emulator = with_inventory(inventory).await;
        let expected: Vec<(String, i64)> = expected
            .iter()
            .map(|(resource_type, count)| (resource_type.to_string(), *count))
            .collect();
        let expected_total: i64 = expected.iter().map(|(_, count)| *count).sum();

        let (reported, total) = counts(case, &emulator, &[]).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the counts {expected:?} in descending order, got \
             {reported:?}"
        );
        assert_eq!(
            total, expected_total,
            "case {case:?}: expected totalDiscoveredResources {expected_total}, got {total}"
        );

        // A report must not change what the next caller sees.
        let (again, total_again) = counts(case, &emulator, &[]).await;
        assert_eq!(
            (again, total_again),
            (reported, total),
            "case {case:?}: a second GetDiscoveredResourceCounts must report the same counts"
        );
    }
}

#[tokio::test]
async fn requested_resource_types_narrow_the_counts_and_the_total() {
    // One inventory, read through a table of `resourceTypes` requests: none at all, a
    // single type, a subset, a type nothing was recorded of, and a mixture of both.
    let inventory = [
        (RECORDED_TYPES[0], 3),
        (RECORDED_TYPES[1], 1),
        (RECORDED_TYPES[2], 2),
    ];
    let emulator = with_inventory(&inventory).await;

    let cases: [RequestedTypesCase; 5] = [
        (
            "no resourceTypes at all",
            &[],
            &[
                (RECORDED_TYPES[0], 3),
                (RECORDED_TYPES[2], 2),
                (RECORDED_TYPES[1], 1),
            ],
        ),
        (
            "a single recorded type",
            &[RECORDED_TYPES[0]],
            &[(RECORDED_TYPES[0], 3)],
        ),
        (
            "two of the three recorded types",
            &[RECORDED_TYPES[1], RECORDED_TYPES[2]],
            &[(RECORDED_TYPES[2], 2), (RECORDED_TYPES[1], 1)],
        ),
        (
            "a type nothing was recorded of",
            &[RECORDED_TYPES[3]],
            &[],
        ),
        (
            "a recorded type beside one nothing was recorded of",
            &[RECORDED_TYPES[3], RECORDED_TYPES[1]],
            &[(RECORDED_TYPES[1], 1)],
        ),
    ];

    for (case, requested, expected) in cases {
        let expected: Vec<(String, i64)> = expected
            .iter()
            .map(|(resource_type, count)| (resource_type.to_string(), *count))
            .collect();
        let expected_total: i64 = expected.iter().map(|(_, count)| *count).sum();

        let (reported, total) = counts(case, &emulator, requested).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the counts {expected:?}, got {reported:?}"
        );
        assert_eq!(
            total, expected_total,
            "case {case:?}: expected totalDiscoveredResources {expected_total} over the \
             requested types, got {total}"
        );
    }
}

#[tokio::test]
async fn the_limit_pages_the_counts_and_the_next_token_walks_them_once() {
    // Five resource types with distinct counts, read at every page size from one to
    // past the number of entries, plus both published bounds of `limit`: 0 (the
    // default page size) and 100.
    let inventory = [
        (RECORDED_TYPES[0], 5),
        (RECORDED_TYPES[1], 4),
        (RECORDED_TYPES[2], 3),
        (RECORDED_TYPES[3], 2),
        (RECORDED_TYPES[4], 1),
    ];
    let emulator = with_inventory(&inventory).await;
    let entries = inventory.len();
    let expected_total: i64 = inventory.iter().map(|(_, count)| *count as i64).sum();
    let expected: Vec<(String, i64)> = inventory
        .iter()
        .map(|(resource_type, count)| (resource_type.to_string(), *count as i64))
        .collect();

    for limit in [1_i64, 2, 3, 4, 5, 6, MAX_LIMIT, 0] {
        let case = format!("limit {limit}");
        let page_size = if limit == 0 {
            DEFAULT_PAGE_SIZE
        } else {
            limit as usize
        };
        let walked = walk(&case, &emulator, limit).await;

        assert_eq!(
            walked.entries, expected,
            "case {case:?}: paging must report every count exactly once in the same \
             order, got {:?}",
            walked.entries
        );
        let expected_pages = entries.div_ceil(page_size).max(1);
        assert_eq!(
            walked.page_sizes.len(),
            expected_pages,
            "case {case:?}: expected {expected_pages} page(s) of at most {page_size} \
             counts, got pages of {:?}",
            walked.page_sizes
        );
        assert!(
            walked.page_sizes.iter().all(|size| *size <= page_size),
            "case {case:?}: no page may carry more than {page_size} counts, got {:?}",
            walked.page_sizes
        );
        assert!(
            walked.totals.iter().all(|total| *total == expected_total),
            "case {case:?}: every page must report the total {expected_total} over all \
             counted resources, got {:?}",
            walked.totals
        );
    }
}

#[tokio::test]
async fn a_limit_past_the_published_range_and_a_token_this_service_did_not_issue_are_refused() {
    let emulator = with_inventory(&[(RECORDED_TYPES[0], 2)]).await;

    // One past each published bound of `limit` (`@range(min: 0, max: 100)`).
    for limit in [MAX_LIMIT + 1, -1] {
        let case = format!("limit {limit}");
        let (status, code) = emulator
            .refused("GetDiscoveredResourceCounts", &json!({ "limit": limit }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a limit outside 0..{MAX_LIMIT} must be refused with HTTP 400, \
             got {status} with {code}"
        );
        assert_eq!(
            code, "ValidationException",
            "case {case:?}: expected the boundary's ValidationException for a limit \
             outside the modelled range, got {code}"
        );
    }

    // A token this operation never issued is not a page: it is malformed.
    for token in ["not-a-token", "", "MTAwfA=="] {
        let case = format!("nextToken {token:?}");
        let (status, code) = emulator
            .refused(
                "GetDiscoveredResourceCounts",
                &json!({ "nextToken": token }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a malformed nextToken must be refused with HTTP 400, got \
             {status} with {code}"
        );
        assert_eq!(
            code, "InvalidNextTokenException",
            "case {case:?}: expected InvalidNextTokenException, got {code}"
        );
    }

    // And both bounds themselves are accepted.
    for limit in [0_i64, MAX_LIMIT] {
        let case = format!("limit {limit}");
        let body = emulator
            .ok("GetDiscoveredResourceCounts", &json!({ "limit": limit }))
            .await;
        assert_eq!(
            total_of(&case, &body),
            2,
            "case {case:?}: a limit on the published bound must be served, got {body}"
        );
    }
}

#[tokio::test]
async fn a_resource_type_is_counted_only_while_config_is_recording_it() {
    // Rows differ along what the recording recorder covers: the recorder turned off,
    // the default group (all supported types), a group that enumerates one other type,
    // and a group that excludes one other type.
    let cases: [RecorderCase; 4] = [
        ("the recorder is turned off", RecorderChange::Stop, &[]),
        (
            "the recorder records all supported resource types",
            RecorderChange::Group(json!({ "allSupported": true })),
            &[(RECORDED_TYPES[0], 2), (RECORDED_TYPES[1], 1)],
        ),
        (
            "the recorder records only another resource type",
            RecorderChange::Group(json!({ "resourceTypes": [NATIVE_TYPE] })),
            &[],
        ),
        (
            "the recorder excludes another resource type",
            RecorderChange::Group(json!({
                "exclusionByResourceTypes": { "resourceTypes": [NATIVE_TYPE] },
                "recordingStrategy": { "useOnly": "EXCLUSION_BY_RESOURCE_TYPES" },
            })),
            &[(RECORDED_TYPES[0], 2), (RECORDED_TYPES[1], 1)],
        ),
    ];

    for (case, change, expected) in cases {
        let emulator = with_inventory(&[(RECORDED_TYPES[0], 2), (RECORDED_TYPES[1], 1)]).await;
        match &change {
            RecorderChange::Stop => {
                emulator
                    .ok(
                        "StopConfigurationRecorder",
                        &json!({ "ConfigurationRecorderName": RECORDER }),
                    )
                    .await;
            }
            RecorderChange::Group(group) => {
                emulator
                    .ok(
                        "PutConfigurationRecorder",
                        &recorder_request(json!({
                            "name": RECORDER,
                            "roleARN": crate::config_setup_and_rule_loop::ROLE_ARN,
                            "recordingGroup": group,
                        })),
                    )
                    .await;
            }
        }

        let expected: Vec<(String, i64)> = expected
            .iter()
            .map(|(resource_type, count)| (resource_type.to_string(), *count))
            .collect();
        let expected_total: i64 = expected.iter().map(|(_, count)| *count).sum();
        let (reported, total) = counts(case, &emulator, &[]).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the counts {expected:?}, got {reported:?}"
        );
        assert_eq!(
            total, expected_total,
            "case {case:?}: expected totalDiscoveredResources {expected_total}, got {total}"
        );

        // Turning recording back on reports the inventory again: stopping a recorder
        // reports nothing, it does not discard what Config recorded.
        if matches!(change, RecorderChange::Stop) {
            start_recording(&emulator).await;
            let (restored, restored_total) = counts(case, &emulator, &[]).await;
            assert_eq!(
                (restored.len(), restored_total),
                (2, 3),
                "case {case:?}: a restarted recorder reports the inventory again, got \
                 {restored:?} totalling {restored_total}"
            );
        }
    }
}

#[tokio::test]
async fn put_resource_config_records_only_a_registered_resource_type() {
    // A configuration item is recorded only for a resource type the CloudFormation
    // registry holds a registration for. Rows differ along WHY the type named has none:
    // an organization nothing registered a type under, each spelling of an organization
    // reserved against custom types, a name that is not a registry type name at all, a
    // registry MODULE name, a misspelled AWS type, a registered type in another case,
    // and the account-level type the registry publishes no resource type for.
    let refused: [(&str, &str); 11] = [
        ("an organization nothing registered", "Acme::Network::Switch"),
        ("another unregistered organization", "MyOrg::MyService::MyResource"),
        ("the reserved organization Amazon", "Amazon::Thing::Widget"),
        ("the reserved organization AMZN", "AMZN::Thing::Widget"),
        ("the reserved organization Alexa", "Alexa::Smart::Home"),
        ("a reserved organization in lower case", "custom::Thing::Widget"),
        ("a type name of one segment", "Switch"),
        ("a type name of two segments", "Acme::Switch"),
        ("a registry module name", "Acme::Network::Switch::MODULE"),
        ("a misspelled AWS resource type", "AWS::S3::Buckets"),
        ("a registered type in another case", "aws::s3::bucket"),
    ];

    for (case, resource_type) in refused {
        let emulator = with_inventory(&[]).await;
        let (status, code, message) = refusal(
            &emulator,
            "PutResourceConfig",
            &resource_config(resource_type, "r-1", "{}"),
        )
        .await;
        assert_eq!(
            (status, code.as_str(), message.as_str()),
            (
                400,
                "ValidationException",
                format!(
                    "Cannot find registered Resource Type with name {resource_type}. Please \
                     register the Resource Type with Cloudformation and try again."
                )
                .as_str(),
            ),
            "case {case:?}: recording {resource_type:?} must be refused with the \
             registration precondition, got {status} with {code} and {message:?}"
        );
        let (reported, total) = counts(case, &emulator, &[]).await;
        assert!(
            reported.is_empty() && total == 0,
            "case {case:?}: a refused PutResourceConfig must record nothing, got \
             {reported:?} totalling {total}"
        );
    }

    // Every registered type the rows record IS accepted, so the refusals above are
    // about the registration and not about this operation refusing everything.
    for resource_type in RECORDED_TYPES {
        let emulator = with_inventory(&[]).await;
        emulator
            .ok("PutResourceConfig", &resource_config(resource_type, "r-1", "{}"))
            .await;
        let (reported, total) = counts(resource_type, &emulator, &[]).await;
        assert_eq!(
            (reported, total),
            (vec![(resource_type.to_string(), 1)], 1),
            "case {resource_type:?}: a registered resource type is recorded"
        );
    }

    // A configuration that is not valid JSON is not a configuration item
    // (`PutResourceConfigRequest$Configuration`: "The configuration object of the
    // resource in valid JSON format").
    for (case, configuration) in [
        ("an empty configuration", ""),
        ("a bare word", "switch"),
        ("a truncated object", "{\"name\":"),
    ] {
        let emulator = with_inventory(&[]).await;
        let (status, code) = emulator
            .refused(
                "PutResourceConfig",
                &resource_config(RECORDED_TYPES[0], "r-1", configuration),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: a configuration that is not valid JSON must be refused with a \
             400 ValidationException, got {status} with {code}"
        );
    }

    // Config records a configuration item only while a recorder is running.
    let no_recorder = Emulator::new();
    let (status, code) = no_recorder
        .refused(
            "PutResourceConfig",
            &resource_config(RECORDED_TYPES[0], "r-1", "{}"),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "NoRunningConfigurationRecorderException"),
        "with no configuration recorder at all, recording a resource must be refused with \
         NoRunningConfigurationRecorderException, got {status} with {code}"
    );

    let idle = Emulator::new();
    with_recorder_and_channel(&idle).await;
    let (status, code) = idle
        .refused(
            "PutResourceConfig",
            &resource_config(RECORDED_TYPES[0], "r-1", "{}"),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "NoRunningConfigurationRecorderException"),
        "with a recorder that has never been started, recording a resource must be refused \
         with NoRunningConfigurationRecorderException, got {status} with {code}"
    );
}

#[tokio::test]
async fn resources_recorded_under_different_names_are_counted_by_their_identity() {
    // A resource's identity in Config is its (resourceType, resourceId) pair, so the
    // custom name it was recorded with changes no count. Rows differ along how many
    // resources of one type are recorded and which of them carry a name.
    let cases: [RecordedResourcesCase; 4] = [
        ("one resource with no name", &[("switch-1", None)]),
        ("one named resource", &[("switch-1", Some("edge-switch"))]),
        (
            "two resources sharing one name",
            &[
                ("switch-1", Some("edge-switch")),
                ("switch-2", Some("edge-switch")),
            ],
        ),
        (
            "three resources, two of them named",
            &[
                ("switch-1", Some("edge-switch")),
                ("switch-2", None),
                ("switch-3", Some("core-switch")),
            ],
        ),
    ];

    for (case, resources) in cases {
        let emulator = with_inventory(&[]).await;
        for (resource_id, resource_name) in resources {
            let mut request = resource_config(RECORDED_TYPES[0], resource_id, "{\"port\":1}");
            match resource_name {
                Some(name) => request["ResourceName"] = json!(name),
                None => {
                    request
                        .as_object_mut()
                        .expect("the request body is an object")
                        .remove("ResourceName");
                }
            }
            emulator.ok("PutResourceConfig", &request).await;
        }

        let recorded = resources.len() as i64;
        let (counted, total) = counts(case, &emulator, &[]).await;
        assert_eq!(
            (counted, total),
            (vec![(RECORDED_TYPES[0].to_string(), recorded)], recorded),
            "case {case:?}: expected {recorded} resource(s) of one type counted by their \
             (resourceType, resourceId) identity"
        );
    }
}

#[tokio::test]
async fn re_recording_a_resource_replaces_its_configuration_rather_than_counting_twice() {
    let emulator = with_inventory(&[]).await;
    let case = "the same resource recorded three times";

    for revision in 0..3 {
        emulator
            .ok(
                "PutResourceConfig",
                &resource_config(
                    RECORDED_TYPES[0],
                    "switch-1",
                    &format!("{{\"revision\":{revision}}}"),
                ),
            )
            .await;
        let (reported, total) = counts(case, &emulator, &[]).await;
        assert_eq!(
            reported,
            vec![(RECORDED_TYPES[0].to_string(), 1)],
            "case {case:?}: revision {revision} must still be one resource of one type, got \
             {reported:?}"
        );
        assert_eq!(
            total, 1,
            "case {case:?}: revision {revision} must still total one resource, got {total}"
        );
    }

    // A different resource id of the same type is a second resource.
    record(&emulator, RECORDED_TYPES[0], "switch-2").await;
    let (reported, total) = counts(case, &emulator, &[]).await;
    assert_eq!(
        (reported.clone(), total),
        (vec![(RECORDED_TYPES[0].to_string(), 2)], 2),
        "case {case:?}: a second resource id must be counted, got {reported:?} totalling \
         {total}"
    );
}
