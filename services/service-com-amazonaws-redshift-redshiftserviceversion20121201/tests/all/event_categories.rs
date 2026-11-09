// SPDX-License-Identifier: CC0-1.0

//! The event catalog `DescribeEventCategories` reports, end to end over the
//! awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds, reusing the harness the cluster-lifecycle suite
//! owns rather than keeping a second copy of it.
//!
//! The suite asserts RULES over tables of inputs rather than one recorded
//! request:
//!
//! * the read reports one entry per source type its own `SourceType` vocabulary
//!   admits, in the order that vocabulary lists them, and every
//!   event it carries is published under a category and a severity this service
//!   publishes;
//! * a `SourceType` selects exactly the entry for that type, in the published
//!   spelling however the request spelled it, while a value outside the closed
//!   vocabulary this read admits is REFUSED on the input — 400
//!   `InvalidParameterValue`, naming the whole vocabulary and echoing nothing of
//!   the request;
//! * the read reports and stores nothing: two calls, before and after a write to
//!   the same store, answer identically, and so do two calls of the SAME
//!   selection;
//! * a selection is a FILTER over the one published catalog: the per-source-type
//!   reads together cover the unfiltered read exactly once, entry for entry;
//! * cross-operation agreement: a category and a severity the catalog publishes
//!   are exactly what a subscription scoped to that source type may name (for
//!   the source types the model lets a subscription be scoped to).

use crate::cluster_lifecycle::{all_text, Answer, Emulator};
use crate::maintenance_tracks::elements;

/// The source types a SUBSCRIPTION can be scoped to, in the order the model
/// lists them, written out here rather than read off the production helper so a
/// wrong entry fails this suite.
///
/// Source: `smithy_query(query="shape", shape="SourceType")` — the enum's values
/// `cluster`, `cluster-parameter-group`, `cluster-security-group`,
/// `cluster-snapshot`, `scheduled-action`.
const SUBSCRIBABLE_SOURCE_TYPES: [&str; 5] = [
    "cluster",
    "cluster-parameter-group",
    "cluster-security-group",
    "cluster-snapshot",
    "scheduled-action",
];

/// The `SourceType` vocabulary this read ADMITS, in the order its own refusal
/// enumerates them, written out here rather than read off the production helper
/// so a wrong entry fails this suite.
///
/// Wider than [`SUBSCRIBABLE_SOURCE_TYPES`] — the types the model's `SourceType`
/// enum lists, which is what a SUBSCRIPTION can be scoped to — by
/// `reserved-node`, which the refusal enumerates among the valid types. This IS
/// the set the unfiltered read reports an entry for.
const ADMITTED_SOURCE_TYPES: [&str; 6] = [
    "cluster",
    "cluster-security-group",
    "cluster-snapshot",
    "cluster-parameter-group",
    "scheduled-action",
    "reserved-node",
];

/// The sentence a `SourceType` outside [`ADMITTED_SOURCE_TYPES`] is refused
/// with: the fixed text, then that vocabulary in single quotes in its own order,
/// with no trailing period and nothing of the request in it. Spelled out here
/// rather than read off the crate under test, so a row cannot pass by restating
/// whatever the handler happened to build.
const INVALID_SOURCE_TYPE_MESSAGE: &str = "Invalid event source type. Valid types are 'cluster', \
     'cluster-security-group', 'cluster-snapshot', 'cluster-parameter-group', 'scheduled-action', \
     'reserved-node'";

/// The categories an event can be published under.
///
/// Source: `smithy_query(query="doc", shape="CreateEventSubscriptionMessage",
/// member="EventCategories")` — "Values: configuration, management, monitoring,
/// security, pending".
const PUBLISHED_CATEGORIES: [&str; 5] = [
    "configuration",
    "management",
    "monitoring",
    "security",
    "pending",
];

/// The severities an event can be published under.
///
/// Source: `smithy_query(query="doc", shape="EventInfoMap", member="Severity")` —
/// "Values: ERROR, INFO".
const PUBLISHED_SEVERITIES: [&str; 2] = ["ERROR", "INFO"];

