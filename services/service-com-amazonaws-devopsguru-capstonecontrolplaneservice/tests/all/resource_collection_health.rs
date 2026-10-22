// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeResourceCollectionHealth`.
//!
//! The harness (`Emulator`, `Reply`, the coverage-filter builders) is the one
//! `configuration_lifecycle` already uses: one buffered HTTP request through the same
//! [`smithy_server_runtime::MultiServiceRouter`] the emulator boots, so every assertion
//! is about wire behaviour.
//!
//! RULE under test: `DescribeResourceCollectionHealth` reports one health entry per
//! MEMBER of the coverage this service persists for the requested
//! `ResourceCollectionType` — one `CloudFormationHealth` per covered stack name, one
//! `TagHealth` per covered (`AppBoundaryKey`, `TagValue`) pair, and one `ServiceHealth`
//! per Amazon Web Services service whose resources were analysed — each entry's insight
//! tallies and `AnalyzedResourceCount` taken over the insights and resources the store
//! holds for that member, paged like every other read of the collection and leaving the
//! store unchanged.

use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

use service_com_amazonaws_devopsguru_capstonecontrolplaneservice::state::{
    InsightRecord, ResourceCoverage, PAGE_SIZE,
};

use crate::configuration_lifecycle::{cloud_formation, tags, Emulator, Reply};

/// `DescribeResourceCollectionHealth`'s HTTP binding, from
/// `smithy_query(query="operation",operation="DescribeResourceCollectionHealth")`:
/// `GET /accounts/health/resource-collection/{ResourceCollectionType}` with `NextToken`
/// as a query parameter.
async fn describe_health(emulator: &Emulator, collection_type: &str, next: Option<&str>) -> Reply {
    let path = match next {
        Some(token) => format!(
            "/accounts/health/resource-collection/{collection_type}?NextToken={token}"
        ),
        None => format!("/accounts/health/resource-collection/{collection_type}"),
    };
    emulator.call("GET", &path, None).await
}

/// One reported health entry, reduced to what the rule constrains: which collection
/// member it is about, and the tallies reported for that member.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    /// The member this entry reports on: a stack name, or `key=value` for a tag.
    member: String,
    open_proactive: Option<i64>,
    open_reactive: Option<i64>,
    mean_time_to_recover: Option<i64>,
    analyzed_resource_count: Option<i64>,
}

fn number(value: Option<&Value>) -> Option<i64> {
    value.and_then(Value::as_i64)
}

fn string(entry: &Value, member: &str) -> String {
    entry
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `CloudFormation` / `Tags` entries of one reply, in the order reported.
fn entries(reply: &Reply, pointer: &str) -> Vec<Entry> {
    reply
        .at(pointer)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    let member = if item.get("StackName").is_some() {
                        string(item, "StackName")
                    } else {
                        format!(
                            "{}={}",
                            string(item, "AppBoundaryKey"),
                            string(item, "TagValue")
                        )
                    };
                    let insight = item.get("Insight");
                    Entry {
                        member,
                        open_proactive: number(
                            insight.and_then(|i| i.get("OpenProactiveInsights")),
                        ),
                        open_reactive: number(insight.and_then(|i| i.get("OpenReactiveInsights"))),
                        mean_time_to_recover: number(
                            insight.and_then(|i| i.get("MeanTimeToRecoverInMilliseconds")),
                        ),
                        analyzed_resource_count: number(item.get("AnalyzedResourceCount")),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Walk every page of one collection type's health report, concatenating the entries.
async fn walk_health(emulator: &Emulator, collection_type: &str, pointer: &str) -> Vec<Entry> {
    let mut all: Vec<Entry> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let reply = describe_health(emulator, collection_type, token.as_deref()).await;
        assert_eq!(
            reply.status, 200,
            "case {collection_type:?}: describing the collection health must succeed, got {}",
            reply.raw
        );
        let page = entries(&reply, pointer);
        assert!(
            page.len() <= PAGE_SIZE,
            "case {collection_type:?}: expected at most {PAGE_SIZE} entries per page, got {}",
            page.len()
        );
        all.extend(page);
        token = reply
            .at("/NextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if token.is_none() {
            return all;
        }
    }
}

/// A covered member no insight is about: its insight tallies are taken over an empty
/// set — nothing open, and no closed insight to average a recovery time over — while
/// its `AnalyzedResourceCount` is the ONE monitored resource the coverage entry itself
/// is, the rule `ResourceCoverage::monitored_resource_count` states and
/// `DescribeAccountHealth` reports the account-level count under.
fn member_without_insights(member: &str) -> Entry {
    Entry {
        member: member.to_string(),
        open_proactive: Some(0),
        open_reactive: Some(0),
        mean_time_to_recover: Some(0),
        analyzed_resource_count: Some(1),
    }
}

/// One step of a coverage case: an `Action` and the filter halves it carries.
type Step<'a> = (&'a str, &'a [&'a str], &'a [(&'a str, &'a [&'a str])]);

