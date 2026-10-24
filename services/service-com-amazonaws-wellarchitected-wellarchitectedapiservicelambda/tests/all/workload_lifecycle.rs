// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the workload lifecycle: `CreateWorkload`, `GetWorkload`,
//! `ListWorkloads`, `UpdateWorkload`, `DeleteWorkload`.
//!
//! The HTTP client, the store seeder and the request builders live in
//! `super::workload_harness`, shared with the other workload suites.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::workload_harness::{created_id, minimal_create, Client};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// How many questions the AWS Well-Architected Framework lens asks — its published
/// numbering, OPS 11, SEC 11, REL 13, PERF 5, COST 11, SUS 6. Spelled out so the roll-up
/// below is anchored to a number rather than only to the catalog it reads.
const FRAMEWORK_LENS_QUESTIONS: usize = 11 + 11 + 13 + 5 + 11 + 6;

/// CreateWorkload stores the whole record and GetWorkload rebuilds it: a table spanning
/// the member classes the request can carry — list sizes, optional strings, tags,
/// both `Environment` values (including `PREPRODUCTION`, which the issue did not cite),
/// and the published `WorkloadName`/`Description` length bounds.
#[tokio::test]
async fn created_workload_round_trips_through_get_workload() {
    // Values taken from `smithy_query(query="shape", shape="WorkloadEnvironment")`.
    let cases: Vec<(&str, Value)> = vec![
        ("minimal", minimal_create("minimal-workload", "tok-minimal")),
        (
            "every-member",
            json!({
                "WorkloadName": "every member workload",
                "Description": "persists every optional member",
                "Environment": "PREPRODUCTION",
                "Lenses": ["wellarchitected", "serverless", "softwareasaservice"],
                "AccountIds": ["111111111111", "222222222222"],
                "AwsRegions": ["us-east-1", "eu-west-1", "ap-south-1"],
                "NonAwsRegions": ["on-prem-dc-1"],
                "PillarPriorities": ["security", "reliability", "costOptimization"],
                "ArchitecturalDesign": "https://example.com/design",
                "ReviewOwner": "architecture-guild",
                "IndustryType": "InfoTech",
                "Industry": "Software",
                "Notes": "created by the integration test",
                "Applications": ["arn:aws:servicecatalog:us-east-1:000000000000:/applications/app1"],
                "Tags": {"env": "test", "owner": "guild"},
                "ClientRequestToken": "tok-every-member",
            }),
        ),
        (
            "non-aws-regions-only",
            json!({
                "WorkloadName": "non aws only",
                "Description": "only non-AWS regions are given",
                "Environment": "PRODUCTION",
                "Lenses": [],
                "NonAwsRegions": ["dc-1", "dc-2", "dc-3", "dc-4", "dc-5"],
                "ReviewOwner": "ops",
                "ClientRequestToken": "tok-non-aws",
            }),
        ),
        // The published `WorkloadName` @length bound is 3..100 and `Description` 3..250.
        (
            "shortest-name-and-description",
            json!({
                "WorkloadName": "abc",
                "Description": "xyz",
                "Environment": "PRODUCTION",
                "Lenses": ["wellarchitected"],
                "AwsRegions": ["us-west-2"],
                "ReviewOwner": "own",
                "ClientRequestToken": "tok-short",
            }),
        ),
        (
            "longest-name-and-description",
            json!({
                "WorkloadName": "n".repeat(100),
                "Description": "d".repeat(250),
                "Environment": "PRODUCTION",
                "Lenses": ["wellarchitected"],
                "AwsRegions": ["us-west-2"],
                "ReviewOwner": "o".repeat(255),
                "ClientRequestToken": "tok-long",
            }),
        ),
    ];

    let client = Client::new();
    for (case, request) in cases {
        let created = client.create(request.clone()).await;
        let workload_id = created_id(&created, case);
        let expected_arn =
            format!("arn:aws:wellarchitected:us-west-2:000000000000:workload/{workload_id}");
        assert_eq!(
            created.body.get("WorkloadArn").and_then(Value::as_str),
            Some(expected_arn.as_str()),
            "case {case:?}: expected WorkloadArn {expected_arn:?}, got {:?}",
            created.body
        );

        let fetched = client.get(&workload_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected GetWorkload 200, got {fetched:?}"
        );
        let workload = fetched
            .body
            .get("Workload")
            .cloned()
            .unwrap_or(Value::Null);

        assert_eq!(
            workload.get("WorkloadId").and_then(Value::as_str),
            Some(workload_id.as_str()),
            "case {case:?}: GetWorkload must echo the created id, got {workload:?}"
        );
        assert_eq!(
            workload.get("WorkloadArn").and_then(Value::as_str),
            Some(expected_arn.as_str()),
            "case {case:?}: GetWorkload must echo the created ARN, got {workload:?}"
        );
        assert_eq!(
            workload.get("Owner").and_then(Value::as_str),
            Some("000000000000"),
            "case {case:?}: Owner must be the emulator account id, got {workload:?}"
        );
        assert_eq!(
            workload.get("ImprovementStatus").and_then(Value::as_str),
            Some("NOT_APPLICABLE"),
            "case {case:?}: a workload with no answered lens review is NOT_APPLICABLE, got {workload:?}"
        );
        assert!(
            workload.get("UpdatedAt").is_some(),
            "case {case:?}: UpdatedAt must be set, got {workload:?}"
        );
        // The risk roll-up carries every `Risk` bucket. Nothing can answer a question
        // yet, so `UNANSWERED` is the number of questions the ATTACHED lenses ask — each
        // attached lens contributes its own AWS-official catalog entry's question set, the
        // Well-Architected Framework lens's being its published 57 — and every other
        // bucket is 0.
        assert_eq!(
            state::lens_question_count("wellarchitected"),
            FRAMEWORK_LENS_QUESTIONS,
            "the framework lens asks {FRAMEWORK_LENS_QUESTIONS} questions"
        );
        let attached: BTreeSet<String> = request
            .get("Lenses")
            .and_then(Value::as_array)
            .map(|lenses| {
                lenses
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|lens| state::lens_alias(lens).to_string())
                    .collect()
            })
            .unwrap_or_default();
        let unanswered: usize = attached
            .iter()
            .map(|lens| state::lens_question_count(lens))
            .sum();
        assert_eq!(
            workload.get("RiskCounts"),
            Some(&json!({
                "UNANSWERED": unanswered,
                "HIGH": 0,
                "MEDIUM": 0,
                "NONE": 0,
                "NOT_APPLICABLE": 0,
            })),
            "case {case:?}: expected the roll-up over the lenses {attached:?} ask, got \
             {workload:?}"
        );
        assert_eq!(
            workload.get("IsReviewOwnerUpdateAcknowledged"),
            Some(&Value::Bool(true)),
            "case {case:?}: a workload created with a review owner has nothing left to acknowledge, got {workload:?}"
        );
        // A workload created without pillar priorities is reviewed in the framework's
        // own pillar order, and an untagged workload reports an empty tag map.
        let expected_pillars = request.get("PillarPriorities").cloned().unwrap_or_else(|| {
            json!([
                "operationalExcellence",
                "security",
                "reliability",
                "performance",
                "costOptimization",
                "sustainability"
            ])
        });
        assert_eq!(
            workload.get("PillarPriorities"),
            Some(&expected_pillars),
            "case {case:?}: expected PillarPriorities {expected_pillars:?}, got {workload:?}"
        );
        let expected_tags = request.get("Tags").cloned().unwrap_or_else(|| json!({}));
        assert_eq!(
            workload.get("Tags"),
            Some(&expected_tags),
            "case {case:?}: expected Tags {expected_tags:?}, got {workload:?}"
        );

        // Every member the request carried must come back unchanged; a member it omitted
        // must come back as the empty collection (lists) or be absent (optional strings).
        for member in [
            "WorkloadName",
            "Description",
            "Environment",
            "AccountIds",
            "AwsRegions",
            "NonAwsRegions",
            "ArchitecturalDesign",
            "ReviewOwner",
            "IndustryType",
            "Industry",
            "Notes",
            "Lenses",
            "Applications",
        ] {
            match request.get(member) {
                Some(sent) => assert_eq!(
                    workload.get(member),
                    Some(sent),
                    "case {case:?}: member {member} must round-trip, got {workload:?}"
                ),
                None => {
                    let actual = workload.get(member);
                    assert!(
                        actual.is_none() || actual == Some(&json!([])),
                        "case {case:?}: omitted member {member} must be absent or empty, got {actual:?}"
                    );
                }
            }
        }
    }
}