/// One `EventInfoMap` as the wire carries it.
#[derive(Debug, PartialEq, Eq)]
struct CatalogEvent {
    event_id: Option<String>,
    categories: Vec<String>,
    description: Option<String>,
    severity: Option<String>,
}

/// One `EventCategoriesMap` as the wire carries it: a source type and the events
/// published for it.
#[derive(Debug, PartialEq, Eq)]
struct CatalogEntry {
    source_type: Option<String>,
    events: Vec<CatalogEvent>,
}

/// Every `EventCategoriesMap` a `DescribeEventCategories` body carries, in the
/// order the response lists them.
///
/// `EventCategoriesMapList`'s member carries `xmlName: EventCategoriesMap`, so
/// one element per entry; `EventInfoMapList`'s carries `xmlName: EventInfoMap`
/// and `EventCategoriesList`'s `xmlName: EventCategory`. A structure serializes
/// its members in declaration order, so an entry's own source type is the FIRST
/// inside its element.
fn reported_catalog(xml: &str) -> Vec<CatalogEntry> {
    elements(xml, "EventCategoriesMap")
        .into_iter()
        .map(|entry| CatalogEntry {
            source_type: elements(&entry, "SourceType").into_iter().next(),
            events: elements(&entry, "EventInfoMap")
                .into_iter()
                .map(|event| CatalogEvent {
                    event_id: all_text(&event, "EventId").into_iter().next(),
                    categories: all_text(&event, "EventCategory"),
                    description: all_text(&event, "EventDescription").into_iter().next(),
                    severity: all_text(&event, "Severity").into_iter().next(),
                })
                .collect(),
        })
        .collect()
}

/// The source types a body reports, in response order.
fn reported_source_types(xml: &str) -> Vec<String> {
    reported_catalog(xml)
        .into_iter()
        .filter_map(|entry| entry.source_type)
        .collect()
}

/// `DescribeEventCategories`, with the `SourceType` the row names or none.
async fn read_catalog(emulator: &Emulator, source_type: Option<&str>) -> Answer {
    match source_type {
        None => emulator.call("DescribeEventCategories", &[]).await,
        Some(source_type) => {
            emulator
                .call("DescribeEventCategories", &[("SourceType", source_type)])
                .await
        }
    }
}

