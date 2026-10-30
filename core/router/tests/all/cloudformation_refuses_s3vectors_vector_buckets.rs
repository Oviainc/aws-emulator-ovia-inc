// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack whose Amazon S3 Vectors declaration that service REFUSES is never
//! reported as created: the resource fails, the stack rolls back, and S3 Vectors holds
//! nothing the stack asked for afterwards.
//!
//! The rule under test: **a resource a stack declares is created through the owning service's
//! own API, so that service's refusal of the declaration fails the resource
//! (`CREATE_FAILED`), and a create whose `OnFailure` is the default `ROLLBACK` then removes
//! what it had made and settles the stack at the terminal `ROLLBACK_COMPLETE` with one
//! `DELETE_COMPLETE` record per resource it acted on — nothing is passed off as created that
//! the owning service never created, and a stack declaring only resources that service
//! accepts still reaches `CREATE_COMPLETE`.** It is the refusal half of the rule
//! `cloudformation_provisions_s3vectors_resources.rs` pins (the identifier a stack publishes
//! is the one S3 Vectors minted), and it needs the same two crates to agree, so it lives
//! here too.
//!
//! The rows differ along the way the declared name leaves the form S3 Vectors publishes for
//! a vector bucket and a vector index — upper case, under the minimum length, over the
//! maximum length, a character outside the set, and a separator at either end — and along
//! WHICH of the two types declares it, including an index refused in a bucket the same
//! create had already made. Each row is followed by a VALID sibling deployed on the same
//! emulator, which must still reach `CREATE_COMPLETE`.
//!
//! That name form is `[a-z0-9][a-z0-9-.]{1,61}[a-z0-9]` with `@length(min: 3, max: 63)`: the
//! pattern on `VectorBucketArn`/`IndexArn` and the bound on `VectorBucketName`/`IndexName` in
//! that service's own Smithy model.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The XML element readers this crate writes ONCE (`tests/all/query_wire.rs`): "Nothing new
// should write another one — call this module."
use super::query_wire::{elements, members_of, opt_element, pairs_of};
// The two-service router and the two wire callers the provisioning suite for these types
// already writes, so both suites drive the SAME emulator through the same calls.
use super::cloudformation_provisions_s3vectors_resources::{
    build_router, cloudformation, index_arn, s3vectors,
};

/// How long a poll waits for a stack to reach a terminal status. The create's own clock-owed
/// span and the rollback's each fall due one `STATUS_TRANSITION_DELAY` after the call that
/// opened them, so a terminal status is two spans away at most; this bound is generous
/// enough that a loaded machine does not read a status that is merely not due yet.
const TERMINAL_WAIT: Duration = Duration::from_secs(10);

/// How often that poll asks, which is how a client watches a stack.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How many pages of a listing this suite drains before it gives up on it. These rows deploy
/// a handful of vector buckets, so the whole listing is one page; the bound is only there so
/// a token the service failed to retire cannot loop forever.
const MAX_LISTING_PAGES: usize = 16;

/// The statuses a `CreateStack` can SETTLE at, as `StackStatus` spells them: the completed
/// create, the create that failed and was left as it failed, and the two ends of the
/// rollback a failed create owes.
const TERMINAL_STATUSES: &[&str] = &[
    "CREATE_COMPLETE",
    "CREATE_FAILED",
    "ROLLBACK_COMPLETE",
    "ROLLBACK_FAILED",
];

/// The logical id every row gives its vector bucket, and the one it gives its index.
const BUCKET_LOGICAL_ID: &str = "Vectors";
const INDEX_LOGICAL_ID: &str = "Embeddings";

/// The Region these rows deploy in, which is the Region the resources they declare would be
/// created in.
const REGION: &str = "us-east-2";

/// Which of the two S3 Vectors types carries the name that service refuses.
enum Declared {
    /// One vector bucket, whose own name is refused.
    Bucket,
    /// A vector index in a vector bucket of the same template, where the BUCKET's name is
    /// accepted and the index's is refused — so the rollback has a resource to remove as
    /// well as one that was never made.
    IndexIn(&'static str),
}

/// One row of the table: a declared name S3 Vectors refuses, and which type declares it.
struct Row {
    case: &'static str,
    /// The name the template declares for the refused resource.
    refused_name: String,
    declared: Declared,
}

impl Row {
    /// The member of the create S3 Vectors refuses for this row's type, which its refusal
    /// names and which the failed resource's reason therefore carries:
    /// `CreateVectorBucket`'s `vectorBucketName` and `CreateIndex`'s `indexName`.
    fn refused_member(&self) -> &'static str {
        match self.declared {
            Declared::Bucket => "vectorBucketName",
            Declared::IndexIn(_) => "indexName",
        }
    }

