// SPDX-License-Identifier: CC0-1.0

//! WHICH WORDING a refusal of this service carries, end to end.
//!
//! Every call goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` by the harness [`crate::virtual_cluster_job_run_loop::Emr`] and
//! driven with restJson1 HTTP requests, so routing, deserialization, constraint
//! validation, the handler and serialization all run exactly as they do for an AWS SDK
//! client.
//!
//! The rule under test: the wording of a refusal belongs to the ERROR CLASS that carries
//! it, not to the resource or the operation. Two classes of this service are worded as
//! FIXED text — the same sentence every time, with nothing of the request interpolated
//! into it:
//!
//! 1. An idempotency conflict — a `clientToken` already spent in its scope, replayed with
//!    a request that differs — is refused with ONE sentence, whichever of the operations
//!    that take a token carries it, naming neither the token, nor the resource it already
//!    minted, nor the member that differs.
//! 2. A resource that is not there, reported by an operation that models no not-found
//!    error and so has to report it through `ValidationException`, is refused with a short
//!    sentence carrying no identifier at all — a DIFFERENT sentence from the one a read of
//!    the same resource answers with in its modelled `ResourceNotFoundException`
//!    ([`crate::not_found_message`] pins that class).
//!
//! The tables span what the rule generalises over: for rule 1, which operation refuses
//! (all six whose input declares a `clientToken`) and which member of the request differs
//! (a top-level string, a member nested two or three levels down, a list's ORDER, a
//! number, a boolean, a map entry, and a member the first request did not carry at all);
//! for rule 2, which resource is missing, which published form the requested id takes
//! (both length bounds of `ResourceIdString`, an all-digit value, a plausible minted one)
//! and whether it never existed or was deleted.
//!
//! It also pins the rule's BOUNDARY: a resource that EXISTS but cannot take the request is
//! refused for what it is, not with the fixed sentence for one that is absent.

use serde_json::{json, Value};

use crate::job_template_idempotency::set_at;
use crate::managed_endpoint_listing::{create_endpoint, endpoint_body, wait_for_the_clock};
use crate::virtual_cluster_job_run_loop::{
    runnable_body, spark_submit_driver, text, Answer, Emr,
};

/// The client error every refusal below is carried by (smithy_query: digest — the only
/// client error `DeleteJobTemplate` and `DeleteSecurityConfiguration` model, and the one
/// each create reports a spent token through).
const INVALID_REQUEST: &str = "ValidationException";

/// The error a read of a missing resource is carried by, where the operation models one.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// AWS's sentence for an idempotency conflict, written here as the service emits it: it is
/// fixed text, so the expectation is the sentence itself and not a format string.
///
/// `pub(crate)` because the two suites that drive their own tables of spent-token conflicts
/// ([`crate::spent_token_with_a_different_request`],
/// [`crate::virtual_cluster_spent_token_parameters`]) hold their refusals to this same
/// sentence, so the test binary carries ONE copy of it.
pub(crate) const SPENT_TOKEN_CONFLICT: &str =
    "At least one input parameter differs from a previous call using the same client token";

/// AWS's sentence for a `DeleteJobTemplate` whose id names no stored template.
const ABSENT_JOB_TEMPLATE: &str = "Template does not exist";

/// AWS's sentence for a `DeleteSecurityConfiguration` whose id names no stored
/// configuration. Note the lower-case spelling, which is NOT the describe path's
/// `Security Configuration` — another reason the two classes cannot share one wording.
const ABSENT_SECURITY_CONFIGURATION: &str = "Security configuration not found";

/// The read-path wording of this service, which rule 2's sentences must NOT be: the
/// resource type and the identifier the request named it by ([`crate::not_found_message`]
/// pins the whole of it).
fn read_path_wording(resource_type: &str, identifier: &str) -> String {
    format!("{resource_type} {identifier} doesn't exist.")
}

/// One row of rule 1: a create that is accepted, and the same `clientToken` carrying a
/// request that differs in one member.
struct Conflict {
    case: String,
    /// The `POST` path both calls are addressed to.
    path: String,
    /// The token both calls carry, which the refusal must not name.
    token: String,
    base: Value,
    conflicting: Value,
}