struct Case<'a> {
    name: &'a str,
    steps: &'a [Step<'a>],
    /// The stack names expected to have a `CloudFormationHealth`, in report order.
    expect_stacks: &'a [&'a str],
    /// The `AppBoundaryKey=TagValue` pairs expected to have a `TagHealth`, in order.
    expect_tags: &'a [&'a str],
}

/// RULE: the health report of a collection type has exactly one entry per member of the
/// persisted coverage of that type — a stack name for `AWS_CLOUD_FORMATION`, an
/// (`AppBoundaryKey`, `TagValue`) pair for `AWS_TAGS` — so coverage a client added is
/// reported and coverage it removed is not, and only the requested type's list appears.
///
/// The rows differ along which half of the collection is covered, the member count, the
/// number of values per key, and whether coverage was later removed; the recorded
/// workflow's single `ADD ["my-stack"]` then `AWS_CLOUD_FORMATION` read is one row.
#[tokio::test]
async fn health_reports_one_entry_per_covered_member() {
    let cases = [
        Case {
            name: "no coverage at all",
            steps: &[],
            expect_stacks: &[],
            expect_tags: &[],
        },
        Case {
            name: "one stack added",
            steps: &[("ADD", &["my-stack"], &[])],
            expect_stacks: &["my-stack"],
            expect_tags: &[],
        },
        Case {
            name: "three stacks added out of order are reported sorted",
            steps: &[("ADD", &["zeta", "alpha", "middle"], &[])],
            expect_stacks: &["alpha", "middle", "zeta"],
            expect_tags: &[],
        },
        Case {
            name: "one app boundary key with two tag values",
            steps: &[(
                "ADD",
                &[],
                &[("Devops-guru-production", &["web", "batch"])],
            )],
            expect_stacks: &[],
            expect_tags: &["Devops-guru-production=batch", "Devops-guru-production=web"],
        },
        Case {
            name: "two app boundary keys alongside stacks",
            steps: &[(
                "ADD",
                &["payments", "ingest"],
                &[
                    ("Devops-guru-rds", &["primary"]),
                    ("Devops-guru-web", &["edge", "origin"]),
                ],
            )],
            expect_stacks: &["ingest", "payments"],
            expect_tags: &[
                "Devops-guru-rds=primary",
                "Devops-guru-web=edge",
                "Devops-guru-web=origin",
            ],
        },
        Case {
            name: "a stack removed again has no health entry",
            steps: &[
                ("ADD", &["kept", "dropped"], &[]),
                ("REMOVE", &["dropped"], &[]),
            ],
            expect_stacks: &["kept"],
            expect_tags: &[],
        },
        Case {
            name: "a tag value removed leaves only the remaining pair",
            steps: &[
                ("ADD", &[], &[("Devops-guru-app", &["keep", "drop"])]),
                ("REMOVE", &[], &[("Devops-guru-app", &["drop"])]),
            ],
            expect_stacks: &[],
            expect_tags: &["Devops-guru-app=keep"],
        },
        Case {
            name: "an app boundary key emptied of values disappears entirely",
            steps: &[
                ("ADD", &[], &[("Devops-guru-app", &["only"])]),
                ("REMOVE", &[], &[("Devops-guru-app", &["only"])]),
            ],
            expect_stacks: &[],
            expect_tags: &[],
        },
        Case {
            name: "a tag value may be the empty string",
            steps: &[("ADD", &[], &[("Devops-guru-app", &[""])])],
            expect_stacks: &[],
            expect_tags: &["Devops-guru-app="],
        },
    ];

    for Case {
        name,
        steps,
        expect_stacks,
        expect_tags,
    } in cases
    {
        let emulator = Emulator::new();
        for (action, stacks, tag_entries) in steps {
            let mut collection = json!({});
            if !stacks.is_empty() {
                collection = cloud_formation(stacks);
            }
            if !tag_entries.is_empty() {
                let tag_filter = tags(tag_entries);
                if stacks.is_empty() {
                    collection = tag_filter;
                } else {
                    collection["Tags"] = tag_filter["Tags"].clone();
                }
            }
            let reply = emulator
                .update_resource_collection(action, collection)
                .await;
            assert_eq!(
                reply.status, 200,
                "case {name:?}: the {action} must succeed, got {}",
                reply.raw
            );
        }

        // The CloudFormation half: one entry per covered stack name.
        let expected: Vec<Entry> = expect_stacks
            .iter()
            .map(|s| member_without_insights(s))
            .collect();
        let reported = walk_health(&emulator, "AWS_CLOUD_FORMATION", "/CloudFormation").await;
        assert_eq!(
            reported, expected,
            "case {name:?}: expected CloudFormation health {expected:?}, got {reported:?}"
        );

        // The tag half: one entry per covered (AppBoundaryKey, TagValue) pair.
        let expected: Vec<Entry> = expect_tags
            .iter()
            .map(|s| member_without_insights(s))
            .collect();
        let reported = walk_health(&emulator, "AWS_TAGS", "/Tags").await;
        assert_eq!(
            reported, expected,
            "case {name:?}: expected Tag health {expected:?}, got {reported:?}"
        );

        // The coverage the health report is derived from is the coverage
        // `GetResourceCollection` reports — the two reads agree on the member set.
        let covered_stacks = emulator.all_stack_names().await;
        assert_eq!(
            covered_stacks,
            expect_stacks
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<String>>(),
            "case {name:?}: GetResourceCollection and the health report must agree, got {covered_stacks:?}"
        );
        let covered_pairs: Vec<String> = emulator
            .all_tag_collections()
            .await
            .into_iter()
            .flat_map(|(key, values)| {
                values
                    .into_iter()
                    .map(move |value| format!("{key}={value}"))
            })
            .collect();
        assert_eq!(
            covered_pairs,
            expect_tags
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<String>>(),
            "case {name:?}: GetResourceCollection and the health report must agree, got {covered_pairs:?}"
        );

        // Only the requested type's list is reported: the other two members of
        // `DescribeResourceCollectionHealthResponse` stay absent.
        let reply = describe_health(&emulator, "AWS_CLOUD_FORMATION", None).await;
        for absent in ["/Tags", "/Service"] {
            assert!(
                reply.at(absent).is_none(),
                "case {name:?}: an AWS_CLOUD_FORMATION read must not report {absent}, got {}",
                reply.raw
            );
        }
        let reply = describe_health(&emulator, "AWS_TAGS", None).await;
        for absent in ["/CloudFormation", "/Service"] {
            assert!(
                reply.at(absent).is_none(),
                "case {name:?}: an AWS_TAGS read must not report {absent}, got {}",
                reply.raw
            );
        }

        // AWS_SERVICE membership is discovered by DevOps Guru's own analysis, which
        // this emulator does not perform, so the reported list is present and empty
        // whatever the client-declared coverage is.
        let reply = describe_health(&emulator, "AWS_SERVICE", None).await;
        assert_eq!(
            reply.status, 200,
            "case {name:?}: an AWS_SERVICE read must succeed, got {}",
            reply.raw
        );
        assert_eq!(
            reply.at("/Service").and_then(Value::as_array),
            Some(&Vec::new()),
            "case {name:?}: expected an empty Service list, got {}",
            reply.raw
        );
        for absent in ["/CloudFormation", "/Tags", "/NextToken"] {
            assert!(
                reply.at(absent).is_none(),
                "case {name:?}: an AWS_SERVICE read must not report {absent}, got {}",
                reply.raw
            );
        }
    }
}

