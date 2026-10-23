// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeregisterJobDefinition`, over the wire.
//!
//! Every case drives real HTTP (restJson1) through the router the emulator mounts,
//! so what is asserted is the response a `batch` client gets.
//!
//! The rule under test, over a SET of reference forms rather than one:
//! deregistering moves the ONE job-definition revision the reference names to
//! `INACTIVE` and leaves it stored (AWS: "Job definitions are permanently deleted
//! after 180 days"), leaving every other revision of the name untouched; the
//! reference is a `name:revision`, a full ARN or a bare name (which names the
//! definition's latest revision, per the member's "name and revision
//! (`name:revision`) or full Amazon Resource Name (ARN)" doc); a reference no
//! stored revision answers to is refused with `ClientException` (HTTP 400) and
//! nothing changes; and because the status is set rather than toggled, repeating
//! the call answers 200 and leaves the same revision `INACTIVE`.
//!
//! The consequences the registry's other operations draw from that status are
//! asserted too: `DescribeJobDefinitions`' `status` filter selects by it, and
//! `SubmitJob` refuses a definition that is no longer `ACTIVE` — including a bare
//! name, which then resolves to the latest revision still `ACTIVE`.

use serde_json::json;

use crate::job_lifecycle::{
    compute_environment, container_properties, describe_job, error_code, job_definition, job_queue,
    Batch,
};

/// The status every revision carries until something deregisters it
/// (`state::STATUS_ACTIVE`).
const ACTIVE: &str = "ACTIVE";

/// The status of a deregistered revision (`state::STATUS_INACTIVE`).
const INACTIVE: &str = "INACTIVE";

/// Register `revisions` revisions of `name`; answer each one's ARN, in revision
/// order (so index 0 is revision 1).
async fn revisions(batch: &Batch, name: &str, revisions: i64) -> Vec<String> {
    let mut arns = Vec::new();
    for expected in 1..=revisions {
        let (arn, revision) = job_definition(batch, name, container_properties()).await;
        assert_eq!(
            revision, expected,
            "case register {name}: expected revision {expected}, got {revision} ({arn})"
        );
        arns.push(arn);
    }
    arns
}

/// `(revision, status)` of every stored revision of `name`, ascending.
async fn statuses(batch: &Batch, name: &str) -> Vec<(i64, String)> {
    let body = batch
        .ok(
            "/v1/describejobdefinitions",
            json!({"jobDefinitionName": name}),
        )
        .await;
    let mut found: Vec<(i64, String)> = body["jobDefinitions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|definition| {
            (
                definition["revision"].as_i64().unwrap_or(-1),
                definition["status"].as_str().unwrap_or("<none>").to_string(),
            )
        })
        .collect();
    found.sort();
    found
}

/// The expected `(revision, status)` table for `total` revisions of which
/// `inactive` is the deregistered one.
fn expected_statuses(total: i64, inactive: i64) -> Vec<(i64, String)> {
    (1..=total)
        .map(|revision| {
            let status = if revision == inactive { INACTIVE } else { ACTIVE };
            (revision, status.to_string())
        })
        .collect()
}

