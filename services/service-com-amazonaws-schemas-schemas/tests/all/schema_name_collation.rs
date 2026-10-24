// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ORDER a registry's schemas come back in, and for the
//! per-source membership of the AWS-managed catalogue.
//!
//! Two rules are under test:
//!
//! 1. a listing of schemas (`ListSchemas`, and the search that reports the same
//!    summaries) is collated by each `SchemaName`'s LETTERS AND DIGITS, case-folded,
//!    with the raw name breaking a tie — so punctuation (`.`, `-`, `@`, `_`) does not
//!    decide the order and neither does case — and `Limit`/`NextToken` page over that
//!    order, which is what makes a page's membership reproducible;
//! 2. which schemas the AWS-managed registry publishes for one event source is
//!    per-source published data: a source carries the event types published for it,
//!    which is not the same set for every source.
//!
//! Every case drives the real wire path, through the harness in
//! `registry_schema_lifecycle.rs` — nothing here touches the state struct directly.

use service_com_amazonaws_schemas_schemas::state::{
    schema_name_order_key, AWS_MANAGED_REGISTRY, CLOUD_TRAIL_EVENT_FAMILY,
};

use crate::registry_schema_lifecycle::{create_registry, schema_names, schema_uri, walk, Emulator};

/// The listing of one registry's schemas.
fn listing_uri(registry: &str) -> String {
    format!("/v1/registries/name/{registry}/schemas")
}

/// The listing of the AWS-managed registry's schemas published for `source`.
fn source_listing_uri(source: &str) -> String {
    format!(
        "{}?schemaNamePrefix={source}@",
        listing_uri(AWS_MANAGED_REGISTRY)
    )
}

