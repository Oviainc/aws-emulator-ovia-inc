// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the INVENTORY of event sources the AWS-managed
//! `aws.events` catalogue publishes.
//!
//! The rule under test: the service-owned catalogue publishes an entry for every event
//! source AWS publishes events for — its own event types, plus the CloudTrail-delivered
//! family where that source carries it — so a client paging the registry reads every
//! published name, and the entries of two sources follow one another in the order the
//! registry collates names in rather than one source's entries standing where another's
//! belong. A source the catalogue does not hold is not merely one absent schema: it
//! moves every later entry of the walk onto an offset that is not its own.
//!
//! The rows below are the sources whose entries the published catalogue carries and the
//! event types it publishes for each of them, read off the catalogue one source at a
//! time; which event types a source publishes is per-source data, not a template over
//! sources.
//!
//! Every case drives the real wire path, through the harness in
//! `registry_schema_lifecycle.rs` — nothing here touches the state struct directly.

use serde_json::json;
use service_com_amazonaws_schemas_schemas::state::{
    AWS_MANAGED_REGISTRY, CLOUD_TRAIL_EVENT_FAMILY, MAX_PAGE_SIZE,
};

use crate::registry_schema_lifecycle::{schema_names, schema_uri, versions_uri, walk, Emulator};

/// The listing of the AWS-managed registry's schemas.
fn catalogue_uri() -> String {
    format!("/v1/registries/name/{AWS_MANAGED_REGISTRY}/schemas")
}

/// The listing of the catalogue's entries published for one event source.
fn source_listing_uri(source: &str) -> String {
    format!("{}?schemaNamePrefix={source}@", catalogue_uri())
}

/// What the catalogue publishes for one event source.
struct SourceRow {
    source: &'static str,
    /// The event types published for it as `<EventName>`, beside the
    /// CloudTrail-delivered family where it carries that family.
    own: &'static [&'static str],
    /// Whether the catalogue publishes the whole CloudTrail-delivered family for it.
    cloud_trail_family: bool,
}

/// The rows the inventory cases run over.
///
/// Source: the published `aws.events` catalogue, one source at a time (the names a walk
/// of the registry reads back). They differ along what the rule generalises over: a
/// source carrying the CloudTrail-delivered family alone, one carrying that family
/// beside an event type of its own, sources publishing only their own event types (ten
/// of them, seven, five, two and one), a source whose namespace is a prefix of another
/// source's namespace, and event names that run their words together.
fn source_rows() -> Vec<SourceRow> {
    vec![
        SourceRow {
            source: "aws.b2bi",
            own: &["AcknowledgementCompleted"],
            cloud_trail_family: true,
        },
        SourceRow {
            source: "aws.chime-sdk-media-pipelines",
            own: &[],
            cloud_trail_family: true,
        },
        SourceRow {
            source: "aws.chime-sdk-voice",
            own: &[],
            cloud_trail_family: true,
        },
        SourceRow {
            source: "aws.ec2fleet",
            own: &[
                "EC2FleetError",
                "EC2FleetInformation",
                "EC2FleetInstanceChange",
                "EC2FleetSpotInstanceRequestChange",
                "EC2FleetStateChange",
                "EC2SpotFleetError",
                "EC2SpotFleetInformation",
                "EC2SpotFleetInstanceChange",
                "EC2SpotFleetSpotInstanceRequestChange",
                "EC2SpotFleetStateChange",
            ],
            cloud_trail_family: false,
        },
        SourceRow {
            source: "aws.epfs",
            own: &[
                "NodeIdentificationJobStateChanged",
                "PowerFlowJobStateChanged",
                "PowerFlowNodeJobStateChanged",
                "SessionStateChanged",
                "SubsystemAnalysisJobStateChanged",
            ],
            cloud_trail_family: false,
        },
        SourceRow {
            source: "aws.factstore",
            own: &["NewCommitPushed"],
            cloud_trail_family: false,
        },
        SourceRow {
            source: "aws.finspace-api",
            own: &[],
            cloud_trail_family: true,
        },
        SourceRow {
            source: "aws.glue-autodebug",
            own: &[
                "AutoVersionJobStateChange",
                "AutodebugmetadatakeysharedfromOrca",
                "Autoversionjobruncompleted",
                "Autoversionjobversionupdated",
                "DataQualityEvaluationResultsAvailable",
                "DataQualityTaskStateChange",
                "EntityMeteringEvent",
            ],
            cloud_trail_family: false,
        },
        SourceRow {
            source: "aws.goshawk",
            own: &[
                "CodeArtifactPackageVersionStateChange",
                "GoshawkRepositoryStateChange",
            ],
            cloud_trail_family: false,
        },
        SourceRow {
            source: "aws.health-agent",
            own: &[],
            cloud_trail_family: true,
        },
        SourceRow {
            source: "aws.health",
            own: &["AWSHealthAbuseEvent", "AWSHealthEvent"],
            cloud_trail_family: true,
        },
        SourceRow {
            source: "aws.inspector-scan",
            own: &[],
            cloud_trail_family: true,
        },
        SourceRow {
            source: "aws.iotsecuredtunneling",
            own: &[],
            cloud_trail_family: true,
        },
    ]
}

