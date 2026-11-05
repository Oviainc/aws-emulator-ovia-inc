// SPDX-License-Identifier: CC0-1.0

//! The attachments a task holds, and the agent's report of their state
//! (`SubmitAttachmentStateChanges`), driven end to end over the shared wire harness in
//! [`crate::ecs_wire`].
//!
//! The rules under test:
//!
//! * A task whose network mode is `awsvpc` "is allocated an elastic network interface"
//!   (`smithy_query(query="doc", shape="RegisterTaskDefinitionRequest",
//!   member="networkMode")`), and that interface is the attachment Amazon ECS reports for
//!   the task; a task in any other mode shares its host's networking and holds none. The
//!   attachment is reported in the state the store holds it in — the state Amazon ECS
//!   created it in until either the CLOCK attaches it (see
//!   [`crate::task_interface_attach_and_connectivity`]) or the AGENT reports on it, and
//!   never a read.
//! * `SubmitAttachmentStateChanges` resolves the cluster its report names, and then every
//!   `attachmentArn` the report carries, against this store: a cluster that resolves to
//!   nothing is refused with `ClientException` and the fixed cluster sentence, an
//!   attachment id no stored attachment of that cluster carries is refused with
//!   `InvalidParameterException` naming THAT id, and a report that resolves whole is
//!   recorded and acknowledged.
//! * The id is the one the reference names whichever form it takes — a full attachment ARN
//!   or the bare id — because both name the same attachment.
//! * The report is all or nothing: a batch carrying one unknown attachment records none of
//!   its entries.
//!
//! Nothing here reaches into the state struct: every attachment asserted is read back
//! through `DescribeTasks`, so a response that is not computed from persisted state cannot
//! pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    awsvpc_network_configuration, cluster_with_capacity, list, ok, refusal, service, string,
    ACCOUNT, SIGNED_REGION,
};
use crate::task_lifecycle::container;

/// The cluster every row below acts in.
const CLUSTER: &str = "attachplane";

/// A second cluster, so an attachment of a task registered ELSEWHERE is one that resolves
/// to nothing in [`CLUSTER`].
const OTHER_CLUSTER: &str = "elsewhereattach";

/// The `awsvpc` family every launch that gets an interface runs.
const AWSVPC_FAMILY: &str = "attachvpcfamily";

/// The image the containers below start from; irrelevant to these rules, so one value.
const IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";

/// `Attachment$type` of the interface an `awsvpc` task is allocated, from the member's own
/// doc: "the type of the attachment, such as `ElasticNetworkInterface`, `Service Connect`,
/// and `AmazonElasticBlockStorage`".
const ELASTIC_NETWORK_INTERFACE: &str = "ElasticNetworkInterface";

/// The `Attachment$status` an attachment Amazon ECS has created and no agent has
/// acknowledged yet carries.
const PRECREATED: &str = "PRECREATED";

/// The sentence a report naming a cluster that resolves to nothing is refused with.
const CLUSTER_NOT_FOUND: &str = "Cluster not found.";

/// Register one revision of `family` in `network_mode`, with a single container.
async fn register_definition(
    router: &MultiServiceRouter,
    family: &str,
    network_mode: Option<&str>,
    case: &str,
) {
    let mut request = json!({
        "family": family,
        "containerDefinitions": [container("web", IMAGE)],
    });
    if let Some(mode) = network_mode {
        request["networkMode"] = json!(mode);
    }
    ok(router, "RegisterTaskDefinition", request, case).await;
}

/// Run ONE task of `family` in `cluster` and return the task it reports.
async fn run_task(
    router: &MultiServiceRouter,
    cluster: &str,
    family: &str,
    network_configuration: Option<Value>,
    case: &str,
) -> Value {
    let mut request = json!({ "cluster": cluster, "taskDefinition": family, "count": 1 });
    if let Some(configuration) = network_configuration {
        request["networkConfiguration"] = configuration;
    }
    let body = ok(router, "RunTask", request, case).await;
    let tasks = list(&body, "tasks", case);
    assert_eq!(tasks.len(), 1, "case {case:?}: expected one task, got {body}");
    tasks[0].clone()
}

/// Read ONE task back through `DescribeTasks` — the stored record, not the answer of the
/// call that changed it.
async fn describe_task(
    router: &MultiServiceRouter,
    cluster: &str,
    task_arn: &str,
    case: &str,
) -> Value {
    let body = ok(
        router,
        "DescribeTasks",
        json!({ "cluster": cluster, "tasks": [task_arn] }),
        case,
    )
    .await;
    list(&body, "tasks", case)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected one task back, got {body}"))
}

