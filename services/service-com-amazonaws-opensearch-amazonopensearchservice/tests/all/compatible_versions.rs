// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetCompatibleVersions`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`]; this suite adds no second copy of it.
//!
//! The tests assert the RULE over sets of inputs rather than one recorded call: the
//! versions a source version can be upgraded to are the published versions of its
//! own engine line that are LATER than it, plus the whole of the line that succeeds
//! it once it has reached the last published version of its own — the OpenSearch line
//! succeeding the Elasticsearch one, each line ordered by major then minor compared
//! as numbers. So:
//!
//! * the unfiltered answer is the whole published table, one entry per version of
//!   `SUPPORTED_ENGINE_VERSIONS`, in that order, each with the targets the rule
//!   gives it;
//! * an upgrade only goes FORWARDS — no version is among its own targets, every
//!   target ranks above its source, targets are ascending, and a version's
//!   same-line targets are exactly the later published versions of that line;
//! * crossing engine lines is the LAST published version of a line's privilege: only
//!   the newest published Elasticsearch version reaches the OpenSearch line, and
//!   every earlier Elasticsearch version's targets are Elasticsearch versions;
//! * `DomainName` limits the answer to the one version THAT domain runs, over a
//!   table of domains spanning both engines, several majors, the newest published
//!   version (which has no target), the default version a request omitting
//!   `EngineVersion` gets, and a pattern-valid version the catalog does not publish;
//!   a name no domain has is refused with the modelled error;
//! * the read changes nothing a later caller sees.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    DEFAULT_ENGINE_VERSION, SUPPORTED_ENGINE_VERSIONS,
};

use super::domain_lifecycle::{error_code, string_member, Answer, Emulator, PREFIX, SETTLE_WAIT};

/// `GetCompatibleVersions`: `GET /2021-01-01/opensearch/compatibleVersions`, with
/// `DomainName` as its one `@httpQuery` member.
async fn compatible(emulator: &Emulator, domain_name: Option<&str>) -> Answer {
    let uri = match domain_name {
        None => format!("{PREFIX}/opensearch/compatibleVersions"),
        Some(name) => format!("{PREFIX}/opensearch/compatibleVersions?domainName={name}"),
    };
    emulator.call("GET", &uri, None).await
}

/// The `CompatibleVersions` list of a successful answer, with the case named on
/// failure.
fn entries(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected HTTP 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.body["CompatibleVersions"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a CompatibleVersions array, got {:?}",
                answer.body
            )
        })
        .clone()
}

/// One entry's `TargetVersions`, as strings. The member is always present: a version
/// with nothing to upgrade to reports an empty list.
fn target_versions(entry: &Value, case: &str) -> Vec<String> {
    entry["TargetVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a TargetVersions array, got {entry:?}"))
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected a string target, got {value:?}"))
                .to_owned()
        })
        .collect()
}

/// Where a version sits in the upgrade order, parsed HERE rather than read from the
/// service, so the expectations below are independent of the handler's own parse:
/// the engine generation first (the OpenSearch line succeeds the Elasticsearch one),
/// then the major and minor numbers as NUMBERS.
///
/// Panics for a value `VersionString`'s pattern
/// `^Elasticsearch_[0-9]{1}\.[0-9]{1,2}$|^OpenSearch_[0-9]{1,2}\.[0-9]{1,2}$` does
/// not allow, which is what makes it an assertion about every version the service
/// reports.
fn rank(version: &str) -> (u8, u32, u32) {
    let (generation, numbers) = if let Some(rest) = version.strip_prefix("OpenSearch_") {
        (1_u8, rest)
    } else if let Some(rest) = version.strip_prefix("Elasticsearch_") {
        (0_u8, rest)
    } else {
        panic!("case {version}: expected an Elasticsearch_ or OpenSearch_ version");
    };
    let (major, minor) = numbers
        .split_once('.')
        .unwrap_or_else(|| panic!("case {version}: expected a <major>.<minor> version"));
    let parse = |part: &str, which: &str| {
        part.parse::<u32>()
            .unwrap_or_else(|_| panic!("case {version}: expected a numeric {which}, got {part}"))
    };
    (generation, parse(major, "major"), parse(minor, "minor"))
}

/// The newest published version of one engine line, as a rank.
fn last_published_of(generation: u8) -> (u8, u32, u32) {
    SUPPORTED_ENGINE_VERSIONS
        .iter()
        .map(|version| rank(version))
        .filter(|candidate| candidate.0 == generation)
        .max()
        .unwrap_or_else(|| panic!("the published set covers engine line {generation}"))
}

/// The rule, applied to one source version: the later published versions of its own
/// engine line, plus the whole of the succeeding line once it has reached the last
/// published version of its own. Oldest first.
fn expected_targets(source_version: &str) -> Vec<String> {
    let source = rank(source_version);
    let crosses_lines = source >= last_published_of(source.0);
    SUPPORTED_ENGINE_VERSIONS
        .iter()
        .filter(|candidate| {
            let candidate = rank(candidate);
            candidate > source && (candidate.0 == source.0 || crosses_lines)
        })
        .map(|candidate| (*candidate).to_string())
        .collect()
}