/// A workload name is unique per account and Region, ignoring spaces and
/// capitalization — so every spelling of a taken name is a 409, and a genuinely
/// different name still succeeds.
#[tokio::test]
async fn duplicate_workload_name_conflicts_ignoring_case_and_spaces() {
    let client = Client::new();
    let first = client.create(minimal_create("Shared Name", "tok-first")).await;
    let original_id = created_id(&first, "original");

    // Rows differ along the transformation applied to the taken name.
    let conflicting = [
        ("identical", "Shared Name"),
        ("lowercased", "shared name"),
        ("uppercased", "SHARED NAME"),
        ("spaces-removed", "SharedName"),
        ("extra-spaces", "  Shared    Name  "),
        ("tab-separated", "Shared\tName"),
    ];
    for (case, name) in conflicting {
        let answer = client
            .create(minimal_create(name, &format!("tok-{case}")))
            .await;
        assert_eq!(
            answer.status, 409,
            "case {case:?}: name {name:?} normalizes onto a taken name, expected 409, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ConflictException",
            "case {case:?}: expected ConflictException, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("ResourceId").and_then(Value::as_str),
            Some(original_id.as_str()),
            "case {case:?}: the conflict must name the workload that holds the name, got {answer:?}"
        );
    }

    for (case, name) in [
        ("different-word", "Shared Names"),
        ("different-prefix", "Unshared Name"),
    ] {
        let answer = client
            .create(minimal_create(name, &format!("tok-ok-{case}")))
            .await;
        let id = created_id(&answer, case);
        assert_ne!(
            id, original_id,
            "case {case:?}: a distinct name must create a distinct workload"
        );
    }

    // And the name is released when the workload is deleted.
    let deleted = client.delete(&original_id, "tok-release").await;
    assert_eq!(
        deleted.status, 200,
        "case \"release\": expected DeleteWorkload 200, got {deleted:?}"
    );
    let reused = client.create(minimal_create("shared name", "tok-reuse")).await;
    created_id(&reused, "reuse-after-delete");
}

