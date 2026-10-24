// SPDX-License-Identifier: CC0-1.0

//! @op UpdateStage
//! IMPLEMENTED: applies the request's `patchOperations` to the stored `Stage` record of
//! `restApiId`/`stageName`, in request order, and reports the updated stage — so a later
//! `GetStage` reads back exactly what the patch set.
//!
//! The whole list is applied to a COPY of the record and written back only once every op
//! has succeeded, so a list carrying one unsupported op leaves the stored stage as it was
//! rather than half-patched.
//!
//! Enabling, resizing or disabling the stage's cache cluster is a transition the CLOCK
//! owes: the due instant is stored on the record (see [`crate::state::StageEntry`]) and
//! `cacheClusterStatus` is computed from it, so a caller reading straight after this call
//! still observes `CREATE_IN_PROGRESS` or `DELETE_IN_PROGRESS`.
//!
//! What an `op` means, how a `path` is read and how a single-valued member, a map
//! member or a bounded number is edited are the service's shared patch semantics
//! (`crate::state`'s patch section); this file only maps each supported path to the
//! member of a stage it names.
//!
//! Two paths of this surface are not a plain member of the `Stage` shape:
//!
//! * the per-method settings. A method-setting path is the method-setting KEY followed
//!   by the setting it writes (`/~1pets/GET/metrics/enabled`), the key being
//!   `{resource-path}/{HTTP-METHOD}` or `*/*` as `Stage$methodSettings` documents, so
//!   the setting is read off the END of the path and the key is reported exactly as the
//!   path addressed it — see [`apply_method_setting_patch`].
//! * `accessLogSettings` and `canarySettings`, structures whose own members are
//!   addressed the way a top-level member is, and which a `remove` of the structure
//!   itself turns off.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `UpdateStage`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-apigateway-backplanecontrolservice/smithy-model/com-amazonaws-apigateway-backplanecontrolservice.json")
//! ```
//!
//! Changes information about a Stage resource.
//!
//! ## Input
//!
//! * `patchOperations` (ListOfPatchOperation) — For more information about supported patch
//!   operations, see Patch Operations.
//! * `restApiId` (String) **required** — The string identifier of the associated RestApi.
//! * `stageName` (String) **required** — The name of the Stage resource to change information
//!   about.
//!
//! ## Output
//!
//! * `accessLogSettings` (AccessLogSettings) — Settings for logging access in this stage.
//! * `cacheClusterEnabled` (Boolean, default false) **required** — Specifies whether a cache
//!   cluster is enabled for the stage. To activate a method-level cache, set CachingEnabled
//!   to true for a method.
//! * `cacheClusterSize` (CacheClusterSize, one of: 0.5 | 1.6 | 118 | 13.5 | 237 | 28.4 | 58.2
//!   | 6.1) — The stage's cache capacity in GB. For more information about choosing a cache
//!   size, see Enabling API caching to enhance responsiveness.
//! * `cacheClusterStatus` (CacheClusterStatus, one of: AVAILABLE | CREATE_IN_PROGRESS |
//!   DELETE_IN_PROGRESS | FLUSH_IN_PROGRESS | NOT_AVAILABLE) — The status of the cache
//!   cluster for the stage, if enabled.
//! * `canarySettings` (CanarySettings) — Settings for the canary deployment in this stage.
//! * `clientCertificateId` (String) — The identifier of a client certificate for an API
//!   stage.
//! * `createdDate` (Timestamp) — The timestamp when the stage was created.
//! * `deploymentId` (String) — The identifier of the Deployment that the stage points to.
//! * `description` (String) — The stage's description.
//! * `documentationVersion` (String) — The version of the associated API documentation.
//! * `lastUpdatedDate` (Timestamp) — The timestamp when the stage last updated.
//! * `methodSettings` (MapOfMethodSettings) — A map that defines the method settings for a
//!   Stage resource. Keys (designated as /{method_setting_key below) are method paths defined
//!   as {resource_path}/{http_method} for an individual method override, or /\*/\* for
//!   overriding all methods in the stage.
//! * `stageName` (String) — The name of the stage is the first path segment in the Uniform
//!   Resource Identifier (URI) of a call to API Gateway. Stage names can only contain
//!   alphanumeric characters, hyphens, and underscores. Maximum length is 128 characters.
//! * `tags` (MapOfStringToString) — The collection of tags. Each tag element is associated
//!   with a given resource.
//! * `tracingEnabled` (Boolean, default false) **required** — Specifies whether active
//!   tracing with X-ray is enabled for the Stage.
//! * `variables` (MapOfStringToString) — A map that defines the stage variables for a Stage
//!   resource. Variable names can have alphanumeric and underscore characters, and the values
//!   must match [A-Za-z0-9-._~:/?#&=,]+.
//! * `webAclArn` (String) — The ARN of the WebAcl associated with the Stage.
//!
//! ## Errors
//!
//! * `BadRequestException` (HTTP 400) — The submitted request is not valid, for example, the
//!   input is incomplete or incorrect. See the accompanying error message for details.
//! * `ConflictException` (HTTP 409) — The request configuration has conflicts. For details,
//!   see the accompanying error message.
//! * `LimitExceededException` (HTTP 429) — The request exceeded the rate limit. Retry after
//!   the specified time period.
//! * `NotFoundException` (HTTP 404) — The requested resource is not found. Make sure that the
//!   request URI is correct.
//! * `TooManyRequestsException` (HTTP 429) — The request has reached its throttling limit.
//!   Retry after the specified time period.
//! * `UnauthorizedException` (HTTP 401) — The request is denied because the caller has
//!   insufficient permissions.

