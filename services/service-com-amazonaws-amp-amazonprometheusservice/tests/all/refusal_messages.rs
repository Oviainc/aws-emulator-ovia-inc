// SPDX-License-Identifier: CC0-1.0

//! What a REFUSAL SAYS, end to end over HTTP.
//!
//! An error message is part of the contract, so every rejection this service decides for
//! itself carries the sentence the service answers for that class — fixed text whose only
//! variable part is state the service substitutes — and NOT a sentence describing a member
//! constraint the model declares (`Invalid <member>: Member must …`, which stays what it
//! is for: a value outside a constraint the request boundary checks).
//!
//! Four suites, one per class of refusal, each a TABLE spanning the class rather than one
//! recorded call:
//!
//! 1. a request whose CONTENT the service rejects on its own rule (a `randomCutForest`
//!    query no supported aggregation operator wraps) — one sentence for the class, naming
//!    no member and echoing nothing;
//! 2. a resource that is MISSING — one sentence per resource family, and per operation
//!    where the operations differ, with the identifier asked for reported in `resourceId`;
//! 3. a conditional write whose `revisionId` is not the current revision — the service's
//!    optimistic-lock sentence, naming the action the refused call would have made;
//! 4. a call the resource's current STATUS forbids — the service's template with that
//!    status, read from the store, as its one variable part.
//!
//! The harness (`Amp`, `Reply`, `text`, `wait_for_due_transition`) is the one
//! `tests/all/workspace_lifecycle.rs` builds, and the per-operation request helpers are
//! the ones the suites for those operations already expose. The sentences themselves are
//! LITERALS here, so the wire text is what is under test: re-wording a constant in the
//! service cannot keep this suite green.

use crate::anomaly_detector_update::{create_detector, describe_detector, put_detector, Spec};
use crate::rule_groups_namespace_lifecycle::{create_body, create_namespace};
use crate::rule_groups_namespace_update::{put_body, put_namespace};
use crate::workspace_lifecycle::{text, wait_for_due_transition, Amp, Reply};
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    stale_revision_message, status_forbids_call_message, NAMESPACE_STATUS_CREATING,
    NO_POLICY_REVISION_ID, POLICY_DELETE_ACTION, POLICY_UPDATE_ACTION,
    QUERY_LOGGING_STATUS_DELETING, RANDOM_CUT_FOREST_AGGREGATION_OPERATORS, STATUS_CREATING,
    STATUS_DELETING,
};
use serde_json::{json, Value};

// The sentences this service answers, as LITERALS — the wire text itself is what this
// suite pins, so a later edit of the wording in `src/state.rs` cannot keep it green.
// Each is the text the behavioural recording shows for its class.

/// `CreateAnomalyDetector`/`PutAnomalyDetector` for a `randomCutForest` query no supported
/// aggregation operator wraps.
const UNSUPPORTED_EXPRESSION: &str = "Unsupported PromQL expression for RandomCutForest, \
                                     expression's root must be a supported aggregation \
                                     operator";

/// A workspace that holds no rules-and-alerting logging configuration, and one that holds
/// no QUERY logging configuration: the service names the family the same way for both.
const LOGGING_NOT_FOUND: &str = "Workspace logging configuration not found.";

/// A workspace that holds no resource-based policy.
const POLICY_NOT_FOUND: &str = "Workspace resource policy not found.";

/// `DescribeWorkspace` for a workspace this account does not hold — the read path's
/// spelling, without a trailing period.
const DESCRIBE_WORKSPACE_NOT_FOUND: &str = "Workspace not found";

/// The same for the operations that CHANGE a workspace, `DeleteWorkspace` among them —
/// with the trailing period.
const WORKSPACE_NOT_FOUND: &str = "Workspace not found.";

/// `PutResourcePolicy` carrying a `revisionId` the policy is no longer at.
const STALE_REVISION_ON_UPDATE: &str =
    "Can't conditionally update a workspace resource policy with a wrong revisionId.";

/// `DeleteWorkspace` on a workspace that is already being deleted.
const DELETE_WORKSPACE_WHILE_DELETING: &str =
    "Can't delete workspace in non-ACTIVE state. Current status is DELETING";