/// RULE: a paginated read walks the whole collection — the pages concatenated are every
/// member exactly once, in order, with no repeats and no gaps, and the last page carries
/// no `NextToken`.
///
/// The row count spans the page boundary (`PAGE_SIZE + 3` covered stacks), which is the
/// dimension this rule generalises over.
#[tokio::test]
async fn health_pages_walk_every_covered_member_exactly_once() {
    let emulator = Emulator::new();
    let total = PAGE_SIZE + 3;

    let mut covered = 0;
    while covered < total {
        let batch: Vec<String> = (covered..(covered + 100).min(total))
            .map(|index| format!("stack-{index:05}"))
            .collect();
        covered += batch.len();
        let reply = emulator
            .update_resource_collection("ADD", json!({ "CloudFormation": { "StackNames": batch } }))
            .await;
        assert_eq!(
            reply.status, 200,
            "covering {covered} stacks must succeed, got {}",
            reply.raw
        );
    }

    let first = describe_health(&emulator, "AWS_CLOUD_FORMATION", None).await;
    assert_eq!(
        entries(&first, "/CloudFormation").len(),
        PAGE_SIZE,
        "a full first page must carry {PAGE_SIZE} entries, got {}",
        first.raw
    );
    assert!(
        first.at("/NextToken").is_some(),
        "a collection larger than one page must offer a NextToken, got {}",
        first.raw
    );

    let walked = walk_health(&emulator, "AWS_CLOUD_FORMATION", "/CloudFormation").await;
    let expected: Vec<Entry> = (0..total)
        .map(|index| member_without_insights(&format!("stack-{index:05}")))
        .collect();
    assert_eq!(
        walked.len(),
        total,
        "the walk must report {total} entries, got {}",
        walked.len()
    );
    assert_eq!(
        walked, expected,
        "the walk must report every covered stack exactly once in order, got {:?}",
        walked.iter().map(|e| e.member.clone()).collect::<Vec<_>>()
    );
}

