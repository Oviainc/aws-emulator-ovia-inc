// SPDX-License-Identifier: CC0-1.0

//! A backup job's state change reaches the SQS target of an EventBridge rule in the whole
//! estate a client builds around it — the vault, the backup plans and selections that
//! assign resources to it, and the rule that watches AWS Backup's job events.
//!
//! The RULE under test: every state a backup job MOVES INTO after the call that started it
//! is published once, by the clock alone, onto the default event bus of the Region the
//! job's vault belongs to, as `source` `aws.backup` with `detail-type`
//! `Backup Job State Change`, carrying that job's own stored members — the id
//! `StartBackupJob` answered, the vault it writes into, the resource it protects and the
//! state it moved into — as the event `detail`; EventBridge then delivers that envelope to
//! every target of every rule whose pattern matches it. WHAT ELSE the account holds does
//! not enter that rule: the plans and selections that schedule backups of the same vault,
//! and the order the rule and the estate were created in, change neither whether the
//! envelope is published nor what it carries.
//!
//! The rows of the table differ along what the rule generalises over:
//!
//! * the estate standing around the job — no plan at all, one plan whose rule targets this
//!   vault, that plan plus a selection assigning the resource to it, and two plans one of
//!   which targets a DIFFERENT vault;
//! * the kind of protected resource, so the `detail` is checked for a resource type
//!   derived from the ARN rather than for one spelling of it;
//! * whether the rule and its target were installed BEFORE or AFTER the estate.
//!
//! And the consumer is the one a client actually writes: a LONG POLL on the queue, parked
//! there before the job settles, with no AWS Backup call of any kind while it waits — so
//! what is asserted is that the publication is owed to the clock and not to a reader. The
//! job is read back through `DescribeBackupJob` only afterwards, to prove the envelope
//! reported the job's own stored state rather than a second, invented copy of it.
//!
//! Every case drives real HTTP through the router: Backup, EventBridge and SQS are mounted
//! on one engine by the shared harness in `job_state_change_events.rs`, which this suite
//! reuses rather than growing a second copy of.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::job_state_change_events::{
    delivered, describe_backup_job, emulator, given_queue, given_rule, of_type, role, sqs_in,
    vault_arn, BACKUP_JOB_STATE_CHANGE, COMPLETED, EVENT_SOURCE,
};
use crate::vault_and_backup_job::{call, create_vault, start_job, string_at, ACCOUNT, REGION};

/// The vault every case backs its resource up into, and the vault a plan of one case
/// targets instead.
const VAULT: &str = "plan-driven-vault";
const OTHER_VAULT: &str = "plan-driven-elsewhere";

/// The rule every case installs, and the queue its one target names.
const RULE: &str = "backup-job-state-change";
const QUEUE: &str = "backup-job-events";

/// The pattern a client writes to watch AWS Backup's backup-job events, spelled out the
/// way the CLI's `--event-pattern` carries it.
fn event_pattern() -> Value {
    json!({ "source": [EVENT_SOURCE], "detail-type": [BACKUP_JOB_STATE_CHANGE] })
}

/// How long the consumer's poll waits for the event, in seconds.
///
/// Inside the documented range of SQS's `WaitTimeSeconds` (0 to 20) and well past the span
/// a started job settles after, so a poll that is parked when the job moves is handed the
/// envelope within this one wait.
const LONG_POLL_SECONDS: i64 = 5;

/// How many long polls one case makes before it gives up, so an envelope that is never
/// published fails the case instead of hanging it.
const MAX_LONG_POLLS: usize = 3;

/// `PUT /backup/plans` — `CreateBackupPlan`'s HTTP binding — for the plan body `plan`.
async fn create_plan(router: &MultiServiceRouter, plan: &Value) -> (u16, Value) {
    call(
        router,
        "PUT",
        "/backup/plans",
        Some(&json!({ "BackupPlan": plan }).to_string()),
    )
    .await
}

/// `PUT /backup/plans/{BackupPlanId}/selections` — `CreateBackupSelection`'s HTTP binding —
/// assigning `resource_arn` to the plan `plan_id`.
async fn create_selection(
    router: &MultiServiceRouter,
    plan_id: &str,
    resource_arn: &str,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/backup/plans/{plan_id}/selections"),
        Some(
            &json!({
                "BackupSelection": {
                    "SelectionName": "assigned-resources",
                    "IamRoleArn": role(),
                    "Resources": [resource_arn],
                },
            })
            .to_string(),
        ),
    )
    .await
}