/// A log group ARN inside `CloudWatchLogDestination$logGroupArn`'s modelled pattern
/// `^arn:aws[a-z0-9-]*:logs:[a-z0-9-]+:[0-9]{12}:log-group:[A-Za-z0-9\.\-\_\#/]{1,512}\:\*$`.
fn log_group_arn(region: &str) -> String {
    format!(
        "arn:aws:logs:{region}:{}:log-group:/aws/prometheus/refusals:*",
        engine::emulator_account_id()
    )
}

/// `LoggingDestinations` is modelled `len 1..1`, so one `cloudWatchLogs` destination.
fn destinations_json(region: &str) -> Value {
    json!([{
        "cloudWatchLogs": { "logGroupArn": log_group_arn(region) },
        "filters": { "qspThreshold": 100 },
    }])
}

/// A resource-based policy document: `policyDocument` is modelled a plain `String`, so
/// what it carries only has to differ between puts for a replacement to be observable.
fn policy_document(sid: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": sid,
            "Effect": "Allow",
            "Principal": { "AWS": format!("arn:aws:iam::{}:root", engine::emulator_account_id()) },
            "Action": ["aps:QueryMetrics"],
            "Resource": "*",
        }],
    })
    .to_string()
}

/// Every assertion a modelled `ResourceNotFoundException` makes: the status and code the
/// model declares, the sentence this operation answers, the identifier asked for in
/// `resourceId` — and nothing of the request inside the sentence.
fn assert_not_found(case: &str, reply: &Reply, message: &str, resource_id: &str) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: a missing resource must answer 404, got {reply:?}"
    );
    assert_eq!(
        reply.error_code(),
        "ResourceNotFoundException",
        "case {case:?}: expected the modelled ResourceNotFoundException, got {reply:?}"
    );
    let reported = text(&reply.body["message"]);
    assert_eq!(
        reported, message,
        "case {case:?}: expected {message:?}, got {reported:?}"
    );
    assert_eq!(
        text(&reply.body["resourceId"]),
        resource_id,
        "case {case:?}: the identifier asked for belongs in resourceId, got {reply:?}"
    );
    assert!(
        !reported.contains(resource_id),
        "case {case:?}: the sentence must carry nothing from the request, got {reported:?}"
    );
}