/// Build a row by pointing at the member that must differ, so a row says WHAT differs and
/// nothing else.
fn conflict(
    case: &str,
    path: String,
    token: &str,
    base: Value,
    pointer: &str,
    value: Value,
) -> Conflict {
    let mut conflicting = base.clone();
    set_at(&mut conflicting, pointer, value, case);
    assert_ne!(
        conflicting, base,
        "case {case:?}: the row left the request unchanged, so it tests nothing"
    );
    Conflict {
        case: case.to_string(),
        path,
        token: token.to_string(),
        base,
        conflicting,
    }
}

/// Drive one row of rule 1: the first call is accepted, the conflicting one is refused with
/// the fixed sentence — and with nothing of the request in it.
async fn assert_conflict_wording(emr: &Emr, row: &Conflict) {
    let case = row.case.as_str();
    let first: Answer = emr.send("POST", &row.path, Some(row.base.clone())).await;
    let minted = text(first.ok(case), "/id", case);

    let refusal = emr
        .send("POST", &row.path, Some(row.conflicting.clone()))
        .await;
    refusal.refused(case, 400, INVALID_REQUEST);
    let message = text(&refusal.body, "/message", case);
    assert_eq!(
        message, SPENT_TOKEN_CONFLICT,
        "case {case:?}: expected message {SPENT_TOKEN_CONFLICT:?}, got {message:?}"
    );
    // The other half of "fixed text": the sentence has no variable parts, so neither the
    // token that was spent nor the resource it already named may appear in it.
    assert!(
        !message.contains(&row.token) && !message.contains(&minted),
        "case {case:?}: expected a sentence naming neither token {:?} nor {minted:?}, got \
         {message:?}",
        row.token
    );
}

/// An acceptable `CreateVirtualCluster` request. Its `containerProvider` carries no
/// `eksInfo`, so it claims no Kubernetes namespace and cannot be refused by the
/// one-cluster-per-namespace rule instead of by the token rule under test.
fn cluster_base(token: &str) -> Value {
    json!({
        "name": "wording.cluster",
        "clientToken": token,
        "containerProvider": {"id": "eks-wording", "type": "EKS"},
        "sessionEnabled": true,
        "tags": {"team": "data"},
    })
}

/// An acceptable `CreateJobTemplate` request (smithy_query: shape `JobTemplateData`, whose
/// three `@required` members the data carries).
fn template_base(token: &str) -> Value {
    json!({
        "name": "wording.template",
        "clientToken": token,
        "jobTemplateData": {
            "executionRoleArn": "arn:aws:iam::000000000000:role/EMRJobRole",
            "releaseLabel": "emr-6.15.0-latest",
            "jobDriver": {"sparkSubmitJobDriver": {
                "entryPoint": "s3://bucket/a.py",
                "entryPointArguments": ["one", "two"],
            }},
        },
    })
}

/// An acceptable `CreateSecurityConfiguration` request (smithy_query: shapes
/// `SecurityConfigurationData`, `AuthorizationConfiguration`, `LakeFormationConfiguration`).
fn configuration_base(token: &str) -> Value {
    json!({
        "name": "wording.configuration",
        "clientToken": token,
        "securityConfigurationData": {
            "authorizationConfiguration": {
                "lakeFormationConfiguration": {
                    "authorizedSessionTagValue": "Emr Session Tag",
                    "secureNamespaceInfo": {
                        "clusterId": "eks-wording-secure",
                        "namespace": "emr-secure",
                    },
                },
            },
        },
    })
}

/// An acceptable `GetManagedEndpointSessionCredentials` request: the two members the model
/// marks `@required` beside the two `@httpLabel` identifiers (smithy_query: operation
/// `GetManagedEndpointSessionCredentials`).
fn session_base(token: &str) -> Value {
    json!({
        "executionRoleArn": "arn:aws:iam::000000000000:role/EMRStudioRole",
        "credentialType":
            service_com_amazonaws_emrcontainers_awschicagowebservice::state::SESSION_CREDENTIAL_TYPE_TOKEN,
        "clientToken": token,
    })
}

