// SPDX-License-Identifier: CC0-1.0

//! Whether the SERVERLESS namespace a registration request names EXISTS: a fact
//! this control plane resolves rather than holds.
//!
//! `RegisterNamespace` and `DeregisterNamespace` take either arm of the modelled
//! `NamespaceIdentifierUnion`. The provisioned arm names a cluster of THIS
//! service, resolved against this store
//! ([`super::ComAmazonawsRedshiftRedshiftserviceversion20121201State::registration_target`]).
//! The serverless arm names a namespace and one of its workgroups, and those are
//! resources of Amazon Redshift Serverless — a different control plane with its
//! own store — so whether the pair exists is a question only that service can
//! answer. It is asked the way every emulated service reaches another: the AWS
//! SDK driven over the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), so nothing here
//! opens a socket or leaves the host.
//!
//! Nothing is invented and nothing is assumed from silence. A control plane that
//! answers that it holds no such pair settles the question
//! ([`ServerlessTarget::Absent`]) and the registration change is refused; one
//! that cannot be asked at all — an emulator with no Redshift Serverless
//! mounted knows of no workgroup either way — leaves it UNDECIDED
//! ([`ServerlessTarget::Unresolvable`]), and the change is admitted rather than
//! refused on the strength of a peer that never answered.
//!
//! The same two-control-plane question is asked of a zero-ETL integration's
//! `TargetArn` ([`resolve_zero_etl_target`]): that member names "the Amazon
//! Redshift data warehouse to use as the target for replication", and its
//! modelled pattern admits a namespace ARN of EITHER control plane, so an ARN
//! naming a `redshift-serverless` namespace is resolved here while one naming a
//! provisioned cluster's namespace is resolved against this service's own store.

use super::NamespaceRefusal;
use crate::ops::types::{NamespaceIdentifierUnion, ServerlessIdentifier};

/// The error codes that say Amazon Redshift Serverless answered about the VALUE
/// the lookup named: it holds no such resource (`ResourceNotFoundException`, the
/// fault `GetWorkgroup` and `GetNamespace` model for a name that resolves to
/// nothing), or the value is not a name it could hold at all
/// (`ValidationException`, the fault they model for a value outside the member's
/// constraints — which therefore names no workgroup and no namespace either).
///
/// Any other code, or none at all — a server fault, a transport failure, or a
/// reply that is not that service's answer because no Redshift Serverless is
/// mounted beside this one — says nothing about the pair and leaves the question
/// undecided.
const PAIR_NEGATIVE_ERROR_CODES: &[&str] = &["ResourceNotFoundException", "ValidationException"];

/// What asking Amazon Redshift Serverless about one (namespace, workgroup) pair
/// found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerlessTarget {
    /// That control plane holds the workgroup the request names, and the
    /// namespace that workgroup belongs to is the one the request names.
    Resolved,
    /// That control plane was asked and holds no such pair, so the request names
    /// no namespace a registration could be made against.
    Absent,
    /// That control plane could not be asked, so whether the pair exists is
    /// UNKNOWN. That is this emulator failing to resolve a fact about itself,
    /// not a caller naming something that does not exist, so the two are never
    /// answered the same way. The reason it gave travels for the
    /// operator-visible log.
    Unresolvable(String),
}

/// How the namespace registration pair words the refusal of a
/// `ServerlessIdentifier` that resolves to no workgroup of this account.
///
/// Both quoted values are interpolated and neither is fixed text: `owner` is the
/// account that owns the request and `workgroup_identifier` is the
/// `WorkgroupIdentifier` the request carried, verbatim. The member carries the
/// workgroup's NAME — which is what the sentence calls it, and the name Amazon
/// Redshift Serverless addresses a workgroup by.
pub fn workgroup_not_found_message(owner: &str, workgroup_identifier: &str) -> String {
    format!(
        "Could not find serverless workgroup for serverless workgroup owner \
         \"{owner}\" with serverless workgroup name \"{workgroup_identifier}\""
    )
}