    /// The logical id of the resource whose create is refused.
    fn failed_logical_id(&self) -> &'static str {
        match self.declared {
            Declared::Bucket => BUCKET_LOGICAL_ID,
            Declared::IndexIn(_) => INDEX_LOGICAL_ID,
        }
    }

    /// The template this row deploys: the refused declaration, plus the vector bucket an
    /// index row holds its index in.
    fn template(&self) -> String {
        match self.declared {
            Declared::Bucket => json!({
                "Resources": {
                    BUCKET_LOGICAL_ID: {
                        "Type": "AWS::S3Vectors::VectorBucket",
                        "Properties": { "VectorBucketName": self.refused_name }
                    }
                }
            })
            .to_string(),
            Declared::IndexIn(bucket) => json!({
                "Resources": {
                    BUCKET_LOGICAL_ID: {
                        "Type": "AWS::S3Vectors::VectorBucket",
                        "Properties": { "VectorBucketName": bucket }
                    },
                    INDEX_LOGICAL_ID: {
                        "Type": "AWS::S3Vectors::Index",
                        "Properties": {
                            "VectorBucketArn": { "Ref": BUCKET_LOGICAL_ID },
                            "IndexName": self.refused_name,
                            "DataType": "float32",
                            "Dimension": 1024,
                            "DistanceMetric": "cosine"
                        }
                    }
                }
            })
            .to_string(),
        }
    }

    /// The vector bucket names that must NOT be listed by S3 Vectors once this row's stack
    /// has rolled back: the refused bucket name of a bucket row, and — for the index row —
    /// the bucket the create had already MADE before the refusal, which the rollback removes
    /// again. The index row's refused name is an index's, so it is read back through that
    /// service's own index read ([`Self::refused_index_arn`]) rather than looked for among
    /// bucket names, where it could not appear whatever the emulator did.
    fn bucket_names_left_nowhere(&self) -> Vec<String> {
        match self.declared {
            Declared::Bucket => vec![self.refused_name.clone()],
            Declared::IndexIn(bucket) => vec![bucket.to_string()],
        }
    }

    /// The ARN of the vector index an index row declares, which S3 Vectors must resolve to
    /// nothing once the stack has rolled back. It is the same total function of the Region
    /// and the two names both services compute an index ARN from ([`index_arn`]), so it is
    /// the handle the declaration would have minted — computed here from what the row
    /// declares rather than read back out of the answer being checked.
    fn refused_index_arn(&self) -> Option<String> {
        match self.declared {
            Declared::Bucket => None,
            Declared::IndexIn(bucket) => {
                Some(index_arn(REGION, bucket, &self.refused_name))
            }
        }
    }
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            // The recorded shape: an upper-case vector bucket name.
            case: "an upper-case vector bucket name",
            refused_name: "Sim2Real-Probe-Vec".to_string(),
            declared: Declared::Bucket,
        },
        Row {
            case: "a vector bucket name under the published minimum of three characters",
            refused_name: "ab".to_string(),
            declared: Declared::Bucket,
        },
        Row {
            // One byte past the published maximum of 63, built from characters the form
            // otherwise admits — so the LENGTH is the only thing wrong with it.
            case: "a vector bucket name over the published maximum of 63 characters",
            refused_name: "a".repeat(64),
            declared: Declared::Bucket,
        },
        Row {
            case: "a vector bucket name carrying a character outside the published set",
            refused_name: "under_score-vectors".to_string(),
            declared: Declared::Bucket,
        },
        Row {
            case: "a vector bucket name beginning with a separator",
            refused_name: "-leading-hyphen".to_string(),
            declared: Declared::Bucket,
        },
        Row {
            case: "a vector bucket name ending with a separator",
            refused_name: "trailing.dot.".to_string(),
            declared: Declared::Bucket,
        },
        Row {
            // The same name form holds for the other type, and this row's bucket IS created
            // before the index is refused, so the rollback removes it again.
            case: "an upper-case index name in a vector bucket the same template declares",
            refused_name: "Titan-V2".to_string(),
            declared: Declared::IndexIn("refused-index-host"),
        },
    ]
}

