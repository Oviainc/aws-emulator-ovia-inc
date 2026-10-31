// SPDX-License-Identifier: CC0-1.0

//! What a collection member a request carried EMPTY means, over the operations of this
//! service whose input takes one, end to end.
//!
//! Every call here goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` by the harness [`crate::virtual_cluster_job_run_loop`] owns, and
//! driven with restJson1 HTTP requests, so routing, deserialization, constraint
//! validation, the handler and serialization all run exactly as they do for an AWS SDK
//! client.
//!
//! The rule under test: a collection member carrying no entry supplies no value, names no
//! filter and asks for nothing, so a request that sent one EMPTY is the same request as
//! one that omitted the member and is answered identically — neither is refused for the
//! member's mere presence, both persist the same resource, and under ONE `clientToken` the
//! second spelling replays the resource the first minted. A member that carries an ENTRY
//! is a different request, and one the operation has nothing to substitute into is still
//! refused.
//!
//! Asserted over a TABLE rather than one request: the rows differ in the operation, in the
//! member's POSITION (a map at the top level of the request, a map and a list nested inside
//! a structure member), in the collection's KIND, and in whether the submission names a
//! job template; the boundary walks the supplied-parameter COUNT up from zero.

use serde_json::{json, Value};

use crate::job_template_idempotency::set_at;
use crate::spent_token_with_a_different_request::{assert_equivalent_replays, Operation};
use crate::virtual_cluster_job_run_loop::{field, runnable_body, spark_submit_driver, text, Emr};

/// The operation one row drives. Which paths each of them creates at, lists at and reads
/// one resource back from is [`paths_of`].
#[derive(Clone, Copy, Debug)]
enum Surface {
    StartJobRun,
    CreateVirtualCluster,
    CreateJobTemplate,
}

/// One row: a request written both ways.
struct Row {
    label: &'static str,
    surface: Surface,
    /// Whether this row's request names the stored job template, so a template is put in
    /// the store only for the rows that name one and a row's own listing holds nothing but
    /// what its two calls persisted.
    names_template: bool,
    /// Where the describe reports the member, when the RESOURCE shape carries it too;
    /// `None` for a member that only exists in the request, or one the record keeps as it
    /// was submitted, where there is nothing for two spellings to agree on.
    reported: Option<&'static str>,
    /// The request with the member omitted, and the same request carrying it EMPTY. Both
    /// are given the `clientToken` to sign the call with and the id of the job template
    /// [`stored_job_template`] put in the store, which a row naming no template ignores.
    omitted: fn(&str, &str) -> Value,
    empty: fn(&str, &str) -> Value,
}

/// The `jobTemplateData` a template needs: the three members `JobTemplateData` marks
/// `@required` (smithy_query: shape `JobTemplateData`). None of its values references a
/// parameter, so a submission naming the template has to supply none.
fn template_data() -> Value {
    json!({
        "executionRoleArn": "arn:aws:iam::000000000000:role/EMRJobRole",
        "releaseLabel": "emr-6.10.0-latest",
        "jobDriver": spark_submit_driver(),
    })
}

/// Store one job template and return its id, so the rows that name one name a template
/// that is really there.
async fn stored_job_template(emr: &Emr) -> String {
    let case = "the job template the rows name";
    let created = emr
        .send(
            "POST",
            "/jobtemplates",
            Some(json!({
                "name": "empty-collection-fixture",
                "clientToken": "tok-empty-collection-template",
                "jobTemplateData": template_data(),
            })),
        )
        .await;
    text(created.ok(case), "/id", case)
}

/// A submission that names no job template, so it carries the three members a run cannot
/// exist without itself.
fn submission(token: &str, _template: &str) -> Value {
    runnable_body(token, spark_submit_driver())
}

/// The same submission, carrying `jobTemplateParameters` with no entry in it: the case the
/// rule is about, where the operation has no template to substitute into either.
fn submission_with_empty_parameters(token: &str, template: &str) -> Value {
    let mut body = submission(token, template);
    set_at(
        &mut body,
        "/jobTemplateParameters",
        json!({}),
        "an empty jobTemplateParameters",
    );
    body
}

