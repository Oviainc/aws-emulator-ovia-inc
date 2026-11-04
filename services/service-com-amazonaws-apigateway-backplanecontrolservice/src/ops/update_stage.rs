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
//! Turning a method's EXECUTION logging on is gated on the account rather than on the
//! stage: API Gateway writes those log events under the account-level CloudWatch Logs
//! role, so a patch that sets a logging level other than off, or turns the full
//! request/response data trace on, is refused while the account singleton carries no role
//! — see [`ACCOUNT_CLOUDWATCH_ROLE_REQUIRED`].
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
//! A `copy` is the canary PROMOTION rather than a patch of a member, so its legal
//! `(path, from)` pairs are a grammar of their own — see [`CANARY_PROMOTIONS`].
//!
//! An `add` names one entry of a collection the stage keys by name and nothing else: every
//! other pointer of a stage names a member that is already there, which is `replace`d, so
//! an `add` of one is refused — see [`add_names_an_entry`]. A `remove` takes away the parts
//! of a stage that can go away; a pointer that is none of them is read as a method
//! setting's path, so it is answered with that sentence — see [`remove_method_setting`].
//! A patch `value` for a boolean member is READ rather than validated: see [`stage_bool`].
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
    patch_enum_string, patch_f64_checked, patch_f64_in, patch_i32_in,
    patch_optional_string, patch_string_map, Patch, PatchFailure, PatchOpKind, StageEntry,
    MAX_COLLECTION_ENTRIES, MAX_STRING_CHARS,
};

// The `path` values `UpdateStage` accepts are the ones `apply_patch` below has an arm
// for: one per updateable member of the `Stage` shape, the `canarySettings` and
// `accessLogSettings` sub-members and the per-method settings included. `stageName`,
// `createdDate`, `lastUpdatedDate` and `cacheClusterStatus` are assigned by the service,
// `tags` are changed through `TagResource`/`UntagResource` and `webAclArn` by associating
// a WebACL through AWS WAF, so none of them is patchable.

/// The pointer a canary promotion copies the deployment id FROM: "to promote a canary
/// deployment, you copy the canary deployment ID to the affiliated deployment ID by
/// calling a PATCH request on a Stage resource with "op":"copy",
/// "from":"/canarySettings/deploymentId" and "path":"/deploymentId""
/// (`PatchOperation$from`).
const CANARY_DEPLOYMENT_POINTER: &str = "/canarySettings/deploymentId";

/// The pointer a canary promotion copies the stage variable overrides FROM: the canary's
/// own `stageVariableOverrides`, "the stage variable overrides used for the canary release
/// deployment" (`CanarySettings$stageVariableOverrides`), which promoting the canary makes
/// the stage's own variables.
const CANARY_OVERRIDES_POINTER: &str = "/canarySettings/stageVariableOverrides";

/// The `copy` grammar of a stage: each pair is a `path` a `copy` may name beside the ONE
/// pointer that `copy`'s `from` may read it out of.
///
/// A `copy` on a stage is the canary promotion — the canary's deployment becomes the
/// stage's, and the canary's stage variable overrides become the stage's variables — so
/// the two sides are a PAIR rather than two independent sets: a `copy` whose path, whose
/// `from`, or whose combination of the two is not one of these pairs applies nothing, and
/// is refused with the sentence [`invalid_copy_operation`] states, which enumerates each
/// side of this table.
const CANARY_PROMOTIONS: &[(&str, &str)] = &[
    ("/deploymentId", CANARY_DEPLOYMENT_POINTER),
    ("/variables", CANARY_OVERRIDES_POINTER),
];

/// The `BadRequestException` message for an `op` a stage cannot apply to the pointer that
/// `op` named: the op and the path, both quoted back exactly as the request spelled them,
/// and nothing else.
///
/// This is the stage's sentence for an op that NAMES a pointer it cannot be applied to —
/// as opposed to one the resource takes no pointer for at all, which is
/// [`STAGE_INVALID_PATCH_OPERATION`] — and the `copy` grammar's refusal below opens with
/// the same text, so the two are built from this one template.
fn invalid_operation(op: PatchOpKind, path: &str) -> String {
    format!("Invalid {} operation with path: {path}", op.as_str())
}