#[tokio::test]
async fn the_published_catalog_is_what_an_unfiltered_read_reports() {
    let emulator = Emulator::new();

    // Answered before anything exists: the events a subscription can be scoped
    // to are catalog data, not something a resource creates.
    let answer = read_catalog(&emulator, None).await;
    assert_eq!(
        answer.status, 200,
        "case read an empty emulator: expected 200, got {} with body {}",
        answer.status, answer.body
    );

    let reported = reported_catalog(&answer.body);
    assert_eq!(
        reported_source_types(&answer.body),
        ADMITTED_SOURCE_TYPES.map(str::to_string).to_vec(),
        "case read an empty emulator: expected one entry per admitted source type, got body {}",
        answer.body
    );

    // Every event the catalog publishes is a complete, well-formed entry: an
    // identifier, a description, a severity this service publishes and at least
    // one category it publishes.
    let mut identifiers: Vec<String> = Vec::new();
    for entry in &reported {
        let source_type = entry
            .source_type
            .clone()
            .unwrap_or_else(|| "<none>".to_string());
        for event in &entry.events {
            let case = format!("{source_type} event {:?}", event.event_id);
            let event_id = event
                .event_id
                .clone()
                .unwrap_or_else(|| panic!("case {case}: expected an EventId, got {event:?}"));
            assert!(
                !event_id.is_empty(),
                "case {case}: expected a non-empty EventId, got {event_id:?}"
            );
            assert!(
                event
                    .description
                    .as_deref()
                    .is_some_and(|text| !text.is_empty()),
                "case {case}: expected a non-empty EventDescription, got {:?}",
                event.description
            );
            assert!(
                event
                    .severity
                    .as_deref()
                    .is_some_and(|severity| PUBLISHED_SEVERITIES.contains(&severity)),
                "case {case}: expected a severity in {PUBLISHED_SEVERITIES:?}, got {:?}",
                event.severity
            );
            assert!(
                !event.categories.is_empty()
                    && event
                        .categories
                        .iter()
                        .all(|category| PUBLISHED_CATEGORIES.contains(&category.as_str())),
                "case {case}: expected categories drawn from {PUBLISHED_CATEGORIES:?}, got {:?}",
                event.categories
            );
            identifiers.push(event_id);
        }
    }
    assert!(
        !identifiers.is_empty(),
        "case read an empty emulator: expected the catalog to publish at least one event, got body {}",
        answer.body
    );
    let mut unique = identifiers.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        identifiers.len(),
        "case read an empty emulator: expected every EventId to be published once, got {identifiers:?}"
    );

    // A read leaves nothing behind, and a WRITE to the same store does not
    // change the catalog either: it is seeded, and no operation adds to it.
    let created = emulator
        .call(
            "CreateClusterParameterGroup",
            &[
                ("ParameterGroupName", "catalog-unchanged"),
                ("ParameterGroupFamily", "redshift-1.0"),
                ("Description", "catalog is unchanged by a write"),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case write beside the catalog: expected 200, got {} with body {}",
        created.status, created.body
    );
    let again = read_catalog(&emulator, None).await;
    assert_eq!(
        (again.status, reported_catalog(&again.body)),
        (answer.status, reported),
        "case read twice around a write: expected an unchanged catalog, got {} with body {}",
        again.status,
        again.body
    );
}