#[allow(unused_imports)]
use super::types::*;

use std::collections::{BTreeMap, BTreeSet};

use crate::state::{
    patch_bool, patch_enum_string, patch_f64_in, patch_i32_in, patch_optional_string,
    patch_string_map, Patch, PatchFailure, PatchOpKind, StageEntry, MAX_COLLECTION_ENTRIES,
    MAX_STRING_CHARS,
};

/// The `path` values `UpdateStage` accepts: one per updateable member of the `Stage`
/// shape, the `canarySettings` and `accessLogSettings` sub-members and the per-method
/// settings included. `stageName`, `createdDate`, `lastUpdatedDate` and
/// `cacheClusterStatus` are assigned by the service, `tags` are changed through
/// `TagResource`/`UntagResource` and `webAclArn` by associating a WebACL through AWS WAF,
/// so none of them is patchable.
const SUPPORTED_PATHS: &[&str] = &[
    "/accessLogSettings",
    "/accessLogSettings/destinationArn",
    "/accessLogSettings/format",
    "/cacheClusterEnabled",
    "/cacheClusterSize",
    "/canarySettings",
    "/canarySettings/deploymentId",
    "/canarySettings/percentTraffic",
    "/canarySettings/stageVariableOverrides/<name>",
    "/canarySettings/useStageCache",
    "/clientCertificateId",
    "/deploymentId",
    "/description",
    "/documentationVersion",
    "/tracingEnabled",
    "/variables/<name>",
    "/{resource_path}/{http_method}/caching/dataEncrypted",
    "/{resource_path}/{http_method}/caching/enabled",
    "/{resource_path}/{http_method}/caching/requireAuthorizationForCacheControl",
    "/{resource_path}/{http_method}/caching/ttlInSeconds",
    "/{resource_path}/{http_method}/caching/unauthorizedCacheControlHeaderStrategy",
    "/{resource_path}/{http_method}/logging/dataTrace",
    "/{resource_path}/{http_method}/logging/loglevel",
    "/{resource_path}/{http_method}/metrics/enabled",
    "/{resource_path}/{http_method}/throttling/burstLimit",
    "/{resource_path}/{http_method}/throttling/rateLimit",
];