/// A `randomCutForest` query the service cannot run is refused with ONE sentence of its
/// own, whatever form the unsupported expression takes and whichever operation carried it:
/// the expression satisfies every constraint the model declares, so the refusal is about
/// what it MEANS — it names no member, lists no operator and echoes no part of the query.
#[tokio::test]
async fn an_unsupported_random_cut_forest_expression_states_one_sentence() {
    let region = "eu-west-1";
    let amp = Amp::new();
    let workspace_id = amp.active_workspace(region, "rcf-refusals").await;

    // The detector the PutAnomalyDetector rows address: created from a query a supported
    // operator DOES wrap, and settled, so each refused put is refused on the expression
    // rather than on the detector's status.
    let seeded = create_detector(
        &amp,
        region,
        &workspace_id,
        &Spec::new("sum(up)").create_body("wrapped", &[]),
    )
    .await;
    assert_eq!(seeded.status, 202, "seeding a detector: {seeded:?}");
    let detector_id = text(&seeded.body["anomalyDetectorId"]);
    wait_for_due_transition();

    // One row per FORM an expression can take that is not a call to a supported
    // aggregation operator: a range-vector function, an over-time function, a bare
    // selector, a selector with a label matcher, an expression with no identifier at all,
    // an aggregation that is not outermost, a parenthesised aggregation, an operator one
    // character longer than a supported one, and an operator the published catalog does
    // not list (`topk`, which `RandomCutForestConfiguration$query` leaves out).
    let unsupported = [
        "rate(requests_total[5m])",
        "avg_over_time(node_load1[1h])",
        "requests_total",
        "up{job=\"api\"}",
        "1 + 2",
        "1 + sum(rate(requests_total[5m]))",
        "(sum(up))",
        "sums(up)",
        "topk(5, rate(requests_total[5m]))",
    ];

    for query in unsupported {
        let replies = [
            (
                "CreateAnomalyDetector",
                create_detector(
                    &amp,
                    region,
                    &workspace_id,
                    &Spec::new(query).create_body("unwrapped", &[]),
                )
                .await,
            ),
            (
                "PutAnomalyDetector",
                put_detector(
                    &amp,
                    region,
                    &workspace_id,
                    &detector_id,
                    &Spec::new(query).put_body(),
                )
                .await,
            ),
        ];
        for (operation, reply) in replies {
            let case = format!("{operation} carrying {query:?}");
            assert_eq!(
                reply.status, 400,
                "case {case:?}: an expression the service cannot run must be refused 400, \
                 got {reply:?}"
            );
            assert_eq!(
                reply.error_code(),
                "ValidationException",
                "case {case:?}: expected the modelled ValidationException, got {reply:?}"
            );
            let message = text(&reply.body["message"]);
            assert_eq!(
                message, UNSUPPORTED_EXPRESSION,
                "case {case:?}: expected the service's sentence for this class, \
                 got {message:?}"
            );
            assert_eq!(
                reply.body["fieldList"],
                Value::Null,
                "case {case:?}: no member failed a constraint, so there is no fieldList, \
                 got {reply:?}"
            );
            assert!(
                !message.contains(query),
                "case {case:?}: the sentence must not echo the expression, got {message:?}"
            );
            for operator in RANDOM_CUT_FOREST_AGGREGATION_OPERATORS {
                assert!(
                    !message.contains(operator),
                    "case {case:?}: the sentence must not list the operators, it names \
                     {operator:?}: {message:?}"
                );
            }
        }
    }

    // The controls: the same two operations carrying an expression a supported operator
    // DOES wrap are accepted, so the rows above are about the expression and nothing else.
    let accepted_create = create_detector(
        &amp,
        region,
        &workspace_id,
        &Spec::new("max by (job) (up)").create_body("accepted", &[]),
    )
    .await;
    assert_eq!(
        accepted_create.status, 202,
        "case \"CreateAnomalyDetector carrying a wrapped expression\": must be accepted, \
         got {accepted_create:?}"
    );
    let accepted_put = put_detector(
        &amp,
        region,
        &workspace_id,
        &detector_id,
        &Spec::new("quantile(0.99, up)").put_body(),
    )
    .await;
    assert_eq!(
        accepted_put.status, 202,
        "case \"PutAnomalyDetector carrying a wrapped expression\": must be accepted, \
         got {accepted_put:?}"
    );
}

/// One operation that can answer a family's not-found sentence: what to call and what to
/// send, as the model binds it under `/workspaces/{workspaceId}`.
struct Lookup {
    operation: &'static str,
    method: &'static str,
    suffix: &'static str,
    body: String,
}

/// One resource family: the sentence the service answers when a workspace does not hold
/// one, and every operation of that family which can answer it.
struct Family {
    message: &'static str,
    lookups: Vec<Lookup>,
}

