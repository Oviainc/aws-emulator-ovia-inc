// SPDX-License-Identifier: CC0-1.0

//! `DeleteSchedulingPolicy`, `UpdateSchedulingPolicy`, `UpdateServiceEnvironment` and
//! `DeleteServiceEnvironment` — the writes behind the readers — plus the status a name
//! collision answers with and the members every Batch reader materialises, over the wire,
//! through the router a client reaches.
//!
//! The rules under test, each over a SET of inputs rather than one recorded request:
//!
//! * a delete reaches the SAME store every reader reads: the policy it removed is gone
//!   from `DescribeSchedulingPolicies`, its name is free for a later create, the policies
//!   beside it are untouched, and a repeat of the delete is the same empty 200;
//! * "You can't delete a scheduling policy that's used in any job queues"
//!   (`DeleteSchedulingPolicy`'s own doc): an ARN a queue still names is refused and the
//!   policy stays describable;
//! * a scheduling policy is identified by its ARN — the only form `DescribeSchedulingPolicies`
//!   takes — or, where the operation resolves one, by the name that ARN carries; an
//!   identifier of neither shape is refused with the modelled `ClientException`, whichever
//!   operation takes it, while an identifier of the right shape naming no stored policy is
//!   reported as absent (a describe) or already deleted (a delete);
//! * an update writes the record the reader answers from, and keeps the policy KIND the
//!   create set ("Once set during creation, a fairsharePolicy cannot be removed or changed
//!   to a quotaSharePolicy"); a refused update changes nothing;
//! * a service environment is updated in place, and a delete — DISABLED first and
//!   disassociated from every job queue, as its doc requires — puts it into the
//!   non-terminal `DELETING` the model declares, after which the CLOCK removes it from
//!   every reader;
//! * a `Create*` that names an object the service already holds answers Batch's CONFLICT
//!   status with the bare `Object already exists`, whatever the resource kind;
//! * every `Describe*` reader materialises the tag map: a resource created with no tags is
//!   described with an EMPTY map, never with the member missing.

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::state;

use crate::job_lifecycle::{
    arn, compute_environment, container_properties, error_code, error_message, Batch,
};

