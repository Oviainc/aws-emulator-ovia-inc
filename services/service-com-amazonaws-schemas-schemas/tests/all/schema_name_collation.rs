// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ORDER a registry's schemas come back in, and for the
//! per-source membership of the AWS-managed catalogue.
//!
//! Two rules are under test:
//!
//! 1. a listing of schemas (`ListSchemas`, and the search that reports the same
//!    summaries) is served in the CODEPOINT order of the `SchemaName`s — every
//!    character of a name decides its place, so an upper-case letter precedes the
//!    lower-case one at the same position and punctuation participates like any other
//!    character — and `Limit`/`NextToken` page over that order, which is what makes a
//!    page's membership reproducible;
//! 2. which schemas the AWS-managed registry publishes for one event source is
//!    per-source published data: a source carries the event types published for it,
//!    which is not the same set for every source.
//!
//! Every case drives the real wire path, through the harness in
//! `registry_schema_lifecycle.rs` — nothing here touches the state struct directly.

use service_com_amazonaws_schemas_schemas::state::{
    AWS_MANAGED_REGISTRY, CLOUD_TRAIL_EVENT_FAMILY,
};

use crate::registry_schema_lifecycle::{
    create_registry, schema_names, schema_uri, wait_for_keyword_index, walk, Emulator,
};

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
/// They differ along what the rule generalises over: the CASE of a letter at the
/// position that decides a pair (two names alike but for one letter's case, and two
/// whose differing letters are of different cases), which punctuation separates the
/// words (`.`, `-`, `@`, `_`, none), WHERE that punctuation falls relative to the
/// letters that follow it, and whether one name is a prefix of another. The first rows
/// are the two orderings the issue cited, with its `aws.` prefix replaced — the rule is
/// about any schema name, not about the published catalogue's names.
///
/// A case-folded collation of these names answers a different order, which
/// `a_case_folded_order_would_answer_another_order` pins.
fn collated_rows() -> Vec<String> {
    [
        // An upper-case first letter precedes every lower-case one.
        "Zeta03",
        // `CRLGeneration` before `CertificateIssuance` before `Creation`: `R` < `e` <
        // `r`, which a case-folded comparator answers the other way round.
        "svc.acm-pca@ACMPrivateCAAuditReportGeneration",
        "svc.acm-pca@ACMPrivateCACRLGeneration",
        "svc.acm-pca@ACMPrivateCACertificateIssuance",
        "svc.acm-pca@ACMPrivateCACreation",
        // Punctuation participates: `-` < `@`, so the longer source name comes first.
        "svc.autoscaling-plans@AWSAPICallviaCloudTrail",
        // Two names alike but for one letter's case: the upper-case one precedes.
        "svc.autoscaling@AWSAPICallViaCloudTrail",
        "svc.autoscaling@AWSAPICallviaCloudTrail",
        // Digits and punctuation collate as the characters they are: `-` < `.` < `1` <
        // `_`.
        "zeta-02",
        // A name that is a prefix of the next one's start sorts first.
        "zeta.10a",
        "zeta1.0a1",
        "zeta_1",
    ]
    .iter()
    .map(|name| (*name).to_string())
    .collect()
}

/// RULE: a listing of a registry's schemas is served in the codepoint order of its
/// schema names — whatever punctuation and casing the names carry and whatever order
/// they were created in.
#[tokio::test]
async fn a_listing_serves_schema_names_in_codepoint_order() {
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
        "a listing must serve the registry's schemas in codepoint order"
    );
}