/// A child resource a workspace does not hold is refused with the sentence of ITS FAMILY,
/// the same one whether the workspace never held it or held it and had it deleted, and
/// whichever of the family's operations looked it up.
#[tokio::test]
async fn a_missing_child_resource_names_its_family_in_one_sentence() {
    let region = "us-east-2";
    let amp = Amp::new();

    // The rules-and-alerting logging configuration, the QUERY logging configuration and
    // the resource-based policy: three families under one workspace, each with its own
    // sentence and its own set of operations that look one up.
    let families = vec![
        Family {
            message: LOGGING_NOT_FOUND,
            lookups: vec![
                Lookup {
                    operation: "DescribeLoggingConfiguration",
                    method: "GET",
                    suffix: "/logging",
                    body: String::new(),
                },
                Lookup {
                    operation: "UpdateLoggingConfiguration",
                    method: "PUT",
                    suffix: "/logging",
                    body: json!({ "logGroupArn": log_group_arn(region) }).to_string(),
                },
                Lookup {
                    operation: "DeleteLoggingConfiguration",
                    method: "DELETE",
                    suffix: "/logging",
                    body: String::new(),
                },
            ],
        },
        Family {
            message: LOGGING_NOT_FOUND,
            lookups: vec![
                Lookup {
                    operation: "DescribeQueryLoggingConfiguration",
                    method: "GET",
                    suffix: "/logging/query",
                    body: String::new(),
                },
                Lookup {
                    operation: "UpdateQueryLoggingConfiguration",
                    method: "PUT",
                    suffix: "/logging/query",
                    body: json!({ "destinations": destinations_json(region) }).to_string(),
                },
                Lookup {
                    operation: "DeleteQueryLoggingConfiguration",
                    method: "DELETE",
                    suffix: "/logging/query",
                    body: String::new(),
                },
            ],
        },
        Family {
            message: POLICY_NOT_FOUND,
            lookups: vec![
                Lookup {
                    operation: "DescribeResourcePolicy",
                    method: "GET",
                    suffix: "/policy",
                    body: String::new(),
                },
                Lookup {
                    operation: "DeleteResourcePolicy",
                    method: "DELETE",
                    suffix: "/policy",
                    body: String::new(),
                },
            ],
        },
    ];

    // Two histories, which must answer alike: a workspace that never held any of the three
    // children, and one that held all three and had each deleted.
    let never_held = amp.active_workspace(region, "never-held").await;

    let emptied = amp.active_workspace(region, "emptied").await;
    for (method, suffix, body) in [
        (
            "POST",
            "/logging",
            json!({ "logGroupArn": log_group_arn(region) }).to_string(),
        ),
        (
            "POST",
            "/logging/query",
            json!({ "destinations": destinations_json(region) }).to_string(),
        ),
        (
            "PUT",
            "/policy",
            json!({ "policyDocument": policy_document("Doomed") }).to_string(),
        ),
    ] {
        let created = amp
            .call(method, &format!("/workspaces/{emptied}{suffix}"), region, &body)
            .await;
        assert!(
            created.status < 300,
            "seeding {suffix}: expected acceptance, got {created:?}"
        );
    }
    wait_for_due_transition();
    for suffix in ["/logging", "/logging/query", "/policy"] {
        let deleted = amp
            .call("DELETE", &format!("/workspaces/{emptied}{suffix}"), region, "")
            .await;
        assert!(
            deleted.status < 300,
            "deleting {suffix}: expected acceptance, got {deleted:?}"
        );
    }
    // The clock owes each removal; once it is due the workspace holds none of them.
    wait_for_due_transition();

    for (history, workspace_id) in [("never held", &never_held), ("deleted", &emptied)] {
        for family in &families {
            for lookup in &family.lookups {
                let case = format!(
                    "{} on a workspace whose child was {history}",
                    lookup.operation
                );
                let reply = amp
                    .call(
                        lookup.method,
                        &format!("/workspaces/{workspace_id}{}", lookup.suffix),
                        region,
                        &lookup.body,
                    )
                    .await;
                assert_not_found(&case, &reply, family.message, workspace_id);
            }
        }
    }
}

/// A workspace this account does not hold is refused with the sentence THAT OPERATION
/// answers: `DescribeWorkspace` is recorded answering one spelling and `DeleteWorkspace`
/// another, so the service does not have one workspace not-found sentence but one per
/// operation — and each is constant across the ids it cannot resolve.
#[tokio::test]
async fn a_workspace_that_is_gone_answers_each_operations_own_sentence() {
    let region = "ap-south-1";
    let amp = Amp::new();

    // A workspace that was held and has finished deleting, beside two ids no store ever
    // minted — in different forms, so the rule is not read off one id shape.
    let deleted = amp.active_workspace(region, "doomed").await;
    let accepted = amp.delete(region, &deleted, None).await;
    assert_eq!(accepted.status, 202, "the deletion must be accepted: {accepted:?}");
    wait_for_due_transition();

    let absent = [
        ("a workspace that finished deleting", deleted.clone()),
        (
            "an id no store ever minted",
            "ws-00000000-0000-4000-8000-000000000000".to_string(),
        ),
        ("an id carrying no ws- prefix", "absent".to_string()),
    ];

    for (case, workspace_id) in &absent {
        let described = amp.describe(region, workspace_id).await;
        assert_not_found(
            &format!("DescribeWorkspace on {case}"),
            &described,
            DESCRIBE_WORKSPACE_NOT_FOUND,
            workspace_id,
        );
        let deleted_again = amp.delete(region, workspace_id, None).await;
        assert_not_found(
            &format!("DeleteWorkspace on {case}"),
            &deleted_again,
            WORKSPACE_NOT_FOUND,
            workspace_id,
        );
        let renamed = amp
            .rename(region, workspace_id, &json!({ "alias": "renamed" }))
            .await;
        assert_not_found(
            &format!("UpdateWorkspaceAlias on {case}"),
            &renamed,
            WORKSPACE_NOT_FOUND,
            workspace_id,
        );
    }

    // The two spellings differ in exactly the trailing period the recording shows, and the
    // read path is the one without it.
    assert_eq!(
        DESCRIBE_WORKSPACE_NOT_FOUND,
        WORKSPACE_NOT_FOUND.trim_end_matches('.'),
        "the two recorded spellings must differ only in the trailing period"
    );
}