/// The attachments one described task holds.
fn attachments_of(task: &Value, case: &str) -> Vec<Value> {
    assert!(
        task["attachments"].is_array(),
        "case {case:?}: a task reports its attachments as a list, got {task}"
    );
    list(task, "attachments", case)
}

/// The one attachment a task holds, which every `awsvpc` row below reports.
fn only_attachment(task: &Value, case: &str) -> Value {
    let attachments = attachments_of(task, case);
    assert_eq!(
        attachments.len(),
        1,
        "case {case:?}: expected the one interface an awsvpc task is allocated, got {task}"
    );
    attachments[0].clone()
}

/// An attachment ARN of `id`: `arn:aws:ecs:<region>:<account>:attachment/<id>`.
fn attachment_arn(id: &str) -> String {
    format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:attachment/{id}")
}

/// A cluster with capacity holding one `awsvpc` revision, and the task it has run: the
/// task's ARN and the id of the interface it was allocated.
async fn fixture_with_an_attachment(case: &str) -> (MultiServiceRouter, String, String) {
    let router = service();
    cluster_with_capacity(&router, CLUSTER, case).await;
    register_definition(&router, AWSVPC_FAMILY, Some("awsvpc"), case).await;
    let task = run_task(
        &router,
        CLUSTER,
        AWSVPC_FAMILY,
        Some(awsvpc_network_configuration()),
        case,
    )
    .await;
    let task_arn = string(&task, "taskArn", case);
    let id = string(&only_attachment(&task, case), "id", case);
    (router, task_arn, id)
}

