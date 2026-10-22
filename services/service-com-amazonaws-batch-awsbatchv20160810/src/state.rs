// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-batch-awsbatchv20160810 service.
//!
//! The job-definition registry of AWS Batch is modelled here as ONE map of the
//! modelled [`JobDefinition`] shape itself, keyed by `(name, revision)`:
//! `RegisterJobDefinition` inserts a revision and `DescribeJobDefinitions`
//! answers from the same map, so a registered revision and a described one can
//! never disagree. Keying by `(name, revision)` in a `BTreeMap` is also the
//! describe order (name, then revision ascending) and the pagination order.

use std::collections::BTreeMap;

use crate::ops::types::{ContainerProperties, EcsProperties, JobDefinition, NodeProperties};

/// The ARN namespace this service stamps into every ARN it mints
/// (`service_info`: `awsService.arnNamespace` = `batch`).
const ARN_NAMESPACE: &str = "batch";

/// The region an ARN carries when the request is unsigned and therefore names
/// no SigV4 credential scope to read a region from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `JobDefinition$status` of a revision that `RegisterJobDefinition` created and
/// nothing has deregistered.
pub const STATUS_ACTIVE: &str = "ACTIVE";

/// `JobDefinition$status` of a deregistered revision. Named here because
/// `DescribeJobDefinitions`' `status` filter is matched against stored status
/// values; `DeregisterJobDefinition` (still a scaffold) is what will set it.
pub const STATUS_INACTIVE: &str = "INACTIVE";

/// `OrchestrationType` of a job definition whose work runs on Amazon ECS — the
/// default, per the `containerOrchestrationType` doc ("The valid values are
/// `ECS` (default) or `EKS`").
pub const ORCHESTRATION_TYPE_ECS: &str = "ECS";
/// `OrchestrationType` of a job definition whose work runs on Amazon EKS.
pub const ORCHESTRATION_TYPE_EKS: &str = "EKS";

/// `PlatformCapability` for jobs on Amazon EC2 — the default when the request
/// names none (`platformCapabilities` doc: "If no value is specified, it
/// defaults to EC2").
pub const PLATFORM_CAPABILITY_EC2: &str = "EC2";
/// `PlatformCapability` for jobs on Fargate.
pub const PLATFORM_CAPABILITY_FARGATE: &str = "FARGATE";
/// `PlatformCapability` for jobs on Amazon ECS Managed Instances.
pub const PLATFORM_CAPABILITY_MANAGED_INSTANCES: &str = "MANAGED_INSTANCES";

/// The longest `jobDefinitionName` the service accepts, from the member's doc:
/// "It can be up to 128 letters long. It can contain uppercase and lowercase
/// letters, numbers, hyphens (-), and underscores (_)." The model types the
/// member as a plain `String`, so the framework cannot refuse a bad one.
pub const MAX_JOB_DEFINITION_NAME_LEN: usize = 128;

/// The lowest `schedulingPriority` the service accepts, from the member's doc:
/// "The minimum supported value is 0 and the maximum supported value is 9999."
pub const MIN_SCHEDULING_PRIORITY: i32 = 0;
/// The highest `schedulingPriority` the service accepts; see
/// [`MIN_SCHEDULING_PRIORITY`].
pub const MAX_SCHEDULING_PRIORITY: i32 = 9999;

/// The shortest job timeout the service accepts, in seconds, from the `timeout`
/// member's doc: "The minimum value for the timeout is 60 seconds."
pub const MIN_ATTEMPT_DURATION_SECONDS: i32 = 60;

/// The most job definitions `DescribeJobDefinitions` accepts in one request,
/// from the `jobDefinitions` member's doc: "A list of up to 100 job
/// definitions."
pub const MAX_DESCRIBE_FILTER_ENTRIES: usize = 100;

/// The page-size bounds `DescribeJobDefinitions` accepts, from the `maxResults`
/// member's doc: "This value can be between 1 and 100. If this parameter isn't
/// used, then `DescribeJobDefinitions` returns up to 100 results".
pub const MIN_DESCRIBE_PAGE_SIZE: i32 = 1;
/// See [`MIN_DESCRIBE_PAGE_SIZE`]; also the page size used when the caller
/// names no `maxResults`.
pub const MAX_DESCRIBE_PAGE_SIZE: i32 = 100;

/// The most job-definition revisions this emulator keeps.
///
/// An EMULATOR-CHOSEN ceiling, not an AWS quota: the store is in memory and
/// every `RegisterJobDefinition` adds to it, so the registry answers the
/// modelled `ClientException` past this point rather than growing without
/// bound. AWS enforces a per-account job-definition quota of its own.
pub const MAX_JOB_DEFINITIONS: usize = 5_000;

/// The ARN of a job-definition revision:
/// `arn:aws:batch:<region>:<account>:job-definition/<name>:<revision>`, the form
/// the `DescribeJobDefinitions` `jobDefinitions` doc spells out.
pub fn job_definition_arn(region: &str, name: &str, revision: i32) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("job-definition/{name}:{revision}"),
    )
}