/// The optimistic lock on a workspace's resource policy: a `revisionId` that is not the
/// revision the policy is at is refused with the service's sentence for a failed
/// condition, which names the ACTION the refused call would have made and nothing of the
/// request — whichever form the stale value takes.
#[tokio::test]
async fn a_failed_revision_precondition_names_the_action_it_refused() {
    let region = "us-west-2";

    // One row per form the stale value can take: a revision this store never minted,
    // `NO_POLICY` (which asserts the workspace holds none, and it holds one), and a value
    // that is no revision id at all.
    let stale = [
        ("a revision this store never minted", "00000000-0000-4000-8000-000000000000"),
        ("NO_POLICY while the workspace holds a policy", NO_POLICY_REVISION_ID),
        ("a value that is no revision id", "not-a-revision"),
    ];

    for (case, revision) in stale {
        let amp = Amp::new();
        let workspace_id = amp.active_workspace(region, "revision-lock").await;
        let put = amp
            .call(
                "PUT",
                &format!("/workspaces/{workspace_id}/policy"),
                region,
                &json!({ "policyDocument": policy_document("First") }).to_string(),
            )
            .await;
        assert_eq!(put.status, 202, "case {case:?}: seeding a policy: {put:?}");
        let current = text(&put.body["revisionId"]);
        wait_for_due_transition();

        let refusals = [
            (
                POLICY_UPDATE_ACTION,
                amp.call(
                    "PUT",
                    &format!("/workspaces/{workspace_id}/policy"),
                    region,
                    &json!({
                        "policyDocument": policy_document("Stale"),
                        "revisionId": revision,
                    })
                    .to_string(),
                )
                .await,
            ),
            (
                POLICY_DELETE_ACTION,
                amp.call(
                    "DELETE",
                    &format!("/workspaces/{workspace_id}/policy?revisionId={revision}"),
                    region,
                    "",
                )
                .await,
            ),
        ];

        for (action, reply) in refusals {
            let label = format!("a conditional {action} naming {case}");
            // The recorded sentence is the conditional UPDATE's; the conditional delete's
            // is the same sentence naming the action that call would have made.
            let expected = if action == POLICY_UPDATE_ACTION {
                STALE_REVISION_ON_UPDATE.to_string()
            } else {
                stale_revision_message(action)
            };
            assert_eq!(
                reply.status, 400,
                "case {label:?}: a failed precondition must be refused 400, got {reply:?}"
            );
            assert_eq!(
                reply.error_code(),
                "ValidationException",
                "case {label:?}: expected the modelled ValidationException, got {reply:?}"
            );
            let message = text(&reply.body["message"]);
            assert_eq!(
                message, expected,
                "case {label:?}: expected the service's optimistic-lock sentence, \
                 got {message:?}"
            );
            assert!(
                !message.contains(revision) && !message.contains(&current),
                "case {label:?}: neither revision belongs in the sentence, got {message:?}"
            );
        }

        // The control: the same calls naming the CURRENT revision are accepted, so the
        // refusals above are about the value and not about the lock itself.
        let accepted = amp
            .call(
                "DELETE",
                &format!("/workspaces/{workspace_id}/policy?revisionId={current}"),
                region,
                "",
            )
            .await;
        assert_eq!(
            accepted.status, 202,
            "case {case:?}: the current revision must be accepted, got {accepted:?}"
        );
    }
}

