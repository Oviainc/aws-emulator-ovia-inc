// SPDX-License-Identifier: CC0-1.0

//! Whether the compute target a request names EXISTS in the caller's Region: a
//! fact this service RESOLVES rather than holds.
//!
//! The Data API runs a statement on a data warehouse it does not own. A
//! provisioned cluster is a resource of Amazon Redshift and a serverless
//! workgroup one of Amazon Redshift Serverless, so whether there is such an
//! endpoint in the Region the request addressed is a question only those
//! services can answer — and it is asked the way any emulated service reaches
//! another: the AWS SDK driven over the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), so this crate names
//! no other service's crate.
//!
//! Nothing is invented and nothing is assumed from silence. A control plane that
//! answers that it holds no such cluster or workgroup settles the question
//! ([`TargetResolution::Absent`]); one that cannot be asked at all — an emulator
//! with no Redshift control plane mounted knows of no cluster — leaves it
//! UNDECIDED ([`TargetResolution::Unresolvable`]), and a submit admits the
//! statement rather than refusing it on the strength of a peer that did not
//! answer.

use super::{ConnectionRequest, ConnectionTarget};

/// The Region a request is resolved in when it carries no SigV4 credential
/// scope that names one. Matches the Region the emulator's own clients default
/// to.
const DEFAULT_REGION: &str = "us-east-1";

/// How many workgroups each `ListWorkgroups` page asks for — the modelled
/// maximum (`ListWorkgroupsRequest$maxResults`, range 1..100), so the walk below
/// makes as few calls as it can.
const WORKGROUP_PAGE_SIZE: i32 = 100;

/// How many pages that walk reads before giving up. The cursor comes from
/// Redshift Serverless rather than from this service, so the loop is bounded
/// here: a peer that kept handing out a token can never spin this handler.
const MAX_WORKGROUP_PAGES: usize = 64;

/// What asking the owning control plane about one compute target found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetResolution {
    /// The target names a cluster or workgroup that exists in the Region.
    Exists,
    /// The owning control plane was asked and holds no such cluster or
    /// workgroup, so there is no endpoint for the statement to run on.
    Absent,
    /// The owning control plane could not be asked, so whether the target
    /// exists is UNKNOWN. That is this emulator failing to resolve a fact about
    /// itself, not a caller naming something that does not exist, so the two are
    /// never answered the same way.
    Unresolvable,
}

