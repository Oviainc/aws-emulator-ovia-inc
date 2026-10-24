// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListInstanceTypeDetails`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`]; this suite adds no second copy of it.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * the listing spans the instance types the NAMED ENGINE VERSION offers — the model's
//!   `OpenSearchPartitionInstanceType` enum, less the families AWS introduced for a
//!   later engine release — over a table of ten versions of both engines, including
//!   each release a family is gated on and one either side of it;
//! * each entry's feature members follow the engine release the feature is available
//!   from AND what the instance family can serve, and the roles follow the family; a
//!   feature available on one version is still available on a later one of the same
//!   engine, over every ordered pair of the version table;
//! * the `instanceType` filter reports exactly the entry it names, over a table of
//!   types drawn from the listing, answers an empty listing for a published type the
//!   version does not offer, and refuses a value that names no published instance type;
//! * paging is a partition: over a table of page sizes the pages concatenate back to
//!   the full listing in order, every page is within its size, only the last page
//!   carries no token, a page size outside the modelled bound is refused at either end,
//!   and a token the operation never issued is refused;
//! * `retrieveAZs` is what puts Availability Zones in the answer, and the zones are
//!   those of the Region the listing is for — the signing Region, or the Region of the
//!   domain the request names;
//! * a `domainName` that names no domain, and an `EngineVersion` that names no engine
//!   release, are refused with the modelled errors;
//! * the read changes nothing a later caller sees.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::OPEN_SEARCH_PARTITION_INSTANCE_TYPE_VALUES;

use super::domain_lifecycle::{error_code, Answer, Emulator, PREFIX};

/// The engine versions every table below spans: the oldest Elasticsearch release, each
/// release an instance family or a feature is gated on, one either side of it, and
/// OpenSearch releases from the first to a current one. Both engines and both sides of
/// every published gate, so no row is the only evidence for a rule.
const VERSIONS: [&str; 10] = [
    "Elasticsearch_1.5",
    "Elasticsearch_5.1",
    "Elasticsearch_6.7",
    "Elasticsearch_6.8",
    "Elasticsearch_7.9",
    "Elasticsearch_7.10",
    "OpenSearch_1.0",
    "OpenSearch_2.10",
    "OpenSearch_2.11",
    "OpenSearch_2.19",
];

/// The instance families AWS publishes an engine-release prerequisite for, each with
/// the release it is offered from. Source: the UltraWarm family arrived with
/// Elasticsearch 6.8, the Graviton families with Elasticsearch 7.9, and the
/// OpenSearch-optimized family is OpenSearch-only from 2.11. Every other family of
/// `OpenSearchPartitionInstanceType` — the model's record of which instance types
/// exist — is offered on every version.
///
/// These are restated here as the test's OWN expectation rather than imported from
/// `crate::state`, so that changing a gate in `state.rs` fails this suite instead of
/// being asserted against itself; the handler's copies live beside
/// `state::offers_instance_type` and have to be kept in step with these.
const ULTRAWARM_FAMILY: &str = "ultrawarm1";
const GRAVITON_FAMILIES: [&str; 5] = ["c6g", "m6g", "r6g", "r6gd", "t4g"];
const OPENSEARCH_OPTIMIZED_FAMILIES: [&str; 1] = ["or1"];

/// The families neither encryption at rest nor node-to-node encryption is available
/// on, whatever the version.
const UNENCRYPTED_FAMILIES: [&str; 2] = ["m3", "t2"];

/// The burstable families, which UltraWarm cannot be attached to.
const BURSTABLE_FAMILIES: [&str; 3] = ["t2", "t3", "t4g"];

/// `ListInstanceTypeDetails`:
/// `GET /2021-01-01/opensearch/instanceTypeDetails/{EngineVersion}`, with `query` as
/// its `@httpQuery` members (empty for a request that sends none).
async fn list(emulator: &Emulator, engine_version: &str, query: &str) -> Answer {
    let uri = if query.is_empty() {
        format!("{PREFIX}/opensearch/instanceTypeDetails/{engine_version}")
    } else {
        format!("{PREFIX}/opensearch/instanceTypeDetails/{engine_version}?{query}")
    };
    emulator.call("GET", &uri, None).await
}