/// `ClientRequestToken` is an `@idempotencyToken`: the same token with the same
/// parameters returns the original result, and the store still holds exactly one
/// workload. Reusing it with different parameters is a conflict instead.
#[tokio::test]
async fn client_request_token_replays_the_original_create() {
    let client = Client::new();

    // Rows differ along how many times the identical request is replayed.
    for (case, replays) in [("once", 1), ("twice", 2), ("five-times", 5)] {
        let request = minimal_create(&format!("replayed-{case}"), &format!("tok-{case}"));
        let original = created_id(&client.create(request.clone()).await, case);
        for attempt in 0..replays {
            let replay = client.create(request.clone()).await;
            let id = created_id(&replay, case);
            assert_eq!(
                id, original,
                "case {case:?}: replay {attempt} must return the original WorkloadId, got {replay:?}"
            );
        }
    }

    let listed = client.list(json!({})).await;
    let summaries = listed
        .body
        .get("WorkloadSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        summaries.len(),
        3,
        "case \"replays-create-nothing\": 3 distinct tokens must yield 3 workloads, got {summaries:?}"
    );

    // Rows differ along which member of the replayed request changed.
    for (case, member, value) in [
        ("description", "Description", json!("a different description")),
        ("environment", "Environment", json!("PREPRODUCTION")),
        ("lenses", "Lenses", json!(["serverless"])),
        ("tags", "Tags", json!({"k": "v"})),
        ("review-owner", "ReviewOwner", json!("somebody-else")),
    ] {
        let client = Client::new();
        let mut request = minimal_create("mismatch-workload", "tok-mismatch");
        let original = created_id(&client.create(request.clone()).await, case);
        request[member] = value;
        let answer = client.create(request).await;
        assert_eq!(
            answer.status, 409,
            "case {case:?}: the token was reused with a different {member}, expected 409, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ConflictException",
            "case {case:?}: expected ConflictException, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("ResourceId").and_then(Value::as_str),
            Some(original.as_str()),
            "case {case:?}: the conflict must name the workload the token created, got {answer:?}"
        );
    }
}