/// The Region this request was signed for, which is the Region its compute
/// target is resolved in. Falls back to [`DEFAULT_REGION`] for an unsigned
/// request.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// The Amazon Redshift client a provisioned cluster is resolved through.
///
/// `region` is the request's own signing region, so the hop stays in the Region
/// the caller addressed. The client is given the callback's own HTTP client, so
/// the endpoint URL below is only the address that transport routes on IN
/// PROCESS: no socket is opened, no name is resolved and nothing leaves this
/// process. The credentials are the callback's, which sign this in-process hop
/// and never appear in a response.
fn redshift(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_redshift::Client {
    let config = ::aws_sdk_redshift::Config::builder()
        .behavior_version(::aws_sdk_redshift::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_redshift::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://redshift.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or
        // watch for a stalled stream, and each of those would need a sleep
        // implementation this client is not given. The same goes for caching the
        // identity: the credentials the callback hands over are used directly.
        .retry_config(::aws_sdk_redshift::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_redshift::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_redshift::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_redshift::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_redshift::Client::from_conf(config)
}

/// The Amazon Redshift Serverless client a workgroup is resolved through,
/// configured exactly as [`redshift`] is and for the same reasons.
fn redshift_serverless(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_redshiftserverless::Client {
    let config = ::aws_sdk_redshiftserverless::Config::builder()
        .behavior_version(::aws_sdk_redshiftserverless::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_redshiftserverless::config::Region::new(
            region.to_string(),
        ))
        .endpoint_url(format!(
            "https://redshift-serverless.{region}.amazonaws.com"
        ))
        .retry_config(::aws_sdk_redshiftserverless::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_redshiftserverless::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_redshiftserverless::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_redshiftserverless::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_redshiftserverless::Client::from_conf(config)
}

/// Resolve `target` against the control plane that owns it.
///
/// Asked BEFORE a submit admits a statement: a statement runs on an endpoint, so
/// a target that resolves to none is a request the service cannot serve. The
/// answer is the owner's, never this service's own state — the Data API holds no
/// cluster and no workgroup of its own.
pub async fn resolve_target(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    target: &ConnectionTarget,
) -> TargetResolution {
    let region = request_region(ctx);
    match target {
        ConnectionTarget::Cluster(identifier) => resolve_cluster(ctx, &region, identifier).await,
        ConnectionTarget::Workgroup(name) => resolve_workgroup(ctx, &region, name).await,
    }
}

/// Whether ANY data warehouse `request` NAMES resolves to no endpoint in the
/// Region.
///
/// Asked before a malformed target SELECTION is reported, because the service
/// resolves the endpoint a request named FIRST: a cluster or workgroup that does
/// not exist in the Region is reported as the missing endpoint even when the
/// selection is also wrong, and only a selection whose named warehouses all
/// resolve is refused as a selection. `false` for a request that names no
/// warehouse at all — there is then no endpoint to resolve and nothing but the
/// selection to report — and `false` for one whose owner could not be asked,
/// which leaves the question UNDECIDED exactly as [`resolve_target`] does.
pub async fn names_absent_warehouse(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    request: ConnectionRequest<'_>,
) -> bool {
    for target in request.named_warehouses() {
        if resolve_target(ctx, &target).await == TargetResolution::Absent {
            return true;
        }
    }
    false
}

/// Resolve one `ClusterIdentifier` against Amazon Redshift.
///
/// `DescribeClusters` answers `ClusterNotFoundFault` for an identifier that
/// "does not refer to an existing cluster", which is exactly this question; any
/// OTHER failure says nothing about whether the cluster exists, so it leaves the
/// question undecided.
async fn resolve_cluster(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    identifier: &str,
) -> TargetResolution {
    let answer = redshift(ctx, region)
        .describe_clusters()
        .cluster_identifier(identifier)
        .send()
        .await;
    match answer {
        Ok(_) => TargetResolution::Exists,
        Err(error) => {
            if error
                .as_service_error()
                .is_some_and(::aws_sdk_redshift::operation::describe_clusters::DescribeClustersError::is_cluster_not_found_fault)
            {
                return TargetResolution::Absent;
            }
            // Visible at default verbosity: a statement admitted because the
            // owner could not be asked reads exactly like one admitted because
            // the cluster is really there, so the degraded path says so rather
            // than passing quietly.
            ::tracing::warn!(
                %region,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the cluster a statement names could not be resolved"
            );
            TargetResolution::Unresolvable
        }
    }
}

/// Resolve one `WorkgroupName` against Amazon Redshift Serverless.
///
/// `WorkgroupNameString` admits the workgroup's NAME or its ARN, and the
/// serverless control plane addresses a workgroup by name — so a name is asked
/// for directly (`GetWorkgroup`, whose `ResourceNotFoundException` settles the
/// question) and an ARN is matched against the workgroups the Region holds.
async fn resolve_workgroup(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
) -> TargetResolution {
    if name.starts_with("arn:") {
        return resolve_workgroup_arn(ctx, region, name).await;
    }
    let answer = redshift_serverless(ctx, region)
        .get_workgroup()
        .workgroup_name(name)
        .send()
        .await;
    match answer {
        Ok(_) => TargetResolution::Exists,
        Err(error) => {
            if error
                .as_service_error()
                .is_some_and(::aws_sdk_redshiftserverless::operation::get_workgroup::GetWorkgroupError::is_resource_not_found_exception)
            {
                return TargetResolution::Absent;
            }
            ::tracing::warn!(
                %region,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the workgroup a statement names could not be resolved"
            );
            TargetResolution::Unresolvable
        }
    }
}

/// Resolve a workgroup named by ARN: the Region's workgroups are walked and
/// their own ARNs compared, so the ARN form and the name form resolve to the
/// same workgroup. A walk that cannot be completed leaves the question
/// undecided, exactly as an unanswerable `GetWorkgroup` does.
async fn resolve_workgroup_arn(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    arn: &str,
) -> TargetResolution {
    let client = redshift_serverless(ctx, region);
    let mut token: Option<String> = None;
    for page_number in 0..MAX_WORKGROUP_PAGES {
        let answer = client
            .list_workgroups()
            .max_results(WORKGROUP_PAGE_SIZE)
            .set_next_token(token)
            .send()
            .await;
        let page = match answer {
            Ok(page) => page,
            Err(error) => {
                ::tracing::warn!(
                    %region,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the Region's workgroups could not be read, so the workgroup a statement \
                     names by ARN is unresolved"
                );
                return TargetResolution::Unresolvable;
            }
        };
        if page
            .workgroups()
            .iter()
            .any(|workgroup| workgroup.workgroup_arn() == Some(arn))
        {
            return TargetResolution::Exists;
        }
        token = page.next_token().map(str::to_string);
        if token.is_none() {
            return TargetResolution::Absent;
        }
        if page_number + 1 == MAX_WORKGROUP_PAGES {
            ::tracing::warn!(
                pages = MAX_WORKGROUP_PAGES,
                "stopped reading workgroups at the page bound, so the workgroup a statement \
                 names by ARN is unresolved"
            );
        }
    }
    TargetResolution::Unresolvable
}
