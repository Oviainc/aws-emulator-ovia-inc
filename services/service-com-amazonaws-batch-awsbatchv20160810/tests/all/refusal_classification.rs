// SPDX-License-Identifier: CC0-1.0

//! How AWS Batch CLASSIFIES and WORDS the refusals it raises, end to end over the wire.
//!
//! Every case drives a real restJson1 request through the router the emulator mounts, so
//! what is asserted is the response a `batch` client branches on, and every test asserts a
//! RULE over a table of inputs — the recorded request is one row of each.
//!
//! The rules under test:
//!
//! * a request whose RESULTANT tag set would leave one resource holding more user tags
//!   than `TagrisTagsMap` admits is refused by the tagging layer with one fixed sentence
//!   naming that maximum — the same code and the same words whether the request alone
//!   carries too many tags or the merge into the tags the resource already holds takes it
//!   past the ceiling, and whichever operation carried the map;
//! * a delete of a resource whose state still accepts work states the rule — the KIND of
//!   resource that cannot be deleted while enabled, and the state to set it to first —
//!   and carries nothing from the request;
//! * `UpdateComputeEnvironment` refuses `computeResources` for an environment Batch does
//!   not manage with the rule's own sentence, naming the environment by its ARN whichever
//!   of name or ARN the request reached it by;
//! * a value outside a closed set the SERVICE applies (the model types the member as a
//!   plain `String`) is refused inside Batch's frontend envelope with the member and the
//!   set it enforced, and no echo of the rejected value;
//! * a NAME outside the pattern the service holds its member to is refused inside that
//!   same envelope with the member and the fact that a pattern governs it — never the
//!   length or the charset;
//! * a scheduling policy carrying both kinds of policy is refused inside that envelope
//!   with the exclusivity rule, by the create and by the update alike.
//!
//! Every enveloped refusal is checked through `refusal_contract::enveloped`, which also
//! asserts the response states a freshly minted request id.

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::state::{self, limits};

use crate::job_lifecycle::{arn, compute_environment, error_code, error_message, job_queue, Batch};
use crate::refusal_contract::enveloped;
use crate::resource_tags::{tag, tags_body};
use crate::tag_mutation_no_content::NO_CONTENT_STATUS;

/// The modelled error code every client refusal of this service's own operations carries.
/// Source: `smithy_query(query="operation", …)` — every Batch operation declares exactly
/// `ClientException` (HTTP 400) and `ServerException` (HTTP 500).
const CLIENT_EXCEPTION: &str = "ClientException";

/// The code the TAGGING layer refuses a request it will not accept under, written out as
/// the literal a client sees rather than taken from the crate's constant.
const TAGGING_BAD_REQUEST: &str = "BadRequestException";

/// The one sentence the tagging layer refuses a resultant tag set past the ceiling with,
/// as a client reads it. Shared with the tag-plane suite so one rule is written once.
pub(crate) const TAG_SET_LIMIT_DETAIL: &str =
    "The resultant tag set must not have more than 50 user tags.";

/// `count` distinct tags, as the `tags` member of a request.
fn many_tags(count: usize) -> Value {
    tags_body(
        &(0..count)
            .map(|index| (format!("tag-{index}"), index.to_string()))
            .collect::<Vec<(String, String)>>(),
    )
}

/// Assert `body` is the tagging layer's refusal of a resultant tag set past the ceiling.
fn assert_tag_set_refusal(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {body}"
    );
    assert_eq!(
        error_code(body),
        TAGGING_BAD_REQUEST,
        "case {case:?}: expected the tagging layer's {TAGGING_BAD_REQUEST}, got {body}"
    );
    assert_eq!(
        error_message(body),
        TAG_SET_LIMIT_DETAIL,
        "case {case:?}: expected the tag-set sentence {TAG_SET_LIMIT_DETAIL:?}, got {:?}",
        error_message(body)
    );
}

/// Assert `body` is the modelled `ClientException` whose message is `detail` inside Batch's
/// frontend envelope.
fn assert_enveloped_refusal(case: &str, status: u16, body: &Value, detail: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {body}"
    );
    assert_eq!(
        error_code(body),
        CLIENT_EXCEPTION,
        "case {case:?}: expected the modelled {CLIENT_EXCEPTION}, got {body}"
    );
    let (stated, _request_id) = enveloped(case, &error_message(body));
    assert_eq!(
        stated, detail,
        "case {case:?}: expected the envelope to state {detail:?}, got {stated:?}"
    );
}