/// The same submission, carrying `tags` with no entry in it.
fn submission_with_empty_tags(token: &str, template: &str) -> Value {
    let mut body = submission(token, template);
    set_at(&mut body, "/tags", json!({}), "an empty tag map");
    body
}

/// A submission whose `configurationOverrides` structure is present and overrides nothing
/// — the position a nested collection member sits in, with the structure itself carried by
/// both spellings (an empty STRUCTURE is a member the request carried, so it is not a
/// spelling of an omitted one).
fn submission_with_overrides(token: &str, template: &str) -> Value {
    let mut body = submission(token, template);
    set_at(
        &mut body,
        "/configurationOverrides",
        json!({}),
        "configurationOverrides overriding nothing",
    );
    body
}

/// The same, with the nested `applicationConfiguration` LIST present and empty.
fn submission_with_empty_application_configuration(token: &str, template: &str) -> Value {
    let mut body = submission_with_overrides(token, template);
    set_at(
        &mut body,
        "/configurationOverrides/applicationConfiguration",
        json!([]),
        "an empty applicationConfiguration",
    );
    body
}

/// A submission built from the stored job template, which supplies the run's values.
fn templated_submission(token: &str, template: &str) -> Value {
    json!({"clientToken": token, "jobTemplateId": template})
}

/// The same, carrying `jobTemplateParameters` with no entry: the template references no
/// parameter, so there is nothing for the member to supply either way.
fn templated_submission_with_empty_parameters(token: &str, template: &str) -> Value {
    let mut body = templated_submission(token, template);
    set_at(
        &mut body,
        "/jobTemplateParameters",
        json!({}),
        "an empty jobTemplateParameters beside a jobTemplateId",
    );
    body
}

/// A `CreateVirtualCluster` request. It carries no `eksInfo`, so it claims no Kubernetes
/// namespace and nothing here can be refused by the one-cluster-per-namespace rule
/// instead of by the rule under test.
fn cluster_request(token: &str, _template: &str) -> Value {
    json!({
        "name": "empty-collection-cluster",
        "clientToken": token,
        "containerProvider": {"id": "eks-fixture", "type": "EKS"},
    })
}

/// The same request, carrying `tags` with no entry in it.
fn cluster_request_with_empty_tags(token: &str, template: &str) -> Value {
    let mut body = cluster_request(token, template);
    set_at(&mut body, "/tags", json!({}), "an empty tag map");
    body
}

/// A `CreateJobTemplate` request.
fn template_request(token: &str, _template: &str) -> Value {
    json!({
        "name": "empty-collection-template",
        "clientToken": token,
        "jobTemplateData": template_data(),
    })
}

/// The same request, whose `jobTemplateData` carries a `parameterConfiguration` declaring
/// no parameter — a map nested one structure down.
fn template_request_with_empty_parameter_configuration(token: &str, template: &str) -> Value {
    let mut body = template_request(token, template);
    set_at(
        &mut body,
        "/jobTemplateData/parameterConfiguration",
        json!({}),
        "an empty parameterConfiguration",
    );
    body
}

