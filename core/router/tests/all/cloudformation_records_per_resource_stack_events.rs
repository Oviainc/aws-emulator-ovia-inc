// SPDX-License-Identifier: CC0-1.0

//! The PER-RESOURCE stack events a CloudFormation deployment records when the resources are
//! really created in the service that owns them.
//!
//! The rule under test: **a stack operation records a `StackEvent` for EVERY transition of
//! every resource it acts on — the `*_IN_PROGRESS` record that opens the transition and the
//! `*_COMPLETE` or `*_FAILED` one that ends it — each naming that resource's own
//! `LogicalResourceId` and `ResourceType`, so the slice of `DescribeStackEvents` filtered on
//! a declared resource's logical id is never empty; where the service that owns the resource
//! REFUSED, the terminal record is the failed status and its `ResourceStatusReason` reports
//! that service's own message, which is the only place the refusal surfaces.** A rollback
//! that removes what the failed deployment had already made records the same pair of
//! `DELETE_*` records for each resource it really deletes, and a resource the walk never
//! reached is named by no event.
//!
//! Two crates have to agree for that to hold — CloudFormation makes the provider call and
//! the owning service answers or refuses it — so the suite lives here rather than in either
//! crate.
//!
//! The router and the signed awsQuery caller are
//! [`cloudformation_provisions_s3vectors_resources`](super::cloudformation_provisions_s3vectors_resources)'s:
//! that suite already mounts this pair of services on one engine, and a second copy of that
//! harness is a second thing to drift. The `StackEvent` reader below ([`wire_events`]) is
//! this crate's own — the CloudFormation crate's `tests/all/stack_resource_events.rs` reads
//! the same shape for the owning services it can mount, and a test harness cannot be shared
//! across two test crates.
//!
//! The rows differ along what the rule generalises over: whether the deployment was carried
//! through or an owning service refused one of its resources, WHICH of the owning service's
//! name rules the refusal falls under (upper case, and shorter than the published minimum),
//! which resource of the template is the one refused (the first, or one whose dependency was
//! already created — which is the row whose rollback then has a resource to delete), how many
//! resources the template declares, both template spellings, and the Region deployed in.
//!
//! Nothing is read back out of the answer being checked: each expected logical id, type and
//! terminal status is what the row declares, and the identifier a completed resource's event
//! carries is held against the one `DescribeStackResources` reports for the same resource.

use std::time::Duration;

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

// The XML element readers this crate writes ONCE (`tests/all/query_wire.rs`): "Nothing new
// should write another one — call this module." `physical_ids_of` is that module's, which
// pairs each `<member>`'s two elements with EACH OTHER rather than zipping two
// document-ordered lists.
use super::query_wire::{elements, members_of, opt_element, physical_ids_of};
// The router and the signed CloudFormation caller of the suite that already exercises these
// two services together.
use super::cloudformation_provisions_s3vectors_resources::{build_router, cloudformation};

use service_com_amazonaws_cloudformation_cloudformation::ops::types::HANDLER_ERROR_CODE_VALUES;

/// How long a row waits for a stack to leave `*_IN_PROGRESS`, and how often it asks. Polled
/// rather than slept for a fixed number of the stack's clock-owed spans, so a row whose
/// rollback has more transitions to drain does not become flaky.
const SETTLE_DEADLINE: Duration = Duration::from_secs(10);
const SETTLE_INTERVAL: Duration = Duration::from_millis(100);

/// The type CloudFormation reports a stack's OWN events under.
const STACK_RESOURCE_TYPE: &str = "AWS::CloudFormation::Stack";

/// The two resource types these rows declare.
const VECTOR_BUCKET: &str = "AWS::S3Vectors::VectorBucket";
const VECTOR_INDEX: &str = "AWS::S3Vectors::Index";

/// One `StackEvent` as the wire reports it.
#[derive(Debug, Clone)]
struct WireEvent {
    event_id: String,
    stack_id: String,
    stack_name: String,
    logical_id: String,
    physical_id: Option<String>,
    resource_type: String,
    status: String,
    status_reason: Option<String>,
    timestamp: String,
}

