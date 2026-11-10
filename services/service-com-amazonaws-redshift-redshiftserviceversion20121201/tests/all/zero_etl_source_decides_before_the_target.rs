// SPDX-License-Identifier: CC0-1.0

//! WHICH of a zero-ETL integration's two ARNs decides a refused
//! `CreateIntegration`, when both would refuse it.
//!
//! The rule this table asserts: **a `SourceArn` whose owning service answers that
//! it holds no such table or bucket is a parameter `CreateIntegration` cannot
//! honour, so the create is refused synchronously with HTTP 400,
//! `InvalidParameterValue` and that source kind's zero-ETL access sentence —
//! and that decision is taken BEFORE any state of the TARGET is judged, so it is
//! the answer whatever class the target would have earned on its own (a namespace
//! that exists nowhere, a namespace whose data warehouse is not active yet, or an
//! active warehouse that would have taken the integration). A source its owner
//! does hold leaves the answer to the target, which keeps its own three
//! classes.**
//!
//! That ORDER is what the table is about, and it is the part a single-target suite
//! cannot see: the sibling `zero_etl_integration_sources` suite names a target
//! that exists nowhere in every row, so a reordering that let the target's class
//! answer first would still pass there while turning the source verdict into a
//! transient conflict (`IntegrationConflictOperationFault`) or a not-found —
//! refusals that tell a client to retry a call that can never succeed, instead of
//! naming the table and the policies the operator has to fix.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter` the
//! production router builds, with Amazon DynamoDB, Amazon S3 and the Redshift
//! Serverless control plane mounted beside Redshift on one engine exactly as the
//! assembled emulator mounts them — the source table, the source bucket and the
//! target namespace are THEIR resources, created here through their own APIs.
//!
//! The rows differ along what the rule generalises over: which service owns the
//! source, whether that owner holds it, and which of the three classes the target
//! would have answered with.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Emulator, REGION};
use crate::zero_etl_integration_sources::{
    create_bucket, create_integration, create_table, table_arn,
};
use crate::zero_etl_integration_targets::{
    await_workgroup, create_namespace, create_workgroup, NOT_ACTIVE_MESSAGE,
};

/// How Amazon Redshift words a zero-ETL source table it cannot reach — it is not
/// there, or the caller may not read it. Entirely FIXED text carrying no value
/// from the request, reproduced here as the recorded answer words it so this
/// suite pins the wording rather than reading the handler's own constant back.
const SOURCE_TABLE_MESSAGE: &str =
    "You don't have access to zero-ETL integrations because the source DynamoDB table doesn't \
     exist or you don't have the required permissions. Check the resource and key policy on the \
     source table and make sure you have the required permissions. Refer to the Amazon DynamoDB \
     Developer Guide for instructions on how to update the policy if required and then continue \
     creating the integration.";

/// The same sentence of the same error class for the other source kind the
/// modelled `SourceArn` pattern admits, with that service's own nouns.
const SOURCE_BUCKET_MESSAGE: &str =
    "You don't have access to zero-ETL integrations because the source Amazon S3 bucket doesn't \
     exist or you don't have the required permissions. Check the resource and key policy on the \
     source bucket and make sure you have the required permissions. Refer to the Amazon S3 User \
     Guide for instructions on how to update the policy if required and then continue creating \
     the integration.";

/// Which of the two ARNs the rule says decides one row, and with what.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decision {
    /// The SOURCE decided: 400 `InvalidParameterValue` with that kind's sentence.
    SourceAbsent(&'static str),
    /// The source passed, so the TARGET decided: it exists nowhere (404).
    TargetAbsent,
    /// The source passed, so the TARGET decided: it exists, not active (400).
    TargetNotActive,
    /// Both ARNs resolve, so the integration is recorded (200, `creating`).
    Recorded,
}

/// One row: the two ARNs a create names, and which of them decides it.
struct Row {
    case: &'static str,
    /// Unique per row, so no row's answer can come from another row's record.
    name: &'static str,
    source: String,
    target: String,
    decision: Decision,
}