/// The `schedulingPolicies` `DescribeSchedulingPolicies` answers for `arns`, requiring a
/// 200.
async fn describe(batch: &Batch, arns: &[&str]) -> Vec<Value> {
    let body = batch
        .ok("/v1/describeschedulingpolicies", json!({"arns": arns}))
        .await;
    body["schedulingPolicies"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// `DeleteSchedulingPolicy` for `arn`.
async fn delete(batch: &Batch, policy_arn: &str) -> (u16, Value) {
    batch
        .call("/v1/deleteschedulingpolicy", json!({"arn": policy_arn}))
        .await
}

/// Create the policy `body` asks for; answers its ARN.
async fn create(batch: &Batch, body: Value) -> String {
    let created = batch.ok("/v1/createschedulingpolicy", body).await;
    created["arn"].as_str().unwrap_or_default().to_string()
}

/// The create requests the teardown rows use: they differ along what a policy can carry —
/// neither policy kind, a fair-share policy with a distribution and tags, a quota-share
/// policy, and the longest name the member documents.
fn teardown_rows() -> Vec<(&'static str, Value)> {
    vec![
        ("no policy of either kind, no tags", json!({"name": "sp-del-bare"})),
        (
            "a fair-share policy and tags",
            json!({
                "name": "sp_del_fair",
                "fairsharePolicy": {
                    "shareDecaySeconds": 600,
                    "computeReservation": 25,
                    "shareDistribution": [
                        {"shareIdentifier": "teamA", "weightFactor": 1.0},
                        {"shareIdentifier": "teamB", "weightFactor": 2.0},
                    ],
                },
                "tags": {"Environment": "probe"},
            }),
        ),
        (
            "a quota-share policy",
            json!({
                "name": "SP-Del-Quota",
                "quotaSharePolicy": {"idleResourceAssignmentStrategy": "FIFO"},
            }),
        ),
        (
            "the longest accepted name",
            json!({"name": "d".repeat(state::MAX_SCHEDULING_POLICY_NAME_LEN)}),
        ),
    ]
}

#[tokio::test]
async fn deleting_a_policy_removes_it_from_describe_and_frees_its_name() {
    let batch = Batch::new();

    // A policy no row deletes, so each delete is shown to remove ITS policy and no other.
    let keeper = create(&batch, json!({"name": "sp-keeper"})).await;

    for (label, body) in teardown_rows() {
        let name = body["name"].as_str().unwrap_or_default().to_string();
        let policy_arn = create(&batch, body.clone()).await;
        assert_eq!(
            policy_arn,
            arn(&format!("scheduling-policy/{name}")),
            "case {label:?}: expected the minted ARN, got {policy_arn:?}"
        );
        assert_eq!(
            describe(&batch, &[&policy_arn]).await.len(),
            1,
            "case {label:?}: the created policy must be describable before the delete"
        );

        // Twice: the second delete asks for a removal that already happened, which is the
        // state the call asks for, so it is the same empty success.
        for attempt in 1..=2 {
            let (status, answered) = delete(&batch, &policy_arn).await;
            assert_eq!(
                status, 200,
                "case {label:?} (attempt {attempt}): expected the empty 200, got {status} \
                 {answered}"
            );
            assert!(
                answered.is_null() || answered == json!({}),
                "case {label:?} (attempt {attempt}): expected an empty output, got {answered}"
            );
        }

        let found = describe(&batch, &[&policy_arn]).await;
        assert!(
            found.is_empty(),
            "case {label:?}: a deleted policy must not be described, got {found:?}"
        );
        assert_eq!(
            describe(&batch, &[&keeper]).await.len(),
            1,
            "case {label:?}: the delete must leave every other policy alone"
        );

        // The name AWS freed is free here too: the same create succeeds again, and mints
        // the same ARN.
        let recreated = create(&batch, body).await;
        assert_eq!(
            recreated, policy_arn,
            "case {label:?}: re-creating the freed name must mint the same ARN, got \
             {recreated:?}"
        );
        assert_eq!(
            describe(&batch, &[&policy_arn]).await.len(),
            1,
            "case {label:?}: the re-created policy must be describable"
        );
    }
}

#[tokio::test]
async fn a_policy_a_job_queue_names_cannot_be_deleted() {
    let batch = Batch::new();

    let environment = compute_environment(&batch, "ce-sp-delete").await;
    let attached = create(&batch, json!({"name": "sp-attached"})).await;
    let unattached = create(&batch, json!({"name": "sp-unattached"})).await;
    batch
        .ok(
            "/v1/createjobqueue",
            json!({
                "jobQueueName": "jq-with-policy",
                "priority": 1,
                "schedulingPolicyArn": &attached,
                "computeEnvironmentOrder": [{"order": 1, "computeEnvironment": environment}],
            }),
        )
        .await;

    // Twice, so the refusal is not a one-shot: the policy is still there to refuse.
    for attempt in 1..=2 {
        let (status, refused) = delete(&batch, &attached).await;
        assert_eq!(
            status, 400,
            "case a policy a queue names (attempt {attempt}): expected 400, got {status} \
             {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ClientException",
            "case a policy a queue names (attempt {attempt}): expected the modelled \
             ClientException, got {refused}"
        );
        assert_eq!(
            error_message(&refused),
            state::SCHEDULING_POLICY_IN_USE,
            "case a policy a queue names (attempt {attempt}): expected the in-use detail, \
             got {refused}"
        );
        assert_eq!(
            describe(&batch, &[&attached]).await.len(),
            1,
            "case a policy a queue names (attempt {attempt}): the refused delete must \
             change nothing"
        );
    }

    // A policy no queue names is deletable, so the refusal is about the ATTACHMENT and not
    // about there being a queue at all.
    let (status, answered) = delete(&batch, &unattached).await;
    assert_eq!(
        status, 200,
        "case a policy no queue names: expected the empty 200, got {status} {answered}"
    );
    assert!(
        describe(&batch, &[&unattached]).await.is_empty(),
        "case a policy no queue names: it must be gone after the delete"
    );
}

#[tokio::test]
async fn an_identifier_that_cannot_name_a_policy_is_refused() {
    let batch = Batch::new();

    let policy_arn = create(&batch, json!({"name": "sp-identity"})).await;
    let environment = compute_environment(&batch, "ce-sp-identity").await;

    // Every shape an identifier can have that names no scheduling policy: an empty
    // identifier, a value outside the charset `name` documents, the ARN of another Batch
    // resource kind, an ARN with nothing after the account, an ARN whose policy name is
    // empty, and one carrying a nested path.
    let refused: Vec<(&str, String)> = vec![
        ("an empty identifier", String::new()),
        ("a name carrying a space", "sp identity".to_string()),
        ("a name carrying a dot", "sp.identity".to_string()),
        (
            "a name one past the documented length",
            "x".repeat(state::MAX_SCHEDULING_POLICY_NAME_LEN + 1),
        ),
        (
            "an ARN with nothing after the account",
            "arn:aws:batch:us-east-1:000000000000".to_string(),
        ),
        ("a compute-environment ARN", environment.clone()),
        ("a job-queue ARN", arn("job-queue/jq-sp-identity")),
        (
            "a consumable-resource ARN",
            arn("consumable-resource/cr-sp-identity"),
        ),
        ("an ARN with an empty policy name", arn("scheduling-policy/")),
        (
            "an ARN carrying a nested path",
            arn("scheduling-policy/sp-identity/extra"),
        ),
    ];

    for (label, reference) in &refused {
        // Both operations that take a scheduling-policy identifier, so the rule is the
        // member's and not one handler's.
        let calls: [(&str, Value); 2] = [
            (
                "/v1/describeschedulingpolicies",
                json!({"arns": [reference]}),
            ),
            ("/v1/deleteschedulingpolicy", json!({"arn": reference})),
        ];
        for (uri, body) in calls {
            let (status, refused_body) = batch.call(uri, body).await;
            assert_eq!(
                status, 400,
                "case {label:?} on {uri}: expected 400, got {status} {refused_body}"
            );
            assert_eq!(
                error_code(&refused_body),
                "ClientException",
                "case {label:?} on {uri}: expected the modelled ClientException, got \
                 {refused_body}"
            );
            assert!(
                error_message(&refused_body).contains(state::SCHEDULING_POLICY_ARN_NOT_VALID),
                "case {label:?} on {uri}: expected the ARN-pattern detail, got {refused_body}"
            );
        }
        // Nothing the refusals touched: the policy is still there, under its ARN.
        assert_eq!(
            describe(&batch, &[&policy_arn]).await.len(),
            1,
            "case {label:?}: a refused request must change nothing"
        );
    }

    // One bad entry refuses the whole describe, however many good ones travel with it.
    let (status, body) = batch
        .call(
            "/v1/describeschedulingpolicies",
            json!({"arns": [&policy_arn, "sp identity"]}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case a good ARN beside a bad entry: expected 400, got {status} {body}"
    );

    // And the identifier forms that CAN name a policy are served. `DescribeSchedulingPolicies`
    // takes the ARN form ALONE (`arns` is "a list of … Amazon Resource Name (ARN) entries"),
    // so a bare name is refused there and resolves for the delete; an identifier of the right
    // shape naming nothing is served by both — reported as absent by the describe, and
    // already deleted by the delete.
    assert_eq!(
        describe(&batch, &[&policy_arn]).await.len(),
        1,
        "case the policy's own ARN: expected the policy to be described"
    );
    let (status, body) = batch
        .call(
            "/v1/describeschedulingpolicies",
            json!({"arns": ["sp-identity"]}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case the policy's own name on a describe: expected 400, got {status} {body}"
    );
    assert!(
        describe(&batch, &[&arn("scheduling-policy/sp-never-created")])
            .await
            .is_empty(),
        "case a well-formed ARN naming nothing: expected an empty list, not a refusal"
    );
    for (label, reference) in [
        (
            "a well-formed ARN naming nothing",
            arn("scheduling-policy/sp-never-created"),
        ),
        ("a name naming nothing", "sp-never-created".to_string()),
    ] {
        let (status, answered) = delete(&batch, &reference).await;
        assert_eq!(
            status, 200,
            "case {label}: deleting it must answer the empty 200, got {status} {answered}"
        );
    }
}

#[tokio::test]
async fn every_create_answers_a_name_collision_with_the_conflict_status() {
    let batch = Batch::new();

    let environment = compute_environment(&batch, "ce-conflict").await;
    batch
        .ok(
            "/v1/createjobqueue",
            json!({
                "jobQueueName": "jq-conflict-host",
                "priority": 1,
                "computeEnvironmentOrder": [{"order": 1, "computeEnvironment": &environment}],
            }),
        )
        .await;

    // One row per resource kind whose create keys on a name the caller chooses, so the
    // rule is the frontend's and not one handler's.
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a compute environment",
            "/v1/createcomputeenvironment",
            json!({"computeEnvironmentName": "ce-conflict-row", "type": "UNMANAGED"}),
        ),
        (
            "a job queue",
            "/v1/createjobqueue",
            json!({
                "jobQueueName": "jq-conflict-row",
                "priority": 1,
                "computeEnvironmentOrder": [{"order": 1, "computeEnvironment": &environment}],
            }),
        ),
        (
            "a scheduling policy",
            "/v1/createschedulingpolicy",
            json!({"name": "sp-conflict-row"}),
        ),
        (
            "a consumable resource",
            "/v1/createconsumableresource",
            json!({"consumableResourceName": "cr-conflict-row", "totalQuantity": 4}),
        ),
        (
            "a service environment",
            "/v1/createserviceenvironment",
            json!({
                "serviceEnvironmentName": "se-conflict-row",
                "serviceEnvironmentType": "SAGEMAKER_TRAINING",
                "capacityLimits": [{"maxCapacity": 1, "capacityUnit": "NUM_INSTANCES"}],
            }),
        ),
        (
            "a quota share",
            "/v1/createquotashare",
            json!({
                "quotaShareName": "qs-conflict-row",
                "jobQueue": "jq-conflict-host",
                "capacityLimits": [{"maxCapacity": 4, "capacityUnit": "ml.m5.large"}],
                "resourceSharingConfiguration": {"strategy": "RESERVE"},
                "preemptionConfiguration": {"inSharePreemption": "DISABLED"},
            }),
        ),
    ];

    for (label, uri, body) in rows {
        let (status, created) = batch.call(uri, body.clone()).await;
        assert_eq!(
            status, 200,
            "case {label}: the first create must be accepted, got {status} {created}"
        );
        // Twice more, so the conflict is the steady answer and not a first-repeat quirk.
        for attempt in 1..=2 {
            let (status, refused) = batch.call(uri, body.clone()).await;
            assert_eq!(
                status,
                state::CONFLICT_STATUS,
                "case {label} (attempt {attempt}): expected the conflict status {}, got \
                 {status} {refused}",
                state::CONFLICT_STATUS
            );
            // The recorded conflict status, pinned as the literal the wire carries.
            assert_eq!(
                status, 409,
                "case {label} (attempt {attempt}): the conflict status on the wire is 409, \
                 got {status} {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "ClientException",
                "case {label} (attempt {attempt}): expected the modelled ClientException, \
                 got {refused}"
            );
            assert!(
                error_message(&refused).contains(state::OBJECT_ALREADY_EXISTS),
                "case {label} (attempt {attempt}): expected the bare collision detail, got \
                 {refused}"
            );
        }
    }
}

#[tokio::test]
async fn every_describe_reader_materializes_the_tag_map() {
    let batch = Batch::new();

    let environment = compute_environment(&batch, "ce-untagged").await;
    batch
        .ok(
            "/v1/createjobqueue",
            json!({
                "jobQueueName": "jq-untagged",
                "priority": 1,
                "computeEnvironmentOrder": [{"order": 1, "computeEnvironment": &environment}],
            }),
        )
        .await;
    batch
        .ok(
            "/v1/createschedulingpolicy",
            json!({"name": "sp-untagged"}),
        )
        .await;
    batch
        .ok(
            "/v1/createconsumableresource",
            json!({"consumableResourceName": "cr-untagged", "totalQuantity": 1}),
        )
        .await;
    batch
        .ok(
            "/v1/createserviceenvironment",
            json!({
                "serviceEnvironmentName": "se-untagged",
                "serviceEnvironmentType": "SAGEMAKER_TRAINING",
                "capacityLimits": [{"maxCapacity": 1, "capacityUnit": "NUM_INSTANCES"}],
            }),
        )
        .await;
    batch
        .ok(
            "/v1/registerjobdefinition",
            json!({
                "jobDefinitionName": "jd-untagged",
                "type": "container",
                "containerProperties": container_properties(),
            }),
        )
        .await;

    // (case, describe request, the path at which that reader puts the record's tags)
    let readers: Vec<(&str, &str, Value, Vec<&str>)> = vec![
        (
            "a compute environment",
            "/v1/describecomputeenvironments",
            json!({"computeEnvironments": ["ce-untagged"]}),
            vec!["computeEnvironments", "0", "tags"],
        ),
        (
            "a job queue",
            "/v1/describejobqueues",
            json!({"jobQueues": ["jq-untagged"]}),
            vec!["jobQueues", "0", "tags"],
        ),
        (
            "a scheduling policy",
            "/v1/describeschedulingpolicies",
            json!({"arns": [arn("scheduling-policy/sp-untagged")]}),
            vec!["schedulingPolicies", "0", "tags"],
        ),
        (
            "a consumable resource",
            "/v1/describeconsumableresource",
            json!({"consumableResource": "cr-untagged"}),
            vec!["tags"],
        ),
        (
            "a service environment",
            "/v1/describeserviceenvironments",
            json!({"serviceEnvironments": ["se-untagged"]}),
            vec!["serviceEnvironments", "0", "tags"],
        ),
        (
            "a job definition revision",
            "/v1/describejobdefinitions",
            json!({"jobDefinitionName": "jd-untagged"}),
            vec!["jobDefinitions", "0", "tags"],
        ),
    ];

    for (case, uri, request, path) in readers {
        let described = batch.ok(uri, request).await;
        let mut at = &described;
        for segment in &path {
            at = match segment.parse::<usize>() {
                Ok(index) => &at[index],
                Err(_) => &at[*segment],
            };
        }
        assert_eq!(
            at,
            &json!({}),
            "case {case}: a record created with no tags must report an EMPTY map, got \
             {described}"
        );
    }
}

/// How long a clock-owed transition takes ([`state::JOB_PHASE_MILLIS`]), with margin for a
/// slow machine.
async fn wait_for_due() {
    let span = u64::try_from(state::JOB_PHASE_MILLIS).unwrap_or(500);
    tokio::time::sleep(std::time::Duration::from_millis(span * 3)).await;
}

#[tokio::test]
async fn updating_a_policy_writes_the_record_every_reader_reads() {
    let batch = Batch::new();

    // Rows: the fair-share policies an update can carry, differing along which members
    // they name, how many share identifiers and of which form. Each is applied to its own
    // policy and read back; the reader materialises the members the request left out.
    let rows: Vec<(&str, Value)> = vec![
        ("an empty fair-share policy", json!({})),
        (
            "decay and reservation only",
            json!({"shareDecaySeconds": 600, "computeReservation": 25}),
        ),
        (
            "one share identifier with an explicit weight",
            json!({"shareDistribution": [{"shareIdentifier": "UserA", "weightFactor": 0.5}]}),
        ),
        (
            "two share identifiers, one of them a prefix with no weight",
            json!({
                "shareDecaySeconds": 3600,
                "shareDistribution": [
                    {"shareIdentifier": "teamA*"},
                    {"shareIdentifier": "teamB1", "weightFactor": 2.0},
                ],
            }),
        ),
    ];

    for (index, (label, fairshare)) in rows.iter().enumerate() {
        let name = format!("sp-update-{index}");
        let policy_arn = create(&batch, json!({"name": &name})).await;
        let (status, answered) = batch
            .call(
                "/v1/updateschedulingpolicy",
                json!({"arn": &policy_arn, "fairsharePolicy": fairshare}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the empty 200, got {status} {answered}"
        );

        // The reader answers from the UPDATED record, with every fairsharePolicy member
        // materialised the way it is for a create.
        let found = describe(&batch, &[&policy_arn]).await;
        assert_eq!(
            found.len(),
            1,
            "case {label:?}: expected the updated policy, got {found:?}"
        );
        let detail = &found[0];
        assert_eq!(
            detail["fairsharePolicy"],
            crate::scheduling_policies::materialized_fairshare(Some(fairshare), None),
            "case {label:?}: expected the submitted fairsharePolicy back, got {detail}"
        );
    }

    // The refusals, each leaving the stored policy exactly as it was.
    let quota = create(
        &batch,
        json!({
            "name": "sp-update-quota",
            "quotaSharePolicy": {"idleResourceAssignmentStrategy": "FIFO"},
        }),
    )
    .await;
    let fairshare = create(
        &batch,
        json!({"name": "sp-update-fair", "fairsharePolicy": {"computeReservation": 7}}),
    )
    .await;
    let refusals: Vec<(&str, Value)> = vec![
        (
            "an identifier that cannot name a policy",
            json!({"arn": "sp update", "fairsharePolicy": {}}),
        ),
        (
            "an identifier naming no stored policy",
            json!({"arn": arn("scheduling-policy/sp-update-absent"), "fairsharePolicy": {}}),
        ),
        (
            "a policy of both kinds",
            json!({
                "arn": &fairshare,
                "fairsharePolicy": {},
                "quotaSharePolicy": {"idleResourceAssignmentStrategy": "FIFO"},
            }),
        ),
        (
            "a quota-share policy on a fair-share policy",
            json!({
                "arn": &fairshare,
                "quotaSharePolicy": {"idleResourceAssignmentStrategy": "FIFO"},
            }),
        ),
        (
            "a fair-share policy on a quota-share policy",
            json!({"arn": &quota, "fairsharePolicy": {"computeReservation": 1}}),
        ),
        (
            "a computeReservation one past the documented maximum",
            json!({
                "arn": &fairshare,
                "fairsharePolicy": {"computeReservation": state::MAX_COMPUTE_RESERVATION + 1},
            }),
        ),
    ];
    let before_fairshare = describe(&batch, &[&fairshare]).await;
    let before_quota = describe(&batch, &[&quota]).await;
    for (label, body) in refusals {
        let (status, refused) = batch.call("/v1/updateschedulingpolicy", body).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ClientException",
            "case {label:?}: expected the modelled ClientException, got {refused}"
        );
        assert_eq!(
            describe(&batch, &[&fairshare]).await,
            before_fairshare,
            "case {label:?}: a refused update must change nothing"
        );
        assert_eq!(
            describe(&batch, &[&quota]).await,
            before_quota,
            "case {label:?}: a refused update must change nothing"
        );
    }
}

#[tokio::test]
async fn a_service_environment_is_updated_in_place_and_deleted_out_of_every_reader() {
    let batch = Batch::new();

    /// The one `ServiceEnvironmentDetail` for `name`, or `None` once nothing holds it.
    async fn describe_environment(batch: &Batch, name: &str) -> Option<Value> {
        let body = batch
            .ok(
                "/v1/describeserviceenvironments",
                json!({"serviceEnvironments": [name]}),
            )
            .await;
        body["serviceEnvironments"]
            .as_array()
            .and_then(|found| found.first().cloned())
    }

    // Rows: the updates an environment can take, differing along which members they carry
    // and how many capacity limits of which unit.
    let rows: Vec<(&str, Value)> = vec![
        ("state only", json!({"state": "DISABLED"})),
        (
            "capacityLimits only, one NUM_INSTANCES entry",
            json!({"capacityLimits": [{"maxCapacity": 9, "capacityUnit": "NUM_INSTANCES"}]}),
        ),
        (
            "state and three limits of mixed units",
            json!({
                "state": "DISABLED",
                "capacityLimits": [
                    {"maxCapacity": 1, "capacityUnit": "ml.m5.large"},
                    {"maxCapacity": 2, "capacityUnit": "ml.p4d.24xlarge"},
                    {"maxCapacity": 3, "capacityUnit": "NUM_INSTANCES"},
                ],
            }),
        ),
        ("neither member: nothing changes", json!({})),
    ];

    for (index, (label, update)) in rows.iter().enumerate() {
        let name = format!("se-update-{index}");
        batch
            .ok(
                "/v1/createserviceenvironment",
                json!({
                    "serviceEnvironmentName": &name,
                    "serviceEnvironmentType": "SAGEMAKER_TRAINING",
                    "capacityLimits": [{"maxCapacity": 4, "capacityUnit": "NUM_INSTANCES"}],
                }),
            )
            .await;
        let before = describe_environment(&batch, &name)
            .await
            .expect("the created environment is describable");

        let mut body = update.clone();
        body["serviceEnvironment"] = json!(&name);
        let answered = batch.ok("/v1/updateserviceenvironment", body).await;
        assert_eq!(
            answered["serviceEnvironmentArn"].as_str(),
            Some(arn(&format!("service-environment/{name}")).as_str()),
            "case {label:?}: expected the environment's ARN back, got {answered}"
        );

        let after = describe_environment(&batch, &name)
            .await
            .expect("the updated environment is describable");
        let expected_state = update["state"].as_str().unwrap_or("ENABLED");
        assert_eq!(
            after["state"].as_str(),
            Some(expected_state),
            "case {label:?}: expected state {expected_state}, got {after}"
        );
        let expected_limits = update
            .get("capacityLimits")
            .cloned()
            .unwrap_or_else(|| before["capacityLimits"].clone());
        assert_eq!(
            after["capacityLimits"], expected_limits,
            "case {label:?}: expected the submitted capacityLimits, got {after}"
        );
        assert_eq!(
            after["status"].as_str(),
            Some("VALID"),
            "case {label:?}: an update must leave the environment VALID, got {after}"
        );
    }

    // The delete's preconditions, from the operation's own doc: DISABLED first, and
    // disassociated from every job queue.
    let environment = compute_environment(&batch, "ce-se-delete").await;
    batch
        .ok(
            "/v1/createserviceenvironment",
            json!({
                "serviceEnvironmentName": "se-delete",
                "serviceEnvironmentType": "SAGEMAKER_TRAINING",
                "capacityLimits": [{"maxCapacity": 1, "capacityUnit": "NUM_INSTANCES"}],
            }),
        )
        .await;
    let _ = environment;

    // ENABLED: refused, and the environment is untouched.
    let (status, refused) = batch
        .call(
            "/v1/deleteserviceenvironment",
            json!({"serviceEnvironment": "se-delete"}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case delete an ENABLED environment: expected 400, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ClientException",
        "case delete an ENABLED environment: expected the modelled ClientException, got \
         {refused}"
    );
    assert!(
        error_message(&refused).contains("DISABLED"),
        "case delete an ENABLED environment: the refusal must state the precondition, got \
         {refused}"
    );
    assert!(
        describe_environment(&batch, "se-delete").await.is_some(),
        "case delete an ENABLED environment: the refused delete must change nothing"
    );

    // A queue still names it: refused too.
    batch
        .ok(
            "/v1/updateserviceenvironment",
            json!({"serviceEnvironment": "se-delete", "state": "DISABLED"}),
        )
        .await;
    batch
        .ok(
            "/v1/createjobqueue",
            json!({
                "jobQueueName": "jq-se-delete",
                "priority": 1,
                "jobQueueType": "SAGEMAKER_TRAINING",
                "serviceEnvironmentOrder": [{"order": 1, "serviceEnvironment": "se-delete"}],
            }),
        )
        .await;
    let (status, refused) = batch
        .call(
            "/v1/deleteserviceenvironment",
            json!({"serviceEnvironment": "se-delete"}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case delete an attached environment: expected 400, got {status} {refused}"
    );
    assert!(
        describe_environment(&batch, "se-delete").await.is_some(),
        "case delete an attached environment: the refused delete must change nothing"
    );

    // Disassociated and DISABLED: the delete is accepted, the environment reports the
    // non-terminal DELETING the model declares, and the CLOCK removes it.
    batch
        .ok(
            "/v1/updatejobqueue",
            json!({"jobQueue": "jq-se-delete", "state": "DISABLED"}),
        )
        .await;
    batch
        .ok("/v1/deletejobqueue", json!({"jobQueue": "jq-se-delete"}))
        .await;
    wait_for_due().await;
    for (attempt, reference) in [(1, "se-delete".to_string()), (2, arn("service-environment/se-delete"))] {
        let (status, answered) = batch
            .call(
                "/v1/deleteserviceenvironment",
                json!({"serviceEnvironment": reference}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case delete (attempt {attempt}): expected the empty 200, got {status} {answered}"
        );
    }
    let deleting = describe_environment(&batch, "se-delete").await;
    assert_eq!(
        deleting.as_ref().and_then(|detail| detail["status"].as_str()),
        Some("DELETING"),
        "case delete: the environment must report DELETING until the removal falls due, \
         got {deleting:?}"
    );
    wait_for_due().await;
    assert!(
        describe_environment(&batch, "se-delete").await.is_none(),
        "case delete: the clock must remove the environment from every reader"
    );
    // And a delete of what is gone is the same empty success.
    let (status, answered) = batch
        .call(
            "/v1/deleteserviceenvironment",
            json!({"serviceEnvironment": "se-delete"}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case delete what is already gone: expected the empty 200, got {status} {answered}"
    );
}