/// Every event one `DescribeStackEvents` answer carries, in wire order (newest first).
///
/// A `StackEvent` holds no list of its own, so each `<member>` of the answer is one event and
/// the shared member reader is what separates them — never two document-ordered element lists
/// zipped, which would misalign the moment one event omitted a member.
fn wire_events(xml: &str) -> Vec<WireEvent> {
    members_of(xml)
        .into_iter()
        .map(|entry| {
            let one = |name: &str| elements(entry, name).into_iter().next();
            WireEvent {
                event_id: one("EventId").unwrap_or_default(),
                stack_id: one("StackId").unwrap_or_default(),
                stack_name: one("StackName").unwrap_or_default(),
                logical_id: one("LogicalResourceId").unwrap_or_default(),
                physical_id: one("PhysicalResourceId"),
                resource_type: one("ResourceType").unwrap_or_default(),
                status: one("ResourceStatus").unwrap_or_default(),
                status_reason: one("ResourceStatusReason"),
                timestamp: one("Timestamp").unwrap_or_default(),
            }
        })
        .collect()
}

/// The events a stack has recorded, newest first.
async fn events_of(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    name_or_id: &str,
) -> Vec<WireEvent> {
    let (status, body) = cloudformation(
        router,
        region,
        "DescribeStackEvents",
        &[("StackName", name_or_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStackEvents({name_or_id}), got {status} \
         with body {body}"
    );
    wire_events(&body)
}

/// The `DescribeStacks` answer for `stack_name` once its operation has left
/// `*_IN_PROGRESS`, or the last answer read when the deadline ran out.
async fn settled_stack(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    stack_name: &str,
) -> String {
    let deadline = tokio::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, described) =
            cloudformation(router, region, "DescribeStacks", &[("StackName", stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body \
             {described}"
        );
        let reached = opt_element(&described, "StackStatus").unwrap_or_default();
        if !reached.ends_with("_IN_PROGRESS") {
            return described;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "case {case:?}: {stack_name} was still {reached:?} after {SETTLE_DEADLINE:?}, so \
             nothing settled it: {described}"
        );
        tokio::time::sleep(SETTLE_INTERVAL).await;
    }
}

/// The position of `logical_id`'s event in `status`, in the newest-first list.
fn position(events: &[WireEvent], logical_id: &str, status: &str) -> Option<usize> {
    events
        .iter()
        .position(|event| event.logical_id == logical_id && event.status == status)
}

/// The OWNING service's own message a refused resource's reason quotes, having held the
/// `HandlerErrorCode` class that reason names to [`HANDLER_ERROR_CODE_VALUES`] — the
/// CloudFormation crate's own generated list of what the model's `HandlerErrorCode` enum
/// accepts, so this assertion cannot drift from the model.
fn reported_failure(case: &str, logical_id: &str, reason: &str) -> String {
    const OPENS: &str = "Resource handler returned message: \"";
    const CLASS: &str = "HandlerErrorCode: ";
    let quoted = reason
        .split_once(OPENS)
        .and_then(|(_, rest)| rest.rsplit_once('"'))
        .map(|(message, _)| message.to_string())
        .unwrap_or_default();
    let class = reason
        .split_once(CLASS)
        .map(|(_, rest)| rest.trim_end_matches(')').to_string())
        .unwrap_or_default();
    assert!(
        !quoted.is_empty(),
        "case {case:?}: expected {logical_id}'s reason to quote the owning service's own \
         message, got {reason:?}"
    );
    assert!(
        HANDLER_ERROR_CODE_VALUES.contains(&class.as_str()),
        "case {case:?}: expected {logical_id}'s reason to name a HandlerErrorCode the model \
         declares, got {class:?} in {reason:?}"
    );
    quoted
}

/// What one resource of a row must have transitioned through.
struct Transit {
    logical_id: &'static str,
    resource_type: &'static str,
    /// The status the resource's transition ended in: `CREATE_COMPLETE` for one the
    /// deployment carried through, `CREATE_FAILED` for one the owning service refused.
    terminal: &'static str,
    /// For a refused resource, a fragment of the OWNING service's own message the reason must
    /// quote — the member of its API whose value it would not accept. `None` for a resource
    /// the deployment carried through, which states no reason at all.
    quotes: Option<&'static str>,
}

/// One row of the table: a template, and what the rule says its events must then say.
struct Row {
    case: &'static str,
    /// The Region the row deploys in, which is the Region the resources are created in.
    region: &'static str,
    /// The template body, in the form the row writes it (JSON or YAML).
    template: fn() -> String,
    resources: &'static [Transit],
    /// The logical ids the walk never reached, which no event may name: a deployment that
    /// stopped at a refusal acted on nothing behind it.
    unreached: &'static [&'static str],
    /// The status the stack itself settles in.
    stack_status: &'static str,
    /// The logical ids the rollback really DELETES again, each of which owes its own
    /// `DELETE_IN_PROGRESS`/`DELETE_COMPLETE` pair.
    deleted_in_rollback: &'static [&'static str],
}