/// The `BadRequestException` message for a `copy` outside the stage's copy grammar.
///
/// Fixed text states the two legal sets — the paths a `copy` may name and the pointers one
/// may read from, both constants of the grammar [`CANARY_PROMOTIONS`] holds, listed from
/// that one table so the sentence cannot offer a pair the stage would refuse — and the only
/// variable parts are the rejected operation's own `path` and `from`, quoted back exactly
/// as the request spelled them.
fn invalid_copy_operation(path: &str, from: &str) -> String {
    let paths: Vec<&str> = CANARY_PROMOTIONS.iter().map(|(path, _)| *path).collect();
    let sources: Vec<&str> = CANARY_PROMOTIONS.iter().map(|(_, from)| *from).collect();
    format!(
        "{} and from {from}. Valid copy:path are [{}] and valid copy:from are [{}]",
        invalid_operation(PatchOpKind::Copy, path),
        paths.join(", "),
        sources.join(", ")
    )
}

/// The `loggingLevel` that is execution logging turned OFF: the one value of the
/// documented set at which a method emits no execution log events at all.
const LOGGING_LEVEL_OFF: &str = "OFF";

/// The values `MethodSetting$loggingLevel` documents: "Valid values are OFF, ERROR, and
/// INFO." The member is a plain string on the wire, so its value set is checked here.
const LOGGING_LEVEL_VALUES: &[&str] = &[LOGGING_LEVEL_OFF, "ERROR", "INFO"];

/// The `BadRequestException` message API Gateway refuses a configuration that turns a
/// stage's execution logging on with while the account has no CloudWatch Logs role.
///
/// API Gateway writes a method's execution logs under the account-level role
/// (`Account$cloudwatchRoleArn`, "the ARN of an Amazon CloudWatch role for the current
/// Account"), so a stage cannot be configured to emit them before the account has one.
/// Entirely fixed text for this condition: it names neither the stage, the method scope
/// nor the setting the request carried.
const ACCOUNT_CLOUDWATCH_ROLE_REQUIRED: &str =
    "CloudWatch Logs role ARN must be set in account settings to enable logging";

/// Whether this patch of the method setting `setting` turns a method's EXECUTION logging
/// ON — the condition the account-level CloudWatch Logs role is a precondition of.
///
/// Execution logging is on for a method scope when its level is one of the documented
/// levels other than [`LOGGING_LEVEL_OFF`], and the full request/response data trace is
/// execution logging too, so either setting being turned on by an `add` or a `replace` is
/// a configuration that needs the role. Turning either off, removing it, and every other
/// method setting (metrics, throttling, caching) emit no execution logs and so are not
/// gated. Each value is read exactly as the applier below reads it, so a value the
/// setting would refuse anyway is left to its own refusal.
fn enables_execution_logging(patch: &Patch, setting: &str) -> bool {
    if !matches!(patch.op, PatchOpKind::Add | PatchOpKind::Replace) {
        return false;
    }
    match setting {
        "logging/loglevel" => patch.value.as_deref().is_some_and(|level| {
            level != LOGGING_LEVEL_OFF && LOGGING_LEVEL_VALUES.contains(&level)
        }),
        "logging/dataTrace" => stage_bool(patch).unwrap_or(false),
        _ => false,
    }
}

/// The boolean a stage's patch `value` spells.
///
/// A patch `value` is a bare string on the wire ("applicable for the add or replace
/// operation", `PatchOperation$value`), and a boolean member of a stage always carries one
/// of its two values, so reading one leaves no third answer for a text that spells
/// neither: the member is TRUE when the text spells `true` and FALSE for every other text,
/// which is what a recorded `cacheClusterEnabled` patch carrying a word shows API Gateway
/// answering — 200, with the flag reported off. Case and surrounding space are not part of
/// the spelling.
fn stage_bool(patch: &Patch) -> Result<bool, PatchFailure> {
    Ok(patch.require_value()?.trim().eq_ignore_ascii_case("true"))
}