/// RULE 1, over every operation of this service whose input declares a `clientToken` and
/// over every kind of member a second request can differ in.
#[tokio::test]
async fn an_idempotency_conflict_is_refused_with_one_fixed_sentence() {
    let emr = Emr::new();
    let host = emr.cluster("wording-host", "tok-wording-host").await;
    // An endpoint a session can be issued for: it is accepted CREATING and reaches ACTIVE
    // on the clock.
    let endpoint = text(
        create_endpoint(
            &emr,
            &host,
            endpoint_body("wording-ep", "tok-wording-ep", "JUPYTER_SERVER"),
        )
        .await
        .ok("an endpoint to issue sessions for"),
        "/id",
        "an endpoint to issue sessions for",
    );
    wait_for_the_clock();

    let clusters = "/virtualclusters".to_string();
    let templates = "/jobtemplates".to_string();
    let configurations = "/securityconfigurations".to_string();
    let runs = format!("/virtualclusters/{host}/jobruns");
    let endpoints = format!("/virtualclusters/{host}/endpoints");
    let sessions = format!("/virtualclusters/{host}/endpoints/{endpoint}/credentials");

    let rows = vec![
        // CreateVirtualCluster: a top-level string, a boolean, a member nested two levels
        // down that the first request did not carry.
        conflict(
            "CreateVirtualCluster, a different name",
            clusters.clone(),
            "tok-vc-name",
            cluster_base("tok-vc-name"),
            "/name",
            json!("wording.cluster.other"),
        ),
        conflict(
            "CreateVirtualCluster, sessionEnabled turned off",
            clusters.clone(),
            "tok-vc-session",
            cluster_base("tok-vc-session"),
            "/sessionEnabled",
            json!(false),
        ),
        conflict(
            "CreateVirtualCluster, a scheduler limit the first request did not carry",
            clusters.clone(),
            "tok-vc-scheduler",
            cluster_base("tok-vc-scheduler"),
            "/schedulerConfiguration/maxConcurrentJobRuns",
            json!(3),
        ),
        // CreateJobTemplate: a member three levels down, and a list in the other ORDER.
        conflict(
            "CreateJobTemplate, a different driver entryPoint",
            templates.clone(),
            "tok-tmpl-entry",
            template_base("tok-tmpl-entry"),
            "/jobTemplateData/jobDriver/sparkSubmitJobDriver/entryPoint",
            json!("s3://bucket/b.py"),
        ),
        conflict(
            "CreateJobTemplate, the driver arguments in the other order",
            templates.clone(),
            "tok-tmpl-order",
            template_base("tok-tmpl-order"),
            "/jobTemplateData/jobDriver/sparkSubmitJobDriver/entryPointArguments",
            json!(["two", "one"]),
        ),
        // CreateSecurityConfiguration: a member four levels down, and a map entry.
        conflict(
            "CreateSecurityConfiguration, a different secure namespace",
            configurations.clone(),
            "tok-sc-namespace",
            configuration_base("tok-sc-namespace"),
            "/securityConfigurationData/authorizationConfiguration/lakeFormationConfiguration/secureNamespaceInfo/namespace",
            json!("emr-other"),
        ),
        conflict(
            "CreateSecurityConfiguration, a tag the first request did not carry",
            configurations.clone(),
            "tok-sc-tags",
            configuration_base("tok-sc-tags"),
            "/tags",
            json!({"team": "platform"}),
        ),
        // StartJobRun: the submitted work, and a number.
        conflict(
            "StartJobRun, a different driver entryPoint",
            runs.clone(),
            "tok-run-entry",
            runnable_body("tok-run-entry", spark_submit_driver()),
            "/jobDriver/sparkSubmitJobDriver/entryPoint",
            json!("s3://bucket/other.py"),
        ),
        conflict(
            "StartJobRun, a retry attempt count the first submission did not carry",
            runs.clone(),
            "tok-run-retry",
            runnable_body("tok-run-retry", spark_submit_driver()),
            "/retryPolicyConfiguration/maxAttempts",
            json!(5),
        ),
        // CreateManagedEndpoint: a top-level string, and a configuration override nested
        // three levels down that the first request did not carry.
        conflict(
            "CreateManagedEndpoint, a different releaseLabel",
            endpoints.clone(),
            "tok-ep-release",
            endpoint_body("wording-ep-a", "tok-ep-release", "JUPYTER_ENTERPRISE_GATEWAY"),
            "/releaseLabel",
            json!("emr-7.0.0-latest"),
        ),
        conflict(
            "CreateManagedEndpoint, monitoring the first request did not carry",
            endpoints.clone(),
            "tok-ep-monitoring",
            endpoint_body("wording-ep-b", "tok-ep-monitoring", "JUPYTER_ENTERPRISE_GATEWAY"),
            "/configurationOverrides/monitoringConfiguration/s3MonitoringConfiguration/logUri",
            json!("s3://bucket/logs"),
        ),
        // GetManagedEndpointSessionCredentials: a number, and a different role.
        conflict(
            "GetManagedEndpointSessionCredentials, a different duration",
            sessions.clone(),
            "tok-session-duration",
            session_base("tok-session-duration"),
            "/durationInSeconds",
            json!(60),
        ),
        conflict(
            "GetManagedEndpointSessionCredentials, a different execution role",
            sessions.clone(),
            "tok-session-role",
            session_base("tok-session-role"),
            "/executionRoleArn",
            json!("arn:aws:iam::000000000000:role/OtherStudioRole"),
        ),
    ];

    for row in &rows {
        assert_conflict_wording(&emr, row).await;
    }
}