/// RULE: a `NextToken` names a page of ONE collection, so a token this service never
/// issued for the collection being read names no page of it and is refused with the
/// operation's HTTP 400 `ValidationException` rather than silently seeking elsewhere.
///
/// The rows differ along how the token fails to belong: another collection type's
/// cursor, another operation's cursor over the same type, an offset edited off a page
/// boundary, and a rewrite outside the modelled lowercase-hex form.
#[tokio::test]
async fn a_token_from_another_collection_is_refused() {
    let emulator = Emulator::new();

    // Enough coverage on both halves that each read offers a second page.
    let mut covered = 0;
    while covered < PAGE_SIZE + 1 {
        let batch: Vec<String> = (covered..(covered + 100).min(PAGE_SIZE + 1))
            .map(|index| format!("stack-{index:05}"))
            .collect();
        covered += batch.len();
        emulator
            .update_resource_collection("ADD", json!({ "CloudFormation": { "StackNames": batch } }))
            .await;
    }
    // `UpdateTagCollectionFilter.TagValues` is modelled len 0..100, so the values are
    // submitted in batches of at most 100.
    let mut valued = 0;
    while valued < PAGE_SIZE + 1 {
        let batch: Vec<String> = (valued..(valued + 100).min(PAGE_SIZE + 1))
            .map(|index| format!("value-{index:05}"))
            .collect();
        valued += batch.len();
        let reply = emulator
            .update_resource_collection(
                "ADD",
                json!({ "Tags": [{ "AppBoundaryKey": "Devops-guru-app", "TagValues": batch }] }),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "covering {valued} tag values must succeed, got {}",
            reply.raw
        );
    }

    let token_of = |reply: &Reply| -> String {
        reply
            .at("/NextToken")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("a second page must offer a NextToken, got {}", reply.raw))
            .to_string()
    };

    let health_cloud_formation =
        token_of(&describe_health(&emulator, "AWS_CLOUD_FORMATION", None).await);
    let health_tags = token_of(&describe_health(&emulator, "AWS_TAGS", None).await);
    let collection_cloud_formation = token_of(
        &emulator
            .get_resource_collection("AWS_CLOUD_FORMATION", None)
            .await,
    );

    let cases: Vec<(&str, Reply)> = vec![
        (
            "an AWS_TAGS health cursor replayed against AWS_CLOUD_FORMATION",
            describe_health(&emulator, "AWS_CLOUD_FORMATION", Some(&health_tags)).await,
        ),
        (
            "an AWS_CLOUD_FORMATION health cursor replayed against AWS_TAGS",
            describe_health(&emulator, "AWS_TAGS", Some(&health_cloud_formation)).await,
        ),
        (
            "a GetResourceCollection cursor replayed against the health report",
            describe_health(
                &emulator,
                "AWS_CLOUD_FORMATION",
                Some(&collection_cloud_formation),
            )
            .await,
        ),
        (
            "a health cursor replayed against GetResourceCollection",
            emulator
                .get_resource_collection("AWS_CLOUD_FORMATION", Some(&health_cloud_formation))
                .await,
        ),
        (
            "a health cursor whose offset was hand-edited off a page boundary",
            {
                // The digest binds the token to its collection but is not a secret, so
                // the offset group is the part a caller can plausibly edit.
                let edited = format!("00000007{}", &health_cloud_formation[8..]);
                describe_health(&emulator, "AWS_CLOUD_FORMATION", Some(&edited)).await
            },
        ),
        (
            "a health cursor rewritten in uppercase hex",
            describe_health(
                &emulator,
                "AWS_CLOUD_FORMATION",
                Some(&health_cloud_formation.to_uppercase()),
            )
            .await,
        ),
    ];

    for (name, reply) in cases {
        assert_eq!(
            reply.status, 400,
            "case {name:?}: expected a 400 refusal, got status {} body {}",
            reply.status, reply.raw
        );
        assert!(
            reply.names_error("ValidationException"),
            "case {name:?}: expected a ValidationException, got {}",
            reply.raw
        );
    }

    // The token each read DID issue resolves against its own collection.
    for (collection_type, pointer, token) in [
        (
            "AWS_CLOUD_FORMATION",
            "/CloudFormation",
            &health_cloud_formation,
        ),
        ("AWS_TAGS", "/Tags", &health_tags),
    ] {
        let reply = describe_health(&emulator, collection_type, Some(token)).await;
        assert_eq!(
            reply.status, 200,
            "case {collection_type:?}: the cursor this read issued must resolve, got {}",
            reply.raw
        );
        assert_eq!(
            entries(&reply, pointer).len(),
            1,
            "case {collection_type:?}: expected the 1 remaining entry on the second page, got {}",
            reply.raw
        );
    }
}