/// Create a domain, optionally at a named engine version, and let the provisioning
/// span fall due so the record is settled before it is read.
async fn settled_domain(emulator: &Emulator, name: &str, engine_version: Option<&str>) {
    let mut body = json!({ "DomainName": name });
    if let Some(version) = engine_version {
        body["EngineVersion"] = json!(version);
    }
    let created = emulator.create(body).await;
    assert_eq!(
        created.status, 200,
        "case {name} / {engine_version:?}: expected the domain to be created, got {} {:?}",
        created.status, created.body
    );
    tokio::time::sleep(SETTLE_WAIT).await;
}

#[tokio::test]
async fn table_reports_every_published_version_in_upgrade_order() {
    let emulator = Emulator::new();
    let table = entries(&compatible(&emulator, None).await, "whole table");

    assert_eq!(
        table.len(),
        SUPPORTED_ENGINE_VERSIONS.len(),
        "case whole table: expected one entry per published version, got {} entries",
        table.len()
    );

    // The published set is itself in upgrade order, so the table a caller walks is
    // ordered oldest first.
    for pair in SUPPORTED_ENGINE_VERSIONS.windows(2) {
        assert!(
            rank(pair[0]) < rank(pair[1]),
            "case {} vs {}: expected the published set in upgrade order, got ranks {:?} \
             and {:?}",
            pair[0],
            pair[1],
            rank(pair[0]),
            rank(pair[1])
        );
    }

    for (entry, published) in table.iter().zip(SUPPORTED_ENGINE_VERSIONS.iter()) {
        let source = string_member(entry, "SourceVersion", "table entry");
        assert_eq!(
            &source, published,
            "case {published}: expected the published version at this place in the table, \
             got {source}"
        );
        let targets = target_versions(entry, &source);
        assert_eq!(
            targets,
            expected_targets(&source),
            "case {source}: expected the published versions later than it, got {targets:?}"
        );
    }

    // The newest published version is the end of the path: nothing to upgrade to, and
    // an empty list rather than a missing member.
    let newest = SUPPORTED_ENGINE_VERSIONS[SUPPORTED_ENGINE_VERSIONS.len() - 1];
    let last = &table[table.len() - 1];
    assert_eq!(
        string_member(last, "SourceVersion", "newest version"),
        newest,
        "case {newest}: expected the newest published version last in the table, got \
         {last:?}"
    );
    assert!(
        target_versions(last, newest).is_empty(),
        "case {newest}: the newest version has nothing to upgrade to, got {:?}",
        last["TargetVersions"]
    );
}

#[tokio::test]
async fn targets_only_ever_go_forwards() {
    let emulator = Emulator::new();
    let table = entries(&compatible(&emulator, None).await, "forwards");

    let published: Vec<String> = SUPPORTED_ENGINE_VERSIONS
        .iter()
        .map(|version| (*version).to_string())
        .collect();

    let mut previous: Option<(String, Vec<String>)> = None;
    for entry in &table {
        let source = string_member(entry, "SourceVersion", "forwards entry");
        let targets = target_versions(entry, &source);

        assert!(
            !targets.contains(&source),
            "case {source}: an upgrade never targets the version it starts from, got \
             {targets:?}"
        );
        for target in &targets {
            assert!(
                rank(target) > rank(&source),
                "case {source} -> {target}: expected a target later than the source, got \
                 ranks {:?} and {:?}",
                rank(&source),
                rank(target)
            );
            assert!(
                published.contains(target),
                "case {source} -> {target}: expected a published version as a target"
            );
        }
        for pair in targets.windows(2) {
            assert!(
                rank(&pair[0]) < rank(&pair[1]),
                "case {source}: expected the targets oldest first, got {:?} then {:?}",
                pair[0],
                pair[1]
            );
        }

        // Within one engine line, the targets are exactly the published versions of
        // that line after the source: no hop is missing and none is invented.
        let same_line: Vec<String> = targets
            .iter()
            .filter(|target| rank(target).0 == rank(&source).0)
            .cloned()
            .collect();
        let expected_same_line: Vec<String> = published
            .iter()
            .filter(|candidate| {
                rank(candidate).0 == rank(&source).0 && rank(candidate) > rank(&source)
            })
            .cloned()
            .collect();
        assert_eq!(
            same_line, expected_same_line,
            "case {source}: expected every later published version of its own engine \
             line, got {same_line:?}"
        );

        // Crossing to the succeeding line is the LAST published version of a line's
        // privilege: an earlier version has to walk its own line first.
        let crossed: Vec<String> = targets
            .iter()
            .filter(|target| rank(target).0 != rank(&source).0)
            .cloned()
            .collect();
        let last_of_line = last_published_of(rank(&source).0);
        if rank(&source) < last_of_line {
            assert!(
                crossed.is_empty(),
                "case {source}: a version short of the last published one of its line \
                 reaches no other line, got {crossed:?}"
            );
        } else {
            let expected_crossed: Vec<String> = published
                .iter()
                .filter(|candidate| rank(candidate).0 > rank(&source).0)
                .cloned()
                .collect();
            assert_eq!(
                crossed, expected_crossed,
                "case {source}: the last published version of a line reaches the whole \
                 of the line that succeeds it, got {crossed:?}"
            );
        }

        // Within one line, each version's targets are the previous one's minus that
        // version itself — the table is one monotone path, not a set of unrelated
        // answers.
        if let Some((earlier, earlier_targets)) = previous {
            if rank(&earlier).0 == rank(&source).0 && rank(&source) < last_of_line {
                assert_eq!(
                    earlier_targets.first().map(String::as_str),
                    Some(source.as_str()),
                    "case {earlier}: expected the next published version first among its \
                     targets, got {earlier_targets:?}"
                );
                assert_eq!(
                    targets,
                    earlier_targets[1..].to_vec(),
                    "case {earlier} -> {source}: expected the later source's targets to be \
                     the earlier one's minus itself, got {targets:?}"
                );
            }
        }
        previous = Some((source, targets));
    }
}