/// Which revision each documented reference form deregisters, and that it leaves
/// every other revision of the name alone — then that repeating the call changes
/// nothing (the operation is idempotent).
#[tokio::test]
async fn deregistering_marks_exactly_the_referenced_revision_inactive() {
    /// Revisions registered per case, so a case can name a first, middle and last one.
    const TOTAL: i64 = 3;

    // (case, how to build the reference from the registered ARNs, the revision it
    //  names)
    #[allow(clippy::type_complexity)]
    let cases: Vec<(&str, Box<dyn Fn(&str, &[String]) -> String>, i64)> = vec![
        (
            "a bare name names the latest revision",
            Box::new(|name: &str, _arns: &[String]| name.to_string()),
            TOTAL,
        ),
        (
            "name:revision, the first revision",
            Box::new(|name: &str, _arns: &[String]| format!("{name}:1")),
            1,
        ),
        (
            "name:revision, a middle revision",
            Box::new(|name: &str, _arns: &[String]| format!("{name}:2")),
            2,
        ),
        (
            "name:revision, the latest revision",
            Box::new(|name: &str, _arns: &[String]| format!("{name}:{TOTAL}")),
            TOTAL,
        ),
        (
            "the full ARN of the first revision",
            Box::new(|_name: &str, arns: &[String]| arns[0].clone()),
            1,
        ),
        (
            "the full ARN of the latest revision",
            Box::new(|_name: &str, arns: &[String]| arns[TOTAL as usize - 1].clone()),
            TOTAL,
        ),
    ];

    for (case, reference_of, inactive) in cases {
        let batch = Batch::new();
        let name = "dereg";
        let arns = revisions(&batch, name, TOTAL).await;
        let reference = reference_of(name, &arns);

        let before = statuses(&batch, name).await;
        assert_eq!(
            before,
            expected_statuses(TOTAL, 0),
            "case {case:?}: expected every revision ACTIVE before the deregister, got {before:?}"
        );

        // Twice: the second call is the idempotence check, and must answer exactly
        // what the first did.
        for attempt in 1..=2 {
            let (status, body) = batch
                .call(
                    "/v1/deregisterjobdefinition",
                    json!({"jobDefinition": reference}),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?} attempt {attempt}: expected 200 for {reference}, got {status} {body}"
            );
            let found = statuses(&batch, name).await;
            assert_eq!(
                found,
                expected_statuses(TOTAL, inactive),
                "case {case:?} attempt {attempt}: expected only revision {inactive} INACTIVE, got {found:?}"
            );
        }
    }
}