/// The pointer a canary promotion copies the deployment id FROM: "to promote a canary
/// deployment, you copy the canary deployment ID to the affiliated deployment ID by
/// calling a PATCH request on a Stage resource with "op":"copy",
/// "from":"/canarySettings/deploymentId" and "path":"/deploymentId""
/// (`PatchOperation$from`).
const CANARY_DEPLOYMENT_POINTER: &str = "/canarySettings/deploymentId";

/// From `CanarySettings$percentTraffic`'s own documentation: "The percent (0-100) of
/// traffic diverted to a canary deployment."
const PERCENT_TRAFFIC_RANGE: ::std::ops::RangeInclusive<f64> = 0.0..=100.0;

/// The values `MethodSetting$loggingLevel` documents: "Valid values are OFF, ERROR, and
/// INFO." The member is a plain string on the wire, so its value set is checked here.
const LOGGING_LEVEL_VALUES: &[&str] = &["OFF", "ERROR", "INFO"];

/// The range of `MethodSetting$cacheTtlInSeconds`: API Gateway documents a default TTL of
/// 300 seconds and a maximum of 3600 seconds for a method cache, and 0 ("the higher the
/// TTL, the longer the response will be cached") disables caching.
const CACHE_TTL_RANGE: ::std::ops::RangeInclusive<i32> = 0..=3600;

/// The range of a method setting's throttle limits: a non-negative number. API Gateway
/// publishes DEFAULT account-level throttle limits that an account can have raised, so no
/// maximum is a contract and only the lower bound is a refusal.
const THROTTLE_BURST_RANGE: ::std::ops::RangeInclusive<i32> = 0..=i32::MAX;

/// The throttle rate's range; see [`THROTTLE_BURST_RANGE`].
const THROTTLE_RATE_RANGE: ::std::ops::RangeInclusive<f64> = 0.0..=f64::MAX;

/// The context variable every access log format must carry: "The format must include at
/// least `$context.requestId`" (`AccessLogSettings$format`).
const ACCESS_LOG_REQUIRED_CONTEXT_VARIABLE: &str = "$context.requestId";

/// The `cacheTtlInSeconds` a method-setting entry carries when the patch does not name
/// it: "The default TTL value for API caching is 300 seconds."
const DEFAULT_CACHE_TTL_SECONDS: i32 = 300;

/// The `throttlingRateLimit` a method-setting entry carries when the patch does not name
/// it: API Gateway's default account-level steady-state request rate of 10,000 requests
/// per second.
const DEFAULT_THROTTLING_RATE_LIMIT: f64 = 10_000.0;

/// The `throttlingBurstLimit` a method-setting entry carries when the patch does not name
/// it: API Gateway's default account-level burst of 5,000 requests.
const DEFAULT_THROTTLING_BURST_LIMIT: i32 = 5_000;

/// The method-setting entry a patch creates for a method path that has none yet.
///
/// A setting the patch does not name is not absent on a stage AWS has method settings
/// for: it carries the service's own default — the documented account-level throttle,
/// the documented default cache TTL, and cache invalidation requiring authorization and
/// answering an unauthorized request with the response header. `loggingLevel` is the one
/// member the model declares without a default, so it stays absent until a patch sets it.
fn default_method_setting() -> MethodSetting {
    MethodSetting {
        metrics_enabled: false,
        logging_level: None,
        data_trace_enabled: false,
        throttling_burst_limit: DEFAULT_THROTTLING_BURST_LIMIT,
        throttling_rate_limit: DEFAULT_THROTTLING_RATE_LIMIT,
        caching_enabled: false,
        cache_ttl_in_seconds: DEFAULT_CACHE_TTL_SECONDS,
        cache_data_encrypted: false,
        require_authorization_for_cache_control: true,
        unauthorized_cache_control_header_strategy: Some(
            "SUCCEED_WITH_RESPONSE_HEADER".to_string(),
        ),
    }
}