/// RULE: a request whose resultant tag set would exceed the maximum `TagrisTagsMap`
/// declares is refused with the tagging layer's one fixed sentence naming that maximum.
///
/// The rows differ along what the rule generalises over: WHICH operation carries the map,
/// how far past the ceiling the request goes, and whether the ceiling is reached by one
/// request or by a merge into tags the resource already holds.
#[tokio::test]
async fn a_resultant_tag_set_past_the_ceiling_is_refused_with_the_one_tag_set_sentence() {
    let batch = Batch::new();
    let environment = compute_environment(&batch, "tag-ceiling-env").await;

    // The sentence names the maximum the model declares, so the crate's bound and the
    // literal asserted here cannot drift apart.
    assert_eq!(
        state::tag_set_limit_message(),
        TAG_SET_LIMIT_DETAIL,
        "case the sentence the handlers emit: expected {TAG_SET_LIMIT_DETAIL:?}, got {:?}",
        state::tag_set_limit_message()
    );

    let over = limits::MAX_TAGS_PER_RESOURCE + 1;
    let far_over = limits::MAX_TAGS_PER_RESOURCE + 10;
    // (case, the operation's URI, the request carrying the oversized map)
    let carried: Vec<(String, &str, Value)> = vec![
        (
            format!("CreateJobQueue carrying {over} tags"),
            "/v1/createjobqueue",
            json!({
                "jobQueueName": "tag-ceiling-queue",
                "priority": 1,
                "computeEnvironmentOrder": [{"order": 1, "computeEnvironment": environment}],
                "tags": many_tags(over),
            }),
        ),
        (
            format!("CreateConsumableResource carrying {far_over} tags"),
            "/v1/createconsumableresource",
            json!({
                "consumableResourceName": "tag-ceiling-resource",
                "totalQuantity": 1,
                "tags": many_tags(far_over),
            }),
        ),
        (
            format!("RegisterJobDefinition carrying {over} tags"),
            "/v1/registerjobdefinition",
            json!({
                "jobDefinitionName": "tag-ceiling-def",
                "type": "container",
                "containerProperties": {"image": "busybox"},
                "tags": many_tags(over),
            }),
        ),
        (
            format!("CreateServiceEnvironment carrying {far_over} tags"),
            "/v1/createserviceenvironment",
            json!({
                "serviceEnvironmentName": "tag-ceiling-service-env",
                "serviceEnvironmentType": "SAGEMAKER_TRAINING",
                "capacityLimits": [{"maxCapacity": 1, "capacityUnit": "NUM_INSTANCES"}],
                "tags": many_tags(far_over),
            }),
        ),
    ];
    for (case, uri, body) in carried {
        let (status, refused) = batch.call(uri, body).await;
        assert_tag_set_refusal(&case, status, &refused);
    }

    // The tagging endpoint itself, which carries nothing else.
    for count in [over, far_over] {
        let case = format!("TagResource carrying {count} tags");
        let (status, refused) = tag(&batch, &environment, many_tags(count)).await;
        assert_tag_set_refusal(&case, status, &refused);
    }

    // The MERGE: the resultant set is what the ceiling is about, so a request that is
    // small enough on its own is refused in the same words once the tags the resource
    // already holds are counted.
    //
    // (case, how many tags the resource is given first, how many the merge adds)
    let merges: &[(&str, usize, usize)] = &[
        ("one tag onto a resource already at the ceiling", 50, 1),
        ("twenty-five tags onto a resource holding thirty", 30, 25),
        ("fifty tags onto a resource holding one", 1, 50),
    ];
    for (index, (case, held, added)) in merges.iter().enumerate() {
        let queue = job_queue(
            &batch,
            &format!("tag-merge-queue-{index}"),
            &environment,
        )
        .await;
        let first = tags_body(
            &(0..*held)
                .map(|slot| (format!("held-{slot}"), slot.to_string()))
                .collect::<Vec<(String, String)>>(),
        );
        let (status, answered) = tag(&batch, &queue, first).await;
        assert_eq!(
            status, NO_CONTENT_STATUS,
            "case {case:?}: the tags held first must be accepted, got {status} {answered}"
        );
        let more = tags_body(
            &(0..*added)
                .map(|slot| (format!("added-{slot}"), slot.to_string()))
                .collect::<Vec<(String, String)>>(),
        );
        let (status, refused) = tag(&batch, &queue, more).await;
        assert_tag_set_refusal(case, status, &refused);
    }
}