/// Paging enumerates every workload exactly once and the last page omits `NextToken`.
/// Checked exhaustively over every (workload count, page size) pair in a bounded
/// range rather than on sampled sizes, so the invariant is pinned at both ends of the
/// range and wherever the page boundary lands.
#[tokio::test]
async fn paging_enumerates_every_workload_exactly_once() {
    for workload_count in 1usize..=6 {
        let client = Client::new();
        let mut expected = BTreeSet::new();
        for index in 0..workload_count {
            let name = format!("pager-{index:02}");
            let answer = client
                .create(minimal_create(&name, &format!("tok-{index}")))
                .await;
            expected.insert(created_id(&answer, &name));
        }

        for page_size in 1usize..=(workload_count + 2) {
            let case = format!("{workload_count} workloads, MaxResults {page_size}");
            let mut seen: Vec<String> = Vec::new();
            let mut next_token: Option<String> = None;
            let mut pages = 0usize;
            loop {
                pages += 1;
                assert!(
                    pages <= workload_count + 2,
                    "case {case:?}: paging did not terminate after {pages} pages"
                );
                let mut request = json!({ "MaxResults": page_size });
                if let Some(token) = &next_token {
                    request["NextToken"] = json!(token);
                }
                let answer = client.list(request).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected ListWorkloads 200, got {answer:?}"
                );
                let summaries = answer
                    .body
                    .get("WorkloadSummaries")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                assert!(
                    summaries.len() <= page_size,
                    "case {case:?}: a page must not exceed MaxResults, got {} entries",
                    summaries.len()
                );
                for summary in &summaries {
                    for member in [
                        "WorkloadId",
                        "WorkloadArn",
                        "WorkloadName",
                        "Owner",
                        "UpdatedAt",
                        "Lenses",
                        "RiskCounts",
                        "ImprovementStatus",
                    ] {
                        assert!(
                            summary.get(member).is_some(),
                            "case {case:?}: WorkloadSummary must carry {member}, got {summary:?}"
                        );
                    }
                    seen.push(
                        summary
                            .get("WorkloadId")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    );
                }
                next_token = answer
                    .body
                    .get("NextToken")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                match &next_token {
                    None => break,
                    Some(token) => assert!(
                        token
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || "+/=_-".contains(c)),
                        "case {case:?}: NextToken must match ^[A-Za-z0-9+/=_-]+$, got {token:?}"
                    ),
                }
            }

            let unique: BTreeSet<String> = seen.iter().cloned().collect();
            assert_eq!(
                unique.len(),
                seen.len(),
                "case {case:?}: paging returned a workload twice: {seen:?}"
            );
            assert_eq!(
                unique, expected,
                "case {case:?}: paging must enumerate every workload exactly once, got {seen:?}"
            );
        }
    }
}

/// `WorkloadNamePrefix` filters the summaries, and the filter composes with paging.
#[tokio::test]
async fn list_workloads_filters_by_name_prefix() {
    let client = Client::new();
    let names = [
        "alpha-one",
        "alpha-two",
        "alpha-three",
        "beta-one",
        "gamma-one",
    ];
    for (index, name) in names.iter().enumerate() {
        created_id(
            &client.create(minimal_create(name, &format!("tok-{index}"))).await,
            name,
        );
    }

    // Rows differ along how many of the stored names the prefix selects.
    for (case, prefix, expected) in [
        ("all", "", 5usize),
        ("three", "alpha", 3),
        ("one", "beta", 1),
        ("exact-name", "gamma-one", 1),
        ("none", "delta", 0),
    ] {
        for page_size in [1usize, 2, 50] {
            let mut collected = Vec::new();
            let mut next_token: Option<String> = None;
            loop {
                let mut request = json!({
                    "WorkloadNamePrefix": prefix,
                    "MaxResults": page_size,
                });
                if let Some(token) = &next_token {
                    request["NextToken"] = json!(token);
                }
                let answer = client.list(request).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}/{page_size}: expected 200, got {answer:?}"
                );
                for summary in answer
                    .body
                    .get("WorkloadSummaries")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                {
                    let name = summary
                        .get("WorkloadName")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    assert!(
                        name.starts_with(prefix),
                        "case {case:?}/{page_size}: {name:?} does not start with {prefix:?}"
                    );
                    collected.push(name);
                }
                next_token = answer
                    .body
                    .get("NextToken")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                if next_token.is_none() {
                    break;
                }
            }
            assert_eq!(
                collected.len(),
                expected,
                "case {case:?}/{page_size}: expected {expected} summaries, got {collected:?}"
            );
        }
    }
}