const ROWS: &[Row] = &[
    Row {
        case: "a vector bucket the owning service creates",
        region: "us-east-1",
        template: || {
            json!({
                "Resources": {
                    "Vectors": {
                        "Type": VECTOR_BUCKET,
                        "Properties": { "VectorBucketName": "events-created-vec" }
                    }
                }
            })
            .to_string()
        },
        resources: &[Transit {
            logical_id: "Vectors",
            resource_type: VECTOR_BUCKET,
            terminal: "CREATE_COMPLETE",
            quotes: None,
        }],
        unreached: &[],
        stack_status: "CREATE_COMPLETE",
        deleted_in_rollback: &[],
    },
    Row {
        // The name rule the refusal reported in the field falls under: S3 Vectors admits no
        // upper case in a vector bucket name.
        case: "an upper-case vector bucket name the owning service refuses",
        region: "us-east-2",
        template: || {
            json!({
                "Resources": {
                    "Vectors": {
                        "Type": VECTOR_BUCKET,
                        "Properties": { "VectorBucketName": "Upper_Case_Vec" }
                    }
                }
            })
            .to_string()
        },
        resources: &[Transit {
            logical_id: "Vectors",
            resource_type: VECTOR_BUCKET,
            terminal: "CREATE_FAILED",
            quotes: Some("vectorBucketName"),
        }],
        unreached: &[],
        stack_status: "ROLLBACK_COMPLETE",
        deleted_in_rollback: &[],
    },
    Row {
        // The same rule's other bound, and the other template spelling: a name shorter than
        // the published three-character minimum.
        case: "a vector bucket name under the published minimum, declared in YAML",
        region: "eu-west-1",
        template: || {
            [
                "Resources:",
                "  Store:",
                "    Type: AWS::S3Vectors::VectorBucket",
                "    Properties:",
                "      VectorBucketName: ab",
                "",
            ]
            .join("\n")
        },
        resources: &[Transit {
            logical_id: "Store",
            resource_type: VECTOR_BUCKET,
            terminal: "CREATE_FAILED",
            quotes: Some("vectorBucketName"),
        }],
        unreached: &[],
        stack_status: "ROLLBACK_COMPLETE",
        deleted_in_rollback: &[],
    },
    Row {
        // The refusal falls on the SECOND resource, whose dependency the deployment had
        // already created: both resources are recorded, and the rollback that removes the
        // bucket records its deletion too.
        case: "an index the owning service refuses, in a bucket the deployment created",
        region: "ap-northeast-1",
        template: || {
            json!({
                "Resources": {
                    "Vectors": {
                        "Type": VECTOR_BUCKET,
                        "Properties": { "VectorBucketName": "events-index-host" }
                    },
                    "Embeddings": {
                        "Type": VECTOR_INDEX,
                        "Properties": {
                            "VectorBucketArn": { "Ref": "Vectors" },
                            "IndexName": "Titan_V2",
                            "DataType": "float32",
                            "Dimension": 1024,
                            "DistanceMetric": "cosine"
                        }
                    }
                }
            })
            .to_string()
        },
        resources: &[
            Transit {
                logical_id: "Vectors",
                resource_type: VECTOR_BUCKET,
                terminal: "CREATE_COMPLETE",
                quotes: None,
            },
            Transit {
                logical_id: "Embeddings",
                resource_type: VECTOR_INDEX,
                terminal: "CREATE_FAILED",
                quotes: Some("indexName"),
            },
        ],
        unreached: &[],
        stack_status: "ROLLBACK_COMPLETE",
        deleted_in_rollback: &["Vectors"],
    },
    Row {
        // A resource declared BEHIND the one that failed: the walk never reached it, so no
        // event names it at all — the event log is what the deployment did, not what the
        // template declared.
        case: "a resource behind the one the owning service refused",
        region: "us-west-2",
        template: || {
            json!({
                "Resources": {
                    "Vectors": {
                        "Type": VECTOR_BUCKET,
                        "Properties": { "VectorBucketName": "NOT-A-VECTOR-BUCKET" }
                    },
                    "Later": {
                        "Type": VECTOR_BUCKET,
                        "DependsOn": "Vectors",
                        "Properties": { "VectorBucketName": "events-later-vec" }
                    }
                }
            })
            .to_string()
        },
        resources: &[Transit {
            logical_id: "Vectors",
            resource_type: VECTOR_BUCKET,
            terminal: "CREATE_FAILED",
            quotes: Some("vectorBucketName"),
        }],
        unreached: &["Later"],
        stack_status: "ROLLBACK_COMPLETE",
        deleted_in_rollback: &[],
    },
];