/// Apply `patch` to a boolean member of a stage, reading the value the way
/// [`stage_bool`] reads one. `remove` is refused, as it is for every boolean member the
/// model declares with a default: such a member always has one of its two values.
fn patch_stage_bool(current: &mut bool, patch: &Patch) -> Result<(), PatchFailure> {
    patch.require_single_valued()?;
    match patch.op {
        PatchOpKind::Add | PatchOpKind::Replace => *current = stage_bool(patch)?,
        _ => return Err(patch.unsupported_op()),
    }
    Ok(())
}

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

/// The sentence API Gateway refuses an `op` a stage supports no path for with.
///
/// The ops a stage takes are `replace` (its single-valued members), `remove` (its optional
/// members and its nested structures) and `copy` (the canary promotion
/// [`CANARY_DEPLOYMENT_POINTER`] documents), and a recorded refusal enumerates exactly
/// those three, worded differently from the sentence the resources that take
/// `add`/`remove`/`replace` use ([`crate::state::INVALID_PATCH_OPERATION`]).
const STAGE_INVALID_PATCH_OPERATION: &str =
    "Invalid patch operation specified. Must be one of: 'replace', 'remove', 'copy'";

/// The pointers a patch may name inside a stage's `accessLogSettings`, spelled from the
/// stage root: the two members the `AccessLogSettings` shape declares
/// (`smithy_query(query="shape", shape="AccessLogSettings")` — `destinationArn` and
/// `format`), which are the arms [`apply_access_log_patch`] has, in the order API
/// Gateway's own refusal enumerates them.
const ACCESS_LOG_PATHS: &[&str] = &[
    "/accessLogSettings/destinationArn",
    "/accessLogSettings/format",
];

/// How a refusal of a pointer inside `accessLogSettings` is worded: by the STRUCTURE, not
/// by the stage — the member's name and the pointers above, with nothing of the rejected
/// path, which is the sentence API Gateway answers such a pointer with. The three ops a
/// stage takes on this structure enumerate the one list; `move`, `copy` and `test` name
/// nothing inside it and are refused as ops.
const ACCESS_LOG_REFUSAL: crate::state::InvalidPathRefusal =
    crate::state::InvalidPathRefusal::NestedStructure {
        member: "accessLogSettings",
        paths: crate::state::SupportedPatchPaths::for_every_op(ACCESS_LOG_PATHS),
    };

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
///
/// A patch VALUE that names no record is a refusal of the REQUEST
/// (`BadRequestException`), not of the resource the URI addresses: the stage and the API
/// this patch was sent to both exist, so the caller has to fix the value rather than
/// abandon the stage. The not-found class stays for the identifiers in the URI.
fn require_deployment(
    deployments: &BTreeSet<String>,
    deployment_id: &str,
) -> Result<(), PatchFailure> {
    if deployments.contains(deployment_id) {
        return Ok(());
    }
    Err(PatchFailure::BadRequest(
        crate::state::NO_SUCH_DEPLOYMENT.to_string(),
    ))
}

/// The stored state a stage's patch is applied against, beside the record itself: what a
/// patch VALUE naming another record is checked against, and the account-level
/// precondition of a stage's execution logging. Read from the store once per request, so
/// every op of one `patchOperations` list is applied against the same state.
struct StagePatchContext<'a> {
    /// The ids of the API's stored deployments: a stage, and a canary of it, serves a
    /// deployment of its own API.
    deployments: &'a BTreeSet<String>,
    /// The ids of the account's client certificates: a stage presents one of them to its
    /// integration endpoint.
    client_certificates: &'a BTreeSet<String>,
    /// Whether the ACCOUNT has a CloudWatch Logs role, which a patch turning execution
    /// logging on requires (see [`ACCOUNT_CLOUDWATCH_ROLE_REQUIRED`]).
    logging_role_configured: bool,
}