/// A plan whose one rule schedules backups into `vault`, in the shape a client passes to
/// `aws backup create-backup-plan`.
fn plan_targeting(name: &str, vault: &str) -> Value {
    json!({
        "BackupPlanName": name,
        "Rules": [{
            "RuleName": "daily",
            "TargetBackupVaultName": vault,
            "ScheduleExpression": "cron(0 5 * * ? *)",
            "StartWindowMinutes": 60,
            "CompletionWindowMinutes": 120,
        }],
    })
}

/// Park one long poll on the queue at `url` and answer the message bodies it handed over,
/// as parsed JSON.
///
/// This is the call the consumer of these events makes: `ReceiveMessage` carrying
/// `WaitTimeSeconds`, which answers as soon as the queue has something rather than at the
/// end of the wait.
async fn long_polled_once(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<Value> {
    let (status, received) = sqs_in(
        router,
        REGION,
        "ReceiveMessage",
        json!({
            "QueueUrl": url,
            "MaxNumberOfMessages": 10,
            "WaitTimeSeconds": LONG_POLL_SECONDS,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ReceiveMessage on {url:?} answered {status} {received}"
    );
    received["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| {
            let raw = message["Body"].as_str().unwrap_or_default();
            serde_json::from_str::<Value>(raw).unwrap_or(Value::Null)
        })
        .collect()
}

/// Long-poll the queue until it has handed over at least one envelope, or the polls run
/// out.
async fn long_polled(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<Value> {
    let mut bodies = Vec::new();
    for _ in 0..MAX_LONG_POLLS {
        bodies.extend(long_polled_once(router, case, url).await);
        if !bodies.is_empty() {
            break;
        }
    }
    bodies
}

/// One row of the table: the estate built around the job, the resource it protects and
/// when the rule was installed.
struct Row {
    case: &'static str,
    /// The plans created before the job is started, each by name and target vault.
    plans: &'static [(&'static str, &'static str)],
    /// Whether the first plan is also given a selection naming the protected resource.
    selection: bool,
    /// The ARN of the resource the job protects.
    resource_arn: String,
    /// The `ResourceType` AWS Backup names that resource's kind by, derived from the
    /// service its ARN belongs to. Source: the resource types the Backup API documents for
    /// `ListBackupJobs`' `ByResourceType` filter, of which these four are the ones whose
    /// ARNs these rows carry.
    resource_type: &'static str,
    /// Whether the rule and its target are installed BEFORE the vault and the plans.
    rule_first: bool,
}

/// The rows: the estate shapes a client builds around an on-demand backup, the protected
/// resources whose ARNs name different services (an EBS volume and an EC2 instance share
/// one namespace and are two resource types), and both orders of installing the rule.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no plan at all, a DynamoDB table",
            plans: &[],
            selection: false,
            resource_arn: format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/orders"),
            resource_type: "DynamoDB",
            rule_first: false,
        },
        Row {
            case: "one plan targeting this vault, an EBS volume",
            plans: &[("daily-plan", VAULT)],
            selection: false,
            resource_arn: format!("arn:aws:ec2:{REGION}:{ACCOUNT}:volume/vol-0123456789abcdef0"),
            resource_type: "EBS",
            rule_first: false,
        },
        Row {
            case: "a plan and a selection assigning the resource, an S3 bucket",
            plans: &[("assigned-plan", VAULT)],
            selection: true,
            resource_arn: "arn:aws:s3:::plan-driven-reports".to_string(),
            resource_type: "S3",
            rule_first: true,
        },
        Row {
            case: "two plans, one targeting another vault, an EFS file system",
            plans: &[("here-plan", VAULT), ("elsewhere-plan", OTHER_VAULT)],
            selection: false,
            resource_arn: format!("arn:aws:elasticfilesystem:{REGION}:{ACCOUNT}:file-system/fs-01"),
            resource_type: "EFS",
            rule_first: true,
        },
    ]
}