/// Every row: each resource the deployment acted on has its own opening and terminal event
/// under its own logical id, a refused resource's terminal event carries the owning service's
/// refusal, and nothing the walk never reached is named at all.
#[tokio::test]
async fn every_resource_a_stack_acts_on_records_its_own_events() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("per-resource-events-{index}");
        let template = (row.template)();

        let (status, created) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[("StackName", &stack_name), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {created}"
        );
        let stack_id = opt_element(&created, "StackId").unwrap_or_default();
        assert!(
            !stack_id.is_empty(),
            "case {case:?}: expected CreateStack to answer a StackId, got {created}"
        );

        let described = settled_stack(&router, region, case, &stack_name).await;
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some(row.stack_status),
            "case {case:?}: expected the stack to settle in {:?}, got {described}",
            row.stack_status
        );

        let events = events_of(&router, region, case, &stack_name).await;

        // The stack's own events are records of the SAME stream, and the oldest of all of
        // them is the one that opened the create.
        let oldest = events
            .last()
            .unwrap_or_else(|| panic!("case {case:?}: a created stack has events, got {events:?}"));
        assert_eq!(
            (
                oldest.logical_id.as_str(),
                oldest.resource_type.as_str(),
                oldest.status.as_str()
            ),
            (
                stack_name.as_str(),
                STACK_RESOURCE_TYPE,
                "CREATE_IN_PROGRESS"
            ),
            "case {case:?}: expected the oldest event to be the stack's own \
             CREATE_IN_PROGRESS, got {events:?}"
        );

        for resource in row.resources {
            let logical_id = resource.logical_id;
            // The assertion the whole rule is read through: the slice of the stream a client
            // filters on one declared resource's logical id is NOT empty.
            let own: Vec<&WireEvent> = events
                .iter()
                .filter(|event| event.logical_id == logical_id)
                .collect();
            assert!(
                !own.is_empty(),
                "case {case:?}: expected the events of {logical_id} not to be empty, got \
                 {events:?}"
            );

            let opened =
                position(&events, logical_id, "CREATE_IN_PROGRESS").unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected a CREATE_IN_PROGRESS event for {logical_id}, \
                         got {own:?}"
                    )
                });
            let ended = position(&events, logical_id, resource.terminal).unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a {} event for {logical_id}, got {own:?}",
                    resource.terminal
                )
            });
            // Newest first, so the transition that ENDED is reported before the one that
            // opened it.
            assert!(
                ended < opened,
                "case {case:?}: expected {logical_id}'s {} to be newer than its \
                 CREATE_IN_PROGRESS, got {ended:?} against {opened:?} in {own:?}",
                resource.terminal
            );

            let terminal = &events[ended];
            assert_eq!(
                terminal.resource_type, resource.resource_type,
                "case {case:?}: expected {logical_id}'s event to carry its declared type {:?}, \
                 got {terminal:?}",
                resource.resource_type
            );
            assert_eq!(
                (terminal.stack_name.as_str(), terminal.stack_id.as_str()),
                (stack_name.as_str(), stack_id.as_str()),
                "case {case:?}: expected {logical_id}'s event to name its stack, got \
                 {terminal:?}"
            );
            assert!(
                !terminal.timestamp.is_empty(),
                "case {case:?}: expected {logical_id}'s event to carry a Timestamp, got \
                 {terminal:?}"
            );

            match resource.quotes {
                Some(fragment) => {
                    let reason = terminal.status_reason.clone().unwrap_or_default();
                    let quoted = reported_failure(case, logical_id, &reason);
                    assert!(
                        quoted.contains(fragment),
                        "case {case:?}: expected {logical_id}'s reason to quote the owning \
                         service's own message naming {fragment:?}, got {quoted:?}"
                    );
                }
                None => {
                    assert!(
                        terminal.status_reason.is_none(),
                        "case {case:?}: a resource the deployment carried through states no \
                         failure reason, got {terminal:?}"
                    );
                    assert!(
                        terminal
                            .physical_id
                            .as_deref()
                            .is_some_and(|physical| !physical.is_empty()),
                        "case {case:?}: expected {logical_id}'s completed event to carry the \
                         identifier it was created under, got {terminal:?}"
                    );
                }
            }
        }

        for logical_id in row.unreached {
            assert!(
                !events.iter().any(|event| &event.logical_id == logical_id),
                "case {case:?}: the walk never reached {logical_id}, so no event may name it, \
                 got {events:?}"
            );
        }

        for logical_id in row.deleted_in_rollback {
            let opened =
                position(&events, logical_id, "DELETE_IN_PROGRESS").unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected the rollback's deletion of {logical_id} to be \
                         recorded as DELETE_IN_PROGRESS, got {events:?}"
                    )
                });
            let ended = position(&events, logical_id, "DELETE_COMPLETE").unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected the rollback's deletion of {logical_id} to be \
                     recorded as DELETE_COMPLETE, got {events:?}"
                )
            });
            assert!(
                ended < opened,
                "case {case:?}: expected {logical_id}'s DELETE_COMPLETE to be newer than its \
                 DELETE_IN_PROGRESS, got {ended:?} against {opened:?} in {events:?}"
            );
        }

        // Every record of the stream is one event: two never share an EventId.
        let mut ids: Vec<&str> = events.iter().map(|event| event.event_id.as_str()).collect();
        let recorded = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(
            ids.len(),
            recorded,
            "case {case:?}: expected every EventId to be distinct, got {events:?}"
        );

        // The event stream and the stack's resource view are ONE answer: the identifier a
        // resource's completed event carries is the one the stack reports for it. (A rolled
        // back stack records no resources, so this holds over the rows that completed.)
        if row.stack_status != "CREATE_COMPLETE" {
            continue;
        }
        let (status, resources) = cloudformation(
            &router,
            region,
            "DescribeStackResources",
            &[("StackName", &stack_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with \
             body {resources}"
        );
        let reported = physical_ids_of(&resources);
        for resource in row.resources {
            let Some(index) = position(&events, resource.logical_id, "CREATE_COMPLETE") else {
                continue;
            };
            assert_eq!(
                events[index].physical_id.as_deref(),
                reported.get(resource.logical_id).map(String::as_str),
                "case {case:?}: expected {}'s event to carry the identifier the stack reports \
                 for it, got {:?} against {:?}",
                resource.logical_id,
                events[index].physical_id,
                reported.get(resource.logical_id)
            );
        }
    }
}

