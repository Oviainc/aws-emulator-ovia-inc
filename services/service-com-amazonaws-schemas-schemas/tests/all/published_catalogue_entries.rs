// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for WHICH entries the AWS-managed `aws.events` catalogue
//! publishes and what each of them reports about itself.
//!
//! Three rules are under test, each about one entry of the catalogue rather than about
//! the catalogue as a whole:
//!
//! 1. MEMBERSHIP is per-source published data: a source carries the event types the
//!    catalogue publishes for it, and where AWS has published ONE event type under more
//!    than one `detail-type` spelling (a legacy name and a current one, differing in a
//!    letter's case) the catalogue holds the entry each spelling yields, each readable
//!    under its own name;
//! 2. `SchemaSummary$VersionCount` ("The number of versions available for the schema",
//!    `smithy_query(query="shape",shape="SchemaSummary")`) is the count the catalogue
//!    publishes for THAT entry — entries of one source carry different counts, and two
//!    names that differ only in a letter's case carry their own counts — and every
//!    version it claims is readable;
//! 3. `SchemaSummary$LastModified` ("The date and time that schema was modified") is the
//!    PUBLISHED schema's own instant: an entry the catalogue publishes a historical
//!    publication instant for reports exactly that instant, the same in every store and
//!    across every read, and it is no function of the entry's name.
//!
//! Every case drives the real wire path, through the harness in
//! `registry_schema_lifecycle.rs` — nothing here touches the state struct directly.

use serde_json::{json, Value};
use service_com_amazonaws_schemas_schemas::state::{AWS_MANAGED_REGISTRY, CLOUD_TRAIL_EVENT_FAMILY};

use crate::registry_schema_lifecycle::{schema_names, schema_uri, versions_uri, walk, Emulator};

/// The listing of the AWS-managed registry's schemas.
fn catalogue_uri() -> String {
    format!("/v1/registries/name/{AWS_MANAGED_REGISTRY}/schemas")
}

/// The listing of the catalogue's entries published for one event source.
fn source_listing_uri(source: &str) -> String {
    format!("{}?schemaNamePrefix={source}@", catalogue_uri())
}