/// The `InstanceTypeDetails` list of an answer, with the case named on failure.
fn details(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected HTTP 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.body["InstanceTypeDetails"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected an InstanceTypeDetails array, got {:?}",
                answer.body
            )
        })
        .clone()
}

/// The whole listing of one version, read in one call.
async fn whole_listing(emulator: &Emulator, engine_version: &str, query: &str) -> Vec<Value> {
    let answer = list(emulator, engine_version, query).await;
    assert!(
        answer.body["NextToken"].is_null(),
        "case {engine_version}: an unpaged read reports the whole listing, so it carries \
         no NextToken, got {:?}",
        answer.body["NextToken"]
    );
    details(&answer, engine_version)
}

/// The `InstanceType` of every entry of a listing, in the order reported.
fn instance_types(listing: &[Value]) -> Vec<String> {
    listing
        .iter()
        .map(|entry| {
            entry["InstanceType"]
                .as_str()
                .unwrap_or_else(|| panic!("every entry carries an InstanceType, got {entry:?}"))
                .to_owned()
        })
        .collect()
}

/// A boolean feature member of one entry, with the case named on failure.
fn flag(entry: &Value, member: &str, case: &str) -> bool {
    entry[member]
        .as_bool()
        .unwrap_or_else(|| panic!("case {case}: expected a boolean {member}, got {entry:?}"))
}