/// A call the resource's CURRENT STATUS forbids is refused with the service's template for
/// that class — the action, the resource and the status the store holds for it — over
/// every resource of this service whose own status can forbid a call.
#[tokio::test]
async fn a_status_that_forbids_a_call_states_the_services_template() {
    let region = "eu-north-1";

    // (case, action, resource, the status the resource is in, and — for the row the
    // recording covers — the sentence it shows verbatim)
    struct Case {
        name: &'static str,
        action: &'static str,
        resource: &'static str,
        status: &'static str,
        recorded: Option<&'static str>,
    }

    let cases = [
        Case {
            name: "DeleteWorkspace while the workspace is still being created",
            action: "delete",
            resource: "workspace",
            status: STATUS_CREATING,
            recorded: None,
        },
        Case {
            name: "UpdateWorkspaceAlias while the workspace is still being created",
            action: "update",
            resource: "workspace",
            status: STATUS_CREATING,
            recorded: None,
        },
        Case {
            name: "DeleteWorkspace while the workspace is already being deleted",
            action: "delete",
            resource: "workspace",
            status: STATUS_DELETING,
            recorded: Some(DELETE_WORKSPACE_WHILE_DELETING),
        },
        Case {
            name: "UpdateWorkspaceAlias while the workspace is being deleted",
            action: "update",
            resource: "workspace",
            status: STATUS_DELETING,
            recorded: None,
        },
        Case {
            name: "DeleteAnomalyDetector while the detector is still being created",
            action: "delete",
            resource: "anomaly detector",
            status: STATUS_CREATING,
            recorded: None,
        },
        Case {
            name: "PutAnomalyDetector while the detector is still being created",
            action: "update",
            resource: "anomaly detector",
            status: STATUS_CREATING,
            recorded: None,
        },
        Case {
            name: "DeleteRuleGroupsNamespace while the namespace is still being created",
            action: "delete",
            resource: "rule groups namespace",
            status: NAMESPACE_STATUS_CREATING,
            recorded: None,
        },
        Case {
            name: "PutRuleGroupsNamespace while the namespace is still being created",
            action: "update",
            resource: "rule groups namespace",
            status: NAMESPACE_STATUS_CREATING,
            recorded: None,
        },
        Case {
            name: "UpdateQueryLoggingConfiguration while the configuration is being deleted",
            action: "update",
            resource: "query logging configuration",
            status: QUERY_LOGGING_STATUS_DELETING,
            recorded: None,
        },
    ];

    for case in &cases {
        let amp = Amp::new();
        let reply = refused_by_status(&amp, region, case.name).await;
        // The row the recording covers is asserted against its text verbatim; the rest
        // against the same template filled in for their own action, resource and status.
        let expected = case.recorded.map_or_else(
            || status_forbids_call_message(case.action, case.resource, case.status),
            ToString::to_string,
        );
        assert_eq!(
            reply.status, 409,
            "case {:?}: a status that forbids the call is the modelled 409, got {reply:?}",
            case.name
        );
        assert_eq!(
            reply.error_code(),
            "ConflictException",
            "case {:?}: expected the modelled ConflictException, got {reply:?}",
            case.name
        );
        let message = text(&reply.body["message"]);
        assert_eq!(
            message, expected,
            "case {:?}: expected the service's status-conflict sentence, got {message:?}",
            case.name
        );
    }
}