/// One entry of a `Schemas` listing, by schema name.
fn summary_of(emulator_answer: &Value, name: &str) -> Value {
    emulator_answer["Schemas"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a Schemas array in {emulator_answer:?}"))
        .iter()
        .find(|item| item["SchemaName"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("expected {name:?} among {emulator_answer:?}"))
        .clone()
}

/// The listing summary the catalogue reports for one entry, selected by its own name so
/// the case does not depend on which page the entry lands on.
async fn published_summary(emulator: &Emulator, name: &str) -> Value {
    let listed = emulator
        .call(
            "GET",
            &format!("{}?schemaNamePrefix={name}", catalogue_uri()),
            None,
        )
        .await;
    assert_eq!(
        listed.status, 200,
        "case {name:?}: expected 200 from ListSchemas, got {} {:?}",
        listed.status, listed.body
    );
    summary_of(&listed.body, name)
}

/// What the catalogue publishes for one event source.
///
/// Source: the published `aws.events` catalogue, one source at a time — which event
/// types a source publishes is data, not a template over sources, so each row is the
/// set read off the catalogue for that source.
struct SourceRow {
    source: &'static str,
    /// The event types published for it, as `<EventName>`, beside the
    /// CloudTrail-delivered family where it carries that family.
    own: &'static [&'static str],
    /// Whether the catalogue publishes the whole CloudTrail-delivered family for it.
    cloud_trail_family: bool,
    /// An `<EventName>` published for the source BESIDE the CloudTrail family's own
    /// spelling of it — AWS has published the same event under two `detail-type`
    /// spellings — or `None` where the source publishes no such second spelling.
    extra_cloud_trail_spelling: Option<&'static str>,
}

/// The rows the membership case runs over: sources that differ along what the rule
/// generalises over — a source whose every entry is its own and that carries no
/// CloudTrail-delivered family, a source whose own entries sit beside that family, a
/// source publishing two names that differ only in a letter's case, a source publishing
/// one entry alone, and an event name carrying punctuation of its own.
fn source_rows() -> Vec<SourceRow> {
    vec![
        SourceRow {
            source: "aws.maester",
            own: &["DocumentCreated", "DocumentDeleted", "DocumentUpdated"],
            cloud_trail_family: false,
            extra_cloud_trail_spelling: None,
        },
        SourceRow {
            source: "aws.labs",
            own: &[
                "LabAutoCancelled",
                "LabCostAlert",
                "LabCostUpdate",
                "LabLaunchStatusChange",
                "LabTerminationStatusChange",
            ],
            cloud_trail_family: false,
            extra_cloud_trail_spelling: None,
        },
        SourceRow {
            source: "aws.acm",
            own: &[
                "ACMCertificateApproachingExpiration",
                "ACMCertificateAvailable",
                "ACMCertificateExpired",
                "ACMCertificateRenewalActionRequired",
                "ACMCertificateRevoked",
                "ACMCertificateRevokedByCA",
                "ACMCertificateRevokedbyCertificateAuthorityCA",
            ],
            cloud_trail_family: true,
            extra_cloud_trail_spelling: None,
        },
        SourceRow {
            source: "aws.aidevops",
            own: &[
                "InvestigationCancelled",
                "InvestigationCompleted",
                "InvestigationCreated",
                "InvestigationFailed",
                "InvestigationInProgress",
                "InvestigationLinked",
                "InvestigationPendingTriage",
            ],
            // The catalogue publishes the investigation lifecycle for this source and no
            // CloudTrail-delivered schema: its recorded page opens with
            // `InvestigationCancelled`. `catalogue_publications.rs` owns that rule.
            cloud_trail_family: false,
            extra_cloud_trail_spelling: None,
        },
        SourceRow {
            source: "aws.tag",
            own: &["TagChangeOnResource", "TagChangeonResource"],
            cloud_trail_family: true,
            extra_cloud_trail_spelling: None,
        },
        SourceRow {
            source: "aws.autoscaling",
            own: &["EC2InstanceLaunchSuccessful"],
            cloud_trail_family: true,
            extra_cloud_trail_spelling: Some("AWSAPICallViaCloudTrail"),
        },
        SourceRow {
            source: "aws.account",
            own: &["RegionOpt-InStatusChange"],
            cloud_trail_family: true,
            extra_cloud_trail_spelling: None,
        },
        SourceRow {
            source: "aws.appflow",
            own: &["AppFlowAutomaticFieldImportReport"],
            cloud_trail_family: true,
            extra_cloud_trail_spelling: None,
        },
    ]
}

/// RULE: the catalogue publishes exactly the event types held for a source — its own
/// ones, the CloudTrail-delivered family only where it carries that family, and every
/// further `detail-type` spelling AWS publishes for one of those events — and each of
/// them is readable under its own name.
#[tokio::test]
async fn a_source_publishes_every_event_type_the_catalogue_holds_for_it() {
    let emulator = Emulator::new();

    for row in source_rows() {
        let published = walk(&emulator, &source_listing_uri(row.source), None, schema_names).await;

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
        expected.extend(
            row.extra_cloud_trail_spelling
                .map(|event| format!("{}@{event}", row.source)),
        );
        // The order a listing serves is the codepoint order of the names, which
        // `schema_name_collation.rs` owns; here it only puts the expectation in the
        // order the comparison needs.
        expected.sort();
        assert_eq!(
            published, expected,
            "case {:?}: the catalogue must publish exactly the event types held for it",
            row.source
        );

        // Every listed name stands behind a readable schema, so the membership is state
        // and not a listing of names nothing backs.
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

/// Two names the catalogue publishes for ONE event type, differing in a letter's case:
/// the `detail-type` spelling each is published under, and the event each document
/// declares.
struct SpellingPair {
    /// The two `<source>@<EventName>` names, in the order a listing serves them.
    names: [&'static str; 2],
    /// The `detail-type` the second name's document declares, which is the first's with
    /// one letter's case changed.
    detail_types: [&'static str; 2],
}

/// RULE: where AWS publishes one event type under two `detail-type` spellings, the
/// catalogue holds BOTH as entries of their own — each readable by its own name, each
/// declaring the spelling it was published under, and neither standing in for the other.
#[tokio::test]
async fn both_published_spellings_of_one_event_type_are_their_own_entries() {
    let emulator = Emulator::new();

    // Rows: the two pairs the recording establishes — one in the CloudTrail-delivered
    // API-call event, one in the tag-change event, so the rule is not about a single
    // event family.
    let pairs = [
        SpellingPair {
            names: [
                "aws.autoscaling@AWSAPICallViaCloudTrail",
                "aws.autoscaling@AWSAPICallviaCloudTrail",
            ],
            detail_types: [
                "AWS API Call Via CloudTrail",
                "AWS API Call via CloudTrail",
            ],
        },
        SpellingPair {
            names: [
                "aws.tag@TagChangeOnResource",
                "aws.tag@TagChangeonResource",
            ],
            detail_types: ["Tag Change On Resource", "Tag Change on Resource"],
        },
    ];

    for pair in pairs {
        for (name, detail_type) in pair.names.iter().zip(pair.detail_types.iter()) {
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
                "case {name:?}: each spelling is read under its own name"
            );
            let document: Value = serde_json::from_str(&described.str_at("Content"))
                .unwrap_or_else(|e| panic!("case {name:?}: Content must be JSON: {e}"));
            let event_name = name
                .split_once('@')
                .unwrap_or_else(|| panic!("case {name:?}: a catalogue name is <source>@<EventName>"))
                .1;
            assert_eq!(
                document["info"]["title"]
                    .as_str()
                    .unwrap_or_default()
                    .split(" published by ")
                    .next()
                    .unwrap_or_default(),
                *detail_type,
                "case {name:?}: the document titles the detail-type this spelling was \
                 published under, got {document:?}"
            );
            assert!(
                document["components"]["schemas"][event_name]["properties"].is_object(),
                "case {name:?}: the document defines the component it is published for, \
                 got {document:?}"
            );
        }

        // Both are listed, in the order the names decide, and the listing reports two
        // entries rather than one standing in for the other.
        let listed = walk(
            &emulator,
            &format!("{}?schemaNamePrefix={}", catalogue_uri(), prefix_of(&pair)),
            None,
            schema_names,
        )
        .await;
        for name in pair.names {
            assert!(
                listed.contains(&name.to_string()),
                "case {name:?}: the catalogue must list it, got {listed:?}"
            );
        }
    }
}

/// The longest prefix both names of a pair share, so one listing selects exactly them
/// and whatever else the catalogue publishes under that prefix.
fn prefix_of(pair: &SpellingPair) -> String {
    let [first, second] = pair.names;
    first
        .chars()
        .zip(second.chars())
        .take_while(|(left, right)| left == right)
        .map(|(left, _)| left)
        .collect()
}

/// One entry and the number of versions the catalogue publishes for it.
struct VersionRow {
    schema: &'static str,
    published: u64,
}

/// RULE: `VersionCount` is the number of versions the catalogue publishes for THAT
/// entry — so entries of one source differ in it, two names a letter's case apart differ
/// in it, and it is no function of the name — and every version the count claims is
/// readable while the one past it is not.
#[tokio::test]
async fn a_published_entry_reports_the_version_count_the_catalogue_publishes() {
    let emulator = Emulator::new();

    // Rows: counts 1, 2, 3 and 4; three entries of ONE source carrying three different
    // counts; two entries whose names differ only in a letter's case carrying different
    // counts; and `aws.a4b@RoomStateChange`, an entry the issue did not cite.
    //
    // Source: the counts the published catalogue reports for these entries (the member
    // is "The number of versions available for the schema",
    // `smithy_query(query="shape",shape="SchemaSummary")`).
    let rows = [
        VersionRow {
            schema: "aws.acm@ACMCertificateApproachingExpiration",
            published: 3,
        },
        VersionRow {
            schema: "aws.acm@ACMCertificateRevoked",
            published: 2,
        },
        VersionRow {
            schema: "aws.acm@ACMCertificateRevokedByCA",
            published: 1,
        },
        VersionRow {
            schema: "aws.aidevops@InvestigationPendingTriage",
            published: 2,
        },
        VersionRow {
            schema: "aws.tag@TagChangeOnResource",
            published: 4,
        },
        VersionRow {
            schema: "aws.tag@TagChangeonResource",
            published: 1,
        },
        VersionRow {
            schema: "aws.a4b@RoomStateChange",
            published: 3,
        },
    ];

    for row in rows {
        let summary = published_summary(&emulator, row.schema).await;
        assert_eq!(
            summary["VersionCount"],
            json!(row.published),
            "case {:?}: the summary must report the published version count, got {summary:?}",
            row.schema
        );

        // The count is what the store holds: every version it claims answers its own
        // number, and the one past it is not a version the catalogue has.
        for version in 1..=row.published {
            let pinned = emulator
                .call(
                    "GET",
                    &format!(
                        "{}?schemaVersion={version}",
                        schema_uri(AWS_MANAGED_REGISTRY, row.schema)
                    ),
                    None,
                )
                .await;
            assert_eq!(
                pinned.status, 200,
                "case {:?} version={version}: expected 200, got {} {:?}",
                row.schema, pinned.status, pinned.body
            );
            assert_eq!(
                pinned.str_at("SchemaVersion"),
                version.to_string(),
                "case {:?} version={version}: the read answers the version asked for",
                row.schema
            );
        }
        let past = emulator
            .call(
                "GET",
                &format!(
                    "{}?schemaVersion={}",
                    schema_uri(AWS_MANAGED_REGISTRY, row.schema),
                    row.published + 1
                ),
                None,
            )
            .await;
        assert_eq!(
            past.status, 404,
            "case {:?}: version {} is not published, got {} {:?}",
            row.schema,
            row.published + 1,
            past.status,
            past.body
        );

        let versions = emulator
            .call(
                "GET",
                &versions_uri(AWS_MANAGED_REGISTRY, row.schema),
                None,
            )
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
            row.published,
            "case {:?}: the version listing must hold every published version",
            row.schema
        );
    }
}

/// One entry and the publication instant the catalogue publishes for it.
struct InstantRow {
    schema: &'static str,
    published: &'static str,
}

/// RULE: an entry the catalogue publishes a publication instant for reports exactly
/// that instant — from the listing and from the read alike, the same in every store and
/// on every read — and the instant is the ENTRY's, not a value computed from its name:
/// two entries whose names differ only in a letter's case report different instants.
#[tokio::test]
async fn a_published_entry_reports_the_publication_instant_the_catalogue_publishes() {
    let emulator = Emulator::new();
    // A SECOND store, built later in this process: an instant read off the clock, or one
    // minted per store, would differ between the two.
    let elsewhere = Emulator::new();

    // Rows: the two entries the recording establishes a fixed publication instant for.
    // They differ in the source, in the year, and in whether the entry is the
    // CloudTrail-delivered event or the source's own.
    let rows = [
        InstantRow {
            schema: "aws.autoscaling@AWSAPICallViaCloudTrail",
            published: "2019-12-01T00:59:43Z",
        },
        InstantRow {
            schema: "aws.tag@TagChangeOnResource",
            published: "2022-03-24T16:40:20Z",
        },
    ];

    for row in rows {
        let summary = published_summary(&emulator, row.schema).await;
        assert_eq!(
            summary["LastModified"],
            json!(row.published),
            "case {:?}: the summary must report the published instant, got {summary:?}",
            row.schema
        );

        for (store, which) in [(&emulator, "this store"), (&elsewhere, "a second store")] {
            let described = store
                .call("GET", &schema_uri(AWS_MANAGED_REGISTRY, row.schema), None)
                .await;
            assert_eq!(
                described.status, 200,
                "case {:?} in {which}: expected 200, got {} {:?}",
                row.schema, described.status, described.body
            );
            assert_eq!(
                described.value_at("LastModified"),
                &json!(row.published),
                "case {:?} in {which}: the read must report the published instant",
                row.schema
            );
        }
    }

    // The instant is the entry's own and no function of its name: two names that differ
    // only in a letter's case report different instants, and the second one — which the
    // catalogue publishes no instant for — is not the first's.
    let publishes = published_summary(&emulator, "aws.tag@TagChangeOnResource").await;
    let derives = published_summary(&emulator, "aws.tag@TagChangeonResource").await;
    assert_ne!(
        publishes["LastModified"], derives["LastModified"],
        "two entries a letter's case apart must report their own instants, got \
         {publishes:?} and {derives:?}"
    );
    let derived = derives["LastModified"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a LastModified timestamp in {derives:?}"))
        .to_string();
    assert!(
        !derived.contains('.'),
        "an entry's instant is whole-second, got {derived:?}"
    );
    // And it does not move between stores either.
    let elsewhere_derived = published_summary(&elsewhere, "aws.tag@TagChangeonResource").await;
    assert_eq!(
        elsewhere_derived["LastModified"],
        json!(derived),
        "an entry with no published instant must still report one instant in every \
         store, got {elsewhere_derived:?}"
    );
}