/// One entry's `InstanceRole`, with the case named on failure.
fn roles(entry: &Value, case: &str) -> Vec<String> {
    entry["InstanceRole"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected an InstanceRole array, got {entry:?}"))
        .iter()
        .map(|role| {
            role.as_str()
                .unwrap_or_else(|| panic!("case {case}: expected a string role, got {role:?}"))
                .to_owned()
        })
        .collect()
}

/// The family token of an instance type, which is `<family>.<size>.search`.
fn family(instance_type: &str) -> &str {
    instance_type.split('.').next().unwrap_or(instance_type)
}

/// An engine version read apart, as the test states the rules against it.
fn parse(engine_version: &str) -> (&str, u32, u32) {
    let (engine, release) = engine_version
        .split_once('_')
        .unwrap_or_else(|| panic!("the version table holds <Engine>_<X.Y> values, got {engine_version}"));
    let (major, minor) = release
        .split_once('.')
        .unwrap_or_else(|| panic!("the version table holds <Engine>_<X.Y> values, got {engine_version}"));
    (
        engine,
        major.parse().expect("a numeric major release"),
        minor.parse().expect("a numeric minor release"),
    )
}

fn is_opensearch(engine_version: &str) -> bool {
    parse(engine_version).0 == "OpenSearch"
}

/// Whether a capability introduced in an Elasticsearch release is available on this
/// version. OpenSearch forked from Elasticsearch 7.10, so every OpenSearch release
/// carries each of the capabilities below.
fn since_elasticsearch(engine_version: &str, release: (u32, u32)) -> bool {
    let (_, major, minor) = parse(engine_version);
    is_opensearch(engine_version) || (major, minor) >= release
}

/// Whether the version is an OpenSearch release at or past `X.Y`.
fn opensearch_at_least(engine_version: &str, release: (u32, u32)) -> bool {
    let (_, major, minor) = parse(engine_version);
    is_opensearch(engine_version) && (major, minor) >= release
}

/// Whether `engine_version` offers `instance_type` at all.
fn offered(engine_version: &str, instance_type: &str) -> bool {
    let family = family(instance_type);
    if family == ULTRAWARM_FAMILY {
        return since_elasticsearch(engine_version, (6, 8));
    }
    if GRAVITON_FAMILIES.contains(&family) {
        return since_elasticsearch(engine_version, (7, 9));
    }
    if OPENSEARCH_OPTIMIZED_FAMILIES.contains(&family) {
        return opensearch_at_least(engine_version, (2, 11));
    }
    true
}

/// Every instance type `engine_version` offers, in the order the model publishes them.
fn expected_types(engine_version: &str) -> Vec<String> {
    OPEN_SEARCH_PARTITION_INSTANCE_TYPE_VALUES
        .iter()
        .filter(|instance_type| offered(engine_version, instance_type))
        .map(|instance_type| (*instance_type).to_string())
        .collect()
}

#[tokio::test]
async fn listing_spans_the_instance_types_the_version_offers() {
    let emulator = Emulator::new();

    for version in VERSIONS {
        let listing = whole_listing(&emulator, version, "").await;
        let reported = instance_types(&listing);

        assert_eq!(
            reported,
            expected_types(version),
            "case {version}: expected the instance types this version offers, in the \
             order the model publishes them, got {} entries",
            reported.len()
        );
        let distinct: BTreeSet<&String> = reported.iter().collect();
        assert_eq!(
            distinct.len(),
            reported.len(),
            "case {version}: expected one entry per instance type, got {} entries for \
             {} types",
            reported.len(),
            distinct.len()
        );
        for instance_type in &reported {
            assert!(
                OPEN_SEARCH_PARTITION_INSTANCE_TYPE_VALUES.contains(&instance_type.as_str()),
                "case {version}: every InstanceType is a value of the modelled enum, got \
                 {instance_type}"
            );
        }
    }

    // The gates themselves, stated as the difference one release makes: the release a
    // family is offered from reports it and the release before it does not.
    for (family_member, before, from) in [
        ("ultrawarm1.medium.search", "Elasticsearch_6.7", "Elasticsearch_6.8"),
        ("r6g.large.search", "Elasticsearch_7.8", "Elasticsearch_7.9"),
        ("t4g.small.search", "Elasticsearch_7.8", "Elasticsearch_7.9"),
        ("or1.medium.search", "OpenSearch_2.10", "OpenSearch_2.11"),
    ] {
        let absent = instance_types(&whole_listing(&emulator, before, "").await);
        let present = instance_types(&whole_listing(&emulator, from, "").await);
        assert!(
            !absent.contains(&family_member.to_string()),
            "case {family_member} on {before}: expected a version before the family was \
             introduced not to offer it"
        );
        assert!(
            present.contains(&family_member.to_string()),
            "case {family_member} on {from}: expected the release the family is offered \
             from to report it"
        );
    }

    // An OpenSearch-only family is offered on no Elasticsearch release, however late.
    let last_elasticsearch = instance_types(&whole_listing(&emulator, "Elasticsearch_7.10", "").await);
    assert!(
        !last_elasticsearch.contains(&"or1.large.search".to_string()),
        "case or1.large.search on Elasticsearch_7.10: expected an OpenSearch-only family \
         to be offered on no Elasticsearch release, got {last_elasticsearch:?}"
    );
}

#[tokio::test]
async fn features_follow_the_engine_release_and_the_instance_family() {
    let emulator = Emulator::new();

    for version in VERSIONS {
        for entry in whole_listing(&emulator, version, "").await {
            let instance_type = instance_types(std::slice::from_ref(&entry))
                .pop()
                .expect("one entry has one InstanceType");
            let family = family(&instance_type);
            let case = format!("{instance_type} on {version}");

            // Both features `EncryptionEnabled` covers need a release that has them and
            // a family that can serve them.
            let encryption = flag(&entry, "EncryptionEnabled", &case);
            assert_eq!(
                encryption,
                !UNENCRYPTED_FAMILIES.contains(&family) && since_elasticsearch(version, (6, 0)),
                "case {case}: expected encryption support to follow the release and the \
                 family, got {encryption}"
            );

            let cognito = flag(&entry, "CognitoEnabled", &case);
            assert_eq!(
                cognito,
                since_elasticsearch(version, (5, 1)),
                "case {case}: expected Cognito support to follow the release, got {cognito}"
            );

            let app_logs = flag(&entry, "AppLogsEnabled", &case);
            assert_eq!(
                app_logs,
                since_elasticsearch(version, (5, 1)),
                "case {case}: expected application-log support to follow the release, got \
                 {app_logs}"
            );

            // Fine-grained access control requires both encryption features, so it is
            // available only where they are.
            let advanced = flag(&entry, "AdvancedSecurityEnabled", &case);
            assert_eq!(
                advanced,
                encryption && since_elasticsearch(version, (6, 7)),
                "case {case}: expected fine-grained access control to require encryption \
                 and the release, got {advanced}"
            );
            assert!(
                !advanced || encryption,
                "case {case}: fine-grained access control is never reported without \
                 encryption"
            );

            let warm = flag(&entry, "WarmEnabled", &case);
            assert_eq!(
                warm,
                since_elasticsearch(version, (6, 8)) && !BURSTABLE_FAMILIES.contains(&family),
                "case {case}: expected UltraWarm support to follow the release and the \
                 family, got {warm}"
            );

            let expected_roles: Vec<String> = if family == ULTRAWARM_FAMILY {
                vec!["ultrawarm".to_string()]
            } else if OPENSEARCH_OPTIMIZED_FAMILIES.contains(&family) {
                vec!["data".to_string()]
            } else {
                vec!["data".to_string(), "master".to_string()]
            };
            assert_eq!(
                roles(&entry, &case),
                expected_roles,
                "case {case}: expected the roles the family can serve, got {:?}",
                entry["InstanceRole"]
            );

            assert!(
                entry["AvailabilityZones"].is_null(),
                "case {case}: a request that did not ask for Availability Zones carries \
                 no AvailabilityZones member, got {:?}",
                entry["AvailabilityZones"]
            );
        }
    }
}

#[tokio::test]
async fn a_feature_available_on_one_release_stays_available_on_a_later_one() {
    let emulator = Emulator::new();
    let members = [
        "EncryptionEnabled",
        "CognitoEnabled",
        "AppLogsEnabled",
        "AdvancedSecurityEnabled",
        "WarmEnabled",
    ];

    // Every ordered pair of the version table that shares an engine, plus each
    // Elasticsearch release paired with an OpenSearch one, which succeeds it.
    for earlier in VERSIONS {
        for later in VERSIONS {
            let (earlier_engine, earlier_major, earlier_minor) = parse(earlier);
            let (later_engine, later_major, later_minor) = parse(later);
            let succeeds = if earlier_engine == later_engine {
                (later_major, later_minor) >= (earlier_major, earlier_minor)
            } else {
                is_opensearch(later)
            };
            if !succeeds {
                continue;
            }

            let earlier_listing = whole_listing(&emulator, earlier, "").await;
            let later_listing = whole_listing(&emulator, later, "").await;
            for entry in &earlier_listing {
                let instance_type = instance_types(std::slice::from_ref(entry))
                    .pop()
                    .expect("one entry has one InstanceType");
                let case = format!("{instance_type}: {earlier} -> {later}");
                let successor = later_listing
                    .iter()
                    .find(|candidate| candidate["InstanceType"].as_str() == Some(&instance_type))
                    .unwrap_or_else(|| {
                        panic!(
                            "case {case}: an instance type offered on a release is offered \
                             on every later one"
                        )
                    });
                for member in members {
                    let before = flag(entry, member, &case);
                    let after = flag(successor, member, &case);
                    assert!(
                        !before || after,
                        "case {case}: expected {member} to stay available, got {before} \
                         then {after}"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn anchored_entries_are_reported_exactly() {
    let emulator = Emulator::new();

    // Hand-written anchors: the WHOLE entry, written out, for one instance type of
    // each family rule on releases either side of each feature gate. They pin the
    // parallel expectation above to fixed values, so a rule error replicated in both
    // the handler and this suite's own gates still fails here.
    let rows = [
        (
            "OpenSearch_2.11",
            json!({
                "InstanceType": "m5.large.search",
                "EncryptionEnabled": true,
                "CognitoEnabled": true,
                "AppLogsEnabled": true,
                "AdvancedSecurityEnabled": true,
                "WarmEnabled": true,
                "InstanceRole": ["data", "master"],
            }),
        ),
        (
            "OpenSearch_2.11",
            json!({
                "InstanceType": "t2.micro.search",
                "EncryptionEnabled": false,
                "CognitoEnabled": true,
                "AppLogsEnabled": true,
                "AdvancedSecurityEnabled": false,
                "WarmEnabled": false,
                "InstanceRole": ["data", "master"],
            }),
        ),
        (
            "OpenSearch_2.11",
            json!({
                "InstanceType": "ultrawarm1.medium.search",
                "EncryptionEnabled": true,
                "CognitoEnabled": true,
                "AppLogsEnabled": true,
                "AdvancedSecurityEnabled": true,
                "WarmEnabled": true,
                "InstanceRole": ["ultrawarm"],
            }),
        ),
        (
            "OpenSearch_2.11",
            json!({
                "InstanceType": "or1.medium.search",
                "EncryptionEnabled": true,
                "CognitoEnabled": true,
                "AppLogsEnabled": true,
                "AdvancedSecurityEnabled": true,
                "WarmEnabled": true,
                "InstanceRole": ["data"],
            }),
        ),
        (
            "Elasticsearch_1.5",
            json!({
                "InstanceType": "m3.medium.search",
                "EncryptionEnabled": false,
                "CognitoEnabled": false,
                "AppLogsEnabled": false,
                "AdvancedSecurityEnabled": false,
                "WarmEnabled": false,
                "InstanceRole": ["data", "master"],
            }),
        ),
        (
            "Elasticsearch_5.1",
            json!({
                "InstanceType": "c4.large.search",
                "EncryptionEnabled": false,
                "CognitoEnabled": true,
                "AppLogsEnabled": true,
                "AdvancedSecurityEnabled": false,
                "WarmEnabled": false,
                "InstanceRole": ["data", "master"],
            }),
        ),
        (
            "Elasticsearch_6.7",
            json!({
                "InstanceType": "t3.small.search",
                "EncryptionEnabled": true,
                "CognitoEnabled": true,
                "AppLogsEnabled": true,
                "AdvancedSecurityEnabled": true,
                "WarmEnabled": false,
                "InstanceRole": ["data", "master"],
            }),
        ),
        (
            "Elasticsearch_6.8",
            json!({
                "InstanceType": "r5.large.search",
                "EncryptionEnabled": true,
                "CognitoEnabled": true,
                "AppLogsEnabled": true,
                "AdvancedSecurityEnabled": true,
                "WarmEnabled": true,
                "InstanceRole": ["data", "master"],
            }),
        ),
    ];

    for (version, expected) in rows {
        let instance_type = expected["InstanceType"]
            .as_str()
            .expect("every anchor names its instance type")
            .to_owned();
        let case = format!("{instance_type} on {version}");
        let answer = list(&emulator, version, &format!("instanceType={instance_type}")).await;
        let entry = details(&answer, &case);
        assert_eq!(
            entry.len(),
            1,
            "case {case}: the filter names one instance type, got {entry:?}"
        );
        assert_eq!(
            entry[0], expected,
            "case {case}: expected the anchored entry, got {:?}",
            entry[0]
        );
    }
}

#[tokio::test]
async fn instance_type_filter_reports_exactly_that_type() {
    let emulator = Emulator::new();
    let version = "OpenSearch_2.11";
    let listing = whole_listing(&emulator, version, "").await;

    // One row per way a filtered type differs: the first and last of the listing, one
    // in the middle, and one of each family with a rule of its own.
    let mut rows: Vec<String> = instance_types(&[
        listing[0].clone(),
        listing[listing.len() / 2].clone(),
        listing[listing.len() - 1].clone(),
    ]);
    rows.extend(
        [
            "ultrawarm1.large.search",
            "or1.2xlarge.search",
            "t2.micro.search",
            "m3.medium.search",
            "t4g.medium.search",
        ]
        .map(str::to_owned),
    );

    for instance_type in rows {
        let expected = listing
            .iter()
            .find(|entry| entry["InstanceType"].as_str() == Some(&instance_type))
            .unwrap_or_else(|| panic!("the listing reports {instance_type}"));
        let answer = list(
            &emulator,
            version,
            &format!("instanceType={instance_type}"),
        )
        .await;
        let filtered = details(&answer, &instance_type);
        assert_eq!(
            filtered.len(),
            1,
            "case {instance_type}: the filter names one instance type, got {filtered:?}"
        );
        assert_eq!(
            &filtered[0], expected,
            "case {instance_type}: expected the entry the listing reports, got {:?}",
            filtered[0]
        );
        assert!(
            answer.body["NextToken"].is_null(),
            "case {instance_type}: one entry is the whole answer, so it carries no \
             NextToken, got {:?}",
            answer.body["NextToken"]
        );
    }

    // A published instance type the version does not offer exists but is not in this
    // listing, so the listing it narrows is empty rather than refused.
    for (not_offered, version) in [
        ("or1.medium.search", "Elasticsearch_7.10"),
        ("ultrawarm1.medium.search", "Elasticsearch_6.7"),
        ("r6g.large.search", "Elasticsearch_7.8"),
    ] {
        let answer = list(
            &emulator,
            version,
            &format!("instanceType={not_offered}"),
        )
        .await;
        let empty = details(&answer, not_offered);
        assert!(
            empty.is_empty(),
            "case {not_offered} on {version}: expected an empty listing, got {empty:?}"
        );
        assert!(
            answer.body["NextToken"].is_null(),
            "case {not_offered} on {version}: an empty listing carries no NextToken, got \
             {:?}",
            answer.body["NextToken"]
        );
    }

    // A value of the member's own shape that names no published instance type is a
    // malformed request: the model's enum is the record of which types exist.
    for unknown in [
        "nosuch.large.search",
        "m5.nosuch.search",
        "r6g.large.search.search",
    ] {
        let answer = list(&emulator, version, &format!("instanceType={unknown}")).await;
        assert_eq!(
            answer.status, 400,
            "case {unknown}: expected HTTP 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {unknown}: expected the modelled validation refusal, got {:?}",
            answer.body
        );
    }

    // A token is scoped to the listing it was issued under, so one naming an entry the
    // filtered listing does not hold is refused rather than served with the filter
    // dropped.
    let first = instance_types(&listing)[0].clone();
    let second = instance_types(&listing)[1].clone();
    let crossed = list(
        &emulator,
        version,
        &format!("instanceType={first}&nextToken={second}"),
    )
    .await;
    assert_eq!(
        crossed.status, 400,
        "case token from another listing: expected HTTP 400, got {} {:?}",
        crossed.status, crossed.body
    );
    assert_eq!(
        error_code(&crossed),
        "ValidationException",
        "case token from another listing: expected the modelled validation refusal, got \
         {:?}",
        crossed.body
    );
}

#[tokio::test]
async fn pages_partition_the_listing() {
    let emulator = Emulator::new();

    // One row per page size: the smallest, two that do not divide the listing, and the
    // largest the modelled range allows — each walked on a version of either engine, so
    // the partition holds over listings of two different lengths.
    for version in ["Elasticsearch_6.7", "OpenSearch_2.19"] {
        let listing = whole_listing(&emulator, version, "").await;
        for page_size in [1_i32, 2, 7, 100] {
            let case = format!("{version} page size {page_size}");
            let mut walked: Vec<Value> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0_usize;
            loop {
                let query = match &token {
                    None => format!("maxResults={page_size}"),
                    Some(token) => format!("maxResults={page_size}&nextToken={token}"),
                };
                let answer = list(&emulator, version, &query).await;
                let page = details(&answer, &case);
                assert!(
                    page.len() <= usize::try_from(page_size).unwrap_or_default(),
                    "case {case}: a page holds at most MaxResults entries, got {}",
                    page.len()
                );
                assert!(
                    !page.is_empty(),
                    "case {case}: a page the service issued a token for reports entries, \
                     got none"
                );
                walked.extend(page);
                pages += 1;
                token = answer.body["NextToken"].as_str().map(str::to_owned);
                if token.is_none() {
                    break;
                }
                assert!(
                    walked.len() <= listing.len(),
                    "case {case}: paging kept issuing tokens past the end of the listing"
                );
            }
            assert_eq!(
                walked, listing,
                "case {case}: expected the pages to concatenate back to the whole listing \
                 in order, got {} entries over {pages} pages",
                walked.len()
            );
            let expected_pages = listing
                .len()
                .div_ceil(usize::try_from(page_size).unwrap_or(1));
            assert_eq!(
                pages, expected_pages,
                "case {case}: expected {expected_pages} pages, got {pages}"
            );
        }

        // `MaxResults` is `@default(0)`, so a request that sends none reports the whole
        // listing in one page.
        for unbounded in ["", "maxResults=0"] {
            let case = format!("{version} unbounded {unbounded:?}");
            let page = details(&list(&emulator, version, unbounded).await, &case);
            assert_eq!(
                page, listing,
                "case {case}: expected the whole listing in one page, got {} entries",
                page.len()
            );
        }
    }

    // A page size outside the modelled bound is refused at either end; the bound itself
    // is not.
    let at_bound = list(&emulator, "OpenSearch_2.11", "maxResults=100").await;
    assert_eq!(
        at_bound.status, 200,
        "case maxResults=100: the published bound is accepted, got {} {:?}",
        at_bound.status, at_bound.body
    );
    for out_of_bound in ["maxResults=101", "maxResults=1000", "maxResults=-1"] {
        let answer = list(&emulator, "OpenSearch_2.11", out_of_bound).await;
        assert_eq!(
            answer.status, 400,
            "case {out_of_bound}: expected HTTP 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {out_of_bound}: expected the modelled validation refusal, got {:?}",
            answer.body
        );
    }

    // A token the operation never issued is a malformed request, not an empty page.
    for invented in [
        "nextToken=not-a-token",
        "nextToken=m5.large.search.page2",
        "maxResults=5&nextToken=00000000-0000-0000-0000-000000000000",
    ] {
        let answer = list(&emulator, "OpenSearch_2.11", invented).await;
        assert_eq!(
            answer.status, 400,
            "case {invented}: expected HTTP 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {invented}: expected the modelled validation refusal, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn retrieve_azs_reports_the_zones_of_the_listing_region() {
    let emulator = Emulator::new();
    let version = "OpenSearch_2.11";

    // The harness signs every request for `us-east-1`, so an unscoped listing is for
    // that Region's zones.
    for query in ["retrieveAZs=true", "maxResults=4&retrieveAZs=true"] {
        let listing = details(&list(&emulator, version, query).await, query);
        assert!(
            !listing.is_empty(),
            "case {query}: expected entries to report zones for"
        );
        for entry in &listing {
            let zones: Vec<String> = entry["AvailabilityZones"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case {query}: expected an AvailabilityZones array, got {entry:?}")
                })
                .iter()
                .map(|zone| zone.as_str().unwrap_or_default().to_owned())
                .collect();
            assert_eq!(
                zones,
                vec!["us-east-1a", "us-east-1b", "us-east-1c"],
                "case {query}: expected the signing Region's zones, got {zones:?}"
            );
        }
    }

    // A request that asks for no zones carries no member at all, whatever else it sends.
    for query in ["", "retrieveAZs=false", "maxResults=3"] {
        let listing = details(&list(&emulator, version, query).await, query);
        for entry in &listing {
            assert!(
                entry["AvailabilityZones"].is_null(),
                "case {query:?}: expected no AvailabilityZones member, got {:?}",
                entry["AvailabilityZones"]
            );
        }
    }

    // Scoped to a domain, the zones are the ones of THAT domain's Region — which the
    // domain record carries, so a domain in another Region reports that Region's zones.
    let name = "zoned-domain";
    let created = emulator.create(json!({ "DomainName": name })).await;
    assert_eq!(
        created.status, 200,
        "case create {name}: expected HTTP 200, got {} {:?}",
        created.status, created.body
    );
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        {
            let store = emulator.store();
            let mut guard = store.lock();
            let domain = guard
                .domains
                .get_mut(name)
                .unwrap_or_else(|| panic!("case {region}: the created domain is in the store"));
            domain.region = region.to_string();
        }
        let query = format!("domainName={name}&retrieveAZs=true&maxResults=1");
        let listing = details(&list(&emulator, version, &query).await, region);
        let zones: Vec<String> = listing[0]["AvailabilityZones"]
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case {region}: expected an AvailabilityZones array, got {:?}",
                    listing[0]
                )
            })
            .iter()
            .map(|zone| zone.as_str().unwrap_or_default().to_owned())
            .collect();
        assert_eq!(
            zones,
            vec![format!("{region}a"), format!("{region}b"), format!("{region}c")],
            "case {region}: expected the domain Region's zones, got {zones:?}"
        );
    }

    // The catalog itself does not depend on the domain: a scoped listing reports the
    // same instance types as an unscoped one.
    let scoped = instance_types(&whole_listing(&emulator, version, &format!("domainName={name}")).await);
    let unscoped = instance_types(&whole_listing(&emulator, version, "").await);
    assert_eq!(
        scoped, unscoped,
        "case scoped listing: expected the same instance types as an unscoped listing, \
         got {} and {} entries",
        scoped.len(),
        unscoped.len()
    );
}

#[tokio::test]
async fn a_request_naming_no_domain_or_no_release_is_refused() {
    let emulator = Emulator::new();
    let version = "OpenSearch_2.11";

    // A `domainName` the account holds no domain for names no resource, over a table of
    // names inside the member's own length bounds.
    for absent in ["nosuch", "absent-domain", "zzz"] {
        let answer = list(&emulator, version, &format!("domainName={absent}")).await;
        assert_eq!(
            answer.status, 409,
            "case {absent}: expected HTTP 409, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {absent}: expected the modelled not-found refusal, got {:?}",
            answer.body
        );
    }

    // A deleted domain holds no record either, so the same refusal follows a delete.
    let name = "transient-domain";
    emulator.create(json!({ "DomainName": name })).await;
    let before = list(&emulator, version, &format!("domainName={name}")).await;
    assert_eq!(
        before.status, 200,
        "case created domain: expected HTTP 200, got {} {:?}",
        before.status, before.body
    );
    emulator
        .call("DELETE", &format!("{PREFIX}/opensearch/domain/{name}"), None)
        .await;
    let after = list(&emulator, version, &format!("domainName={name}")).await;
    assert_eq!(
        after.status, 409,
        "case deleted domain: expected HTTP 409, got {} {:?}",
        after.status, after.body
    );

    // An `EngineVersion` of the member's own length that names no engine release is a
    // malformed request: the catalog is a function of the release.
    for malformed in [
        "Opensearch_2.11",
        "OpenSearch_abc",
        "Elasticsearch_x.y",
        "AmazonSearch_2.11",
    ] {
        let answer = list(&emulator, malformed, "").await;
        assert_eq!(
            answer.status, 400,
            "case {malformed}: expected HTTP 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {malformed}: expected the modelled validation refusal, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn reading_the_catalog_changes_nothing() {
    let emulator = Emulator::new();
    let version = "OpenSearch_2.11";
    let first = whole_listing(&emulator, version, "").await;

    // Paged, filtered, zone-carrying and refused reads, all of which only report.
    list(&emulator, version, "maxResults=3").await;
    list(&emulator, version, "instanceType=m5.large.search").await;
    list(&emulator, version, "retrieveAZs=true").await;
    list(&emulator, version, "domainName=nosuch").await;
    list(&emulator, "Elasticsearch_1.5", "").await;

    let second = whole_listing(&emulator, version, "").await;
    assert_eq!(
        first, second,
        "case repeat read: expected the same listing the first read reported, got {} \
         entries",
        second.len()
    );

    let store = emulator.store();
    let state = store.lock();
    assert!(
        state.domains.is_empty(),
        "case stored domains: reporting instance types creates no domain, got {:?}",
        state.domains.keys().collect::<Vec<_>>()
    );
}