/// The order a CASE-FOLDED collation of [`collated_rows`] would answer — which the
/// order a listing serves must not be, or the rows would prove nothing.
#[test]
fn a_case_folded_order_would_answer_another_order() {
    // The key a comparator that folds case and drops punctuation collates under —
    // computed here, because it is what the rows must NOT be ordered by.
    let folded = |name: &str| -> String {
        name.chars()
            .filter(|character| character.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    };

    let mut by_folding = collated_rows();
    by_folding.sort_by_key(|name| folded(name));
    assert_ne!(
        by_folding,
        collated_rows(),
        "the rows must be a case where a case-folded order and the served order disagree"
    );

    // And the orderings the issue cited are each a neighbouring pair of rows the folded
    // order inverts: one decided by a letter's case, one by punctuation, one by the case
    // of a name's first letter.
    for (first, second) in [
        (
            "svc.acm-pca@ACMPrivateCACRLGeneration",
            "svc.acm-pca@ACMPrivateCACertificateIssuance",
        ),
        (
            "svc.autoscaling-plans@AWSAPICallviaCloudTrail",
            "svc.autoscaling@AWSAPICallviaCloudTrail",
        ),
        ("Zeta03", "svc.acm-pca@ACMPrivateCAAuditReportGeneration"),
    ] {
        assert!(
            first < second,
            "case {first:?}: must be served before {second:?}"
        );
        assert!(
            folded(first) > folded(second),
            "case {first:?}: a case-folded comparator must put it after {second:?}, or \
             the case proves nothing"
        );
    }
}

/// RULE (an invariant): `Limit`/`NextToken` page over the codepoint order — every page
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
            "case limit={limit:?}: paging must walk the codepoint order exactly once"
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
            "case limit={limit}: the first page is the first {limit} of the codepoint order"
        );
    }
}

/// RULE: `SearchSchemas` reports its matches in the same codepoint order a listing of
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
    // and what the case observes is the ORDER alone — once the registry's keyword index
    // has caught up with the writes, which is what makes the keyword select at all.
    wait_for_keyword_index();
    let search = format!("{}/search?keywords=my.events", listing_uri(registry));
    for limit in [None, Some(1), Some(3)] {
        let walked = walk(&emulator, &search, limit, schema_names).await;
        assert_eq!(
            walked,
            collated_rows(),
            "case limit={limit:?}: a search must serve the codepoint order too"
        );
    }
}

/// RULE: the AWS-managed catalogue is served in that same codepoint order, so paging it
/// yields pages a client can reproduce — in particular two spellings of one event type
/// that differ in a letter's case are served upper-case first, and a source whose name
/// is another source's name plus punctuation does not interleave with it.
#[tokio::test]
async fn the_published_catalogue_is_served_in_codepoint_order() {
    let emulator = Emulator::new();

    // Rows: a source publishing its own event types among its CloudTrail-delivered
    // ones, a source whose entries a case-folded order would place differently
    // (`aws.acm-pca`), a source that publishes its own event types and nothing else,
    // and sources the issue did not cite.
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
        collated.sort();
        assert_eq!(
            listed, collated,
            "case {source:?}: the catalogue must be served in codepoint order"
        );
    }

    // The recorded ordering this generalises from: the capital-`V` spelling of the
    // API-call event precedes the lower-case one, and every `aws.autoscaling-plans@`
    // entry precedes every `aws.autoscaling@` entry, because `-` < `@`.
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
    let own_from = both
        .iter()
        .position(|name| name.starts_with("aws.autoscaling@"))
        .unwrap_or_else(|| panic!("the catalogue must publish aws.autoscaling, got {both:?}"));
    assert!(
        both[..own_from]
            .iter()
            .all(|name| name.starts_with("aws.autoscaling-plans@")),
        "every aws.autoscaling-plans@ entry must precede the first aws.autoscaling@ \
         one, got {both:?}"
    );
    assert!(
        both[own_from..]
            .iter()
            .all(|name| name.starts_with("aws.autoscaling@")),
        "the aws.autoscaling@ entries must be one contiguous run, got {both:?}"
    );
    let position = |name: &str| {
        both.iter()
            .position(|listed| listed == name)
            .unwrap_or_else(|| panic!("the catalogue must publish {name:?}, got {both:?}"))
    };
    assert!(
        position("aws.autoscaling@AWSAPICallViaCloudTrail")
            < position("aws.autoscaling@AWSAPICallviaCloudTrail"),
        "the capital-V spelling must be served before the lower-case one, got {both:?}"
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
        expected.sort();
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
    expected.sort();

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