/// Every row of the rule.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "StartJobRun: jobTemplateParameters sent empty, naming no job template",
            surface: Surface::StartJobRun,
            names_template: false,
            reported: None,
            omitted: submission,
            empty: submission_with_empty_parameters,
        },
        Row {
            label: "StartJobRun: jobTemplateParameters sent empty beside a jobTemplateId",
            surface: Surface::StartJobRun,
            names_template: true,
            reported: None,
            omitted: templated_submission,
            empty: templated_submission_with_empty_parameters,
        },
        Row {
            label: "StartJobRun: tags sent empty",
            surface: Surface::StartJobRun,
            names_template: false,
            reported: Some("/tags"),
            omitted: submission,
            empty: submission_with_empty_tags,
        },
        Row {
            label: "StartJobRun: a list sent empty inside configurationOverrides",
            surface: Surface::StartJobRun,
            names_template: false,
            reported: None,
            omitted: submission_with_overrides,
            empty: submission_with_empty_application_configuration,
        },
        Row {
            label: "CreateVirtualCluster: tags sent empty",
            surface: Surface::CreateVirtualCluster,
            names_template: false,
            reported: Some("/tags"),
            omitted: cluster_request,
            empty: cluster_request_with_empty_tags,
        },
        Row {
            label: "CreateJobTemplate: a map sent empty inside jobTemplateData",
            surface: Surface::CreateJobTemplate,
            names_template: false,
            reported: None,
            omitted: template_request,
            empty: template_request_with_empty_parameter_configuration,
        },
    ]
}

/// Where this surface creates and lists, and the pointer its describe answers one
/// resource under. `cluster` is the virtual cluster the job-run paths hang off.
fn paths_of(surface: Surface, cluster: &str) -> (Operation, &'static str) {
    match surface {
        Surface::StartJobRun => (
            Operation {
                create_path: format!("/virtualclusters/{cluster}/jobruns"),
                list_path: format!("/virtualclusters/{cluster}/jobruns"),
                list_pointer: "/jobRuns",
            },
            "/jobRun",
        ),
        Surface::CreateVirtualCluster => (
            Operation {
                create_path: "/virtualclusters".to_string(),
                list_path: "/virtualclusters".to_string(),
                list_pointer: "/virtualClusters",
            },
            "/virtualCluster",
        ),
        Surface::CreateJobTemplate => (
            Operation {
                create_path: "/jobtemplates".to_string(),
                list_path: "/jobtemplates".to_string(),
                list_pointer: "/templates",
            },
            "/jobTemplate",
        ),
    }
}

/// What one row needs in the store before it runs, and nothing more: a virtual cluster
/// for the rows that submit a job run into one, and a job template for the rows that name
/// one. A row's listing then holds only what its own two calls persisted.
async fn preconditions(emr: &Emr, row: &Row) -> (String, String) {
    let cluster = match row.surface {
        Surface::StartJobRun => {
            emr.cluster("empty-collection", "tok-empty-collection-vc")
                .await
        }
        _ => String::new(),
    };
    let template = if row.names_template {
        stored_job_template(emr).await
    } else {
        String::new()
    };
    (cluster, template)
}

/// The resource `id` reads back as, out of the describe of it.
async fn described(emr: &Emr, operation: &Operation, pointer: &str, id: &str, case: &str) -> Value {
    let answer = emr
        .send("GET", &format!("{}/{id}", operation.create_path), None)
        .await;
    let resource = field(answer.ok(case), pointer, case).clone();
    assert_eq!(
        text(&resource, "/id", case),
        id,
        "case {case:?}: the describe answered a different resource: {resource}"
    );
    resource
}