#[tokio::test]
async fn domain_filter_reports_the_version_that_domain_runs() {
    let emulator = Emulator::new();

    // One row per way a domain's engine version can differ: both engines, several
    // majors, a two-digit minor, the newest published version (no target at all), the
    // default a request omitting `EngineVersion` gets, and a pattern-valid version the
    // catalog does not publish.
    let rows: [(&str, Option<&str>, &str); 7] = [
        ("filter-es-five", Some("Elasticsearch_5.6"), "Elasticsearch_5.6"),
        ("filter-es-six", Some("Elasticsearch_6.8"), "Elasticsearch_6.8"),
        (
            "filter-es-seven-ten",
            Some("Elasticsearch_7.10"),
            "Elasticsearch_7.10",
        ),
        ("filter-os-one", Some("OpenSearch_1.3"), "OpenSearch_1.3"),
        ("filter-os-newest", Some("OpenSearch_2.19"), "OpenSearch_2.19"),
        ("filter-default", None, DEFAULT_ENGINE_VERSION),
        (
            "filter-unpublished",
            Some("OpenSearch_2.12"),
            "OpenSearch_2.12",
        ),
    ];

    for (name, requested, expected_source) in rows {
        settled_domain(&emulator, name, requested).await;
        let answer = compatible(&emulator, Some(name)).await;
        let filtered = entries(&answer, name);
        assert_eq!(
            filtered.len(),
            1,
            "case {name}: naming a domain limits the answer to that one domain's version, \
             got {filtered:?}"
        );
        assert_eq!(
            string_member(&filtered[0], "SourceVersion", name),
            expected_source,
            "case {name}: expected the version the domain runs, got {:?}",
            filtered[0]
        );
        let targets = target_versions(&filtered[0], name);
        assert_eq!(
            targets,
            expected_targets(expected_source),
            "case {name}: expected the published versions later than {expected_source}, got \
             {targets:?}"
        );
    }

    // The newest published version is a terminus even when it is a domain's version.
    let newest = compatible(&emulator, Some("filter-os-newest")).await;
    assert!(
        target_versions(&entries(&newest, "filter-os-newest")[0], "filter-os-newest").is_empty(),
        "case filter-os-newest: a domain on the newest version has nothing to upgrade to, \
         got {:?}",
        newest.body
    );

    // A pattern-valid name no domain has names no resource.
    for absent in ["no-such-domain", "filter-es-fivex", "abc"] {
        let answer = compatible(&emulator, Some(absent)).await;
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
}

#[tokio::test]
async fn reading_the_table_changes_nothing() {
    let emulator = Emulator::new();
    settled_domain(&emulator, "compat-read-twice", Some("Elasticsearch_6.8")).await;

    let first_table = entries(&compatible(&emulator, None).await, "first read");
    let first_filtered = entries(
        &compatible(&emulator, Some("compat-read-twice")).await,
        "first filtered read",
    );
    let described = emulator.describe("compat-read-twice").await.body;

    // A refused read reports nothing and changes nothing either.
    compatible(&emulator, Some("no-such-domain")).await;

    let second_table = entries(&compatible(&emulator, None).await, "second read");
    let second_filtered = entries(
        &compatible(&emulator, Some("compat-read-twice")).await,
        "second filtered read",
    );
    assert_eq!(
        first_table, second_table,
        "case repeat read: expected the same table the first read reported, got {} entries",
        second_table.len()
    );
    assert_eq!(
        first_filtered, second_filtered,
        "case repeat filtered read: expected the same answer the first read reported, got \
         {second_filtered:?}"
    );
    assert_eq!(
        described,
        emulator.describe("compat-read-twice").await.body,
        "case describe after reads: expected the domain record the first describe \
         reported"
    );

    let store = emulator.store();
    let state = store.lock();
    assert_eq!(
        state.domains.len(),
        1,
        "case stored domains: reporting compatible versions creates no domain, got {:?}",
        state.domains.keys().collect::<Vec<_>>()
    );
    let domain = state
        .domains
        .get("compat-read-twice")
        .expect("the created domain is stored");
    assert_eq!(
        domain.engine_version, "Elasticsearch_6.8",
        "case stored engine version: reporting compatible versions upgrades nothing, got {}",
        domain.engine_version
    );
}