/// One row of rule 2: an id nothing holds, and the two wordings the two classes answer it
/// with.
struct Absent {
    case: String,
    identifier: String,
    /// `DELETE` of the resource, which models no not-found error.
    delete_path: String,
    /// `GET` of the same resource, which models one.
    describe_path: String,
    /// The fixed sentence the `ValidationException` must carry.
    fixed_sentence: &'static str,
    /// How the read path spells this resource's type.
    resource_type: &'static str,
}

/// Drive one row of rule 2: the delete answers the fixed sentence with no id in it, the
/// read answers the identifier-bearing wording, and the two differ.
async fn assert_absent_wording(emr: &Emr, row: &Absent) {
    let case = row.case.as_str();

    let refused = emr.send("DELETE", &row.delete_path, None).await;
    refused.refused(case, 400, INVALID_REQUEST);
    let message = text(&refused.body, "/message", case);
    // Equality against ONE sentence for every row is what pins it as fixed text: a wording
    // with a variable part could not be equal to it for every identifier the rows send.
    assert_eq!(
        message, row.fixed_sentence,
        "case {case:?}: expected message {:?}, got {message:?}",
        row.fixed_sentence
    );

    let read = emr.send("GET", &row.describe_path, None).await;
    read.refused(case, 400, NOT_FOUND);
    let reported = text(&read.body, "/message", case);
    let expected = read_path_wording(row.resource_type, &row.identifier);
    assert_eq!(
        reported, expected,
        "case {case:?}: expected the read to keep the wording {expected:?}, got {reported:?}"
    );
    assert_ne!(
        reported, message,
        "case {case:?}: the two error classes must not share one wording, both said \
         {reported:?}"
    );
}

/// Identifiers no resource of this service holds, spanning the forms a `ResourceIdString`
/// takes: both published length bounds (len 1..64, `^[0-9a-z]+$` — smithy_query: shape
/// `ResourceIdString`), an all-digit value and a plausible minted one.
fn absent_ids() -> Vec<(&'static str, String)> {
    vec![
        ("the shortest id the model allows", "a".to_string()),
        ("the longest id the model allows", "z".repeat(64)),
        ("an all-digit id", "0123456789".to_string()),
        ("a plausible minted id", "neverminted9".to_string()),
    ]
}

/// The two rows an absent id produces: the job template and the security configuration,
/// the two resources whose delete models no not-found error and whose own `/{id}` read
/// models one.
fn absent_rows(label: &str, identifier: &str) -> Vec<Absent> {
    vec![
        Absent {
            case: format!("DeleteJobTemplate, {label}"),
            identifier: identifier.to_string(),
            delete_path: format!("/jobtemplates/{identifier}"),
            describe_path: format!("/jobtemplates/{identifier}"),
            fixed_sentence: ABSENT_JOB_TEMPLATE,
            resource_type: "Job Template",
        },
        Absent {
            case: format!("DeleteSecurityConfiguration, {label}"),
            identifier: identifier.to_string(),
            delete_path: format!("/securityconfigurations/{identifier}"),
            describe_path: format!("/securityconfigurations/{identifier}"),
            fixed_sentence: ABSENT_SECURITY_CONFIGURATION,
            resource_type: "Security Configuration",
        },
    ]
}