/// Assert one row against the decision the rule gives it.
async fn assert_row(emulator: &Emulator, row: &Row) {
    let answer = create_integration(emulator, row.name, &row.source, &row.target).await;
    match row.decision {
        Decision::SourceAbsent(sentence) => {
            assert_eq!(
                (
                    answer.status,
                    answer.error_code().as_deref(),
                    answer.text("Message").as_deref()
                ),
                (400, Some("InvalidParameterValue"), Some(sentence)),
                "case {}: the source decides before the target, so expected 400 \
                 InvalidParameterValue with the source-access sentence; got {} / {:?} / {:?} \
                 with body {}",
                row.case,
                answer.status,
                answer.error_code(),
                answer.text("Message"),
                answer.body
            );
        }
        Decision::TargetAbsent => {
            assert_eq!(
                (answer.status, answer.error_code().as_deref()),
                (404, Some("IntegrationTargetNotFoundFault")),
                "case {}: a source its owner holds leaves the answer to the target, so expected \
                 404 IntegrationTargetNotFoundFault; got {} / {:?} with body {}",
                row.case,
                answer.status,
                answer.error_code(),
                answer.body
            );
        }
        Decision::TargetNotActive => {
            assert_eq!(
                (
                    answer.status,
                    answer.error_code().as_deref(),
                    answer.text("Message").as_deref()
                ),
                (
                    400,
                    Some("IntegrationConflictOperationFault"),
                    Some(NOT_ACTIVE_MESSAGE)
                ),
                "case {}: a source its owner holds leaves the answer to the target, so expected \
                 400 IntegrationConflictOperationFault with the wait-for-the-warehouse sentence; \
                 got {} / {:?} / {:?} with body {}",
                row.case,
                answer.status,
                answer.error_code(),
                answer.text("Message"),
                answer.body
            );
        }
        Decision::Recorded => {
            assert_eq!(
                (answer.status, answer.text("Status").as_deref()),
                (200, Some(state::INTEGRATION_STATUS_CREATING)),
                "case {}: both ARNs resolve, so expected 200 and the creating status; got {} / \
                 {:?} with body {}",
                row.case,
                answer.status,
                answer.error_code(),
                answer.body
            );
            assert_eq!(
                (
                    answer.text("SourceArn").as_deref(),
                    answer.text("TargetArn").as_deref()
                ),
                (Some(row.source.as_str()), Some(row.target.as_str())),
                "case {}: expected the two ARNs the request named back, got {:?} / {:?}",
                row.case,
                answer.text("SourceArn"),
                answer.text("TargetArn")
            );
        }
    }
}