/// UpdateWorkload applies only the members the request carries; everything else keeps
/// its stored value. One row per updatable member, each checked against the full
/// baseline record.
#[tokio::test]
async fn update_workload_applies_only_the_members_present() {
    let baseline = json!({
        "WorkloadName": "update-baseline",
        "Description": "the original description",
        "Environment": "PRODUCTION",
        "Lenses": ["wellarchitected", "serverless"],
        "AccountIds": ["111111111111"],
        "AwsRegions": ["us-east-1"],
        "NonAwsRegions": ["dc-1"],
        "PillarPriorities": ["security"],
        "ArchitecturalDesign": "https://example.com/original",
        "ReviewOwner": "original-owner",
        "IndustryType": "InfoTech",
        "Industry": "Software",
        "Notes": "original notes",
        "Applications": [],
        "ClientRequestToken": "tok-baseline",
    });

    // Rows differ along WHICH member is patched, and along its type (string, enum,
    // list, boolean). `ImprovementStatus` values come from
    // `smithy_query(query="shape", shape="WorkloadImprovementStatus")`.
    let patches: Vec<(&str, &str, Value)> = vec![
        ("description", "Description", json!("an updated description")),
        ("environment", "Environment", json!("PREPRODUCTION")),
        ("improvement-not-started", "ImprovementStatus", json!("NOT_STARTED")),
        ("improvement-complete", "ImprovementStatus", json!("COMPLETE")),
        (
            "improvement-risk-acknowledged",
            "ImprovementStatus",
            json!("RISK_ACKNOWLEDGED"),
        ),
        ("account-ids", "AccountIds", json!(["333333333333", "444444444444"])),
        ("aws-regions", "AwsRegions", json!(["eu-central-1"])),
        ("non-aws-regions", "NonAwsRegions", json!([])),
        (
            "pillar-priorities",
            "PillarPriorities",
            json!(["costOptimization", "performance"]),
        ),
        (
            "architectural-design",
            "ArchitecturalDesign",
            json!("https://example.com/updated"),
        ),
        ("review-owner", "ReviewOwner", json!("new-owner@example.com")),
        (
            "review-owner-acknowledged",
            "IsReviewOwnerUpdateAcknowledged",
            json!(true),
        ),
        ("industry-type", "IndustryType", json!("Healthcare")),
        ("industry", "Industry", json!("Hospitals")),
        ("notes", "Notes", json!("updated notes")),
        ("workload-name", "WorkloadName", json!("update-renamed")),
    ];

    for (case, member, value) in patches {
        let client = Client::new();
        let workload_id = created_id(&client.create(baseline.clone()).await, case);
        let before = client
            .get(&workload_id)
            .await
            .body
            .get("Workload")
            .cloned()
            .unwrap_or(Value::Null);

        let updated = client
            .update(&workload_id, json!({ member: value.clone() }))
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected UpdateWorkload 200, got {updated:?}"
        );
        let after = updated
            .body
            .get("Workload")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            after.get(member),
            Some(&value),
            "case {case:?}: member {member} must be {value:?}, got {after:?}"
        );

        // Every other member the baseline set is untouched — not nulled.
        for other in [
            "WorkloadName",
            "Description",
            "Environment",
            "AccountIds",
            "AwsRegions",
            "NonAwsRegions",
            "PillarPriorities",
            "ArchitecturalDesign",
            "ReviewOwner",
            "IndustryType",
            "Industry",
            "Notes",
            "Lenses",
            "ImprovementStatus",
            "WorkloadId",
            "WorkloadArn",
            "Owner",
        ] {
            if other == member {
                continue;
            }
            assert_eq!(
                after.get(other),
                before.get(other),
                "case {case:?}: patching {member} must leave {other} untouched, got {after:?}"
            );
        }

        assert!(
            after.get("UpdatedAt").and_then(Value::as_f64)
                >= before.get("UpdatedAt").and_then(Value::as_f64),
            "case {case:?}: UpdatedAt must not go backwards, before {before:?} after {after:?}"
        );

        // The update is persisted, not just echoed.
        let refetched = client
            .get(&workload_id)
            .await
            .body
            .get("Workload")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            refetched.get(member),
            Some(&value),
            "case {case:?}: GetWorkload must show the persisted {member}, got {refetched:?}"
        );

        // A rename must also move the uniqueness reservation.
        if member == "WorkloadName" {
            let taken = client
                .create(minimal_create("update-renamed", "tok-taken"))
                .await;
            assert_eq!(
                taken.status, 409,
                "case {case:?}: the new name must be reserved, got {taken:?}"
            );
            let released = client
                .create(minimal_create("update-baseline", "tok-released"))
                .await;
            created_id(&released, "old-name-released");
        }
    }
}