/// Apply one patch to a COPY of a stored stage.
///
/// An `add` is applied only where it names something to add ([`add_names_an_entry`]); on
/// every other pointer of a stage it is refused as an op. The pointer and the VALUE are
/// judged first, though — a pointer the stage could not apply at all, or a value the
/// member would refuse, is refused on its own terms whichever op carried it — so such an
/// `add` is run against a throwaway copy of the record and refused as an op only once that
/// copy took it.
fn apply_patch(
    entry: &mut StageEntry,
    patch: &Patch,
    context: &StagePatchContext<'_>,
) -> Result<(), PatchFailure> {
    if matches!(patch.op, PatchOpKind::Add) && !add_names_an_entry(patch) {
        apply_stage_patch(&mut entry.clone(), patch, context)?;
        return Err(PatchFailure::BadRequest(invalid_operation(
            patch.op,
            &patch.path,
        )));
    }
    apply_stage_patch(entry, patch, context)
}

/// Whether an `add` names a pointer of a stage it applies to.
///
/// `add` is the op that puts something THERE that was not: on a stage it names one entry
/// of a collection the stage keys by name — a stage variable, one of the canary's variable
/// overrides, or a method's setting, which the stage carries no entry for until a patch
/// names it. Every other pointer of a stage names a single-valued member the stage already
/// has, which is `replace`d, so an `add` of one puts nothing anywhere and is refused.
///
/// Stated positively, from the three collections only, so it cannot drift from the
/// dispatch below: a pointer that names none of them names no entry.
fn add_names_an_entry(patch: &Patch) -> bool {
    match patch.member() {
        "variables" => patch.tokens.len() > 1,
        "canarySettings" => {
            patch.tokens.len() > 2
                && patch
                    .tokens
                    .get(1)
                    .is_some_and(|member| member == "stageVariableOverrides")
        }
        // A method's settings, which the stage holds no entry for until one is added. A
        // single-valued member of the `Stage` shape names no setting, so it answers here
        // too.
        _ => method_setting_tail(patch).is_some(),
    }
}

/// Apply one patch that named an op and a pointer the stage takes, to the member of the
/// record that pointer names.
fn apply_stage_patch(
    entry: &mut StageEntry,
    patch: &Patch,
    context: &StagePatchContext<'_>,
) -> Result<(), PatchFailure> {
    // A `copy` is the canary promotion, whose legal (path, from) pairs are the stage's
    // copy grammar — judged for the op as a whole rather than per member, because the
    // refusal names both sides of the pair the request sent.
    if matches!(patch.op, PatchOpKind::Copy) {
        return apply_canary_promotion(entry, patch, context.deployments);
    }
    match patch.member() {
        "deploymentId" => apply_deployment_id(entry, patch, context.deployments),
        "description" => patch_optional_string(&mut entry.description, patch, MAX_STRING_CHARS),
        "documentationVersion" => {
            patch_optional_string(&mut entry.documentation_version, patch, MAX_STRING_CHARS)
        }
        "clientCertificateId" => {
            patch_optional_string(&mut entry.client_certificate_id, patch, MAX_STRING_CHARS)?;
            // "The identifier of a client certificate for an API stage"
            // (`Stage$clientCertificateId`): the stage presents that certificate to its
            // integration endpoint, so the value has to name one the ACCOUNT has —
            // refused as a bad REQUEST for the reason [`require_deployment`] gives.
            if let Some(certificate) = &entry.client_certificate_id {
                if !context.client_certificates.contains(certificate) {
                    return Err(PatchFailure::BadRequest(
                        crate::state::NO_SUCH_CLIENT_CERTIFICATE.to_string(),
                    ));
                }
            }
            Ok(())
        }
        "tracingEnabled" => patch_stage_bool(&mut entry.tracing_enabled, patch),
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
        "canarySettings" => apply_canary_patch(entry, patch, context.deployments),
        "accessLogSettings" => apply_access_log_patch(entry, patch),
        // Everything else is addressed as a method path rather than as a member of the
        // stage: `/{resource_path}/{http_method}/<setting>`.
        _ => apply_method_setting_patch(entry, patch, context.logging_role_configured),
    }
}