/// Poll `DescribeStacks` until the stack SETTLES, and answer the terminal status it settled
/// at beside the answer it was read from — which is how a client waits for a stack.
async fn settled_status(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
) -> (String, String) {
    let deadline = std::time::Instant::now() + TERMINAL_WAIT;
    loop {
        let (status, described) =
            cloudformation(router, REGION, "DescribeStacks", &[("StackName", stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks on {stack_name:?}, got \
             {status} with body {described}"
        );
        let reached = opt_element(&described, "StackStatus").unwrap_or_default();
        if TERMINAL_STATUSES.contains(&reached.as_str()) {
            return (reached, described);
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: expected {stack_name:?} to settle at a terminal status within \
             {TERMINAL_WAIT:?}, it is still {reached:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Every vector bucket Amazon S3 Vectors holds, by name — drained through `nextToken`, so a
/// name is never missed because it fell on a later page.
async fn held_bucket_names(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut next_token: Option<String> = None;
    // Bounded, so a token the service failed to retire cannot loop forever — and the bound
    // PANICS below rather than answering a short list, since the assertions this feeds are
    // absences, which an undrained page would satisfy for the wrong reason.
    for _ in 0..MAX_LISTING_PAGES {
        let request = match &next_token {
            Some(token) => json!({ "nextToken": token }),
            None => json!({}),
        };
        let (status, answered) = s3vectors(router, REGION, "ListVectorBuckets", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListVectorBuckets, got {status} with body \
             {answered}"
        );
        if let Some(listed) = answered.get("vectorBuckets").and_then(Value::as_array) {
            names.extend(listed.iter().filter_map(|bucket| {
                bucket
                    .get("vectorBucketName")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            }));
        }
        next_token = answered
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next_token.is_none() {
            return names;
        }
    }
    panic!(
        "case {case:?}: ListVectorBuckets still offered a nextToken after \
         {MAX_LISTING_PAGES} pages, so the listing was not drained: {names:?}"
    );
}

/// The `(logical id, resource status)` rows `DescribeStackResources` reports for a stack,
/// each read from the member that carries both.
async fn resource_statuses(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
) -> BTreeMap<String, String> {
    let (status, body) = cloudformation(
        router,
        REGION,
        "DescribeStackResources",
        &[("StackName", stack_name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with body \
         {body}"
    );
    pairs_of(&body, "LogicalResourceId", "ResourceStatus")
}

/// The reasons the stack's events report for `CREATE_FAILED` on one logical resource.
async fn create_failure_reasons(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    logical_id: &str,
) -> Vec<String> {
    let (status, events) = cloudformation(
        router,
        REGION,
        "DescribeStackEvents",
        &[("StackName", stack_name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStackEvents, got {status} with body \
         {events}"
    );
    members_of(&events)
        .into_iter()
        .filter(|event| {
            opt_element(event, "LogicalResourceId").as_deref() == Some(logical_id)
                && opt_element(event, "ResourceStatus").as_deref() == Some("CREATE_FAILED")
        })
        .flat_map(|event| elements(event, "ResourceStatusReason"))
        .collect()
}

/// Every row: the refused declaration fails its resource, the stack settles at
/// `ROLLBACK_COMPLETE` with a `DELETE_COMPLETE` record per resource the create acted on, S3
/// Vectors holds nothing the template asked for, and a valid sibling still deploys.
#[tokio::test]
async fn an_s3vectors_name_the_owning_service_refuses_rolls_the_stack_back() {
    let router = build_router();

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let stack_name = format!("s3vectors-refusal-{index}");
        let template = row.template();

        let (status, body) = cloudformation(
            &router,
            REGION,
            "CreateStack",
            &[("StackName", &stack_name), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack — the template declares a type \
             CloudFormation publishes, and the refusal belongs to the resource — got {status} \
             with body {body}"
        );

        // The stack SETTLES at the rolled-back terminal status, carrying the failure that
        // caused the rollback, which names the resource that failed.
        let (reached, described) = settled_status(&router, case, &stack_name).await;
        assert_eq!(
            reached, "ROLLBACK_COMPLETE",
            "case {case:?}: expected the stack to settle at ROLLBACK_COMPLETE, got {reached:?} \
             in {described}"
        );
        let failed_logical_id = row.failed_logical_id();
        let reason = opt_element(&described, "StackStatusReason").unwrap_or_default();
        assert!(
            reason.contains(failed_logical_id),
            "case {case:?}: expected the stack's reason to name {failed_logical_id:?}, got \
             {reason:?}"
        );

        // The resource itself FAILED, with the owning service's own refusal of the member it
        // refused as the reason.
        let member = row.refused_member();
        let failures = create_failure_reasons(&router, case, &stack_name, failed_logical_id).await;
        assert!(
            failures.iter().any(|reason| reason.contains(member)),
            "case {case:?}: expected a CREATE_FAILED event for {failed_logical_id:?} whose \
             reason names {member:?}, got {failures:?}"
        );

        // ...and the rollback disposed of every resource the create acted on, so each one's
        // record is at the terminal delete status rather than reporting a live resource.
        let statuses = resource_statuses(&router, case, &stack_name).await;
        assert!(
            statuses.contains_key(failed_logical_id),
            "case {case:?}: expected a record for {failed_logical_id:?}, got {statuses:?}"
        );
        for (logical_id, status) in &statuses {
            assert_eq!(
                status, "DELETE_COMPLETE",
                "case {case:?}: expected the rollback to leave {logical_id:?} at \
                 DELETE_COMPLETE, got {status:?} in {statuses:?}"
            );
        }

        // And S3 Vectors holds nothing the template asked for.
        let held = held_bucket_names(&router, case).await;
        for name in row.bucket_names_left_nowhere() {
            assert!(
                !held.contains(&name),
                "case {case:?}: expected no vector bucket named {name:?} after the rollback, S3 \
                 Vectors holds {held:?}"
            );
        }
        if let Some(arn) = row.refused_index_arn() {
            // S3 Vectors resolves the index ARN the declaration would have minted to no
            // index, with one of the two refusals its reads model: the 404 for an index it
            // does not hold, or — as here, where the refused name is outside the form
            // `IndexArn` publishes — the 400 that ARN's own constraint earns.
            let (status, answered) =
                s3vectors(&router, REGION, "GetIndex", &json!({ "indexArn": arn })).await;
            assert!(
                status == 400 || status == 404,
                "case {case:?}: expected S3 Vectors to resolve the rolled-back index {arn:?} to \
                 nothing, the read answered {status} with body {answered}"
            );
            // ...and the vector bucket that index was declared in is gone with the rollback,
            // so the listing of its indexes is the modelled 404 for a bucket the service does
            // not hold rather than an empty page.
            if let Declared::IndexIn(bucket) = row.declared {
                let (status, answered) = s3vectors(
                    &router,
                    REGION,
                    "ListIndexes",
                    &json!({ "vectorBucketName": bucket }),
                )
                .await;
                assert_eq!(
                    status, 404,
                    "case {case:?}: expected HTTP 404 — the modelled NotFoundException — from \
                     ListIndexes on the rolled-back vector bucket {bucket:?}, got {status} with \
                     body {answered}"
                );
            }
        }

        // The valid sibling: a stack declaring a vector bucket under a name S3 Vectors ACCEPTS
        // still reaches CREATE_COMPLETE on the same emulator, and that service holds it.
        let sibling_bucket = format!("valid.sibling.{index}");
        let sibling_name = format!("s3vectors-valid-{index}");
        let sibling_template = json!({
            "Resources": {
                BUCKET_LOGICAL_ID: {
                    "Type": "AWS::S3Vectors::VectorBucket",
                    "Properties": { "VectorBucketName": sibling_bucket }
                }
            },
            "Outputs": { "BucketArn": { "Value": { "Ref": BUCKET_LOGICAL_ID } } }
        })
        .to_string();
        let (status, body) = cloudformation(
            &router,
            REGION,
            "CreateStack",
            &[
                ("StackName", &sibling_name),
                ("TemplateBody", &sibling_template),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the valid sibling's CreateStack, got {status} \
             with body {body}"
        );
        let (reached, described) = settled_status(&router, case, &sibling_name).await;
        assert_eq!(
            reached, "CREATE_COMPLETE",
            "case {case:?}: expected the valid sibling to reach CREATE_COMPLETE, got {reached:?} \
             in {described}"
        );
        let held = held_bucket_names(&router, case).await;
        assert!(
            held.contains(&sibling_bucket),
            "case {case:?}: expected S3 Vectors to hold the sibling's bucket \
             {sibling_bucket:?}, it holds {held:?}"
        );
    }
}