#[tokio::test]
async fn each_source_type_publishes_the_catalogued_events_of_its_resource() {
    let emulator = Emulator::new();

    // One row per published source type, naming an event the catalog must
    // publish for it: its identifier, the categories and the severity it is
    // published under, written out here rather than read off the production
    // table so a wrong entry fails this suite.
    //
    // Source: the published Amazon Redshift event catalog ("Amazon Redshift
    // event categories and event messages"), whose identifiers run in ranges
    // that follow the category loosely — 1000 configuration, 2000 management,
    // 3000 monitoring, 4000 security — with the published catalog, not the
    // range, deciding each event's own identifier.
    let rows: [(&str, &str, &[&str], &str); 6] = [
        ("cluster", "REDSHIFT-EVENT-2000", &["management"], "INFO"),
        (
            "cluster-parameter-group",
            "REDSHIFT-EVENT-1003",
            &["configuration"],
            "INFO",
        ),
        (
            "cluster-security-group",
            "REDSHIFT-EVENT-1100",
            &["configuration"],
            "INFO",
        ),
        (
            "cluster-snapshot",
            "REDSHIFT-EVENT-3100",
            &["monitoring"],
            "INFO",
        ),
        (
            "scheduled-action",
            "REDSHIFT-EVENT-5200",
            &["pending"],
            "INFO",
        ),
        (
            "reserved-node",
            "REDSHIFT-EVENT-2300",
            &["management"],
            "INFO",
        ),
    ];

    for (source_type, event_id, categories, severity) in rows {
        let answer = read_catalog(&emulator, Some(source_type)).await;
        assert_eq!(
            answer.status, 200,
            "case select {source_type}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        let reported = reported_catalog(&answer.body);
        let entry = reported.first().unwrap_or_else(|| {
            panic!("case select {source_type}: expected one entry, got {reported:?}")
        });

        // Every source type raises Amazon Redshift events, so none of them is
        // reported with an empty event list.
        assert!(
            !entry.events.is_empty(),
            "case select {source_type}: expected the published events of that source type, got {entry:?}"
        );

        let published = entry
            .events
            .iter()
            .find(|event| event.event_id.as_deref() == Some(event_id))
            .unwrap_or_else(|| {
                panic!(
                    "case select {source_type}: expected {event_id} among its events, got {:?}",
                    entry.events
                )
            });
        assert_eq!(
            (published.categories.as_slice(), published.severity.as_deref()),
            (
                categories
                    .iter()
                    .map(|category| (*category).to_string())
                    .collect::<Vec<String>>()
                    .as_slice(),
                Some(severity)
            ),
            "case select {source_type} / {event_id}: expected the published categories and severity, got {published:?}"
        );
    }

    // The published category and severity surfaces are covered by the catalog:
    // a client that subscribes by category or by severity has an event to
    // receive for every value this service publishes.
    let whole = read_catalog(&emulator, None).await;
    let catalog = reported_catalog(&whole.body);
    for category in PUBLISHED_CATEGORIES {
        assert!(
            catalog.iter().any(|entry| entry
                .events
                .iter()
                .any(|event| event.categories.iter().any(|published| published == category))),
            "case category {category}: expected a published event in that category, got body {}",
            whole.body
        );
    }
    for severity in PUBLISHED_SEVERITIES {
        assert!(
            catalog.iter().any(|entry| entry
                .events
                .iter()
                .any(|event| event.severity.as_deref() == Some(severity))),
            "case severity {severity}: expected a published event of that severity, got body {}",
            whole.body
        );
    }
}

#[tokio::test]
async fn a_source_type_selects_exactly_that_entry_in_the_published_spelling() {
    let emulator = Emulator::new();

    let whole = read_catalog(&emulator, None).await;
    let whole_catalog = reported_catalog(&whole.body);

    // Each row names one source type, in a spelling that differs from the
    // published one only in case: one catalog value is reported in exactly one
    // form however a caller spelled it, the same rule the subscription path
    // resolves a `SourceType` under.
    let mut rows: Vec<(String, String, &str)> = Vec::new();
    for source_type in ADMITTED_SOURCE_TYPES {
        rows.push((
            format!("the published spelling of {source_type}"),
            source_type.to_string(),
            source_type,
        ));
        rows.push((
            format!("the upper-case spelling of {source_type}"),
            source_type.to_ascii_uppercase(),
            source_type,
        ));
        rows.push((
            format!("a mixed-case spelling of {source_type}"),
            capitalize_segments(source_type),
            source_type,
        ));
    }

    for (case, requested, expected) in rows {
        let answer = read_catalog(&emulator, Some(&requested)).await;
        assert_eq!(
            answer.status, 200,
            "case {case} ({requested}): expected 200, got {} with body {}",
            answer.status, answer.body
        );
        let selected = reported_catalog(&answer.body);
        let published = whole_catalog
            .iter()
            .find(|entry| entry.source_type.as_deref() == Some(expected))
            .unwrap_or_else(|| {
                panic!("case {case} ({requested}): the unfiltered read published no {expected} entry")
            });
        assert_eq!(
            selected.len(),
            1,
            "case {case} ({requested}): expected exactly one entry, got {selected:?}"
        );
        assert_eq!(
            selected[0].source_type.as_deref(),
            Some(expected),
            "case {case} ({requested}): expected the published spelling {expected}, got {:?}",
            selected[0].source_type
        );
        assert_eq!(
            selected[0].events, published.events,
            "case {case} ({requested}): expected the same events the unfiltered read reports, got {:?}",
            selected[0].events
        );
    }
}

#[tokio::test]
async fn a_value_outside_the_admitted_vocabulary_is_refused_on_the_input() {
    let emulator = Emulator::new();

    // Spanning the ways a value can fall outside the admitted vocabulary:
    // another service's source type, a plural, a prefix of an admitted value, an
    // extension of one, the same words with a space instead of a hyphen, an
    // empty value and a value of another shape entirely. Each is refused on the
    // INPUT, with the one sentence that enumerates the vocabulary.
    for requested in [
        "db-instance",
        "clusters",
        "clust",
        "cluster-parameter-groups",
        "cluster parameter group",
        "reserved-nodes",
        "arn:aws:redshift:us-west-2:000000000000:cluster:c",
    ] {
        let answer = read_catalog(&emulator, Some(requested)).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (400, Some("InvalidParameterValue".to_string())),
            "case select unadmitted {requested:?}: expected a 400 refusal, got {} / {:?} with body {}",
            answer.status,
            answer.error_code(),
            answer.body
        );
        // Message equality pins both the wording and that the refusal echoes
        // NOTHING of the request: the sentence has no variable part.
        assert_eq!(
            answer.text("Message").as_deref(),
            Some(INVALID_SOURCE_TYPE_MESSAGE),
            "case select unadmitted {requested:?}: expected the message {INVALID_SOURCE_TYPE_MESSAGE:?}, got {:?}",
            answer.text("Message")
        );
        assert_eq!(
            reported_catalog(&answer.body),
            Vec::new(),
            "case select unadmitted {requested:?}: expected no catalog in the refusal, got body {}",
            answer.body
        );
    }

    // Every value the refusal enumerates IS accepted, in each spelling the read
    // resolves: the enumerated set is the admitted set, not a longer or shorter
    // list, and each selects its own entry of the one published catalog.
    for admitted in ADMITTED_SOURCE_TYPES {
        for (case, requested) in [
            ("the admitted spelling", admitted.to_string()),
            ("the upper-case spelling", admitted.to_ascii_uppercase()),
            ("a mixed-case spelling", capitalize_segments(admitted)),
        ] {
            let answer = read_catalog(&emulator, Some(&requested)).await;
            assert_eq!(
                (answer.status, answer.error_code()),
                (200, None),
                "case {case} of admitted {admitted}: expected a successful read, got {} / {:?} with body {}",
                answer.status,
                answer.error_code(),
                answer.body
            );
            let expected = vec![admitted.to_string()];
            assert_eq!(
                reported_source_types(&answer.body),
                expected,
                "case {case} of admitted {admitted}: expected {expected:?}, got body {}",
                answer.body
            );
        }
    }
}