/// Every `<source>@<EventName>` the catalogue publishes for one row, in the order a
/// listing serves them (the codepoint order of the names, which
/// `schema_name_collation.rs` owns).
fn published_names(row: &SourceRow) -> Vec<String> {
    let mut names: Vec<String> = row
        .own
        .iter()
        .map(|event| format!("{}@{event}", row.source))
        .collect();
    if row.cloud_trail_family {
        names.extend(
            CLOUD_TRAIL_EVENT_FAMILY
                .iter()
                .map(|(_, event)| format!("{}@{event}", row.source)),
        );
    }
    names.sort();
    names
}

/// RULE: the catalogue publishes an entry for every event source AWS publishes events
/// for — exactly the event types held for that source, and the CloudTrail-delivered
/// family where the source carries it — and every listed name stands behind a readable
/// schema rather than a name nothing backs.
#[tokio::test]
async fn the_catalogue_publishes_every_event_source_it_holds_entries_for() {
    let emulator = Emulator::new();

    for row in source_rows() {
        let listed = walk(&emulator, &source_listing_uri(row.source), None, schema_names).await;
        assert_eq!(
            listed,
            published_names(&row),
            "case {:?}: the catalogue must publish exactly the event types held for it, \
             got {listed:?}",
            row.source
        );

        for name in &listed {
            let described = emulator
                .call("GET", &schema_uri(AWS_MANAGED_REGISTRY, name), None)
                .await;
            assert_eq!(
                described.status, 200,
                "case {name:?}: expected 200 from DescribeSchema, got {} {:?}",
                described.status, described.body
            );
            assert_eq!(
                described.str_at("SchemaName"),
                *name,
                "case {name:?}: the read answers the schema the listing named"
            );
        }
    }
}

/// RULE (an invariant): because the catalogue holds every published source, a walk of
/// the registry puts each source's entries at the offset its membership implies — every
/// entry of a source that collates earlier precedes every entry of the next source, so
/// no source's entries stand where another's belong, whatever page size the walk asks
/// for.
#[tokio::test]
async fn one_source_block_follows_another_at_the_offset_membership_implies() {
    let emulator = Emulator::new();

    // Rows: pairs of namespaces that collate next to one another, where one is a prefix
    // of the other (`aws.health-agent@` before `aws.health@`, because `-` collates
    // before `@`) and where they merely share a stem. Each pair is a place the walk
    // would skew if the earlier source were missing.
    let neighbours = [
        ("aws.b2bi@", "aws.backup@"),
        ("aws.chime-sdk-media-pipelines@", "aws.chime-sdk-voice@"),
        ("aws.chime-sdk-voice@", "aws.chime@"),
        ("aws.ec2fleet@", "aws.ecr@"),
        ("aws.epfs@", "aws.es@"),
        ("aws.factstore@", "aws.finspace-api@"),
        ("aws.finspace-api@", "aws.finspace@"),
        ("aws.glue-autodebug@", "aws.glue@"),
        ("aws.goshawk@", "aws.grafana@"),
        ("aws.health-agent@", "aws.health@"),
        ("aws.inspector-scan@", "aws.inspector2@"),
        ("aws.iotsecuredtunneling@", "aws.iotsitewise@"),
    ];

    // Page sizes: the default, a size the catalogue is no multiple of, and the largest
    // page the service serves. The offsets a walk reads must not depend on it.
    for limit in [None, Some(64), Some(MAX_PAGE_SIZE)] {
        let walked = walk(&emulator, &catalogue_uri(), limit, schema_names).await;
        let mut sorted = walked.clone();
        sorted.sort();
        assert_eq!(
            walked, sorted,
            "case limit={limit:?}: a walk reads the catalogue in the order it collates \
             names in"
        );

        for (earlier, later) in neighbours {
            let first = walked.iter().position(|name| name.starts_with(earlier));
            let last = walked.iter().rposition(|name| name.starts_with(earlier));
            let next = walked.iter().position(|name| name.starts_with(later));
            assert!(
                first.is_some() && last.is_some() && next.is_some(),
                "case limit={limit:?} {earlier:?} before {later:?}: both sources must be \
                 published, got first={first:?} last={last:?} next={next:?}"
            );
            let (Some(first), Some(last), Some(next)) = (first, last, next) else {
                continue;
            };
            assert!(
                last < next,
                "case limit={limit:?} {earlier:?} before {later:?}: every entry of the \
                 earlier source precedes the later source's, got {first}..={last} and \
                 {next}"
            );
            // The earlier source's entries are one contiguous block, so the offset of
            // everything after it is shifted by exactly its membership.
            let block = &walked[first..=last];
            assert!(
                block.iter().all(|name| name.starts_with(earlier)),
                "case limit={limit:?} {earlier:?}: a source's entries are contiguous, \
                 got {block:?}"
            );
        }
    }
}