/// Refuse a deployment id this API has no deployment for: a stage, and a canary of it,
/// serves a deployment of its own API — the same rule `CreateStage` applies.
fn require_deployment(
    deployments: &BTreeSet<String>,
    deployment_id: &str,
) -> Result<(), PatchFailure> {
    if deployments.contains(deployment_id) {
        return Ok(());
    }
    Err(PatchFailure::NotFound(
        crate::state::NO_SUCH_DEPLOYMENT.to_string(),
    ))
}

/// Apply one patch to a COPY of a stored stage.
///
/// `deployments` is the set of the API's stored deployment ids, so a stage can only be
/// pointed at a deployment that exists.
fn apply_patch(
    entry: &mut StageEntry,
    patch: &Patch,
    deployments: &BTreeSet<String>,
) -> Result<(), PatchFailure> {
    match patch.member() {
        "deploymentId" => apply_deployment_id(entry, patch, deployments),
        "description" => patch_optional_string(&mut entry.description, patch, MAX_STRING_CHARS),
        "documentationVersion" => {
            patch_optional_string(&mut entry.documentation_version, patch, MAX_STRING_CHARS)
        }
        "clientCertificateId" => {
            patch_optional_string(&mut entry.client_certificate_id, patch, MAX_STRING_CHARS)
        }
        "tracingEnabled" => patch_bool(&mut entry.tracing_enabled, patch),
        "cacheClusterEnabled" => apply_cache_cluster_enabled(entry, patch),
        "cacheClusterSize" => apply_cache_cluster_size(entry, patch),
        "variables" => {
            let name = patch.map_key(1)?;
            // The value bound is the one `CreateStage` enforces for a stage variable.
            patch_string_map(
                &mut entry.variables,
                patch,
                name,
                MAX_COLLECTION_ENTRIES,
                MAX_STRING_CHARS,
            )
        }
        "canarySettings" => apply_canary_patch(entry, patch, deployments),
        "accessLogSettings" => apply_access_log_patch(entry, patch),
        // Everything else is addressed as a method path rather than as a member of the
        // stage: `/{resource_path}/{http_method}/<setting>`.
        _ => apply_method_setting_patch(entry, patch),
    }
}

/// Point the stage at another deployment, either from the op's `value` or — for a canary
/// promotion — from the canary's own deployment id (see [`CANARY_DEPLOYMENT_POINTER`]).
fn apply_deployment_id(
    entry: &mut StageEntry,
    patch: &Patch,
    deployments: &BTreeSet<String>,
) -> Result<(), PatchFailure> {
    patch.require_single_valued()?;
    let deployment_id = match patch.op {
        PatchOpKind::Add | PatchOpKind::Replace => patch.require_value()?.to_string(),
        PatchOpKind::Copy => {
            let from = patch.from.as_deref().unwrap_or_default();
            if from != CANARY_DEPLOYMENT_POINTER {
                return Err(PatchFailure::BadRequest(format!(
                    "a copy of {} takes from {CANARY_DEPLOYMENT_POINTER}, got {from:?}",
                    patch.path
                )));
            }
            entry
                .canary_settings
                .as_ref()
                .and_then(|canary| canary.deployment_id.clone())
                .ok_or_else(|| {
                    PatchFailure::BadRequest(format!(
                        "{CANARY_DEPLOYMENT_POINTER} names no deployment on this stage, so there \
                         is nothing to copy to {}",
                        patch.path
                    ))
                })?
        }
        // A stage always points at a deployment, so the member cannot be removed.
        _ => return Err(patch.unsupported_op()),
    };
    if deployment_id.is_empty() {
        return Err(PatchFailure::BadRequest(format!(
            "{} must not be empty",
            patch.path
        )));
    }
    require_deployment(deployments, &deployment_id)?;
    entry.deployment_id = deployment_id;
    Ok(())
}