/// The refusal the SERVERLESS arm of a registration request earns, or `Ok(())`
/// when the request may proceed.
///
/// Half of the one precondition rule `RegisterNamespace` and
/// `DeregisterNamespace` share, and the half that is a call to ANOTHER control
/// plane: it is therefore asked before this store's lock is taken, and the
/// provisioned arm — which this store answers on its own, through
/// [`super::ComAmazonawsRedshiftRedshiftserviceversion20121201State::registration_target`]
/// — passes straight through.
///
/// A pair that resolves to nothing is the modelled `ClusterNotFoundFault` (404,
/// awsQuery code `ClusterNotFound`), the same fault the provisioned arm answers
/// for a `ClusterIdentifier` that names no cluster: ONE rule for both arms, so
/// no registration is ever recorded against a target that does not exist.
pub async fn resolve_registration_target(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    identifier: &NamespaceIdentifierUnion,
) -> Result<(), NamespaceRefusal> {
    let NamespaceIdentifierUnion::ServerlessIdentifier(serverless) = identifier else {
        return Ok(());
    };

    // The model bounds both members only by length, so a blank one passes the
    // boundary and still names no namespace and no workgroup to reach it
    // through. Answered here rather than asked of the peer: a blank value is not
    // a name any control plane could answer about.
    if serverless.namespace_identifier.trim().is_empty()
        || serverless.workgroup_identifier.trim().is_empty()
    {
        return Err(NamespaceRefusal::InvalidNamespace(
            "The namespace isn't valid: ServerlessIdentifier must carry a NamespaceIdentifier and a WorkgroupIdentifier."
                .to_string(),
        ));
    }

    match resolve(ctx, serverless).await {
        ServerlessTarget::Resolved => Ok(()),
        ServerlessTarget::Absent => Err(NamespaceRefusal::ClusterNotFound(
            workgroup_not_found_message(
                ::engine::emulator_account_id(),
                &serverless.workgroup_identifier,
            ),
        )),
        ServerlessTarget::Unresolvable(reason) => {
            // Visible at default verbosity: a registration change admitted
            // because the owning control plane could not be asked reads exactly
            // like one admitted because the pair is really there, so the
            // degraded path says so rather than passing quietly.
            ::tracing::warn!(
                reason = %reason,
                "the serverless namespace a registration change names could not be resolved"
            );
            Ok(())
        }
    }
}

/// Ask Amazon Redshift Serverless about one `ServerlessIdentifier`.
///
/// A serverless namespace is reached THROUGH one of its workgroups —
/// `ServerlessIdentifier$WorkgroupIdentifier` is "the unique identifier for the
/// workgroup associated with the serverless namespace" — so the pair resolves
/// only when that control plane holds the workgroup the request names AND the
/// namespace that workgroup belongs to is the one the request names. The
/// namespace is matched against either of its own unique identifiers, the name
/// it is addressed by or the id the service minted for it, because the model
/// calls the member a unique identifier and a namespace has both.
async fn resolve(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    serverless: &ServerlessIdentifier,
) -> ServerlessTarget {
    let region = super::request_region(ctx);
    let client = serverless_client(ctx, &region);

    let answer = match client
        .get_workgroup()
        .workgroup_name(&serverless.workgroup_identifier)
        .send()
        .await
    {
        Ok(answer) => answer,
        Err(error) => return absent_or_unresolvable(&error),
    };
    let Some(namespace_name) = answer
        .workgroup()
        .and_then(::aws_sdk_redshiftserverless::types::Workgroup::namespace_name)
    else {
        // A workgroup belongs to a namespace, so one reported without the
        // namespace it belongs to settles nothing about the pair.
        return ServerlessTarget::Unresolvable(
            "the workgroup was reported without the namespace it belongs to".to_string(),
        );
    };
    if namespace_name == serverless.namespace_identifier {
        return ServerlessTarget::Resolved;
    }

    // The request may name that same namespace by the id the service minted for
    // it rather than by its name, so the namespace the workgroup belongs to is
    // read and its id compared before the pair is called absent.
    match client
        .get_namespace()
        .namespace_name(namespace_name)
        .send()
        .await
    {
        Ok(answer) => {
            if answer
                .namespace()
                .and_then(::aws_sdk_redshiftserverless::types::Namespace::namespace_id)
                .is_some_and(|id| id == serverless.namespace_identifier)
            {
                ServerlessTarget::Resolved
            } else {
                ServerlessTarget::Absent
            }
        }
        Err(error) => absent_or_unresolvable(&error),
    }
}

/// What a failed lookup says about the pair: see [`PAIR_NEGATIVE_ERROR_CODES`].
fn absent_or_unresolvable<E>(
    error: &::aws_sdk_redshiftserverless::error::SdkError<E>,
) -> ServerlessTarget
where
    E: std::error::Error + ::aws_sdk_redshiftserverless::error::ProvideErrorMetadata + 'static,
{
    let reason = ::aws_smithy_types::error::display::DisplayErrorContext(error).to_string();
    // The status that service answered with, when it answered at all: a 5xx is
    // about the service, not about the value, whatever code travels with it.
    let client_error = error
        .raw_response()
        .is_some_and(|response| response.status().is_client_error());
    let code = ::aws_sdk_redshiftserverless::error::ProvideErrorMetadata::code(error).unwrap_or_default();
    let about_the_value = PAIR_NEGATIVE_ERROR_CODES
        .iter()
        .any(|known| known.eq_ignore_ascii_case(code));
    if client_error && about_the_value {
        return ServerlessTarget::Absent;
    }
    ServerlessTarget::Unresolvable(reason)
}