#[tokio::test]
async fn a_task_holds_the_interface_its_network_mode_allocates_and_nothing_else() {
    // The rows span the `NetworkMode` enum (`smithy_query(query="shape",
    // shape="NetworkMode")` -> awsvpc, bridge, host, none) plus the documented default a
    // registration that names no mode takes, so the rule is asserted over every mode the
    // model names rather than over the one the interface belongs to.
    let rows: [(&str, Option<&str>, usize); 5] = [
        ("awsvpc", Some("awsvpc"), 1),
        ("bridge", Some("bridge"), 0),
        ("host", Some("host"), 0),
        ("none", Some("none"), 0),
        ("the documented default mode", None, 0),
    ];

    for (case, network_mode, expected) in rows {
        // A fresh engine per row, so one row's tasks cannot leak into another.
        let router = service();
        cluster_with_capacity(&router, CLUSTER, case).await;
        let family = format!("attach{}", case.replace(' ', ""));
        register_definition(&router, &family, network_mode, case).await;
        // A launch carries a network configuration exactly when the mode is `awsvpc`.
        let configuration = (network_mode == Some("awsvpc")).then(awsvpc_network_configuration);
        let task = run_task(&router, CLUSTER, &family, configuration, case).await;
        let task_arn = string(&task, "taskArn", case);

        // The run's own answer and the later read are two projections of ONE stored task,
        // so both report the same attachments.
        let read = describe_task(&router, CLUSTER, &task_arn, case).await;
        for (source, view) in [("RunTask", &task), ("DescribeTasks", &read)] {
            let reported = attachments_of(view, case);
            assert_eq!(
                reported.len(),
                expected,
                "case {case:?}: {source} must report {expected} attachment(s), got {view}"
            );
            for attachment in &reported {
                assert_eq!(
                    attachment["type"].as_str(),
                    Some(ELASTIC_NETWORK_INTERFACE),
                    "case {case:?}: {source} reports the interface the mode allocates, got \
                     {attachment}"
                );
                assert_eq!(
                    attachment["status"].as_str(),
                    Some(PRECREATED),
                    "case {case:?}: {source} reports the state Amazon ECS created the \
                     attachment in, got {attachment}"
                );
                assert!(
                    !string(attachment, "id", case).is_empty(),
                    "case {case:?}: {source} reports an identifier for the attachment, got \
                     {attachment}"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_interface_reports_the_subnet_its_launch_configured() {
    // The detail the store KNOWS is the subnet the launch named, so the rows differ along
    // how many subnets the configuration carries: the reported `subnetId` is one of them
    // whichever row ran.
    let rows: [(&str, Vec<&str>); 3] = [
        ("one subnet", vec!["subnet-0123456789abcdef0"]),
        (
            "two subnets",
            vec!["subnet-00000000000000001", "subnet-00000000000000002"],
        ),
        (
            "four subnets",
            vec![
                "subnet-0000000000000000a",
                "subnet-0000000000000000b",
                "subnet-0000000000000000c",
                "subnet-0000000000000000d",
            ],
        ),
    ];

    for (case, subnets) in rows {
        let router = service();
        cluster_with_capacity(&router, CLUSTER, case).await;
        register_definition(&router, AWSVPC_FAMILY, Some("awsvpc"), case).await;
        let task = run_task(
            &router,
            CLUSTER,
            AWSVPC_FAMILY,
            Some(json!({ "awsvpcConfiguration": { "subnets": subnets } })),
            case,
        )
        .await;
        let task_arn = string(&task, "taskArn", case);
        let read = describe_task(&router, CLUSTER, &task_arn, case).await;
        let attachment = only_attachment(&read, case);

        let details = list(&attachment, "details", case);
        let subnet = details
            .iter()
            .find(|detail| detail["name"].as_str() == Some("subnetId"))
            .map(|detail| string(detail, "value", case))
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected the interface to report its subnet, got {attachment}")
            });
        assert!(
            subnets.contains(&subnet.as_str()),
            "case {case:?}: expected one of the configured subnets {subnets:?}, got \
             {subnet:?}"
        );
    }
}

#[tokio::test]
async fn the_agent_report_records_the_state_it_names_and_is_acknowledged() {
    // The rows span the statuses `Attachment$status` documents ("Valid values are
    // PRECREATED, CREATED, ATTACHING, ATTACHED, DETACHING, DETACHED, DELETED, and
    // FAILED") and the two forms the reference takes, so the recorded status is asserted
    // to be the one the report named and the resolution to be insensitive to the form.
    let statuses = [
        "CREATED",
        "ATTACHING",
        "ATTACHED",
        "DETACHING",
        "DETACHED",
        "DELETED",
        "FAILED",
    ];

    for status in statuses {
        for form in ["the full ARN", "the bare id"] {
            let case = &format!("{status} by {form}");
            let (router, task_arn, id) = fixture_with_an_attachment(case).await;
            let reference = if form == "the full ARN" {
                attachment_arn(&id)
            } else {
                id.clone()
            };

            let acknowledged = ok(
                &router,
                "SubmitAttachmentStateChanges",
                json!({
                    "cluster": CLUSTER,
                    "attachments": [{ "attachmentArn": reference, "status": status }],
                }),
                case,
            )
            .await;
            assert_eq!(
                acknowledged["acknowledgment"].as_str(),
                Some("ACK"),
                "case {case:?}: the agent is told the report was taken, got {acknowledged}"
            );

            // The report is read back off the task, not off the answer of the call that
            // made it.
            let attachment = only_attachment(
                &describe_task(&router, CLUSTER, &task_arn, case).await,
                case,
            );
            assert_eq!(
                attachment["status"].as_str(),
                Some(status),
                "case {case:?}: the task must hold the state the report named, got \
                 {attachment}"
            );
            assert_eq!(
                attachment["id"].as_str(),
                Some(id.as_str()),
                "case {case:?}: the report must leave the attachment it named, got \
                 {attachment}"
            );
        }
    }
}

#[tokio::test]
async fn a_report_whose_cluster_resolves_to_nothing_is_refused() {
    let case = "cluster references";
    let (router, _, id) = fixture_with_an_attachment(case).await;
    let reference = attachment_arn(&id);

    // The rows differ along the FORM of the cluster the report names and why it resolves
    // to nothing: a name no cluster carries, the ARN form of such a name, and the
    // omitted member, which names the default cluster this fixture never created.
    let rows: [(&str, Option<String>); 3] = [
        ("a name no cluster carries", Some("missingcluster".to_string())),
        (
            "the ARN of a cluster that does not exist",
            Some(format!(
                "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:cluster/missingcluster"
            )),
        ),
        ("no cluster named at all", None),
    ];

    for (row, cluster) in rows {
        let mut request = json!({
            "attachments": [{ "attachmentArn": reference, "status": "ATTACHED" }],
        });
        if let Some(cluster) = cluster {
            request["cluster"] = json!(cluster);
        }
        let (code, message) =
            refusal(&router, "SubmitAttachmentStateChanges", request, row).await;
        assert_eq!(
            code, "ClientException",
            "case {row:?}: expected a ClientException, got {code:?} with {message:?}"
        );
        assert_eq!(
            message, CLUSTER_NOT_FOUND,
            "case {row:?}: the refusal states the condition and names no cluster, got \
             {message:?}"
        );
    }
}

#[tokio::test]
async fn a_report_naming_an_attachment_the_cluster_does_not_hold_is_refused_with_its_id() {
    let case = "attachment references";
    let router = service();
    cluster_with_capacity(&router, CLUSTER, case).await;
    cluster_with_capacity(&router, OTHER_CLUSTER, case).await;
    register_definition(&router, AWSVPC_FAMILY, Some("awsvpc"), case).await;
    // One `awsvpc` task in each cluster, so a reference to the attachment of the task in
    // the OTHER cluster is a reference that resolves to nothing in this one.
    let here = run_task(
        &router,
        CLUSTER,
        AWSVPC_FAMILY,
        Some(awsvpc_network_configuration()),
        case,
    )
    .await;
    let here_arn = string(&here, "taskArn", case);
    let here_id = string(&only_attachment(&here, case), "id", case);
    let elsewhere = run_task(
        &router,
        OTHER_CLUSTER,
        AWSVPC_FAMILY,
        Some(awsvpc_network_configuration()),
        case,
    )
    .await;
    let elsewhere_id = string(&only_attachment(&elsewhere, case), "id", case);

    // The rows differ along the form of the reference and why it resolves to nothing: an
    // id nothing minted, given as an ARN and bare; an ARN whose id is a UUID of zeros (the
    // shape a caller writes by hand); and the id of an attachment that belongs to another
    // cluster's task. The expected id is the final ARN segment in every row, so the
    // sentence names what the request referred to and nothing else.
    let rows: [(&str, String, &str); 5] = [
        (
            "an ARN of an id nothing minted",
            attachment_arn("11111111-2222-3333-4444-555555555555"),
            "11111111-2222-3333-4444-555555555555",
        ),
        (
            "a bare id nothing minted",
            "99999999-8888-7777-6666-555555555555".to_string(),
            "99999999-8888-7777-6666-555555555555",
        ),
        (
            "an ARN of a UUID of zeros",
            attachment_arn("00000000-0000-0000-0000-000000000000"),
            "00000000-0000-0000-0000-000000000000",
        ),
        (
            "the attachment of another cluster's task, by ARN",
            attachment_arn(&elsewhere_id),
            elsewhere_id.as_str(),
        ),
        (
            "the attachment of another cluster's task, bare",
            elsewhere_id.clone(),
            elsewhere_id.as_str(),
        ),
    ];

    for (row, reference, expected_id) in rows {
        let (code, message) = refusal(
            &router,
            "SubmitAttachmentStateChanges",
            json!({
                "cluster": CLUSTER,
                "attachments": [{ "attachmentArn": reference, "status": "ATTACHED" }],
            }),
            row,
        )
        .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {row:?}: expected an InvalidParameterException, got {code:?} with \
             {message:?}"
        );
        assert_eq!(
            message,
            format!("Attachment with id {expected_id} does not exist"),
            "case {row:?}: the refusal must name the attachment the report referred to, \
             got {message:?}"
        );
    }

    // None of the refusals touched the attachment this cluster DOES hold.
    let attachment = only_attachment(
        &describe_task(&router, CLUSTER, &here_arn, case).await,
        case,
    );
    assert_eq!(
        (attachment["id"].as_str(), attachment["status"].as_str()),
        (Some(here_id.as_str()), Some(PRECREATED)),
        "case {case:?}: a refused report must change nothing, got {attachment}"
    );
}

#[tokio::test]
async fn a_batch_carrying_one_unknown_attachment_records_none_of_it() {
    // The rows differ along WHERE the unknown entry sits in the batch, so the rule is
    // asserted to be about the batch and not about the order it is walked in.
    let rows: [(&str, bool); 2] = [("the unknown entry first", true), ("the unknown entry last", false)];

    for (case, unknown_first) in rows {
        let (router, task_arn, id) = fixture_with_an_attachment(case).await;
        let unknown = attachment_arn("deadbeef-0000-0000-0000-000000000000");
        let known = json!({ "attachmentArn": attachment_arn(&id), "status": "ATTACHED" });
        let missing = json!({ "attachmentArn": unknown, "status": "ATTACHED" });
        let batch = if unknown_first {
            vec![missing, known]
        } else {
            vec![known, missing]
        };

        let (code, message) = refusal(
            &router,
            "SubmitAttachmentStateChanges",
            json!({ "cluster": CLUSTER, "attachments": batch }),
            case,
        )
        .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected an InvalidParameterException, got {code:?} with \
             {message:?}"
        );
        assert_eq!(
            message,
            "Attachment with id deadbeef-0000-0000-0000-000000000000 does not exist",
            "case {case:?}: the refusal must name the entry that resolved to nothing, got \
             {message:?}"
        );

        // The entry that DID resolve was not recorded: the report was refused whole.
        let attachment = only_attachment(
            &describe_task(&router, CLUSTER, &task_arn, case).await,
            case,
        );
        assert_eq!(
            attachment["status"].as_str(),
            Some(PRECREATED),
            "case {case:?}: a refused batch must record none of its entries, got \
             {attachment}"
        );
    }
}