/// Promote the canary: apply the one `copy` of the stage's copy grammar
/// ([`CANARY_PROMOTIONS`]) the request sent, or refuse a `copy` outside it.
fn apply_canary_promotion(
    entry: &mut StageEntry,
    patch: &Patch,
    deployments: &BTreeSet<String>,
) -> Result<(), PatchFailure> {
    let from = patch.from.as_deref().unwrap_or_default();
    let promoted = CANARY_PROMOTIONS
        .iter()
        .find(|(path, source)| *path == patch.path && *source == from)
        .map(|(path, _)| *path)
        .ok_or_else(|| PatchFailure::BadRequest(invalid_copy_operation(&patch.path, from)))?;
    let canary = entry.canary_settings.clone();
    if promoted == "/deploymentId" {
        // The deployment the canary SERVES becomes the one the stage serves, so it has to
        // be a deployment of this API, exactly as a `replace` of the member does. A canary
        // that is configured but carries no deployment of its own serves the stage's
        // ([`crate::ops::create_stage::stage_body`] reports that), so promoting it leaves
        // the stage serving what it already serves; a stage with no canary at all has
        // nothing to promote.
        let deployment_id = canary
            .as_ref()
            .map(|canary| {
                canary
                    .deployment_id
                    .clone()
                    .unwrap_or_else(|| entry.deployment_id.clone())
            })
            .ok_or_else(|| {
                PatchFailure::BadRequest(format!(
                    "{CANARY_DEPLOYMENT_POINTER} names no deployment on this stage, so there is \
                     nothing to copy to {}",
                    patch.path
                ))
            })?;
        require_deployment(deployments, &deployment_id)?;
        entry.deployment_id = deployment_id;
        return Ok(());
    }
    // `/variables`: the overrides the canary served are promoted ONTO the stage's own
    // variables — an overridden name takes the canary's value and a name the canary did
    // not override keeps the stage's — so a canary that overrode nothing promotes nothing.
    // The result is bounded like every other caller-driven collection.
    let mut variables = entry.variables.clone();
    variables.extend(
        canary
            .and_then(|canary| canary.stage_variable_overrides)
            .unwrap_or_default(),
    );
    crate::state::bound_entries("variables", variables.len(), MAX_COLLECTION_ENTRIES)
        .map_err(PatchFailure::BadRequest)?;
    entry.variables = variables;
    Ok(())
}