/// Create a schema in `registry` under `name`, asserting it succeeded.
async fn create_schema(emulator: &Emulator, registry: &str, name: &str) {
    let created = emulator
        .call(
            "POST",
            &schema_uri(registry, name),
            Some(serde_json::json!({
                "Content": format!(
                    r##"{{"openapi":"3.0.0","info":{{"title":"{name}","version":"1"}},"paths":{{}},"components":{{"schemas":{{"AWSEvent":{{"type":"object","x-amazon-events-source":"my.events","x-amazon-events-detail-type":"Collated","properties":{{"detail":{{"$ref":"#/components/schemas/Detail"}}}}}},"Detail":{{"type":"object"}}}}}}}}"##
                ),
                "Type": "OpenApi3",
            })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "case {name:?}: expected 201 from CreateSchema, got {} {:?}",
        created.status, created.body
    );
}

/// The rows the collation cases run over, in the order a listing must serve them.
///
/// They differ along what the rule generalises over: which punctuation separates the
/// words (`.`, `-`, `@`, `_`, none), WHERE that punctuation falls relative to the
/// letters that follow it, the case of the letters, and whether a name's letters and
/// digits are a prefix of another's. The first four rows are the two orderings the
/// issue cited, with its `aws.` prefix replaced — the rule is about any schema name,
/// not about the published catalogue's names.
///
/// A byte comparison of these names would answer a different order for every
/// neighbouring pair below, which `a_byte_order_would_answer_another_order` pins.
fn collated_rows() -> Vec<String> {
    [
        // Case folded: byte-wise `A` < `a` would put `AWSAPICall...` first.
        "svc.application-autoscaling@ApplicationAutoScalingScalingActivityStateChange",
        "svc.application-autoscaling@AWSAPICallviaCloudTrail",
        // The raw name breaks a tie between two names that fold alike.
        "svc.autoscaling@AWSAPICallViaCloudTrail",
        "svc.autoscaling@AWSAPICallviaCloudTrail",
        // Punctuation does not participate: byte-wise `-` < `@` would put
        // `autoscaling-plans` first.
        "svc.autoscaling-plans@AWSAPICallviaCloudTrail",
        // Digits collate as the characters they are, and the punctuation around them
        // does not decide anything.
        "zeta-02",
        "Zeta03",
        "zeta_1",
        // A name whose letters and digits are a prefix of the next one's sorts first.
        "zeta.10a",
        "zeta1.0a1",
    ]
    .iter()
    .map(|name| (*name).to_string())
    .collect()
}

/// RULE: a listing of a registry's schemas is collated by each name's letters and
/// digits, case-folded, with the raw name breaking a tie — whatever punctuation the
/// names carry and whatever order they were created in.
#[tokio::test]
async fn a_listing_collates_schema_names_by_letters_and_digits() {
    let emulator = Emulator::new();
    let registry = "collation";
    create_registry(&emulator, registry).await;

    // Created in REVERSE of the order they must come back in, so no case could pass by
    // reporting insertion order.
    for name in collated_rows().iter().rev() {
        create_schema(&emulator, registry, name).await;
    }

    let listed = emulator.call("GET", &listing_uri(registry), None).await;
    assert_eq!(
        listed.status, 200,
        "expected 200, got {} {:?}",
        listed.status, listed.body
    );
    assert_eq!(
        schema_names(&listed),
        collated_rows(),
        "a listing must serve the registry's schemas in the collated order"
    );
}

/// The order a byte comparison of [`collated_rows`] would answer — which the collation
/// must NOT be, or the rows would prove nothing.
#[test]
fn a_byte_order_would_answer_another_order() {
    let mut by_bytes = collated_rows();
    by_bytes.sort_unstable();
    assert_ne!(
        by_bytes,
        collated_rows(),
        "the rows must be a case where a byte sort and the collation disagree"
    );
    // And the two orderings the issue cited are each a neighbouring pair the byte sort
    // inverts.
    for (first, second) in [
        (
            "svc.application-autoscaling@ApplicationAutoScalingScalingActivityStateChange",
            "svc.application-autoscaling@AWSAPICallviaCloudTrail",
        ),
        (
            "svc.autoscaling@AWSAPICallviaCloudTrail",
            "svc.autoscaling-plans@AWSAPICallviaCloudTrail",
        ),
    ] {
        assert!(
            schema_name_order_key(first) < schema_name_order_key(second),
            "case {first:?}: must collate before {second:?}"
        );
        assert!(
            first > second,
            "case {first:?}: a byte comparison must put it after {second:?}, or the \
             case proves nothing"
        );
    }
}

/// RULE (an invariant): `Limit`/`NextToken` page over the collated order — every page
/// is a contiguous run of it, and following the token to exhaustion walks the registry
/// exactly once with no repeat and no gap, for every page size.
#[tokio::test]
async fn paging_a_listing_walks_the_collated_order_exactly_once() {
    let emulator = Emulator::new();
    let registry = "paged";
    create_registry(&emulator, registry).await;
    for name in collated_rows().iter().rev() {
        create_schema(&emulator, registry, name).await;
    }
    let expected = collated_rows();

    // Rows: a page per item, page sizes that divide the total and that do not, a page
    // as large as the total, and one larger.
    for limit in [None, Some(1), Some(2), Some(5), Some(expected.len())] {
        let walked = walk(&emulator, &listing_uri(registry), limit, schema_names).await;
        assert_eq!(
            walked, expected,
            "case limit={limit:?}: paging must walk the collated order exactly once"
        );
    }

    // And one page, taken on its own, is the FIRST run of that order — a page is not
    // assembled from another order and then sorted.
    for limit in [1_usize, 2, 3] {
        let page = emulator
            .call(
                "GET",
                &format!("{}?limit={limit}", listing_uri(registry)),
                None,
            )
            .await;
        assert_eq!(
            schema_names(&page),
            expected[..limit].to_vec(),
            "case limit={limit}: the first page is the first {limit} of the collated order"
        );
    }
}

/// RULE: `SearchSchemas` reports its matches in the same collated order a listing of
/// the registry serves — the two report the same summaries of the same registry, so
/// they cannot disagree about the order, and a search pages over it too.
#[tokio::test]
async fn a_search_reports_its_matches_in_the_collated_order() {
    let emulator = Emulator::new();
    let registry = "searched";
    create_registry(&emulator, registry).await;
    for name in collated_rows().iter().rev() {
        create_schema(&emulator, registry, name).await;
    }

    // Every created schema declares the same event, so the keyword selects all of them
    // and what the case observes is the ORDER alone.
    let search = format!("{}/search?keywords=my.events", listing_uri(registry));
    for limit in [None, Some(1), Some(3)] {
        let walked = walk(&emulator, &search, limit, schema_names).await;
        assert_eq!(
            walked,
            collated_rows(),
            "case limit={limit:?}: a search must serve the collated order too"
        );
    }
}

/// RULE: the AWS-managed catalogue is served in that same collated order, so paging it
/// yields pages a client can reproduce — in particular a source's own event types
/// collate among its CloudTrail-delivered ones by their letters, and a source whose
/// name is another source's name plus punctuation does not interleave with it.
#[tokio::test]
async fn the_published_catalogue_is_served_in_the_collated_order() {
    let emulator = Emulator::new();

    // Rows: sources whose entries the collation and a byte order disagree about
    // (`aws.application-autoscaling`, whose own event type collates before its
    // CloudTrail family), a source that publishes its own event types and nothing
    // else, and sources the issue did not cite.
    for source in [
        "aws.application-autoscaling",
        "aws.autoscaling",
        "aws.access-analyzer",
        "aws.acm-pca",
        "aws.events",
        "aws.deadline",
    ] {
        let listed = walk(&emulator, &source_listing_uri(source), None, schema_names).await;
        let mut collated = listed.clone();
        collated.sort_by_key(|name| schema_name_order_key(name));
        assert_eq!(
            listed, collated,
            "case {source:?}: the catalogue must be served in the collated order"
        );
    }

    // The recorded ordering this generalises from: every `aws.autoscaling@` entry
    // precedes every `aws.autoscaling-plans@` entry, which a byte order inverts
    // because `-` < `@`.
    let both = walk(
        &emulator,
        &format!(
            "{}?schemaNamePrefix=aws.autoscaling",
            listing_uri(AWS_MANAGED_REGISTRY)
        ),
        None,
        schema_names,
    )
    .await;
    let plans_from = both
        .iter()
        .position(|name| name.starts_with("aws.autoscaling-plans@"))
        .unwrap_or_else(|| panic!("the catalogue must publish aws.autoscaling-plans, got {both:?}"));
    assert!(
        both[..plans_from]
            .iter()
            .all(|name| name.starts_with("aws.autoscaling@")),
        "every aws.autoscaling@ entry must precede the first aws.autoscaling-plans@ \
         one, got {both:?}"
    );
    assert!(
        both[plans_from..]
            .iter()
            .all(|name| name.starts_with("aws.autoscaling-plans@")),
        "the aws.autoscaling-plans@ entries must be one contiguous run, got {both:?}"
    );
}

/// What the catalogue publishes for one event source: the event types of its own, and
/// whether it carries the CloudTrail-delivered family at all.
///
/// Source: the published `aws.events` catalogue, one source at a time — which schemas a
/// source publishes is data, not a template over sources, so each row below is read off
/// the catalogue rather than derived from the source's name.
struct SourceRow {
    source: &'static str,
    /// The event types the source publishes of its own, as `<EventName>`.
    own: &'static [&'static str],
    /// Whether the catalogue publishes the whole CloudTrail-delivered family for it.
    cloud_trail_family: bool,
}