/// RULE: a delete of a resource that is still enabled states the rule for that KIND of
/// resource, and nothing from the request.
///
/// The rows are the three kinds of this service whose delete requires a disabled resource.
#[tokio::test]
async fn deleting_a_resource_that_is_still_enabled_states_the_rule_for_its_kind() {
    let batch = Batch::new();
    compute_environment(&batch, "delete-enabled-env").await;
    // The queue hangs off a SECOND environment, so the first one is free of relationships
    // and the only thing standing in the way of its delete is its state.
    let queue_environment = compute_environment(&batch, "delete-enabled-queue-env").await;
    job_queue(&batch, "delete-enabled-queue", &queue_environment).await;
    batch
        .ok(
            "/v1/createserviceenvironment",
            json!({
                "serviceEnvironmentName": "delete-enabled-service-env",
                "serviceEnvironmentType": "SAGEMAKER_TRAINING",
                "state": "ENABLED",
                "capacityLimits": [{"maxCapacity": 1, "capacityUnit": "NUM_INSTANCES"}],
            }),
        )
        .await;

    // (the kind as the refusal names it, the delete's URI, the delete's body)
    let kinds: Vec<(&str, &str, Value)> = vec![
        (
            "compute environment",
            "/v1/deletecomputeenvironment",
            json!({"computeEnvironment": "delete-enabled-env"}),
        ),
        (
            "job queue",
            "/v1/deletejobqueue",
            json!({"jobQueue": "delete-enabled-queue"}),
        ),
        (
            "service environment",
            "/v1/deleteserviceenvironment",
            json!({"serviceEnvironment": "delete-enabled-service-env"}),
        ),
    ];

    for (kind, uri, body) in kinds {
        let expected = format!("Cannot delete an enabled {kind}, set the state to DISABLED first.");
        let (status, refused) = batch.call(uri, body).await;
        assert_eq!(
            status, 400,
            "case {kind:?}: expected HTTP 400, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            CLIENT_EXCEPTION,
            "case {kind:?}: expected the modelled {CLIENT_EXCEPTION}, got {refused}"
        );
        assert_eq!(
            error_message(&refused),
            expected,
            "case {kind:?}: expected the bare sentence {expected:?}, got {:?}",
            error_message(&refused)
        );
        // The wording the handlers emit is the wording asserted here.
        assert_eq!(
            state::delete_enabled_message(kind),
            expected,
            "case {kind:?}: expected the handlers to word it {expected:?}, got {:?}",
            state::delete_enabled_message(kind)
        );
    }

    // The refusal is about the STATE, not the identity: disabled, the same delete is
    // accepted and the environment is gone.
    batch
        .ok(
            "/v1/updatecomputeenvironment",
            json!({"computeEnvironment": "delete-enabled-env", "state": "DISABLED"}),
        )
        .await;
    let (status, answered) = batch
        .call(
            "/v1/deletecomputeenvironment",
            json!({"computeEnvironment": "delete-enabled-env"}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case a disabled compute environment: expected the delete to be accepted, got {status} \
         {answered}"
    );
}

/// RULE: `computeResources` for an environment Batch does not manage is refused with the
/// rule's own sentence, naming the environment by the ARN it is stored under.
///
/// The rows differ in the reference FORM the update used (the name, and the full ARN) and
/// in what the `computeResources` object carries: neither changes the sentence, and the
/// ARN in it is always the stored one.
#[tokio::test]
async fn updating_compute_resources_on_an_unmanaged_environment_names_its_arn() {
    let batch = Batch::new();
    let environment = compute_environment(&batch, "unmanaged-ce").await;
    let expected = format!(
        "Cannot update, compute resources not supported on unmanaged compute environment \
         {environment}."
    );

    let resources: Vec<(&str, Value)> = vec![
        ("minvCpus and maxvCpus", json!({"minvCpus": 1, "maxvCpus": 4})),
        ("maxvCpus alone", json!({"maxvCpus": 8})),
        (
            "subnets alone",
            json!({"subnets": ["subnet-0123456789abcdef0"]}),
        ),
        ("an empty object", json!({})),
    ];

    for (label, compute_resources) in resources {
        for reference in ["unmanaged-ce", environment.as_str()] {
            let case = format!("{label} by {reference}");
            let (status, refused) = batch
                .call(
                    "/v1/updatecomputeenvironment",
                    json!({
                        "computeEnvironment": reference,
                        "computeResources": compute_resources,
                    }),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {refused}"
            );
            assert_eq!(
                error_code(&refused),
                CLIENT_EXCEPTION,
                "case {case:?}: expected the modelled {CLIENT_EXCEPTION}, got {refused}"
            );
            assert_eq!(
                error_message(&refused),
                expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                error_message(&refused)
            );
        }
    }

    assert_eq!(
        state::unmanaged_compute_resources_message(&environment),
        expected,
        "case the sentence the handler emits: expected {expected:?}, got {:?}",
        state::unmanaged_compute_resources_message(&environment)
    );
}

/// RULE: a value outside a closed set the SERVICE applies is refused inside Batch's
/// frontend envelope, stating the member and the set it enforced and echoing nothing of
/// the value.
///
/// The two members are the ones this service holds to a set of its own, each a plain
/// `String` in the model (`smithy_query(query="operation",
/// operation="CreateConsumableResource,UpdateConsumableResource")`), and the sets are the
/// ones those members' docs publish: `resourceType` "REPLENISHABLE … NON_REPLENISHABLE",
/// `operation` "Must be one of: SET, ADD, REMOVE". The rows differ in how the value misses
/// the set: an unknown value, the right value in the wrong case, and one with stray
/// whitespace.
#[tokio::test]
async fn a_value_outside_a_closed_set_the_service_applies_states_the_member_and_the_set() {
    let batch = Batch::new();
    batch
        .ok(
            "/v1/createconsumableresource",
            json!({"consumableResourceName": "closed-set-resource", "totalQuantity": 10}),
        )
        .await;

    // The catalogs asserted below are the handlers', read off those member docs.
    assert_eq!(
        state::CONSUMABLE_RESOURCE_TYPES.to_vec(),
        vec!["REPLENISHABLE", "NON_REPLENISHABLE"],
        "case the resourceType catalog: got {:?}",
        state::CONSUMABLE_RESOURCE_TYPES
    );
    assert_eq!(
        state::CONSUMABLE_RESOURCE_UPDATE_OPERATIONS.to_vec(),
        vec!["SET", "ADD", "REMOVE"],
        "case the operation catalog: got {:?}",
        state::CONSUMABLE_RESOURCE_UPDATE_OPERATIONS
    );

    let type_detail = "resourceType needs to be one of the following values: \
                       [REPLENISHABLE, NON_REPLENISHABLE]";
    let operation_detail = "operation needs to be one of the following values: [SET, ADD, REMOVE]";

    // (case, the operation's URI, the request, the detail the envelope must state)
    let rows: Vec<(String, &str, Value, &str)> = vec![
        (
            "an unknown resourceType".to_string(),
            "/v1/createconsumableresource",
            json!({
                "consumableResourceName": "closed-set-unknown",
                "totalQuantity": 1,
                "resourceType": "INFINITE",
            }),
            type_detail,
        ),
        (
            "a resourceType in the wrong case".to_string(),
            "/v1/createconsumableresource",
            json!({
                "consumableResourceName": "closed-set-case",
                "totalQuantity": 1,
                "resourceType": "replenishable",
            }),
            type_detail,
        ),
        (
            "a resourceType with stray whitespace".to_string(),
            "/v1/createconsumableresource",
            json!({
                "consumableResourceName": "closed-set-space",
                "totalQuantity": 1,
                "resourceType": "NON_REPLENISHABLE ",
            }),
            type_detail,
        ),
        (
            "an unknown update operation".to_string(),
            "/v1/updateconsumableresource",
            json!({
                "consumableResource": "closed-set-resource",
                "operation": "MULTIPLY",
                "quantity": 1,
            }),
            operation_detail,
        ),
        (
            "an update operation in the wrong case".to_string(),
            "/v1/updateconsumableresource",
            json!({
                "consumableResource": "closed-set-resource",
                "operation": "add",
                "quantity": 1,
            }),
            operation_detail,
        ),
    ];

    for (case, uri, body, detail) in rows {
        let (status, refused) = batch.call(uri, body).await;
        assert_enveloped_refusal(&case, status, &refused, detail);
        let message = error_message(&refused);
        assert!(
            !message.contains("MULTIPLY") && !message.contains("INFINITE"),
            "case {case:?}: the refusal must not echo the rejected value, got {message:?}"
        );
    }
}

/// RULE: a name outside the pattern the service holds its member to is refused inside
/// Batch's frontend envelope, stating the member and that a pattern governs it — never a
/// length or a charset.
///
/// The rows span every name member this service checks itself and every way a name can
/// miss the pattern: empty, whitespace, punctuation the charset excludes, a path
/// separator, non-ASCII, and one character past the documented maximum.
#[tokio::test]
async fn a_malformed_name_states_the_pattern_requirement_for_its_member() {
    let batch = Batch::new();
    let environment = compute_environment(&batch, "pattern-env").await;
    let queue = job_queue(&batch, "pattern-queue", &environment).await;
    batch
        .ok(
            "/v1/createserviceenvironment",
            json!({
                "serviceEnvironmentName": "pattern-service-env",
                "serviceEnvironmentType": "SAGEMAKER_TRAINING",
                "capacityLimits": [{"maxCapacity": 1, "capacityUnit": "NUM_INSTANCES"}],
            }),
        )
        .await;
    batch
        .ok(
            "/v1/createjobqueue",
            json!({
                "jobQueueName": "pattern-service-queue",
                "priority": 1,
                "jobQueueType": "SAGEMAKER_TRAINING",
                "serviceEnvironmentOrder": [
                    {"order": 1, "serviceEnvironment": "pattern-service-env"},
                ],
            }),
        )
        .await;

    // Every way a name can miss the pattern. The maximum is the one the members' docs
    // publish ("up to 128 characters long"), so the last row is one character past it.
    let names: Vec<(&str, String)> = vec![
        ("an empty name", String::new()),
        ("a name with a space", "has a space".to_string()),
        ("a name with dots", "dots.not.allowed".to_string()),
        ("a name with a path separator", "slash/inside".to_string()),
        ("a non-ASCII name", "naïve-name".to_string()),
        ("one character past the maximum", "n".repeat(129)),
    ];

    for (label, name) in names {
        // (the operation's URI, the member the name arrives on, the request)
        let operations: Vec<(&str, &str, Value)> = vec![
            (
                "/v1/registerjobdefinition",
                "jobDefinitionName",
                json!({
                    "jobDefinitionName": name,
                    "type": "container",
                    "containerProperties": {"image": "busybox"},
                }),
            ),
            (
                "/v1/createschedulingpolicy",
                "name",
                json!({"name": name}),
            ),
            (
                "/v1/createserviceenvironment",
                "serviceEnvironmentName",
                json!({
                    "serviceEnvironmentName": name,
                    "serviceEnvironmentType": "SAGEMAKER_TRAINING",
                    "capacityLimits": [{"maxCapacity": 1, "capacityUnit": "NUM_INSTANCES"}],
                }),
            ),
            (
                "/v1/createquotashare",
                "quotaShareName",
                json!({
                    "quotaShareName": name,
                    "jobQueue": queue,
                    "capacityLimits": [{"maxCapacity": 1, "capacityUnit": "NUM_INSTANCES"}],
                    "resourceSharingConfiguration": {"strategy": "RESERVE"},
                    "preemptionConfiguration": {"inSharePreemption": "DISABLED"},
                }),
            ),
            (
                "/v1/submitservicejob",
                "jobName",
                json!({
                    "jobName": name,
                    "jobQueue": "pattern-service-queue",
                    "serviceJobType": "SAGEMAKER_TRAINING",
                    "serviceRequestPayload": "{}",
                }),
            ),
        ];

        for (uri, member, body) in operations {
            let case = format!("{label} on {member}");
            let detail = format!("{member} should match a valid pattern.");
            let (status, refused) = batch.call(uri, body).await;
            assert_enveloped_refusal(&case, status, &refused, &detail);
            // The sentence states the rule, not a bound the caller could encode. The
            // assertion reads the STATED sentence out of the envelope rather than the
            // whole message, because the envelope also carries the refused call's
            // freshly minted request id — per-run hex whose digits can spell any
            // literal, which is not something a claim about the WORDING may read.
            let (stated, _request_id) = enveloped(&case, &error_message(&refused));
            assert!(
                !stated.contains("128") && !stated.contains("underscores"),
                "case {case:?}: the refusal must publish no length or charset, got {stated:?}"
            );
            assert_eq!(
                state::name_pattern_detail(member),
                detail,
                "case {case:?}: expected the handlers to word it {detail:?}, got {:?}",
                state::name_pattern_detail(member)
            );
        }
    }
}

/// RULE: a scheduling policy carrying BOTH kinds of policy is refused inside Batch's
/// frontend envelope with the exclusivity rule, by the create and by the update alike.
///
/// The rows differ in what each kind carries, so the refusal is about the PAIR being
/// present and not about either value.
#[tokio::test]
async fn a_scheduling_policy_of_both_kinds_states_the_exclusivity_rule_in_the_envelope() {
    let batch = Batch::new();
    let created = batch
        .ok("/v1/createschedulingpolicy", json!({"name": "exclusive-sp"}))
        .await;
    let policy_arn = created["arn"].as_str().unwrap_or_default().to_string();
    assert_eq!(
        policy_arn,
        arn("scheduling-policy/exclusive-sp"),
        "case the stored policy: expected the minted ARN, got {created}"
    );
    let before = describe_policy(&batch, &policy_arn).await;

    let detail =
        "A scheduling policy can have either fairsharePolicy or quotaSharePolicy, but not both.";
    assert_eq!(
        state::POLICY_KINDS_EXCLUSIVE, detail,
        "case the sentence the handlers emit: expected {detail:?}, got {:?}",
        state::POLICY_KINDS_EXCLUSIVE
    );

    let pairs: Vec<(&str, Value, Value)> = vec![
        (
            "both carrying nothing",
            json!({}),
            json!({"idleResourceAssignmentStrategy": "FIFO"}),
        ),
        (
            "a fair-share distribution beside a quota share",
            json!({"shareDistribution": [{"shareIdentifier": "UserA", "weightFactor": 1.0}]}),
            json!({"idleResourceAssignmentStrategy": "FIFO"}),
        ),
        (
            "fair-share decay beside a quota share",
            json!({"shareDecaySeconds": 60}),
            json!({"idleResourceAssignmentStrategy": "FIFO"}),
        ),
    ];

    for (index, (label, fairshare, quota_share)) in pairs.iter().enumerate() {
        let create_case = format!("CreateSchedulingPolicy with {label}");
        let (status, refused) = batch
            .call(
                "/v1/createschedulingpolicy",
                json!({
                    "name": format!("exclusive-sp-{index}"),
                    "fairsharePolicy": fairshare,
                    "quotaSharePolicy": quota_share,
                }),
            )
            .await;
        assert_enveloped_refusal(&create_case, status, &refused, detail);

        let update_case = format!("UpdateSchedulingPolicy with {label}");
        let (status, refused) = batch
            .call(
                "/v1/updateschedulingpolicy",
                json!({
                    "arn": policy_arn,
                    "fairsharePolicy": fairshare,
                    "quotaSharePolicy": quota_share,
                }),
            )
            .await;
        assert_enveloped_refusal(&update_case, status, &refused, detail);
    }

    // Neither refusal stored or changed anything.
    let after = describe_policy(&batch, &policy_arn).await;
    assert_eq!(
        after, before,
        "case the stored policy: a refused create or update must leave it exactly as it was, got \
         {after}"
    );
}

/// The one `SchedulingPolicyDetail` stored under `policy_arn`.
async fn describe_policy(batch: &Batch, policy_arn: &str) -> Value {
    let body = batch
        .ok(
            "/v1/describeschedulingpolicies",
            json!({"arns": [policy_arn]}),
        )
        .await;
    body["schedulingPolicies"][0].clone()
}