/// A deleted workload disappears from every reader, and a delete under a token this
/// service has not spent is a 404.
#[tokio::test]
async fn deleted_workload_disappears_from_every_reader() {
    let client = Client::new();
    // Rows differ along how many workloads are stored when one of them is deleted.
    for cohort in 1usize..=3 {
        let mut ids = Vec::new();
        for index in 0..cohort {
            let name = format!("cohort-{cohort}-{index}");
            ids.push(created_id(
                &client.create(minimal_create(&name, &format!("tok-{cohort}-{index}"))).await,
                &name,
            ));
        }
        let victim = ids.remove(0);
        let case = format!("cohort of {cohort}");

        let deleted = client.delete(&victim, &format!("tok-del-{cohort}")).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected DeleteWorkload 200, got {deleted:?}"
        );
        assert_eq!(
            deleted.body,
            Value::Null,
            "case {case:?}: DeleteWorkload returns an empty body, got {deleted:?}"
        );

        let fetched = client.get(&victim).await;
        assert_eq!(
            fetched.status, 404,
            "case {case:?}: GetWorkload on a deleted id must be 404, got {fetched:?}"
        );
        assert_eq!(
            fetched.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {fetched:?}"
        );

        // A FRESH token is a new request, so it resolves the id and finds nothing. (The
        // retry under the ORIGINAL token is the `@idempotencyToken` replay, pinned by
        // `delete_idempotency`.)
        let again = client
            .delete(&victim, &format!("tok-del-again-{cohort}"))
            .await;
        assert_eq!(
            again.status, 404,
            "case {case:?}: a second DeleteWorkload under a fresh token must be 404, got {again:?}"
        );

        let listed = client.list(json!({})).await;
        let remaining: Vec<String> = listed
            .body
            .get("WorkloadSummaries")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|summary| {
                summary
                    .get("WorkloadId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert!(
            !remaining.contains(&victim),
            "case {case:?}: ListWorkloads still reports the deleted {victim}: {remaining:?}"
        );
        for survivor in &ids {
            assert!(
                remaining.contains(survivor),
                "case {case:?}: deleting one workload must not remove {survivor}: {remaining:?}"
            );
            // One token per teardown: a `ClientRequestToken` names ONE request, so
            // reusing one across workloads would not be a retry.
            let kept = client
                .delete(survivor, &format!("tok-cleanup-{cohort}-{survivor}"))
                .await;
            assert_eq!(
                kept.status, 200,
                "case {case:?}: tearing down {survivor} must succeed, got {kept:?}"
            );
        }
    }
}