/// RULE: a source carries exactly the event types the catalogue publishes for it — its
/// own ones, plus the CloudTrail-delivered family only where the catalogue publishes
/// that family for it.
#[tokio::test]
async fn a_source_publishes_exactly_the_event_types_the_catalogue_holds_for_it() {
    let emulator = Emulator::new();

    // Rows: a source that publishes its own event types and no CloudTrail family, one
    // that publishes both, one that publishes a family of its own event types, and one
    // that publishes the CloudTrail family alone.
    let rows = [
        SourceRow {
            source: "aws.access-analyzer",
            own: &["AccessAnalyzerFinding", "UnusedAccessFindingforIAMentities"],
            cloud_trail_family: false,
        },
        SourceRow {
            source: "aws.acm-pca",
            own: &[
                "ACMPrivateCAAuditReportGeneration",
                "ACMPrivateCACRLGeneration",
                "ACMPrivateCACertificateIssuance",
                "ACMPrivateCACertificateRevocation",
                "ACMPrivateCACreation",
            ],
            cloud_trail_family: true,
        },
        SourceRow {
            source: "aws.deadline",
            own: &[
                "BudgetThresholdReached",
                "FleetSizeRecommendationChange",
                "JobLifecycleStatusChange",
                "JobRunStatusChange",
                "StepLifecycleStatusChange",
                "StepRunStatusChange",
            ],
            cloud_trail_family: true,
        },
        SourceRow {
            source: "aws.cleanrooms",
            own: &[
                "AnalysisTemplateCreated",
                "AnalysisTemplateDeleted",
                "AnalysisTemplateUpdated",
            ],
            cloud_trail_family: true,
        },
        SourceRow {
            source: "aws.application-autoscaling",
            own: &["ApplicationAutoScalingScalingActivityStateChange"],
            cloud_trail_family: true,
        },
        SourceRow {
            source: "aws.access-insights",
            own: &[],
            cloud_trail_family: true,
        },
    ];

    for row in rows {
        let published = walk(
            &emulator,
            &source_listing_uri(row.source),
            None,
            schema_names,
        )
        .await;

        let mut expected: Vec<String> = row
            .own
            .iter()
            .map(|event| format!("{}@{event}", row.source))
            .collect();
        if row.cloud_trail_family {
            expected.extend(
                CLOUD_TRAIL_EVENT_FAMILY
                    .iter()
                    .map(|(_, event)| format!("{}@{event}", row.source)),
            );
        }
        expected.sort_by_key(|name| schema_name_order_key(name));
        assert_eq!(
            published, expected,
            "case {:?}: the catalogue must publish exactly the event types held for it",
            row.source
        );

        // Every one of them is readable, so the membership is state and not a listing
        // that reports names nothing stands behind.
        for name in &published {
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

/// RULE: the `aws.events` registry publishes the event types EventBridge itself emits
/// — its API-destination and connection events and its scheduled events — beside the
/// CloudTrail-delivered family.
///
/// Source: the published `aws.events` catalogue. The connection events are the pair of
/// "started" events reporting work beginning and the state events reporting what a
/// connection reached.
#[tokio::test]
async fn the_event_bridge_source_publishes_its_own_event_types() {
    let emulator = Emulator::new();

    let mut expected: Vec<String> = [
        "APIDestinationActivated",
        "APIDestinationDeactivated",
        "ConnectionActivated",
        "ConnectionAuthorizationStarted",
        "ConnectionAuthorized",
        "ConnectionCreationStarted",
        "ConnectionDeauthorizationStarted",
        "ConnectionDeauthorized",
        "ConnectionDeletionStarted",
        "ConnectionFailedConnectivity",
        "ConnectionUpdateStarted",
        "ScheduledEvent",
        "ScheduledJson",
    ]
    .iter()
    .map(|event| format!("aws.events@{event}"))
    .collect();
    expected.extend(
        CLOUD_TRAIL_EVENT_FAMILY
            .iter()
            .map(|(_, event)| format!("aws.events@{event}")),
    );
    expected.sort_by_key(|name| schema_name_order_key(name));

    let published = walk(
        &emulator,
        &source_listing_uri("aws.events"),
        None,
        schema_names,
    )
    .await;
    assert_eq!(
        published, expected,
        "the aws.events registry must publish exactly these event types"
    );
}