/// Enable or disable the stage's cache cluster, recording the instant the transition the
/// clock now owes is due.
fn apply_cache_cluster_enabled(entry: &mut StageEntry, patch: &Patch) -> Result<(), PatchFailure> {
    let was_enabled = entry.cache_cluster_enabled;
    patch_bool(&mut entry.cache_cluster_enabled, patch)?;
    if entry.cache_cluster_enabled == was_enabled {
        // Asking for what the stage already has provisions nothing, so no transition is
        // owed and an outstanding one is left alone.
        return Ok(());
    }
    let now = ::std::time::Instant::now();
    if entry.cache_cluster_enabled {
        entry.cache_cluster_ready_at = Some(now + crate::state::CACHE_CLUSTER_CREATE_SPAN);
        entry.cache_flush_done_at = None;
        entry.cache_cluster_removed_at = None;
    } else {
        // The cluster is torn down over its span; a flush of the cluster that is going
        // away is no longer outstanding.
        entry.cache_cluster_ready_at = None;
        entry.cache_flush_done_at = None;
        entry.cache_cluster_removed_at = Some(now + crate::state::CACHE_CLUSTER_DELETE_SPAN);
    }
    Ok(())
}

/// Change the stage's cache capacity. Re-sizing a cluster the stage HAS provisions a
/// cluster of the new capacity, which is a creation the clock owes; a size set on a stage
/// with no cluster is stored and provisions nothing.
fn apply_cache_cluster_size(entry: &mut StageEntry, patch: &Patch) -> Result<(), PatchFailure> {
    let before = entry.cache_cluster_size.clone();
    patch_enum_string(
        &mut entry.cache_cluster_size,
        patch,
        CACHE_CLUSTER_SIZE_VALUES,
    )?;
    if entry.cache_cluster_enabled && entry.cache_cluster_size != before {
        entry.cache_cluster_ready_at =
            Some(::std::time::Instant::now() + crate::state::CACHE_CLUSTER_CREATE_SPAN);
        entry.cache_flush_done_at = None;
        entry.cache_cluster_removed_at = None;
    }
    Ok(())
}

/// Apply one patch that names the `canarySettings` structure or reaches into it.
fn apply_canary_patch(
    entry: &mut StageEntry,
    patch: &Patch,
    deployments: &BTreeSet<String>,
) -> Result<(), PatchFailure> {
    // The path naming the structure itself removes the whole canary configuration; there
    // is no value a single op could set every member of it to.
    if patch.tokens.len() == 1 {
        return match patch.op {
            PatchOpKind::Remove => {
                entry.canary_settings = None;
                Ok(())
            }
            _ => Err(patch.unsupported_path(SUPPORTED_PATHS)),
        };
    }
    let mut canary = entry.canary_settings.clone().unwrap_or_default();
    // The structure's own members are addressed the way a top-level member is, with the
    // `canarySettings` token dropped.
    let nested = patch.rebased(1);
    match nested.member() {
        "percentTraffic" => {
            patch_f64_in(&mut canary.percent_traffic, &nested, PERCENT_TRAFFIC_RANGE)?;
        }
        "useStageCache" => patch_bool(&mut canary.use_stage_cache, &nested)?,
        "deploymentId" => {
            patch_optional_string(&mut canary.deployment_id, &nested, MAX_STRING_CHARS)?;
            if let Some(deployment_id) = &canary.deployment_id {
                require_deployment(deployments, deployment_id)?;
            }
        }
        "stageVariableOverrides" => {
            let name = nested.map_key(1)?;
            let mut overrides: BTreeMap<String, String> = canary
                .stage_variable_overrides
                .clone()
                .unwrap_or_default()
                .into_iter()
                .collect();
            patch_string_map(
                &mut overrides,
                &nested,
                name,
                MAX_COLLECTION_ENTRIES,
                MAX_STRING_CHARS,
            )?;
            canary.stage_variable_overrides =
                (!overrides.is_empty()).then(|| overrides.into_iter().collect());
        }
        _ => return Err(patch.unsupported_path(SUPPORTED_PATHS)),
    }
    entry.canary_settings = Some(canary);
    Ok(())
}