/// Drive one row of the status-conflict table: put the resource in the status that forbids
/// the call, then make the call. Each row runs on its own `Amp`, so nothing it leaves
/// behind reaches another.
async fn refused_by_status(amp: &Amp, region: &str, case: &str) -> Reply {
    match case {
        "DeleteWorkspace while the workspace is still being created" => {
            let created = amp
                .call("POST", "/workspaces", region, &json!({ "alias": "creating" }).to_string())
                .await;
            let workspace_id = text(&created.body["workspaceId"]);
            amp.delete(region, &workspace_id, None).await
        }
        "UpdateWorkspaceAlias while the workspace is still being created" => {
            let created = amp
                .call("POST", "/workspaces", region, &json!({ "alias": "creating" }).to_string())
                .await;
            let workspace_id = text(&created.body["workspaceId"]);
            amp.rename(region, &workspace_id, &json!({ "alias": "too-early" }))
                .await
        }
        "DeleteWorkspace while the workspace is already being deleted" => {
            let workspace_id = amp.active_workspace(region, "deleting").await;
            let accepted = amp.delete(region, &workspace_id, None).await;
            assert_eq!(accepted.status, 202, "the first deletion: {accepted:?}");
            amp.delete(region, &workspace_id, None).await
        }
        "UpdateWorkspaceAlias while the workspace is being deleted" => {
            let workspace_id = amp.active_workspace(region, "deleting").await;
            let accepted = amp.delete(region, &workspace_id, None).await;
            assert_eq!(accepted.status, 202, "the deletion: {accepted:?}");
            amp.rename(region, &workspace_id, &json!({ "alias": "too-late" }))
                .await
        }
        "DeleteAnomalyDetector while the detector is still being created" => {
            let (workspace_id, detector_id) = creating_detector(amp, region).await;
            amp.call(
                "DELETE",
                &format!("/workspaces/{workspace_id}/anomalydetectors/{detector_id}"),
                region,
                "",
            )
            .await
        }
        "PutAnomalyDetector while the detector is still being created" => {
            let (workspace_id, detector_id) = creating_detector(amp, region).await;
            put_detector(
                amp,
                region,
                &workspace_id,
                &detector_id,
                &Spec::new("sum(rate(requests_total[5m]))").put_body(),
            )
            .await
        }
        "DeleteRuleGroupsNamespace while the namespace is still being created" => {
            let (workspace_id, name) = creating_namespace(amp, region).await;
            amp.call(
                "DELETE",
                &format!("/workspaces/{workspace_id}/rulegroupsnamespaces/{name}"),
                region,
                "",
            )
            .await
        }
        "PutRuleGroupsNamespace while the namespace is still being created" => {
            let (workspace_id, name) = creating_namespace(amp, region).await;
            put_namespace(amp, region, &workspace_id, &name, &put_body(b"groups: []")).await
        }
        "UpdateQueryLoggingConfiguration while the configuration is being deleted" => {
            let workspace_id = amp.active_workspace(region, "query-logging").await;
            let created = amp
                .call(
                    "POST",
                    &format!("/workspaces/{workspace_id}/logging/query"),
                    region,
                    &json!({ "destinations": destinations_json(region) }).to_string(),
                )
                .await;
            assert_eq!(created.status, 202, "seeding a configuration: {created:?}");
            wait_for_due_transition();
            let deleted = amp
                .call(
                    "DELETE",
                    &format!("/workspaces/{workspace_id}/logging/query"),
                    region,
                    "",
                )
                .await;
            assert_eq!(deleted.status, 202, "the deletion: {deleted:?}");
            amp.call(
                "PUT",
                &format!("/workspaces/{workspace_id}/logging/query"),
                region,
                &json!({ "destinations": destinations_json(region) }).to_string(),
            )
            .await
        }
        other => panic!("no row drives {other:?}"),
    }
}

/// An active workspace holding a detector that is still `CREATING`, with the detector's
/// status read back through `DescribeAnomalyDetector` so the row is about the status the
/// store really holds.
async fn creating_detector(amp: &Amp, region: &str) -> (String, String) {
    let workspace_id = amp.active_workspace(region, "detector-status").await;
    let created = create_detector(
        amp,
        region,
        &workspace_id,
        &Spec::new("sum(up)").create_body("creating", &[]),
    )
    .await;
    assert_eq!(created.status, 202, "seeding a detector: {created:?}");
    let detector_id = text(&created.body["anomalyDetectorId"]);
    let described = describe_detector(amp, region, &workspace_id, &detector_id).await;
    assert_eq!(
        text(&described.body["anomalyDetector"]["status"]["statusCode"]),
        STATUS_CREATING,
        "the detector must still be creating, got {described:?}"
    );
    (workspace_id, detector_id)
}

/// An active workspace holding a rule groups namespace that is still `CREATING`.
async fn creating_namespace(amp: &Amp, region: &str) -> (String, String) {
    let workspace_id = amp.active_workspace(region, "namespace-status").await;
    let name = "refusals";
    let created = create_namespace(
        amp,
        region,
        &workspace_id,
        &create_body(name, b"groups: []", &[]),
    )
    .await;
    assert_eq!(created.status, 202, "seeding a namespace: {created:?}");
    assert_eq!(
        text(&created.body["status"]["statusCode"]),
        NAMESPACE_STATUS_CREATING,
        "the namespace must still be creating, got {created:?}"
    );
    (workspace_id, name.to_string())
}