/// An id no workload carries is a 404 on every operation that resolves one — never a
/// canned workload and never a silent success.
#[tokio::test]
async fn unknown_workload_id_is_not_found() {
    let client = Client::new();
    created_id(
        &client.create(minimal_create("present-workload", "tok-present")).await,
        "present",
    );

    // Rows differ along the shape of the absent id, each still satisfying
    // `^[0-9a-f]{32}$`.
    for (case, workload_id) in [
        ("all-zero", "0".repeat(32)),
        ("all-f", "f".repeat(32)),
        ("mixed", "0123456789abcdef0123456789abcdef".to_string()),
    ] {
        for (operation, answer) in [
            ("GetWorkload", client.get(&workload_id).await),
            (
                "UpdateWorkload",
                client
                    .update(&workload_id, json!({"Description": "nope"}))
                    .await,
            ),
            (
                "DeleteWorkload",
                client.delete(&workload_id, "tok-absent").await,
            ),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {case:?}/{operation}: expected 404, got {answer:?}"
            );
            assert_eq!(
                answer.error_code, "ResourceNotFoundException",
                "case {case:?}/{operation}: expected ResourceNotFoundException, got {answer:?}"
            );
            assert_eq!(
                answer.body.get("ResourceId").and_then(Value::as_str),
                Some(workload_id.as_str()),
                "case {case:?}/{operation}: the error must name the id asked for, got {answer:?}"
            );
        }
    }
}

/// Requests outside a declared constraint or a documented rule are refused with HTTP
/// 400, not accepted and not answered 501. Each published bound is tested one step
/// past it.
#[tokio::test]
async fn invalid_requests_are_refused_with_400() {
    let client = Client::new();

    // Rows differ along which rule the request breaks: a published @length / @range
    // bound, a modelled enum value set, or a rule the CreateWorkload doc states.
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "name-too-short",
            "/workloads",
            json!({
                "WorkloadName": "ab",
                "Description": "two character name",
                "Environment": "PRODUCTION",
                "Lenses": [],
                "AwsRegions": ["us-east-1"],
                "ReviewOwner": "owner",
                "ClientRequestToken": "tok-short-name",
            }),
        ),
        (
            "name-too-long",
            "/workloads",
            json!({
                "WorkloadName": "n".repeat(101),
                "Description": "101 character name",
                "Environment": "PRODUCTION",
                "Lenses": [],
                "AwsRegions": ["us-east-1"],
                "ReviewOwner": "owner",
                "ClientRequestToken": "tok-long-name",
            }),
        ),
        (
            "description-too-short",
            "/workloads",
            json!({
                "WorkloadName": "bad-description",
                "Description": "ab",
                "Environment": "PRODUCTION",
                "Lenses": [],
                "AwsRegions": ["us-east-1"],
                "ReviewOwner": "owner",
                "ClientRequestToken": "tok-short-desc",
            }),
        ),
        (
            "description-too-long",
            "/workloads",
            json!({
                "WorkloadName": "bad-description-long",
                "Description": "d".repeat(251),
                "Environment": "PRODUCTION",
                "Lenses": [],
                "AwsRegions": ["us-east-1"],
                "ReviewOwner": "owner",
                "ClientRequestToken": "tok-long-desc",
            }),
        ),
        (
            "environment-outside-the-enum",
            "/workloads",
            json!({
                "WorkloadName": "bad-environment",
                "Description": "an environment the enum does not list",
                "Environment": "STAGING",
                "Lenses": [],
                "AwsRegions": ["us-east-1"],
                "ReviewOwner": "owner",
                "ClientRequestToken": "tok-bad-env",
            }),
        ),
        (
            "no-regions-at-all",
            "/workloads",
            json!({
                "WorkloadName": "no-regions",
                "Description": "neither AwsRegions nor NonAwsRegions",
                "Environment": "PRODUCTION",
                "Lenses": [],
                "ReviewOwner": "owner",
                "ClientRequestToken": "tok-no-regions",
            }),
        ),
        (
            "no-review-owner",
            "/workloads",
            json!({
                "WorkloadName": "no-review-owner",
                "Description": "ReviewOwner is documented as required",
                "Environment": "PRODUCTION",
                "Lenses": [],
                "AwsRegions": ["us-east-1"],
                "ClientRequestToken": "tok-no-owner",
            }),
        ),
        (
            "max-results-below-the-range",
            "/workloadsSummaries",
            json!({ "MaxResults": 0 }),
        ),
        (
            "max-results-above-the-range",
            "/workloadsSummaries",
            json!({ "MaxResults": 51 }),
        ),
        (
            "next-token-not-issued-here",
            "/workloadsSummaries",
            json!({ "NextToken": "not-a-cursor" }),
        ),
    ];

    for (case, path, body) in cases {
        let answer = client.send("POST", path, Some(body)).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
    }

    // MaxResults at each published bound is accepted.
    for max_results in [1, 50] {
        let answer = client.list(json!({ "MaxResults": max_results })).await;
        assert_eq!(
            answer.status, 200,
            "case \"MaxResults {max_results}\": a value on the published bound must be accepted, got {answer:?}"
        );
    }
}