/// One entry of a newly published source and what the catalogue publishes for it.
struct EntryRow {
    schema: &'static str,
    /// The number of versions the catalogue publishes for it.
    version_count: u64,
    /// The publication instant it reports, where the catalogue publishes one of its
    /// own.
    published: Option<&'static str>,
}

/// RULE: `ListSchemas` reports each entry's own published `VersionCount` — and every
/// version it claims is readable — and, for an entry the catalogue publishes a
/// publication instant for, that instant rather than one derived from the entry's name.
#[tokio::test]
async fn a_published_entry_of_a_new_source_reports_its_own_count_and_instant() {
    let emulator = Emulator::new();

    // Rows: counts 1, 2 and 4; entries with a published instant and entries with none;
    // one entry of a source that also carries the CloudTrail-delivered family and one
    // of a source that carries only its own event types.
    let rows = [
        EntryRow {
            schema: "aws.b2bi@AcknowledgementCompleted",
            version_count: 2,
            published: None,
        },
        EntryRow {
            schema: "aws.b2bi@AWSAPICallviaCloudTrail",
            version_count: 1,
            published: None,
        },
        EntryRow {
            schema: "aws.health@AWSHealthAbuseEvent",
            version_count: 4,
            published: Some("2026-03-12T21:12:29Z"),
        },
        EntryRow {
            schema: "aws.ec2fleet@EC2FleetStateChange",
            version_count: 1,
            published: Some("2020-12-17T18:43:39Z"),
        },
        EntryRow {
            schema: "aws.goshawk@GoshawkRepositoryStateChange",
            version_count: 1,
            published: None,
        },
    ];

    for row in rows {
        let listed = emulator
            .call(
                "GET",
                &format!("{}?schemaNamePrefix={}", catalogue_uri(), row.schema),
                None,
            )
            .await;
        assert_eq!(
            listed.status, 200,
            "case {:?}: expected 200 from ListSchemas, got {} {:?}",
            row.schema, listed.status, listed.body
        );
        let summary = listed.body["Schemas"]
            .as_array()
            .unwrap_or_else(|| panic!("case {:?}: expected a Schemas array", row.schema))
            .iter()
            .find(|item| item["SchemaName"].as_str() == Some(row.schema))
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the catalogue must publish it, got {:?}",
                    row.schema, listed.body
                )
            })
            .clone();
        assert_eq!(
            summary["VersionCount"],
            json!(row.version_count),
            "case {:?}: the summary must report the published version count, got \
             {summary:?}",
            row.schema
        );
        if let Some(published) = row.published {
            assert_eq!(
                summary["LastModified"],
                json!(published),
                "case {:?}: the summary must report the published instant, got {summary:?}",
                row.schema
            );
        }

        let versions = emulator
            .call("GET", &versions_uri(AWS_MANAGED_REGISTRY, row.schema), None)
            .await;
        let held = versions.body["SchemaVersions"]
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected SchemaVersions in {:?}",
                    row.schema, versions.body
                )
            })
            .len();
        assert_eq!(
            u64::try_from(held).unwrap_or(u64::MAX),
            row.version_count,
            "case {:?}: the store must hold every version the count claims",
            row.schema
        );
    }
}