/// A stack is addressable by its stack ID as well as by its name, and the event stream is the
/// same one either way — which is how a client reads the history of a stack whose deployment
/// failed.
#[tokio::test]
async fn the_event_stream_is_the_same_read_by_stack_id() {
    let case = "read by stack id";
    let router = build_router();
    let region = "ca-central-1";
    let stack_name = "per-resource-events-by-id";
    let template = json!({
        "Resources": {
            "Vectors": {
                "Type": VECTOR_BUCKET,
                "Properties": { "VectorBucketName": "Refused_By_Case" }
            }
        }
    })
    .to_string();

    let (status, created) = cloudformation(
        &router,
        region,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", &template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {created}"
    );
    let stack_id = opt_element(&created, "StackId").unwrap_or_default();
    assert!(
        !stack_id.is_empty(),
        "case {case:?}: expected CreateStack to answer a StackId, got {created}"
    );
    settled_stack(&router, region, case, stack_name).await;

    let by_name = events_of(&router, region, case, stack_name).await;
    let by_id = events_of(&router, region, case, &stack_id).await;
    // The same RECORDS, not merely the same transitions: an EventId names one event.
    let records = |events: &[WireEvent]| -> Vec<String> {
        events
            .iter()
            .map(|event| format!("{}:{}:{}", event.event_id, event.logical_id, event.status))
            .collect()
    };
    assert_eq!(
        records(&by_name),
        records(&by_id),
        "case {case:?}: expected the same stream either way, got {by_name:?} against {by_id:?}"
    );
    assert!(
        position(&by_id, "Vectors", "CREATE_FAILED").is_some(),
        "case {case:?}: expected the refused resource's CREATE_FAILED event, got {by_id:?}"
    );
}