#[tokio::test]
async fn an_absent_source_decides_whatever_the_target_would_have_answered() {
    let emulator = Emulator::with_integration_owners();

    // The one table and the one bucket that really exist, each created through the
    // service that owns it.
    create_table(&emulator, "seed the real source table", "zetl-ord-orders").await;
    create_bucket(&emulator, "seed the real source bucket", "zetl-ord-landing").await;

    // The three targets, one per class the target decision has: a namespace ARN
    // nothing created, a namespace whose data warehouse does not exist, and a
    // namespace whose data warehouse is active.
    let absent_target = format!(
        "arn:aws:redshift-serverless:{REGION}:{}:namespace/00000000-0000-4000-8000-000000000000",
        engine::emulator_account_id()
    );
    let (bare_target, _bare_id) = create_namespace(&emulator, "zetl-ord-bare").await;
    let (live_target, _live_id) = create_namespace(&emulator, "zetl-ord-live").await;
    create_workgroup(&emulator, "zetl-ord-live-wg", "zetl-ord-live").await;
    await_workgroup(&emulator, "zetl-ord-live-wg").await;

    let absent_table = table_arn("zetl-ord-never-created");
    let absent_bucket = "arn:aws:s3:::zetl-ord-never-created".to_string();
    let real_table = table_arn("zetl-ord-orders");
    let real_bucket = "arn:aws:s3:::zetl-ord-landing".to_string();

    let rows = vec![
        // A source its owner does not hold answers the same way against each of
        // the three targets: the target's own class never reaches the caller.
        Row {
            case: "a table that was never created, into an ACTIVE warehouse",
            name: "zetlOrdTableLive",
            source: absent_table.clone(),
            target: live_target.clone(),
            decision: Decision::SourceAbsent(SOURCE_TABLE_MESSAGE),
        },
        Row {
            case: "a table that was never created, into a warehouse that is not active",
            name: "zetlOrdTableBare",
            source: absent_table.clone(),
            target: bare_target.clone(),
            decision: Decision::SourceAbsent(SOURCE_TABLE_MESSAGE),
        },
        Row {
            case: "a table that was never created, into a namespace that exists nowhere",
            name: "zetlOrdTableAbsent",
            source: absent_table.clone(),
            target: absent_target.clone(),
            decision: Decision::SourceAbsent(SOURCE_TABLE_MESSAGE),
        },
        Row {
            case: "a bucket that was never created, into an ACTIVE warehouse",
            name: "zetlOrdBucketLive",
            source: absent_bucket.clone(),
            target: live_target.clone(),
            decision: Decision::SourceAbsent(SOURCE_BUCKET_MESSAGE),
        },
        Row {
            case: "a bucket that was never created, into a warehouse that is not active",
            name: "zetlOrdBucketBare",
            source: absent_bucket.clone(),
            target: bare_target.clone(),
            decision: Decision::SourceAbsent(SOURCE_BUCKET_MESSAGE),
        },
        Row {
            case: "a bucket that was never created, into a namespace that exists nowhere",
            name: "zetlOrdBucketAbsent",
            source: absent_bucket.clone(),
            target: absent_target.clone(),
            decision: Decision::SourceAbsent(SOURCE_BUCKET_MESSAGE),
        },
        // ... while a source its owner DOES hold leaves each of those three
        // targets to answer for itself.
        Row {
            case: "a table the owner holds, into a namespace that exists nowhere",
            name: "zetlOrdRealAbsent",
            source: real_table.clone(),
            target: absent_target.clone(),
            decision: Decision::TargetAbsent,
        },
        Row {
            case: "a table the owner holds, into a warehouse that is not active",
            name: "zetlOrdRealBare",
            source: real_table.clone(),
            target: bare_target.clone(),
            decision: Decision::TargetNotActive,
        },
        Row {
            case: "a table the owner holds, into an ACTIVE warehouse",
            name: "zetlOrdRealLive",
            source: real_table.clone(),
            target: live_target.clone(),
            decision: Decision::Recorded,
        },
        Row {
            case: "a bucket the owner holds, into an ACTIVE warehouse",
            name: "zetlOrdBucketReal",
            source: real_bucket.clone(),
            target: live_target.clone(),
            decision: Decision::Recorded,
        },
    ];

    for row in &rows {
        assert_row(&emulator, row).await;
    }

    // ROUND TRIP: the integrations the two accepted rows recorded are the ONLY
    // ones any later read reports — every refused row, whichever ARN decided it,
    // recorded nothing.
    let recorded: Vec<String> = rows
        .iter()
        .filter(|row| row.decision == Decision::Recorded)
        .map(|row| row.name.to_string())
        .collect();
    let listed = emulator.call("DescribeIntegrations", &[]).await;
    assert_eq!(
        all_text(&listed.body, "IntegrationName"),
        recorded,
        "case read the integrations back: expected only the accepted rows, in creation order; \
         got body {}",
        listed.body
    );
    assert_eq!(
        all_text(&listed.body, "SourceArn"),
        vec![real_table, real_bucket],
        "case read the integrations back: expected the sources the accepted rows named; got body \
         {}",
        listed.body
    );
}