/// The region this request's ARNs carry: the one the caller signed for, from the
/// SigV4 credential scope, falling back to [`DEFAULT_REGION`] for an unsigned
/// request.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The request id a refusal names when the response carries none.
///
/// Batch's frontend wraps a client-side refusal together with the id of the
/// request it refused (see [`client_exception_message`]). This emulator's
/// restJson1 error path returns no request id to the caller — the framework sets
/// no request-id header on a JSON error — so there is no id to name and the
/// segment renders this placeholder, which is what a client that reads the
/// response's request id reports for an absent one.
const ABSENT_REQUEST_ID: &str = "None";

/// How Batch words a `ClientException` its frontend raises: the detail of what
/// was wrong, wrapped in `Error executing request, Exception : <detail>,
/// RequestId: <id>`.
///
/// Every refusal this service raises for a request it understood but cannot
/// accept goes through here, so one wording rule covers all of them.
pub fn client_exception_message(detail: &str) -> String {
    format!("Error executing request, Exception : {detail}, RequestId: {ABSENT_REQUEST_ID}")
}

/// Whether `value` is a `jobDefinitionName` the service accepts: up to
/// [`MAX_JOB_DEFINITION_NAME_LEN`] letters, numbers, hyphens and underscores
/// (the member's own doc). The model types it as a plain `String`, so the
/// framework admits anything and this rule is applied in the handler.
pub fn is_valid_job_definition_name(value: &str) -> bool {
    !value.is_empty()
        && value.chars().count() <= MAX_JOB_DEFINITION_NAME_LEN
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Encode a `DescribeJobDefinitions` cursor: the `(name, revision)` key of the
/// first item of the NEXT page.
///
/// Hex-encoded so the token is opaque to the caller, as the member's doc asks
/// ("Treat this token as an opaque identifier").
pub fn encode_page_token(name: &str, revision: i32) -> String {
    hex::encode(format!("{name}:{revision}"))
}

/// Decode a cursor produced by [`encode_page_token`]; `None` when the token was
/// not produced by this service.
///
/// A job-definition name holds no `:` ([`is_valid_job_definition_name`]), so the
/// LAST `:` separates the name from the revision unambiguously.
pub fn decode_page_token(token: &str) -> Option<(String, i32)> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let (name, revision) = decoded.rsplit_once(':')?;
    Some((name.to_string(), revision.parse().ok()?))
}

/// The execution role of EVERY container-properties object a job definition
/// carries, one entry per object, in declaration order: the top-level
/// `containerProperties`, the `container` and the `ecsProperties` tasks of each
/// node range of a multi-node definition, and each `ecsProperties` task (which is
/// where an ECS task carries the role for its containers).
///
/// One traversal, so a rule about the execution role — "For jobs that run on
/// Fargate resources, you must provide an execution role" — is applied to every
/// place a job definition can carry container properties rather than only to the
/// top-level member.
pub fn execution_roles_of<'a>(
    container_properties: Option<&'a ContainerProperties>,
    node_properties: Option<&'a NodeProperties>,
    ecs_properties: Option<&'a EcsProperties>,
) -> Vec<Option<&'a str>> {
    let mut roles = Vec::new();
    if let Some(container) = container_properties {
        roles.push(container.execution_role_arn.as_deref());
    }
    if let Some(node) = node_properties {
        for range in &node.node_range_properties {
            if let Some(container) = range.container.as_ref() {
                roles.push(container.execution_role_arn.as_deref());
            }
            for task in range
                .ecs_properties
                .as_ref()
                .map(|ecs| ecs.task_properties.as_slice())
                .unwrap_or_default()
            {
                roles.push(task.execution_role_arn.as_deref());
            }
        }
    }
    if let Some(ecs) = ecs_properties {
        for task in &ecs.task_properties {
            roles.push(task.execution_role_arn.as_deref());
        }
    }
    roles
}

/// Per-service state for com-amazonaws-batch-awsbatchv20160810. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsBatchAwsbatchv20160810State {
    /// Registered job-definition revisions, keyed by `(jobDefinitionName,
    /// revision)` — the describe and pagination order. Bounded by
    /// [`MAX_JOB_DEFINITIONS`].
    pub job_definitions: BTreeMap<(String, i32), JobDefinition>,
}

impl ComAmazonawsBatchAwsbatchv20160810State {
    /// The revision `RegisterJobDefinition` gives the next registration of
    /// `name`: one past the highest revision stored for it, and 1 for a name
    /// nothing is stored for. A revision is never reused, so deregistering the
    /// highest one does not hand its number out again.
    pub fn next_revision(&self, name: &str) -> i32 {
        self.revisions_of(name)
            .max()
            .map_or(1, |highest| highest.saturating_add(1))
    }

    /// Every stored revision number of `name`, ascending.
    pub fn revisions_of<'a>(&'a self, name: &'a str) -> impl Iterator<Item = i32> + 'a {
        self.job_definitions
            .keys()
            .filter(move |(stored, _)| stored == name)
            .map(|(_, revision)| *revision)
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBatchAwsbatchv20160810State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBatchAwsbatchv20160810State>)
}