/// RULE: a read must not change what a later caller sees — describing the health of a
/// collection neither mutates the coverage nor makes the next report differ.
///
/// The rows are every modelled `ResourceCollectionType`, read repeatedly and in turn.
#[tokio::test]
async fn describing_health_leaves_the_store_unchanged() {
    let emulator = Emulator::new();
    emulator
        .update_resource_collection("ADD", cloud_formation(&["alpha", "beta"]))
        .await;
    emulator
        .update_resource_collection("ADD", tags(&[("Devops-guru-app", &["web", "batch"])]))
        .await;

    let coverage_before = (
        emulator.all_stack_names().await,
        emulator.all_tag_collections().await,
    );

    // `ResourceCollectionType`'s full value list, from
    // `smithy_query(query="shape",shape="ResourceCollectionType")`.
    let collection_types = ["AWS_CLOUD_FORMATION", "AWS_SERVICE", "AWS_TAGS"];
    let mut first_bodies: Vec<String> = Vec::new();
    for collection_type in collection_types {
        let reply = describe_health(&emulator, collection_type, None).await;
        assert_eq!(
            reply.status, 200,
            "case {collection_type:?}: the read must succeed, got {}",
            reply.raw
        );
        first_bodies.push(reply.body.to_string());
    }

    for round in 1..=3 {
        for (index, collection_type) in collection_types.iter().enumerate() {
            let reply = describe_health(&emulator, collection_type, None).await;
            assert_eq!(
                reply.body.to_string(),
                first_bodies[index],
                "case {collection_type:?} round {round}: a repeated read must report the same health, got {}",
                reply.raw
            );
        }
    }

    let coverage_after = (
        emulator.all_stack_names().await,
        emulator.all_tag_collections().await,
    );
    assert_eq!(
        coverage_after, coverage_before,
        "reading the health must not change the coverage: {coverage_before:?} before, {coverage_after:?} after"
    );
}

/// RULE: `ResourceCollectionType` is a closed enum, so a value outside it is refused as
/// a bad request (HTTP 400 `ValidationException`) rather than answered with a report.
///
/// The rows differ in how the value misses the enum: a plausible AWS-shaped name, a
/// value of the wrong case, and a near-miss spelling of a real one.
#[tokio::test]
async fn a_collection_type_outside_the_enum_is_refused() {
    let emulator = Emulator::new();

    for value in ["AWS_LAMBDA", "aws_tags", "AWS_CLOUDFORMATION"] {
        let reply = describe_health(&emulator, value, None).await;
        assert_eq!(
            reply.status, 400,
            "case {value:?}: expected a 400 refusal, got status {} body {}",
            reply.status, reply.raw
        );
        assert!(
            reply.names_error("ValidationException"),
            "case {value:?}: expected a ValidationException, got {}",
            reply.raw
        );
    }
}

// ---------------------------------------------------------------------------
// The insight tallies.
//
// No modelled operation mints an insight — DevOps Guru derives one from anomalies in
// the covered resources — so the suite seeds the store's `insights` map the way that
// analysis would and then reads the health over the wire. That is what makes the
// tallies checkable at all: with an empty map every count is zero, which on its own
// could not tell a computed tally from a constant.
// ---------------------------------------------------------------------------

/// `InsightType`'s and `InsightStatus`'s values, from
/// `smithy_query(query="shape",shape="InsightType,InsightStatus")`.
const PROACTIVE: &str = "PROACTIVE";
const REACTIVE: &str = "REACTIVE";
const ONGOING: &str = "ONGOING";
const CLOSED: &str = "CLOSED";