/// Apply one patch that names the `accessLogSettings` structure or reaches into it.
fn apply_access_log_patch(entry: &mut StageEntry, patch: &Patch) -> Result<(), PatchFailure> {
    if patch.tokens.len() == 1 {
        return match patch.op {
            PatchOpKind::Remove => {
                entry.access_log_settings = None;
                Ok(())
            }
            _ => Err(patch.unsupported_path(SUPPORTED_PATHS)),
        };
    }
    let mut settings = entry.access_log_settings.clone().unwrap_or_default();
    let nested = patch.rebased(1);
    match nested.member() {
        "destinationArn" => {
            patch_optional_string(&mut settings.destination_arn, &nested, MAX_STRING_CHARS)?;
        }
        "format" => {
            patch_optional_string(&mut settings.format, &nested, MAX_STRING_CHARS)?;
            // From `AccessLogSettings$format`: "The format must include at least
            // $context.requestId."
            if let Some(format) = &settings.format {
                if !format.contains(ACCESS_LOG_REQUIRED_CONTEXT_VARIABLE) {
                    return Err(PatchFailure::BadRequest(format!(
                        "{} must include at least {ACCESS_LOG_REQUIRED_CONTEXT_VARIABLE}, got {format:?}",
                        patch.path
                    )));
                }
            }
        }
        _ => return Err(patch.unsupported_path(SUPPORTED_PATHS)),
    }
    entry.access_log_settings = Some(settings);
    Ok(())
}

/// Each method setting's published patch name — its group and its own name — beside the
/// `MethodSetting` member that names the SAME setting in ONE reference token
/// (`smithy_query(query="shape", shape="MethodSetting")`).
///
/// A patch may address a setting either way: the grouped name is what API Gateway's
/// patch-operations table and console use, and the member name is what a recorded AWS
/// answer shows the service accepting (`/~1<resource>/<METHOD>/metricsEnabled` is
/// answered with the stage's settings, not a refusal). Listed once, so the two spellings
/// of one setting can never write different members.
const METHOD_SETTING_NAMES: &[(&str, &str)] = &[
    ("metrics/enabled", "metricsEnabled"),
    ("logging/loglevel", "loggingLevel"),
    ("logging/dataTrace", "dataTraceEnabled"),
    ("throttling/burstLimit", "throttlingBurstLimit"),
    ("throttling/rateLimit", "throttlingRateLimit"),
    ("caching/enabled", "cachingEnabled"),
    ("caching/ttlInSeconds", "cacheTtlInSeconds"),
    ("caching/dataEncrypted", "cacheDataEncrypted"),
    (
        "caching/requireAuthorizationForCacheControl",
        "requireAuthorizationForCacheControl",
    ),
    (
        "caching/unauthorizedCacheControlHeaderStrategy",
        "unauthorizedCacheControlHeaderStrategy",
    ),
];

/// The setting a method path's TAIL names, with how many reference tokens the method
/// path itself takes. `None` for a tail that names no setting.
///
/// A published name is two reference tokens (`metrics/enabled`) and a member name is one
/// (`metricsEnabled`), so the tail is tried at both widths — the WIDER first, because
/// every published name ends with its own member name and matching the shorter one first
/// would swallow the group token into the method path. The answer is always the PUBLISHED
/// name, so one setting is applied in one place however the request spelled it.
fn method_setting_tail(patch: &Patch) -> Option<(usize, &'static str)> {
    for width in [2usize, 1] {
        let Some(depth) = patch
            .tokens
            .len()
            .checked_sub(width)
            .filter(|depth| *depth > 0)
        else {
            continue;
        };
        // The log level is the one setting with two spellings of its published name in
        // circulation, so `loglevel` and `logLevel` are normalised to the published one.
        let named = match patch.tokens[depth..].join("/").as_str() {
            "logging/logLevel" => Some("logging/loglevel"),
            tail => METHOD_SETTING_NAMES
                .iter()
                .find(|(published, member)| *published == tail || *member == tail)
                .map(|(published, _)| *published),
        };
        if let Some(named) = named {
            return Some((depth, named));
        }
    }
    None
}