#[tokio::test]
async fn a_selection_filters_the_one_catalog_and_repeats_unchanged() {
    let emulator = Emulator::new();

    let unfiltered = read_catalog(&emulator, None).await;
    assert_eq!(
        unfiltered.status, 200,
        "case the unfiltered read: expected 200, got {} with body {}",
        unfiltered.status, unfiltered.body
    );
    let whole = reported_catalog(&unfiltered.body);

    // One row per selection a caller can make: no `SourceType` at all, and each
    // source type the read admits. A selection FILTERS the one catalog, so each
    // row's expected answer is read off the unfiltered body rather than written
    // out a second time. (A value outside the admitted vocabulary is not a
    // selection at all: it is refused on the input, which
    // [`a_value_outside_the_admitted_vocabulary_is_refused_on_the_input`] pins.)
    let mut selections: Vec<(String, Option<String>)> =
        vec![("the whole catalog".to_string(), None)];
    for source_type in ADMITTED_SOURCE_TYPES {
        selections.push((format!("select {source_type}"), Some(source_type.to_string())));
    }

    let mut covered: Vec<CatalogEntry> = Vec::new();
    for (case, requested) in &selections {
        let expected: Vec<&CatalogEntry> = match requested.as_deref() {
            None => whole.iter().collect(),
            Some(source_type) => whole
                .iter()
                .filter(|entry| entry.source_type.as_deref() == Some(source_type))
                .collect(),
        };

        // The SAME selection read twice: the second answer is the first,
        // because a reporting read leaves nothing behind for a later caller.
        for attempt in 1..=2u8 {
            let answer = read_catalog(&emulator, requested.as_deref()).await;
            assert_eq!(
                (answer.status, answer.error_code()),
                (200, None),
                "case {case} (read {attempt}): expected a successful read, got {} / {:?}",
                answer.status,
                answer.error_code()
            );
            let reported = reported_catalog(&answer.body);
            assert_eq!(
                reported.iter().collect::<Vec<&CatalogEntry>>(),
                expected,
                "case {case} (read {attempt}): expected the entries the unfiltered read publishes, got {reported:?}"
            );
            if attempt == 2 && requested.is_some() {
                covered.extend(reported);
            }
        }
    }

    // The selections together cover the published catalog exactly once: every
    // entry is selectable by its own source type, and no entry is selected by
    // two of them or by a value the catalog does not publish.
    assert_eq!(
        covered, whole,
        "case the selections together: expected them to cover the published catalog exactly once, got {covered:?}"
    );
}