/// The service field a namespace ARN of Amazon Redshift Serverless carries.
///
/// Source: `smithy_query(query="operation", operation="CreateIntegration")` —
/// `TargetArn` @pattern
/// `^arn:aws[a-z\-]*:redshift(-serverless)?:[a-z0-9\-]+:[0-9]{12}:(namespace\/|namespace:)[a-z0-9\-]+$`,
/// which admits this service beside the provisioned control plane's `redshift`.
const SERVERLESS_ARN_SERVICE: &str = "redshift-serverless";

/// Whether `value` is an ARN of the Amazon Redshift Serverless control plane —
/// a resource of that service, not of this one.
pub fn is_serverless_arn(value: &str) -> bool {
    value.split(':').nth(2) == Some(SERVERLESS_ARN_SERVICE)
}

/// The two resource prefixes that same pattern admits before the namespace a
/// `TargetArn` names.
const NAMESPACE_RESOURCE_PREFIXES: [&str; 2] = ["namespace/", "namespace:"];

/// The page bound each lookup listing asks for: the largest the owning service's
/// `maxResults` admits (`smithy_query(query="operation",
/// operation="ListNamespaces")` — @range 1..100), so one page answers unless
/// that account holds more than a hundred of them.
const LOOKUP_PAGE_SIZE: i32 = 100;

/// How many pages of one lookup listing are read before the question is left
/// undecided — a bound, so a peer that keeps handing out a page token cannot
/// spin this resolution without end.
const MAX_LOOKUP_PAGES: usize = 16;

/// What resolving a zero-ETL integration's `TargetArn` against Amazon Redshift
/// Serverless found.
///
/// The three answers are the three a target can give: it is there and ready, it
/// is there and not ready yet, or it is not there — which a caller has to be
/// able to tell apart, because the first two are transient and the third is not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WarehouseTarget {
    /// The namespace the ARN names exists and the data warehouse on it is
    /// active, so it can take the data an integration replicates into it.
    Active,
    /// The namespace the ARN names exists, and no data warehouse on it is active
    /// yet.
    NotActive,
    /// That control plane was asked and holds no namespace the ARN names.
    Absent,
    /// That control plane could not be asked, so the target's state is UNKNOWN;
    /// see [`ServerlessTarget::Unresolvable`]. The reason travels for the
    /// operator-visible log.
    Unresolvable(String),
}

/// The namespace a `TargetArn` names, when the ARN names one of Amazon Redshift
/// Serverless — `None` for a namespace ARN of the provisioned control plane,
/// which this service resolves against its own store.
///
/// The token is the namespace's own unique identifier: the `namespaceId` the ARN
/// that service mints carries, or the name it is addressed by, which is why
/// [`resolve_zero_etl_target`] matches it against either.
pub fn serverless_namespace_token(target_arn: &str) -> Option<&str> {
    let mut fields = target_arn.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    let _partition = fields.next()?;
    if fields.next()? != SERVERLESS_ARN_SERVICE {
        return None;
    }
    let _region = fields.next()?;
    let _account = fields.next()?;
    let resource = fields.next()?;
    NAMESPACE_RESOURCE_PREFIXES
        .iter()
        .find_map(|prefix| resource.strip_prefix(prefix))
        .filter(|token| !token.is_empty())
}

/// Resolve a zero-ETL integration's `TargetArn` against Amazon Redshift
/// Serverless, or answer `None` when the ARN names no namespace of that service.
///
/// A serverless data warehouse is the COMPUTE attached to a namespace — a
/// workgroup — so a namespace is a target that can take data only while it is
/// itself available and at least one workgroup on it is available; a namespace
/// with none, or with one still coming up, exists but is not ready. Asked over
/// the same in-process callback as [`resolve_registration_target`], before any
/// store lock is taken.
pub async fn resolve_zero_etl_target(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    target_arn: &str,
) -> Option<WarehouseTarget> {
    let token = serverless_namespace_token(target_arn)?;
    Some(resolve_warehouse(ctx, target_arn, token).await)
}