/// A job started in an estate of vaults, plans and selections publishes its move to the
/// rule's SQS target, and the client waiting in a long poll is handed it without reading
/// the job.
///
/// On the MULTI-THREADED runtime, because the clock pump that publishes the move is a task
/// of its own and what this case asserts is that nothing of the consumer's doing is what
/// publishes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_job_started_beside_backup_plans_reaches_the_rules_queue_target() {
    for row in rows() {
        let case = row.case;
        let router = emulator();

        let (url, arn) = given_queue(&router, case, QUEUE).await;
        if row.rule_first {
            given_rule(
                &router,
                case,
                RULE,
                &event_pattern(),
                std::slice::from_ref(&arn),
            )
            .await;
        }

        // The estate: the vault the job writes into, the vault a plan of one row targets
        // instead, and the plans and selection that assign resources to them.
        for vault in [VAULT, OTHER_VAULT] {
            let (status, created) = create_vault(&router, vault, "{}").await;
            assert_eq!(
                status, 200,
                "case {case:?}: CreateBackupVault {vault:?} answered {status} {created}"
            );
        }
        let mut first_plan_id = String::new();
        for (name, vault) in row.plans {
            let (status, created) = create_plan(&router, &plan_targeting(name, vault)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: CreateBackupPlan {name:?} answered {status} {created}"
            );
            if first_plan_id.is_empty() {
                first_plan_id = string_at(&created, "BackupPlanId");
            }
        }
        if row.selection {
            let (status, created) =
                create_selection(&router, &first_plan_id, &row.resource_arn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: CreateBackupSelection answered {status} {created}"
            );
        }

        if !row.rule_first {
            given_rule(
                &router,
                case,
                RULE,
                &event_pattern(),
                std::slice::from_ref(&arn),
            )
            .await;
        }

        // The on-demand job: started once, and never read while the consumer waits.
        let (status, started) = start_job(
            &router,
            &json!({
                "BackupVaultName": VAULT,
                "ResourceArn": row.resource_arn,
                "IamRoleArn": role(),
            })
            .to_string(),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartBackupJob answered {status} {started}"
        );
        let backup_job_id = string_at(&started, "BackupJobId");
        let recovery_point_arn = string_at(&started, "RecoveryPointArn");
        assert!(
            !backup_job_id.is_empty(),
            "case {case:?}: StartBackupJob must answer a BackupJobId, got {started}"
        );

        // The consumer: parked in a long poll from before the job settles.
        let bodies = long_polled(&router, case, &url).await;
        let envelopes = of_type(&bodies, BACKUP_JOB_STATE_CHANGE);
        assert_eq!(
            envelopes.len(),
            1,
            "case {case:?}: the job's settle is the ONE envelope the rule's target is owed, \
             got {bodies:?}"
        );
        let event = &envelopes[0];
        assert_eq!(
            event["source"],
            json!(EVENT_SOURCE),
            "case {case:?}: expected source {EVENT_SOURCE:?}, got {event}"
        );
        assert_eq!(
            event["detail-type"],
            json!(BACKUP_JOB_STATE_CHANGE),
            "case {case:?}: expected detail-type {BACKUP_JOB_STATE_CHANGE:?}, got {event}"
        );
        for (member, expected) in [
            ("backupJobId", json!(backup_job_id)),
            ("state", json!(COMPLETED)),
            ("backupVaultName", json!(VAULT)),
            ("backupVaultArn", json!(vault_arn(REGION, VAULT))),
            ("recoveryPointArn", json!(recovery_point_arn)),
            ("resourceArn", json!(row.resource_arn)),
            ("resourceType", json!(row.resource_type)),
            ("iamRoleArn", json!(role())),
        ] {
            assert_eq!(
                event["detail"][member], expected,
                "case {case:?}: expected detail.{member} {expected}, got {event}"
            );
        }
        assert_eq!(
            event["resources"],
            json!([recovery_point_arn]),
            "case {case:?}: the envelope names the recovery point the job wrote, got {event}"
        );

        // Only now is the job read: the envelope reported the job's own stored state.
        let described = describe_backup_job(&router, &backup_job_id).await;
        for (event_member, read_member) in [
            ("state", "State"),
            ("backupVaultName", "BackupVaultName"),
            ("recoveryPointArn", "RecoveryPointArn"),
            ("resourceArn", "ResourceArn"),
            ("resourceType", "ResourceType"),
            ("percentDone", "PercentDone"),
        ] {
            assert_eq!(
                event["detail"][event_member], described[read_member],
                "case {case:?}: detail.{event_member} must be what DescribeBackupJob reports \
                 as {read_member}, got {event} and {described}"
            );
        }

        // And the target is owed nothing further: the state the job settled in has not
        // changed, however many times it is now read.
        describe_backup_job(&router, &backup_job_id).await;
        let again = delivered(&router, case, &url).await;
        assert!(
            again.is_empty(),
            "case {case:?}: a state that has not changed owes no further envelope, got \
             {again:?}"
        );
    }
}