/// Both spellings of one request are accepted, and what each persisted reads back.
#[tokio::test]
async fn an_empty_collection_member_is_accepted_as_an_omitted_one() {
    for row in rows() {
        let emr = Emr::new();
        let (cluster, template) = preconditions(&emr, &row).await;
        let (operation, pointer) = paths_of(row.surface, &cluster);

        let omitted_case = format!("{}, with the member omitted", row.label);
        let omitted = emr
            .send(
                "POST",
                &operation.create_path,
                Some((row.omitted)("tok-omitted", &template)),
            )
            .await;
        let empty_case = format!("{}, with the member carried empty", row.label);
        let empty = emr
            .send(
                "POST",
                &operation.create_path,
                Some((row.empty)("tok-empty", &template)),
            )
            .await;

        assert_eq!(
            (empty.status, empty.error_code.as_deref()),
            (omitted.status, omitted.error_code.as_deref()),
            "case {empty_case:?}: the two spellings of one request must be answered alike, \
             got {} {:?} for the empty one against {} {:?} for the omitted one, body {}",
            empty.status,
            empty.error_code,
            omitted.status,
            omitted.error_code,
            empty.body
        );
        let omitted_id = text(omitted.ok(&omitted_case), "/id", &omitted_case);
        let empty_id = text(empty.ok(&empty_case), "/id", &empty_case);
        assert_ne!(
            omitted_id, empty_id,
            "case {empty_case:?}: two distinct clientTokens must mint two resources"
        );

        // Each call persisted a resource, so each reads back through its own describe.
        let from_omitted = described(&emr, &operation, pointer, &omitted_id, &omitted_case).await;
        let from_empty = described(&emr, &operation, pointer, &empty_id, &empty_case).await;
        if let Some(reported) = row.reported {
            assert_eq!(
                from_empty.pointer(reported),
                from_omitted.pointer(reported),
                "case {empty_case:?}: the empty spelling must persist the same {reported} as the \
                 omitted one, got {from_empty} against {from_omitted}"
            );
        }
    }
}

/// The identity half of the rule: under ONE `clientToken` the second spelling is a REPLAY
/// of the first, in either order, and persists nothing new.
#[tokio::test]
async fn the_two_spellings_replay_under_one_client_token() {
    for row in rows() {
        for (order, first, second) in [
            ("the omitted member first", row.omitted, row.empty),
            ("the empty member first", row.empty, row.omitted),
        ] {
            let case = format!("{}, {order}", row.label);
            let emr = Emr::new();
            let (cluster, template) = preconditions(&emr, &row).await;
            let (operation, _) = paths_of(row.surface, &cluster);
            assert_equivalent_replays(
                &emr,
                &operation,
                &case,
                first("tok-one-request", &template),
                second("tok-one-request", &template),
            )
            .await;
        }
    }
}

/// The boundary of the rule, walked over the number of parameters supplied: a submission
/// that names no `jobTemplateId` has nothing to substitute a parameter into, so one
/// supplying ANY parameter is refused with the modelled `ValidationException` (HTTP 400)
/// and starts nothing, while the request supplying none — the member omitted or carried
/// empty — starts its run.
#[tokio::test]
async fn parameters_without_a_job_template_are_refused_from_the_first_one() {
    let emr = Emr::new();
    let cluster = emr.cluster("parameter-counts", "tok-parameter-counts").await;
    let operation = Operation {
        create_path: format!("/virtualclusters/{cluster}/jobruns"),
        list_path: format!("/virtualclusters/{cluster}/jobruns"),
        list_pointer: "/jobRuns",
    };

    for count in 0..=3 {
        let case = format!("{count} supplied parameters and no jobTemplateId");
        let mut body = runnable_body(&format!("tok-count-{count}"), spark_submit_driver());
        let parameters: Value = (0..count)
            .map(|index| (format!("name{index}"), Value::String(format!("value{index}"))))
            .collect();
        set_at(&mut body, "/jobTemplateParameters", parameters, &case);

        let answer = emr.send("POST", &operation.create_path, Some(body)).await;
        if count == 0 {
            // No parameter is supplied, so the submission is the one that omits the
            // member: it starts a run, which reads back out of state.
            let id = text(answer.ok(&case), "/id", &case);
            let run = described(&emr, &operation, "/jobRun", &id, &case).await;
            assert_eq!(
                text(&run, "/virtualClusterId", &case),
                cluster,
                "case {case:?}: the run must belong to the cluster it was submitted to, got {run}"
            );
        } else {
            answer.refused(&case, 400, "ValidationException");
            // The refusal is the one about parameters with no template, not some other
            // `ValidationException`: it names both members it is about.
            let message = text(&answer.body, "/message", &case);
            assert!(
                message.contains("jobTemplateParameters") && message.contains("jobTemplateId"),
                "case {case:?}: expected a refusal naming jobTemplateParameters and \
                 jobTemplateId, got {message:?}"
            );
        }
    }
}