/// [`resolve_zero_etl_target`] once the ARN is known to name a serverless
/// namespace.
async fn resolve_warehouse(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    target_arn: &str,
    token: &str,
) -> WarehouseTarget {
    let region = super::request_region(ctx);
    let client = serverless_client(ctx, &region);

    let namespaces = match namespaces(&client).await {
        Ok(namespaces) => namespaces,
        Err(reason) => return WarehouseTarget::Unresolvable(reason),
    };
    // The ARN the request carried is matched against the ARN that service
    // minted first, so a target names exactly what a `CreateNamespace` answered
    // with; the token is then matched against either unique identifier of a
    // namespace, since an ARN may be written with either.
    let Some(namespace) = namespaces.iter().find(|namespace| {
        namespace.namespace_arn() == Some(target_arn)
            || namespace.namespace_id() == Some(token)
            || namespace.namespace_name() == Some(token)
    }) else {
        return WarehouseTarget::Absent;
    };

    if namespace.status() != Some(&::aws_sdk_redshiftserverless::types::NamespaceStatus::Available) {
        return WarehouseTarget::NotActive;
    }
    let Some(namespace_name) = namespace.namespace_name() else {
        // A workgroup names the namespace it belongs to by that namespace's
        // name, so a namespace reported without one settles nothing about the
        // compute attached to it.
        return WarehouseTarget::Unresolvable(
            "the namespace was reported without the name its workgroups carry".to_string(),
        );
    };

    let workgroups = match workgroups(&client).await {
        Ok(workgroups) => workgroups,
        Err(reason) => return WarehouseTarget::Unresolvable(reason),
    };
    let active = workgroups.iter().any(|workgroup| {
        workgroup.namespace_name() == Some(namespace_name)
            && workgroup.status()
                == Some(&::aws_sdk_redshiftserverless::types::WorkgroupStatus::Available)
    });
    if active {
        WarehouseTarget::Active
    } else {
        WarehouseTarget::NotActive
    }
}

/// Every namespace Amazon Redshift Serverless holds for this account, or why the
/// listing could not be read.
async fn namespaces(
    client: &::aws_sdk_redshiftserverless::Client,
) -> Result<Vec<::aws_sdk_redshiftserverless::types::Namespace>, String> {
    let mut found = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..MAX_LOOKUP_PAGES {
        let page = client
            .list_namespaces()
            .max_results(LOOKUP_PAGE_SIZE)
            .set_next_token(token)
            .send()
            .await
            .map_err(|error| {
                ::aws_smithy_types::error::display::DisplayErrorContext(&error).to_string()
            })?;
        found.extend(page.namespaces().iter().cloned());
        token = page.next_token().map(str::to_string);
        if token.is_none() {
            return Ok(found);
        }
    }
    Err(TOO_MANY_LOOKUP_PAGES.to_string())
}

/// Every workgroup Amazon Redshift Serverless holds for this account, or why the
/// listing could not be read.
async fn workgroups(
    client: &::aws_sdk_redshiftserverless::Client,
) -> Result<Vec<::aws_sdk_redshiftserverless::types::Workgroup>, String> {
    let mut found = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..MAX_LOOKUP_PAGES {
        let page = client
            .list_workgroups()
            .max_results(LOOKUP_PAGE_SIZE)
            .set_next_token(token)
            .send()
            .await
            .map_err(|error| {
                ::aws_smithy_types::error::display::DisplayErrorContext(&error).to_string()
            })?;
        found.extend(page.workgroups().iter().cloned());
        token = page.next_token().map(str::to_string);
        if token.is_none() {
            return Ok(found);
        }
    }
    Err(TOO_MANY_LOOKUP_PAGES.to_string())
}

/// Why a lookup that kept being handed another page token is left undecided.
const TOO_MANY_LOOKUP_PAGES: &str =
    "the listing handed out more pages than this lookup reads";

/// An Amazon Redshift Serverless client over the in-process callback into this
/// same emulator.
///
/// Configured exactly as the KMS client `CreateSnapshotCopyGrant` resolves a key
/// through is, and for the same reasons: the callback routes IN PROCESS to this
/// emulator's own inbound listener, so no network client is configured here;
/// `region` is the request's own signing region, so the pair is resolved in the
/// Region the caller addressed; and the credentials are the callback's, which
/// sign this in-process hop and never appear in a response.
fn serverless_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_redshiftserverless::Client {
    let config = ::aws_sdk_redshiftserverless::Config::builder()
        .behavior_version(::aws_sdk_redshiftserverless::config::BehaviorVersion::latest())
        .region(::aws_sdk_redshiftserverless::config::Region::new(
            region.to_string(),
        ))
        .endpoint_url(format!(
            "https://redshift-serverless.{region}.amazonaws.com"
        ))
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        // The transport is in-process: there is nothing to retry, time out or
        // watch for a stalled stream, and each of those would need a sleep
        // implementation this client is not given. The same goes for caching the
        // identity: the credentials are handed over directly.
        .retry_config(::aws_sdk_redshiftserverless::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_redshiftserverless::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_redshiftserverless::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_redshiftserverless::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_redshiftserverless::Client::from_conf(config)
}