/// RULE 2 for an id that never resolved, over every form the model admits.
#[tokio::test]
async fn an_absent_resource_is_refused_by_the_class_that_reports_it() {
    let emr = Emr::new();
    for (label, identifier) in absent_ids() {
        for row in absent_rows(label, &identifier) {
            assert_absent_wording(&emr, &row).await;
        }
    }
}

/// RULE 2 for an id that DID resolve once: a second delete of a resource the first delete
/// removed is the same miss, so it carries the same fixed sentence — which is what makes an
/// idempotent teardown recognise it — while the read of that id keeps the other wording.
#[tokio::test]
async fn a_second_delete_is_refused_with_the_same_fixed_sentence() {
    let emr = Emr::new();

    let template = text(
        emr.send("POST", "/jobtemplates", Some(template_base("tok-redelete-tmpl")))
            .await
            .ok("a job template to delete"),
        "/id",
        "a job template to delete",
    );
    emr.send("DELETE", &format!("/jobtemplates/{template}"), None)
        .await
        .ok("deleting the job template");

    let configuration = text(
        emr.send(
            "POST",
            "/securityconfigurations",
            Some(configuration_base("tok-redelete-sc")),
        )
        .await
        .ok("a security configuration to delete"),
        "/id",
        "a security configuration to delete",
    );
    emr.send(
        "DELETE",
        &format!("/securityconfigurations/{configuration}"),
        None,
    )
    .await
    .ok("deleting the security configuration");

    // One row per deleted resource: the job template row of its own id, the security
    // configuration row of its own.
    let label = "a deleted resource's own id";
    let rows = [
        absent_rows(label, &template).remove(0),
        absent_rows(label, &configuration).remove(1),
    ];
    for row in &rows {
        assert_absent_wording(&emr, row).await;
    }
}

/// The rule's BOUNDARY: a resource that EXISTS is never refused with the sentence for one
/// that is absent. A security configuration a registered virtual cluster still references
/// cannot be deleted, and that refusal says what it is about — it names the configuration
/// and the cluster holding it.
#[tokio::test]
async fn a_resource_that_exists_is_not_refused_as_an_absent_one() {
    let emr = Emr::new();
    let configuration = text(
        emr.send(
            "POST",
            "/securityconfigurations",
            Some(configuration_base("tok-inuse-sc")),
        )
        .await
        .ok("a referenced security configuration"),
        "/id",
        "a referenced security configuration",
    );
    let mut body = cluster_base("tok-inuse-vc");
    set_at(
        &mut body,
        "/securityConfigurationId",
        json!(configuration.clone()),
        "a cluster holding the configuration",
    );
    let cluster = text(
        emr.send("POST", "/virtualclusters", Some(body))
            .await
            .ok("a cluster holding the configuration"),
        "/id",
        "a cluster holding the configuration",
    );

    let case = "DeleteSecurityConfiguration of a configuration a cluster references";
    let refused = emr
        .send(
            "DELETE",
            &format!("/securityconfigurations/{configuration}"),
            None,
        )
        .await;
    refused.refused(case, 400, INVALID_REQUEST);
    let message = text(&refused.body, "/message", case);
    assert_ne!(
        message, ABSENT_SECURITY_CONFIGURATION,
        "case {case:?}: a configuration that exists must not be refused as an absent one"
    );
    assert!(
        message.contains(&configuration) && message.contains(&cluster),
        "case {case:?}: expected a refusal naming configuration {configuration:?} and cluster \
         {cluster:?}, got {message:?}"
    );

    // And the configuration really is still there, so the refusal above was about what it
    // says and nothing else.
    let described = emr
        .send("GET", &format!("/securityconfigurations/{configuration}"), None)
        .await;
    assert_eq!(
        text(
            described.ok(case),
            "/securityConfiguration/id",
            case
        ),
        configuration,
        "case {case:?}: the refused delete must have removed nothing"
    );
}