/// An arbitrary instant the seeded insights start from, in milliseconds since the
/// epoch.
const START: i64 = 1_700_000_000_000;

/// One insight, as the analysis that produces them would store it: about the stacks and
/// tag pairs in its own `ResourceCollection`, naming `services`, and — when
/// `recovery_millis` is given — closed after that many milliseconds.
fn insight(
    id: &str,
    insight_type: &str,
    status: &str,
    recovery_millis: Option<i64>,
    stacks: &[&str],
    tag_pairs: &[(&str, &str)],
    services: &[&str],
) -> InsightRecord {
    let coverage = ResourceCoverage {
        cloudformation_stack_names: stacks.iter().map(ToString::to_string).collect(),
        tag_collections: tag_pairs.iter().fold(
            BTreeMap::<String, BTreeSet<String>>::new(),
            |mut acc, (key, value)| {
                acc.entry(key.to_string())
                    .or_default()
                    .insert(value.to_string());
                acc
            },
        ),
    };
    let names_resources = !stacks.is_empty() || !tag_pairs.is_empty();
    InsightRecord {
        id: id.to_string(),
        insight_type: insight_type.to_string(),
        status: status.to_string(),
        start_time_millis: START,
        end_time_millis: recovery_millis.map(|millis| START + millis),
        resource_collection: names_resources.then_some(coverage),
        service_names: services.iter().map(ToString::to_string).collect(),
        ..InsightRecord::default()
    }
}

/// The entry reported for `member`, or `None` when the report has none.
fn entry_for(reply: &Reply, pointer: &str, member: &str) -> Option<Entry> {
    entries(reply, pointer)
        .into_iter()
        .find(|entry| entry.member == member)
}

/// The tallies one case expects: open proactive insights, open reactive insights and
/// the mean recovery time in milliseconds.
type Tallies = (i64, i64, i64);

/// One row of the tally table: the case name, the insights seeded, the tallies
/// expected for the member under test.
type TallyCase<'a> = (&'a str, Vec<InsightRecord>, Tallies);

/// RULE: a member's `InsightHealth` counts the stored insights ABOUT THAT MEMBER — the
/// ONGOING ones by `InsightType`, and the mean recovery time of the CLOSED ones, which
/// is the mean of each one's `InsightTimeRange` end minus its start.
///
/// The rows differ along insight type, status, how many insights there are, the
/// recovery times averaged, which member the insight is about, and whether it names
/// resources at all.
#[tokio::test]
async fn a_members_tallies_count_the_insights_about_that_member() {
    // (case, seeded insights, expected open proactive / open reactive / MTTR)
    let cases: Vec<TallyCase> = vec![
        ("no insight at all", vec![], (0, 0, 0)),
        (
            "two ongoing proactive and one ongoing reactive insight",
            vec![
                insight("i1", PROACTIVE, ONGOING, None, &["watched"], &[], &[]),
                insight("i2", PROACTIVE, ONGOING, None, &["watched"], &[], &[]),
                insight("i3", REACTIVE, ONGOING, None, &["watched"], &[], &[]),
            ],
            (2, 1, 0),
        ),
        (
            "an ongoing insight about another stack is not this member's",
            vec![insight(
                "i1",
                PROACTIVE,
                ONGOING,
                None,
                &["elsewhere"],
                &[],
                &[],
            )],
            (0, 0, 0),
        ),
        (
            "an insight that names no resources is nobody's member",
            vec![insight("i1", REACTIVE, ONGOING, None, &[], &[], &["RDS"])],
            (0, 0, 0),
        ),
        (
            "two closed insights average their recovery times",
            vec![
                insight("i1", REACTIVE, CLOSED, Some(1_000), &["watched"], &[], &[]),
                insight("i2", PROACTIVE, CLOSED, Some(3_000), &["watched"], &[], &[]),
            ],
            (0, 0, 2_000),
        ),
        (
            "a closed insight with no end instant has no recovery time to average",
            vec![insight("i1", REACTIVE, CLOSED, None, &["watched"], &[], &[])],
            (0, 0, 0),
        ),
        (
            "open and closed insights are tallied independently",
            vec![
                insight("i1", PROACTIVE, ONGOING, None, &["watched"], &[], &[]),
                insight("i2", REACTIVE, ONGOING, None, &["watched"], &[], &[]),
                insight("i3", REACTIVE, CLOSED, Some(9_000), &["watched"], &[], &[]),
            ],
            (1, 1, 9_000),
        ),
        (
            "an insight about several stacks counts for each of them",
            vec![insight(
                "i1",
                PROACTIVE,
                ONGOING,
                None,
                &["watched", "elsewhere"],
                &[],
                &[],
            )],
            (1, 0, 0),
        ),
    ];

    for (name, insights, (proactive, reactive, mttr)) in cases {
        let emulator = Emulator::new();
        emulator
            .update_resource_collection("ADD", cloud_formation(&["watched", "elsewhere"]))
            .await;
        emulator.seed_insights(insights);

        let reply = describe_health(&emulator, "AWS_CLOUD_FORMATION", None).await;
        assert_eq!(
            reply.status, 200,
            "case {name:?}: the read must succeed, got {}",
            reply.raw
        );
        let reported = entry_for(&reply, "/CloudFormation", "watched");
        assert_eq!(
            reported,
            Some(Entry {
                member: "watched".to_string(),
                open_proactive: Some(proactive),
                open_reactive: Some(reactive),
                mean_time_to_recover: Some(mttr),
                // One monitored resource per coverage entry, whatever the insights.
                analyzed_resource_count: Some(1),
            }),
            "case {name:?}: expected ({proactive}, {reactive}, {mttr}) for the covered stack, got {reported:?}"
        );
        // The other covered stack is still reported, with its own tally, so one
        // member's insights never stand in for another's.
        let other = entry_for(&reply, "/CloudFormation", "elsewhere");
        assert!(
            other.is_some(),
            "case {name:?}: every covered stack must still be reported, got {}",
            reply.raw
        );
    }
}