/// A profile or review template ARN names a resource that has to exist: an ARN that
/// resolves to no stored resource is refused with 404 rather than silently dropped. (No
/// profile can be created in this emulator yet — `CreateProfile` is still a stub — so no
/// profile ARN resolves at all; a review template ARN resolves once the template has
/// been created, which `review_template_lifecycle` covers.)
#[tokio::test]
async fn associating_a_resource_that_does_not_exist_is_not_found() {
    let client = Client::new();
    for (case, member, arn) in [
        (
            "profile",
            "ProfileArns",
            "arn:aws:wellarchitected:us-west-2:000000000000:profile/00000000000000000000000000000000",
        ),
        (
            "review-template",
            "ReviewTemplateArns",
            "arn:aws:wellarchitected:us-west-2:000000000000:review-template/00000000000000000000000000000000",
        ),
    ] {
        let mut request = minimal_create(&format!("absent-{case}"), &format!("tok-{case}"));
        request[member] = json!([arn]);
        let answer = client.create(request).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404 for an absent {member} target, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
    }
}

/// Workloads are metered per account and Region: the store accepts workloads up to the
/// quota and refuses the one past it with the modelled `ServiceQuotaExceededException`
/// (HTTP 402), so the store cannot grow without bound. Checked at the quota minus one,
/// at the quota, and one past it — and a delete frees a slot again.
#[tokio::test]
async fn the_workload_quota_bounds_the_store() {
    // The service's own ceiling, so the test cannot drift from the handler.
    const QUOTA: usize = service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::MAX_WORKLOADS_PER_ACCOUNT_REGION;

    let client = Client::new();
    client.seed_workloads(QUOTA - 1);

    // One slot left: the create succeeds.
    let last = client
        .create(minimal_create("quota-last-slot", "tok-last"))
        .await;
    let last_id = created_id(&last, "the last free slot");

    // No slot left: every further create is refused, whatever the request carries.
    for (case, name) in [
        ("plain", "quota-over-one"),
        ("another-name", "quota-over-two"),
        ("name-of-a-deleted-shape", "quota-over-three"),
    ] {
        let answer = client
            .create(minimal_create(name, &format!("tok-{case}")))
            .await;
        assert_eq!(
            answer.status, 402,
            "case {case:?}: the quota is reached, expected HTTP 402, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ServiceQuotaExceededException",
            "case {case:?}: expected ServiceQuotaExceededException, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("ServiceCode").and_then(Value::as_str),
            Some("wellarchitected"),
            "case {case:?}: the quota error must name this service, got {answer:?}"
        );
    }

    // A replay of an already-accepted token is NOT a new workload, so the quota does not
    // refuse it.
    let replay = client
        .create(minimal_create("quota-last-slot", "tok-last"))
        .await;
    assert_eq!(
        created_id(&replay, "replay at the quota"),
        last_id,
        "case \"replay at the quota\": an idempotent replay must still return the original id"
    );

    // Deleting frees exactly one slot.
    let deleted = client.delete(&last_id, "tok-free").await;
    assert_eq!(
        deleted.status, 200,
        "case \"free a slot\": expected DeleteWorkload 200, got {deleted:?}"
    );
    let refilled = client
        .create(minimal_create("quota-refill", "tok-refill"))
        .await;
    created_id(&refilled, "refill the freed slot");
    let over_again = client
        .create(minimal_create("quota-over-again", "tok-over-again"))
        .await;
    assert_eq!(
        over_again.status, 402,
        "case \"one past the quota again\": expected HTTP 402, got {over_again:?}"
    );
}