#[tokio::test]
async fn what_the_catalog_publishes_is_what_a_subscription_may_name() {
    let emulator = Emulator::new();

    let answer = read_catalog(&emulator, None).await;
    assert_eq!(
        answer.status, 200,
        "case read the catalog: expected 200, got {} with body {}",
        answer.status, answer.body
    );

    // Every (source type, category, severity) the catalog publishes is accepted
    // by `CreateEventSubscription` scoped to that source type: a client that
    // enumerates the catalog before building a subscription can use what it read.
    // Only the source types the model lets a subscription be scoped to
    // (`SUBSCRIBABLE_SOURCE_TYPES`) can be subscribed to at all.
    let mut subscriptions = 0usize;
    for entry in reported_catalog(&answer.body) {
        let Some(source_type) = entry.source_type.clone() else {
            continue;
        };
        if !SUBSCRIBABLE_SOURCE_TYPES.contains(&source_type.as_str()) {
            continue;
        }
        for event in &entry.events {
            let severity = event
                .severity
                .clone()
                .unwrap_or_else(|| panic!("case {source_type}: expected a severity on {event:?}"));
            for category in &event.categories {
                let name = format!("catalog-{subscriptions}");
                let case = format!("{source_type} / {category} / {severity}");
                let created = emulator
                    .call(
                        "CreateEventSubscription",
                        &[
                            ("SubscriptionName", &name),
                            (
                                "SnsTopicArn",
                                "arn:aws:sns:us-west-2:000000000000:catalog-topic",
                            ),
                            ("SourceType", &source_type),
                            ("EventCategories.EventCategory.1", category),
                            ("Severity", &severity),
                        ],
                    )
                    .await;
                assert_eq!(
                    created.status, 200,
                    "case subscribe to {case}: expected 200, got {} with body {}",
                    created.status, created.body
                );
                assert_eq!(
                    all_text(&created.body, "EventCategory"),
                    vec![category.clone()],
                    "case subscribe to {case}: expected the category reported back, got body {}",
                    created.body
                );
                subscriptions += 1;
            }
        }
    }
    assert!(
        subscriptions > 0,
        "case subscribe to the catalog: expected the catalog to publish something to subscribe to, got body {}",
        answer.body
    );

    // A category the catalog does not publish is refused, so the published set is
    // a bound and not just a hint.
    let refused = emulator
        .call(
            "CreateEventSubscription",
            &[
                ("SubscriptionName", "catalog-unpublished"),
                (
                    "SnsTopicArn",
                    "arn:aws:sns:us-west-2:000000000000:catalog-topic",
                ),
                ("SourceType", "cluster-parameter-group"),
                ("EventCategories.EventCategory.1", "no-such-category"),
            ],
        )
        .await;
    assert_eq!(
        (refused.status, refused.error_code()),
        (404, Some("SubscriptionCategoryNotFound".to_string())),
        "case subscribe to an unpublished category: expected the modelled refusal, got {} / {:?}",
        refused.status,
        refused.error_code()
    );
}

/// `cluster-parameter-group` as `Cluster-Parameter-Group`: each hyphen-separated
/// segment with its first character upper-cased, so a row differs from the
/// published spelling in case alone.
fn capitalize_segments(value: &str) -> String {
    value
        .split('-')
        .map(|segment| {
            let mut characters = segment.chars();
            match characters.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + characters.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<String>>()
        .join("-")
}