/// RULE: the same tally, for a tag member, counts the insights whose own tag collection
/// names that (`AppBoundaryKey`, `TagValue`) pair — a pair is a member in its own right,
/// so an insight about one value of a key is not about the key's other values.
#[tokio::test]
async fn a_tag_members_tallies_count_the_insights_about_that_pair() {
    let emulator = Emulator::new();
    emulator
        .update_resource_collection("ADD", tags(&[("Devops-guru-app", &["web", "batch"])]))
        .await;
    emulator.seed_insights(vec![
        insight(
            "i1",
            PROACTIVE,
            ONGOING,
            None,
            &[],
            &[("Devops-guru-app", "web")],
            &[],
        ),
        insight(
            "i2",
            REACTIVE,
            ONGOING,
            None,
            &[],
            &[("Devops-guru-app", "web")],
            &[],
        ),
        insight(
            "i3",
            REACTIVE,
            CLOSED,
            Some(5_000),
            &[],
            &[("Devops-guru-app", "batch")],
            &[],
        ),
        // Another key entirely: a member of neither pair above.
        insight(
            "i4",
            PROACTIVE,
            ONGOING,
            None,
            &[],
            &[("Devops-guru-other", "web")],
            &[],
        ),
    ]);

    let reply = describe_health(&emulator, "AWS_TAGS", None).await;
    let expected: [(&str, Tallies); 2] = [
        ("Devops-guru-app=batch", (0, 0, 5_000)),
        ("Devops-guru-app=web", (1, 1, 0)),
    ];
    for (member, (proactive, reactive, mttr)) in expected {
        let reported = entry_for(&reply, "/Tags", member);
        assert_eq!(
            reported,
            Some(Entry {
                member: member.to_string(),
                open_proactive: Some(proactive),
                open_reactive: Some(reactive),
                mean_time_to_recover: Some(mttr),
                // One monitored resource per coverage entry, whatever the insights.
                analyzed_resource_count: Some(1),
            }),
            "case {member:?}: expected ({proactive}, {reactive}, {mttr}), got {reported:?}"
        );
    }
}