/// A reference no stored revision answers to is refused with `ClientException`
/// (HTTP 400), and nothing in the registry changes.
#[tokio::test]
async fn deregistering_an_unknown_reference_is_refused_and_changes_nothing() {
    const TOTAL: i64 = 2;
    let batch = Batch::new();
    let name = "known";
    let arns = revisions(&batch, name, TOTAL).await;
    // The ARN of a revision that was never registered: the stored ARN of revision 1
    // with its revision replaced.
    let unregistered_arn = arns[0].replace(":1", ":9");

    let cases: Vec<(&str, String)> = vec![
        ("a name nothing is registered under", "absent".to_string()),
        (
            "a revision of a known name that was never registered",
            format!("{name}:9"),
        ),
        (
            "a revision that is not a number",
            format!("{name}:latest"),
        ),
        ("a negative revision", format!("{name}:-1")),
        ("the ARN of an unregistered revision", unregistered_arn),
        (
            "an ARN of another resource kind",
            format!("arn:aws:batch:us-east-1:000000000000:job-queue/{name}"),
        ),
        ("an empty reference", String::new()),
    ];

    for (case, reference) in cases {
        let (status, body) = batch
            .call(
                "/v1/deregisterjobdefinition",
                json!({"jobDefinition": reference}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for {reference:?}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ClientException",
            "case {case:?}: expected a ClientException for {reference:?}, got {body}"
        );
        let found = statuses(&batch, name).await;
        assert_eq!(
            found,
            expected_statuses(TOTAL, 0),
            "case {case:?}: expected the registry unchanged, got {found:?}"
        );
    }
}

/// What the rest of the registry reads off that status: `DescribeJobDefinitions`
/// filters by it, and `SubmitJob` refuses a revision that is no longer `ACTIVE`
/// while a bare name falls back to the latest revision that still is.
#[tokio::test]
async fn an_inactive_revision_is_filtered_by_status_and_cannot_be_submitted() {
    let batch = Batch::new();
    let name = "lifecycle";
    let arns = revisions(&batch, name, 2).await;
    let environment = compute_environment(&batch, "dereg-ce").await;
    let queue = job_queue(&batch, "dereg-queue", &environment).await;

    // Deregister the LATEST revision; revision 1 stays ACTIVE.
    let (status, body) = batch
        .call(
            "/v1/deregisterjobdefinition",
            json!({"jobDefinition": arns[1]}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case deregister the latest revision: expected 200, got {status} {body}"
    );

    // "The status used to filter job definitions" (`status`): each revision is
    // selected by its own status and by nothing else.
    for (case, filter, expected) in [
        ("the ACTIVE filter", ACTIVE, vec![1_i64]),
        ("the INACTIVE filter", INACTIVE, vec![2_i64]),
    ] {
        let body = batch
            .ok(
                "/v1/describejobdefinitions",
                json!({"jobDefinitionName": name, "status": filter}),
            )
            .await;
        let found: Vec<i64> = body["jobDefinitions"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|definition| definition["revision"].as_i64().unwrap_or(-1))
            .collect();
        assert_eq!(
            found, expected,
            "case {case:?}: expected revisions {expected:?}, got {found:?} ({body})"
        );
    }

    // A submit naming the deregistered revision is refused; one naming the bare
    // name resolves to revision 1, which is still ACTIVE.
    let (status, body) = batch
        .call(
            "/v1/submitjob",
            json!({"jobName": "after-dereg", "jobQueue": queue, "jobDefinition": arns[1]}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case submit the deregistered revision: expected 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ClientException",
        "case submit the deregistered revision: expected a ClientException, got {body}"
    );

    let body = batch
        .ok(
            "/v1/submitjob",
            json!({"jobName": "latest-active", "jobQueue": queue, "jobDefinition": name}),
        )
        .await;
    let job_id = body["jobId"].as_str().unwrap_or_default().to_string();
    let detail = describe_job(&batch, &job_id).await;
    assert_eq!(
        detail["jobDefinition"].as_str().unwrap_or_default(),
        arns[0],
        "case submit by bare name: expected the latest ACTIVE revision, got {detail}"
    );

    // Deregister the last ACTIVE revision too: now no revision of the name can be
    // submitted at all.
    let (status, body) = batch
        .call(
            "/v1/deregisterjobdefinition",
            json!({"jobDefinition": format!("{name}:1")}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case deregister the remaining revision: expected 200, got {status} {body}"
    );
    let (status, body) = batch
        .call(
            "/v1/submitjob",
            json!({"jobName": "none-active", "jobQueue": queue, "jobDefinition": name}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case submit with no ACTIVE revision left: expected 400, got {status} {body}"
    );

    // Both revisions are still STORED, which is what distinguishes deregistering
    // from deleting.
    let found = statuses(&batch, name).await;
    assert_eq!(
        found,
        vec![(1, INACTIVE.to_string()), (2, INACTIVE.to_string())],
        "case both revisions deregistered: expected both stored and INACTIVE, got {found:?}"
    );
}

/// A revision number is never reused, so registering after a deregister adds a new
/// revision beside the `INACTIVE` one instead of reviving it.
#[tokio::test]
async fn registering_after_a_deregister_takes_the_next_revision() {
    let batch = Batch::new();
    let name = "revived";
    revisions(&batch, name, 1).await;
    let (status, body) = batch
        .call(
            "/v1/deregisterjobdefinition",
            json!({"jobDefinition": format!("{name}:1")}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case deregister revision 1: expected 200, got {status} {body}"
    );

    let (arn, revision) = job_definition(&batch, name, container_properties()).await;
    assert_eq!(
        revision, 2,
        "case register after a deregister: expected revision 2, got {revision} ({arn})"
    );
    let found = statuses(&batch, name).await;
    assert_eq!(
        found,
        vec![(1, INACTIVE.to_string()), (2, ACTIVE.to_string())],
        "case register after a deregister: expected revision 1 INACTIVE beside a new \
         ACTIVE revision 2, got {found:?}"
    );
}