/// Apply one patch addressed as a METHOD path:
/// `/{resource_path}/{http_method}/{group}/{setting}`, or `/*/*/{group}/{setting}` for
/// every method of the stage.
///
/// The tail names the setting — in either spelling [`METHOD_SETTING_NAMES`] lists — and
/// everything before it the method, which is the `methodSettings` key the stage reports
/// — spelled exactly as the request spelled it, escapes included, because that is the key
/// the patch addressed.
fn apply_method_setting_patch(entry: &mut StageEntry, patch: &Patch) -> Result<(), PatchFailure> {
    let Some((depth, setting)) = method_setting_tail(patch) else {
        return Err(patch.unsupported_path(SUPPORTED_PATHS));
    };
    let raw: Vec<&str> = patch
        .path
        .strip_prefix('/')
        .unwrap_or(&patch.path)
        .split('/')
        .collect();
    let Some(method_path) = raw.get(..depth).map(|segments| segments.join("/")) else {
        return Err(patch.unsupported_path(SUPPORTED_PATHS));
    };
    if !entry.method_settings.contains_key(&method_path) {
        crate::state::bound_entries(
            "methodSettings",
            entry.method_settings.len() + 1,
            MAX_COLLECTION_ENTRIES,
        )
        .map_err(PatchFailure::BadRequest)?;
    }
    let settings = entry
        .method_settings
        .entry(method_path)
        .or_insert_with(default_method_setting);
    // One setting is one reference token, so the op is applied to the last token the
    // path carries.
    let nested = patch.rebased(patch.tokens.len() - 1);
    match setting {
        "metrics/enabled" => patch_bool(&mut settings.metrics_enabled, &nested),
        "logging/loglevel" => {
            patch_enum_string(&mut settings.logging_level, &nested, LOGGING_LEVEL_VALUES)
        }
        "logging/dataTrace" => patch_bool(&mut settings.data_trace_enabled, &nested),
        "throttling/burstLimit" => patch_i32_in(
            &mut settings.throttling_burst_limit,
            &nested,
            THROTTLE_BURST_RANGE,
        ),
        "throttling/rateLimit" => patch_f64_in(
            &mut settings.throttling_rate_limit,
            &nested,
            THROTTLE_RATE_RANGE,
        ),
        "caching/enabled" => patch_bool(&mut settings.caching_enabled, &nested),
        "caching/ttlInSeconds" => {
            patch_i32_in(&mut settings.cache_ttl_in_seconds, &nested, CACHE_TTL_RANGE)
        }
        "caching/dataEncrypted" => patch_bool(&mut settings.cache_data_encrypted, &nested),
        "caching/requireAuthorizationForCacheControl" => patch_bool(
            &mut settings.require_authorization_for_cache_control,
            &nested,
        ),
        "caching/unauthorizedCacheControlHeaderStrategy" => patch_enum_string(
            &mut settings.unauthorized_cache_control_header_strategy,
            &nested,
            UNAUTHORIZED_CACHE_CONTROL_HEADER_STRATEGY_VALUES,
        ),
        _ => Err(patch.unsupported_path(SUPPORTED_PATHS)),
    }
}

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateStageError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-apigateway-backplanecontrolservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateStageError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.apigateway#UpdateStage")]
#[derive(Debug)]
pub enum UpdateStageErrorModeled {
    BadRequestException(BadRequestException),
    ConflictException(ConflictException),
    LimitExceededException(LimitExceededException),
    NotFoundException(NotFoundException),
    TooManyRequestsException(TooManyRequestsException),
    UnauthorizedException(UnauthorizedException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_stage::UpdateStageError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateStageError = crate::scaffolded::Scaffolded<UpdateStageErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateStageErrorModeled> for UpdateStageError {
    fn from(modeled: UpdateStageErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateStage` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateStage<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateStage<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateStage<Eng>
{
    type Input = UpdateStageInput;
    type Output = UpdateStageOutput;
    type Error = UpdateStageError;

    const NAME: &'static str = "UpdateStage";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_STAGE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_STAGE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine)
            .ok_or_else(crate::state::state_unavailable)?;

        // The list is read BEFORE the store lock is taken, so a malformed patch never
        // reaches the records at all.
        let patches = crate::state::parse_patches(input.patch_operations.as_ref())
            .map_err(patch_failure)?;

        let now = crate::state::now();
        let mut store = state.lock();
        let api = store.rest_apis.get(&input.rest_api_id).ok_or_else(|| {
            UpdateStageErrorModeled::NotFoundException(NotFoundException {
                message: Some(crate::state::no_such_rest_api(&input.rest_api_id)),
            })
        })?;
        // Patched on a COPY: a list whose later op is refused leaves the stored stage
        // untouched, rather than keeping the ops before it.
        let mut updated = api
            .stages
            .get(&input.stage_name)
            .ok_or_else(|| {
                UpdateStageErrorModeled::NotFoundException(NotFoundException {
                    message: Some(crate::state::NO_SUCH_STAGE.to_string()),
                })
            })?
            .clone();
        let deployments: BTreeSet<String> = api.deployments.keys().cloned().collect();

        for patch in &patches {
            apply_patch(&mut updated, patch, &deployments).map_err(patch_failure)?;
        }
        // A successful patch is when the record last changed. An empty list changes
        // nothing, so it moves nothing.
        if !patches.is_empty() {
            updated.last_updated_date = Some(now);
        }

        let api = store
            .rest_apis
            .get_mut(&input.rest_api_id)
            .ok_or_else(crate::state::state_unavailable)?;
        // Rendered through the one `Stage` body this crate builds, from the record about
        // to be stored, so the update reports the stage exactly as `GetStage` will read
        // it back.
        let body = super::create_stage::stage_body(&updated);
        api.stages.insert(input.stage_name.clone(), updated);
        let output = UpdateStageOutput {
            deployment_id: body.deployment_id,
            client_certificate_id: body.client_certificate_id,
            stage_name: body.stage_name,
            description: body.description,
            cache_cluster_enabled: body.cache_cluster_enabled,
            cache_cluster_size: body.cache_cluster_size,
            cache_cluster_status: body.cache_cluster_status,
            method_settings: body.method_settings,
            variables: body.variables,
            documentation_version: body.documentation_version,
            access_log_settings: body.access_log_settings,
            canary_settings: body.canary_settings,
            tracing_enabled: body.tracing_enabled,
            web_acl_arn: body.web_acl_arn,
            tags: body.tags,
            created_date: body.created_date,
            last_updated_date: body.last_updated_date,
        };
        // Everything below is owned, so the lock is released before the audit record is
        // emitted: the critical section covers the state change only.
        drop(store);
        // The access record for a state-changing operation: who, what, and the outcome.
        ::tracing::info!(
            target: crate::state::AUDIT_TARGET,
            operation = Self::NAME,
            caller = ?crate::state::request_caller(ctx.parts()),
            rest_api_id = ?input.rest_api_id,
            stage_name = ?input.stage_name,
            patch_operations = patches.len(),
            outcome = "updated",
            "stage updated",
        );
        Ok(output)
    }
}

/// The modelled failure for a patch this operation cannot apply.
fn patch_failure(failure: PatchFailure) -> UpdateStageError {
    match failure {
        PatchFailure::BadRequest(message) => {
            UpdateStageErrorModeled::BadRequestException(BadRequestException {
                message: Some(message),
            })
        }
        PatchFailure::NotFound(message) => {
            UpdateStageErrorModeled::NotFoundException(NotFoundException {
                message: Some(message),
            })
        }
        PatchFailure::Conflict(message) => {
            UpdateStageErrorModeled::ConflictException(ConflictException {
                message: Some(message),
            })
        }
    }
    .into()
}