/// Point the stage at another deployment from the op's `value`. A canary promotion reads
/// the canary's own deployment id instead and is applied by [`apply_canary_promotion`].
fn apply_deployment_id(
    entry: &mut StageEntry,
    patch: &Patch,
    deployments: &BTreeSet<String>,
) -> Result<(), PatchFailure> {
    patch.require_single_valued()?;
    let deployment_id = match patch.op {
        PatchOpKind::Add | PatchOpKind::Replace => patch.require_value()?.to_string(),
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
    patch_stage_bool(&mut entry.cache_cluster_enabled, patch)?;
    if entry.cache_cluster_enabled == was_enabled {
        // Asking for what the stage already has provisions nothing, so no transition is
        // owed and an outstanding one is left alone.
        return Ok(());
    }
    let now = ::std::time::Instant::now();
    // A cluster that is provisioned or torn down holds nothing: the entries of the one
    // that was there do not survive it (`StageEntry::response_cache`).
    entry.flush_response_cache();
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
///
/// The capacity is stored exactly as the request spells it. A patch `value` is a bare
/// string on the wire (`PatchOperation$value`), so the published catalogue of capacities
/// the `CacheClusterSize` enum declares constrains the MODELLED member — where
/// `CreateStage` or `CreateDeployment` carries one and the request-validation layer
/// checks it — and not this member's stored string, which every reader of the stage
/// echoes back as it was set.
fn apply_cache_cluster_size(entry: &mut StageEntry, patch: &Patch) -> Result<(), PatchFailure> {
    let before = entry.cache_cluster_size.clone();
    patch_optional_string(&mut entry.cache_cluster_size, patch, MAX_STRING_CHARS)?;
    if entry.cache_cluster_enabled && entry.cache_cluster_size != before {
        entry.cache_cluster_ready_at =
            Some(::std::time::Instant::now() + crate::state::CACHE_CLUSTER_CREATE_SPAN);
        entry.cache_flush_done_at = None;
        entry.cache_cluster_removed_at = None;
        // A cluster of the new capacity comes up holding nothing, as a provisioned one
        // does in [`apply_cache_cluster_enabled`].
        entry.flush_response_cache();
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
            _ => Err(patch.unsupported_path()),
        };
    }
    let mut canary = entry.canary_settings.clone().unwrap_or_default();
    // The structure's own members are addressed the way a top-level member is, with the
    // `canarySettings` token dropped.
    let nested = patch.rebased(1);
    match nested.member() {
        "percentTraffic" => {
            patch_f64_checked(
                &mut canary.percent_traffic,
                &nested,
                crate::state::accepted_percent_traffic,
            )?;
        }
        "useStageCache" => patch_stage_bool(&mut canary.use_stage_cache, &nested)?,
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
        _ => return Err(patch.unsupported_path()),
    }
    entry.canary_settings = Some(canary);
    Ok(())
}

/// Apply one patch that names the `accessLogSettings` structure or reaches into it.
fn apply_access_log_patch(entry: &mut StageEntry, patch: &Patch) -> Result<(), PatchFailure> {
    // Re-labelled once, here: every way a pointer into this structure can fail to apply —
    // the structure named as a whole by an op that cannot address it that way, a
    // sub-member it does not have, a pointer reaching past one it does — is answered with
    // the structure's own sentence (see [`ACCESS_LOG_REFUSAL`]), which the nested patch
    // below inherits.
    let patch = &patch.refused_with(ACCESS_LOG_REFUSAL);
    if patch.tokens.len() == 1 {
        return match patch.op {
            PatchOpKind::Remove => {
                entry.access_log_settings = None;
                Ok(())
            }
            _ => Err(patch.unsupported_path()),
        };
    }
    let mut settings = entry.access_log_settings.clone().unwrap_or_default();
    let nested = patch.rebased(1);
    match nested.member() {
        "destinationArn" => {
            patch_optional_string(&mut settings.destination_arn, &nested, MAX_STRING_CHARS)?;
            // "The Amazon Resource Name (ARN) of the CloudWatch Logs log group or Kinesis
            // Data Firehose delivery stream to receive access logs"
            // (`AccessLogSettings$destinationArn`): the destination is named by an ARN, so
            // a value that is none is refused the way this service refuses an unreadable
            // ARN on any member.
            if let Some(arn) = &settings.destination_arn {
                crate::state::require_arn(arn).map_err(PatchFailure::BadRequest)?;
            }
        }
        "format" => {
            // The format is not a part of a stage a `remove` takes away on its own: it is
            // "a single line format of the access logs of data" the destination receives
            // (`AccessLogSettings$format`), so it goes with the settings rather than
            // separately, and the pointer is read the way every pointer that is no
            // removable part of a stage is — as a method setting's, see
            // [`remove_method_setting`]. A pointer reaching PAST the format names nothing
            // of this structure whatever the op, so it keeps the structure's own refusal.
            if matches!(patch.op, PatchOpKind::Remove) && nested.tokens.len() == 1 {
                return remove_method_setting(entry, patch);
            }
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
        _ => return Err(patch.unsupported_path()),
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

/// The `methodSettings` key a patch's path addresses: the first `depth` segments of the
/// path exactly as the request spelled them, escapes included, because that is the key
/// the patch addressed.
fn method_settings_key(patch: &Patch, depth: usize) -> Option<String> {
    let raw: Vec<&str> = patch
        .path
        .strip_prefix('/')
        .unwrap_or(&patch.path)
        .split('/')
        .collect();
    raw.get(..depth).map(|segments| segments.join("/"))
}

/// The `BadRequestException` message API Gateway refuses a `remove` of a method setting
/// the stage carries none of with.
///
/// A `remove` names a setting that goes AWAY, so the stage has to carry settings for the
/// method the path addresses. The only variable part is the setting path the request
/// sent, with the pointer's leading `/` dropped; the rest, the trailing space included,
/// is the service's own fixed text.
fn cannot_remove_method_setting(setting: &str) -> String {
    format!(
        "Cannot remove method setting {setting} because there is no method setting for this \
         method "
    )
}

/// Take a method's settings away.
///
/// `methodSettings` is "a map that defines the method settings for a Stage resource"
/// (`Stage$methodSettings`), so a `remove` whose path ends in a setting puts that one
/// setting back to the value a stage carries for a setting nobody configured, and a path
/// naming the method alone takes its whole entry out of the map.
///
/// Either way the method has to HAVE an entry on this stage: there is nothing to take
/// away otherwise, and that is the condition [`cannot_remove_method_setting`] words.
/// Every pointer that is no removable part of a stage is read as a method setting's path,
/// so such a `remove` lands here too and is refused by the same sentence.
fn remove_method_setting(entry: &mut StageEntry, patch: &Patch) -> Result<(), PatchFailure> {
    let refuse = || {
        PatchFailure::BadRequest(cannot_remove_method_setting(
            patch.path.strip_prefix('/').unwrap_or(&patch.path),
        ))
    };
    let named = method_setting_tail(patch);
    let key = method_settings_key(patch, named.map_or(patch.tokens.len(), |(depth, _)| depth))
        .ok_or_else(refuse)?;
    let Some((_, setting)) = named else {
        return entry
            .method_settings
            .remove(&key)
            .map(|_| ())
            .ok_or_else(refuse);
    };
    // The value a setting nobody configured carries, so a setting that was removed reads
    // back exactly as it does on a method the patch never named.
    let unconfigured = default_method_setting();
    let settings = entry.method_settings.get_mut(&key).ok_or_else(refuse)?;
    match setting {
        "metrics/enabled" => settings.metrics_enabled = unconfigured.metrics_enabled,
        "logging/loglevel" => settings.logging_level = unconfigured.logging_level,
        "logging/dataTrace" => settings.data_trace_enabled = unconfigured.data_trace_enabled,
        "throttling/burstLimit" => {
            settings.throttling_burst_limit = unconfigured.throttling_burst_limit;
        }
        "throttling/rateLimit" => {
            settings.throttling_rate_limit = unconfigured.throttling_rate_limit;
        }
        "caching/enabled" => settings.caching_enabled = unconfigured.caching_enabled,
        "caching/ttlInSeconds" => {
            settings.cache_ttl_in_seconds = unconfigured.cache_ttl_in_seconds;
        }
        "caching/dataEncrypted" => {
            settings.cache_data_encrypted = unconfigured.cache_data_encrypted;
        }
        "caching/requireAuthorizationForCacheControl" => {
            settings.require_authorization_for_cache_control =
                unconfigured.require_authorization_for_cache_control;
        }
        "caching/unauthorizedCacheControlHeaderStrategy" => {
            settings.unauthorized_cache_control_header_strategy =
                unconfigured.unauthorized_cache_control_header_strategy;
        }
        _ => return Err(patch.unsupported_path()),
    }
    Ok(())
}

/// Apply one patch addressed as a METHOD path:
/// `/{resource_path}/{http_method}/{group}/{setting}`, or `/*/*/{group}/{setting}` for
/// every method of the stage.
///
/// The tail names the setting — in either spelling [`METHOD_SETTING_NAMES`] lists — and
/// everything before it the method, which is the `methodSettings` key the stage reports
/// — spelled exactly as the request spelled it, escapes included, because that is the key
/// the patch addressed.
fn apply_method_setting_patch(
    entry: &mut StageEntry,
    patch: &Patch,
    logging_role_configured: bool,
) -> Result<(), PatchFailure> {
    // A `remove` takes settings AWAY rather than configuring one, so it is read against
    // the settings the stage already carries — see [`remove_method_setting`].
    if matches!(patch.op, PatchOpKind::Remove) {
        return remove_method_setting(entry, patch);
    }
    let Some((depth, setting)) = method_setting_tail(patch) else {
        return Err(patch.unsupported_path());
    };
    // The account-level logging role is a precondition of the CONFIGURATION, checked
    // before any entry of `methodSettings` is touched: a stage cannot be set to emit
    // execution logs the account has granted API Gateway no role to write.
    if !logging_role_configured && enables_execution_logging(patch, setting) {
        return Err(PatchFailure::BadRequest(
            ACCOUNT_CLOUDWATCH_ROLE_REQUIRED.to_string(),
        ));
    }
    let Some(method_path) = method_settings_key(patch, depth) else {
        return Err(patch.unsupported_path());
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
        "metrics/enabled" => patch_stage_bool(&mut settings.metrics_enabled, &nested),
        "logging/loglevel" => {
            patch_enum_string(&mut settings.logging_level, &nested, LOGGING_LEVEL_VALUES)
        }
        "logging/dataTrace" => patch_stage_bool(&mut settings.data_trace_enabled, &nested),
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
        "caching/enabled" => patch_stage_bool(&mut settings.caching_enabled, &nested),
        "caching/ttlInSeconds" => {
            patch_i32_in(&mut settings.cache_ttl_in_seconds, &nested, CACHE_TTL_RANGE)
        }
        "caching/dataEncrypted" => patch_stage_bool(&mut settings.cache_data_encrypted, &nested),
        "caching/requireAuthorizationForCacheControl" => patch_stage_bool(
            &mut settings.require_authorization_for_cache_control,
            &nested,
        ),
        "caching/unauthorizedCacheControlHeaderStrategy" => patch_enum_string(
            &mut settings.unauthorized_cache_control_header_strategy,
            &nested,
            UNAUTHORIZED_CACHE_CONTROL_HEADER_STRATEGY_VALUES,
        ),
        _ => Err(patch.unsupported_path()),
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
        let patches = crate::state::parse_patches_refused_with_op(
            input.patch_operations.as_ref(),
            crate::state::InvalidPathRefusal::PathOnly,
            STAGE_INVALID_PATCH_OPERATION,
        )
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
        // The account singleton's CloudWatch Logs role, which a patch that turns this
        // stage's execution logging on is a configuration of: a role set to no ARN at all
        // is an account with none, exactly as `GetAccount` reports it.
        let logging_role_configured = store
            .account
            .cloudwatch_role_arn
            .as_deref()
            .is_some_and(|arn| !arn.trim().is_empty());
        // The account's client certificates, one of which a stage presents to its
        // integration endpoint.
        let client_certificates: BTreeSet<String> =
            store.client_certificates.keys().cloned().collect();
        let context = StagePatchContext {
            deployments: &deployments,
            client_certificates: &client_certificates,
            logging_role_configured,
        };

        for patch in &patches {
            apply_patch(&mut updated, patch, &context).map_err(patch_failure)?;
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
        let body = super::create_stage::stage_body(&updated, None);
        let region = api.arn_region().to_string();
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
            web_acl_arn: None,
            tags: body.tags,
            created_date: body.created_date,
            last_updated_date: body.last_updated_date,
        };
        // Everything below is owned, so the lock is released before the audit record is
        // emitted: the critical section covers the state change only.
        drop(store);
        // "The ARN of the WebAcl associated with the Stage": WAFv2's own record, read
        // with this service's store released, so the update reports the web ACL in front
        // of the stage exactly as `GetStage` reads it back.
        let output = UpdateStageOutput {
            web_acl_arn: crate::state::associated_web_acl_arn(
                &*self.engine,
                &region,
                &input.rest_api_id,
                &input.stage_name,
            ),
            ..output
        };
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