/// RULE: `AWS_SERVICE` reports one entry per Amazon Web Services service the stored
/// insights name — the store's record of a service DevOps Guru analysed — each carrying
/// that service's open proactive and reactive counts and nothing else, so a service no
/// insight names is not reported.
#[tokio::test]
async fn service_health_reports_the_services_the_stored_insights_name() {
    let emulator = Emulator::new();
    emulator.seed_insights(vec![
        insight("i1", PROACTIVE, ONGOING, None, &[], &[], &["RDS", "LAMBDA"]),
        insight("i2", REACTIVE, ONGOING, None, &[], &[], &["RDS"]),
        insight("i3", REACTIVE, CLOSED, Some(7_000), &[], &[], &["S3"]),
    ]);

    let reply = describe_health(&emulator, "AWS_SERVICE", None).await;
    assert_eq!(
        reply.status, 200,
        "the AWS_SERVICE read must succeed, got {}",
        reply.raw
    );
    let reported = reply
        .at("/Service")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let names: Vec<String> = reported
        .iter()
        .map(|entry| {
            entry
                .get("ServiceName")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        names,
        vec!["LAMBDA".to_string(), "RDS".to_string(), "S3".to_string()],
        "expected the named services, sorted, got {}",
        reply.raw
    );

    // `ServiceInsightHealth` models the two open counts and no MTTR.
    let expected = [("LAMBDA", (1, 0)), ("RDS", (1, 1)), ("S3", (0, 0))];
    for (service, (proactive, reactive)) in expected {
        let entry = reported
            .iter()
            .find(|entry| entry.get("ServiceName").and_then(Value::as_str) == Some(service))
            .unwrap_or_else(|| panic!("case {service:?}: must be reported, got {}", reply.raw));
        let insight = entry.get("Insight");
        assert_eq!(
            insight
                .and_then(|i| i.get("OpenProactiveInsights"))
                .and_then(Value::as_i64),
            Some(proactive),
            "case {service:?}: expected {proactive} open proactive insights, got {entry}"
        );
        assert_eq!(
            insight
                .and_then(|i| i.get("OpenReactiveInsights"))
                .and_then(Value::as_i64),
            Some(reactive),
            "case {service:?}: expected {reactive} open reactive insights, got {entry}"
        );
        assert!(
            insight
                .and_then(|i| i.get("MeanTimeToRecoverInMilliseconds"))
                .is_none(),
            "case {service:?}: ServiceInsightHealth models no MTTR, got {entry}"
        );
        // The coverage attributes no entry to a service, so the store does not know
        // how many monitored resources belong to one: the optional member is omitted
        // rather than guessed.
        assert!(
            entry.get("AnalyzedResourceCount").is_none(),
            "case {service:?}: a service's analysed-resource count is not recorded, got {entry}"
        );
    }
}

/// One row of the sum table: the case name, the covered stack names, and the covered
/// app-boundary keys with their tag values.
type CoverageRow<'a> = (&'a str, &'a [&'a str], &'a [(&'a str, &'a [&'a str])]);

/// RULE: the per-member `AnalyzedResourceCount` follows the same analysed-resource rule
/// as the account-level one — each coverage entry is one monitored resource — so the
/// counts this report lists for a collection type, summed over every member of both
/// types, equal the `AnalyzedResourceCount` `DescribeAccountHealth` reports over the
/// same store. Two reads of one store must not contradict each other.
///
/// The rows differ along how the coverage is distributed between the two halves and how
/// many entries each holds.
#[tokio::test]
async fn member_counts_sum_to_the_account_analysed_resource_count() {
    // (case, covered stack names, covered (key, values) pairs)
    let cases: Vec<CoverageRow> = vec![
        ("no coverage", &[], &[]),
        ("one stack", &["only"], &[]),
        ("three stacks", &["a", "b", "c"], &[]),
        (
            "one key with three values",
            &[],
            &[("Devops-guru-app", &["x", "y", "z"])],
        ),
        (
            "both halves at once",
            &["alpha", "beta"],
            &[
                ("Devops-guru-app", &["web"]),
                ("Devops-guru-rds", &["primary", "replica"]),
            ],
        ),
    ];

    for (name, stacks, tag_entries) in cases {
        let emulator = Emulator::new();
        if !stacks.is_empty() {
            emulator
                .update_resource_collection("ADD", cloud_formation(stacks))
                .await;
        }
        if !tag_entries.is_empty() {
            emulator
                .update_resource_collection("ADD", tags(tag_entries))
                .await;
        }

        let reported: i64 = walk_health(&emulator, "AWS_CLOUD_FORMATION", "/CloudFormation")
            .await
            .into_iter()
            .chain(walk_health(&emulator, "AWS_TAGS", "/Tags").await)
            .map(|entry| entry.analyzed_resource_count.unwrap_or_default())
            .sum();

        // `DescribeAccountHealth`: GET /accounts/health, from
        // `smithy_query(query="operation",operation="DescribeAccountHealth")`.
        let account = emulator.call("GET", "/accounts/health", None).await;
        assert_eq!(
            account.status, 200,
            "case {name:?}: the account health read must succeed, got {}",
            account.raw
        );
        let account_count = account
            .at("/AnalyzedResourceCount")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        assert_eq!(
            reported, account_count,
            "case {name:?}: the members' counts sum to {reported}, but DescribeAccountHealth reports {account_count}"
        );
    }
}
