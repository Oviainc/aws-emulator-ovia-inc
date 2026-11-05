// SPDX-License-Identifier: CC0-1.0

//! Input-to-stored projections, split out of [`crate::state`] unchanged: the OUTPUT
//! shape a record stores for each INPUT shape a request carries, the ids a create
//! mints, and the endpoint names a domain publishes.

use super::*;

/// `AdvancedSecurityOptionsInput` as the service reports it back.
///
/// `MasterUserOptions` is deliberately dropped: it carries the master user's name
/// and PASSWORD, and `AdvancedSecurityOptions` — the shape every read path reports
/// — declares no member for either. A credential a caller sent is never echoed.
pub fn stored_advanced_security_options(
    input: AdvancedSecurityOptionsInput,
) -> AdvancedSecurityOptions {
    AdvancedSecurityOptions {
        enabled: input.enabled,
        internal_user_database_enabled: input.internal_user_database_enabled,
        saml_options: input.saml_options.map(|saml| SAMLOptionsOutput {
            enabled: saml.enabled,
            idp: saml.idp,
            subject_key: saml.subject_key,
            roles_key: saml.roles_key,
            session_timeout_minutes: saml.session_timeout_minutes,
        }),
        jwt_options: input.jwt_options.map(|jwt| JWTOptionsOutput {
            enabled: jwt.enabled,
            subject_key: jwt.subject_key,
            roles_key: jwt.roles_key,
            jwks_url: jwt.jwks_url,
            public_key: jwt.public_key,
        }),
        iam_federation_options: input
            .iam_federation_options
            .map(|federation| IAMFederationOptionsOutput {
                enabled: federation.enabled,
                subject_key: federation.subject_key,
                roles_key: federation.roles_key,
            }),
        // The date anonymous auth was disabled is set by the call that disables it,
        // which this slice does not model.
        anonymous_auth_disable_date: None,
        anonymous_auth_enabled: input.anonymous_auth_enabled,
    }
}

/// `IdentityCenterOptionsInput` as the service reports it back.
///
/// The application ARN and identity store id are minted by IAM Identity Center
/// itself, which this emulator does not model, so they stay absent rather than
/// being invented.
pub fn stored_identity_center_options(input: IdentityCenterOptionsInput) -> IdentityCenterOptions {
    IdentityCenterOptions {
        enabled_api_access: input.enabled_api_access,
        identity_center_instance_arn: input.identity_center_instance_arn,
        identity_center_instance_region: input.identity_center_instance_region,
        subject_key: input.subject_key,
        roles_key: input.roles_key,
        identity_center_application_arn: None,
        identity_store_id: None,
    }
}

/// `IamIdentityCenterOptionsInput` as the service reports it back.
///
/// The same reasoning as [`stored_identity_center_options`]: the IAM Identity Center
/// application's own ARN is minted by Identity Center itself, which this emulator does
/// not model, so it stays absent rather than being invented.
pub fn stored_iam_identity_center_options(
    input: IamIdentityCenterOptionsInput,
) -> IamIdentityCenterOptions {
    IamIdentityCenterOptions {
        enabled: input.enabled,
        iam_identity_center_instance_arn: input.iam_identity_center_instance_arn,
        iam_role_for_identity_center_application_arn: input
            .iam_role_for_identity_center_application_arn,
        iam_identity_center_application_arn: None,
    }
}

/// `IamIdentityCenterOptionsInput` as an UPDATE leaves the settings a record already
/// carries.
///
/// The same rule an update applies to the members of the request, one level down: a
/// member the request carries replaces what the record held, and a member it leaves
/// out keeps the stored value. The Identity Center application's own ARN is minted by
/// that service rather than by a caller, so an update never moves it — the same
/// reasoning as [`stored_iam_identity_center_options`].
pub fn updated_iam_identity_center_options(
    stored: Option<IamIdentityCenterOptions>,
    input: IamIdentityCenterOptionsInput,
) -> IamIdentityCenterOptions {
    let mut options = stored.unwrap_or_default();
    if input.enabled.is_some() {
        options.enabled = input.enabled;
    }
    if input.iam_identity_center_instance_arn.is_some() {
        options.iam_identity_center_instance_arn = input.iam_identity_center_instance_arn;
    }
    if input
        .iam_role_for_identity_center_application_arn
        .is_some()
    {
        options.iam_role_for_identity_center_application_arn =
            input.iam_role_for_identity_center_application_arn;
    }
    options
}

/// `VPCOptions` as the service reports it back (`VPCDerivedInfo`).
///
/// The VPC id and the availability zones are DERIVED from the subnets by EC2, which
/// this emulator does not model, so they stay absent rather than being invented.
pub fn stored_vpc_options(input: VPCOptions) -> VPCDerivedInfo {
    VPCDerivedInfo {
        vpc_id: None,
        subnet_ids: input.subnet_ids,
        availability_zones: None,
        security_group_ids: input.security_group_ids,
        egress_enabled: input.egress_enabled,
    }
}

/// `AIMLOptionsInput` as the service reports it back.
///
/// The emulator applies the change in one step, so a feature's current state is the
/// state the caller asked for.
pub fn stored_aiml_options(input: AIMLOptionsInput) -> AIMLOptionsOutput {
    AIMLOptionsOutput {
        natural_language_query_generation_options: input
            .natural_language_query_generation_options
            .map(|options| NaturalLanguageQueryGenerationOptionsOutput {
                current_state: options.desired_state.clone(),
                desired_state: options.desired_state,
            }),
        s3_vectors_engine: input.s3_vectors_engine,
        serverless_vector_acceleration: input.serverless_vector_acceleration,
    }
}

/// `AutomatedSnapshotPauseRequestOptions` as the service reports it back.
pub fn stored_automated_snapshot_pause_options(
    input: AutomatedSnapshotPauseRequestOptions,
) -> AutomatedSnapshotPauseOptions {
    AutomatedSnapshotPauseOptions {
        enabled: input.enabled,
        start_time: input.start_time,
        end_time: input.end_time,
        // The pause state is reported by the snapshot subsystem, which this slice
        // does not model.
        state: None,
    }
}

/// `AutoTuneOptionsInput` (what `CreateDomain` carries) as the `AutoTuneOptions`
/// the record stores and `DomainConfig` reports.
///
/// `RollbackOnDisable` has no member on the create input — only
/// `UpdateDomainConfig` can set it — so it stays absent.
pub fn stored_auto_tune_options(input: AutoTuneOptionsInput) -> AutoTuneOptions {
    AutoTuneOptions {
        desired_state: input.desired_state,
        rollback_on_disable: None,
        maintenance_schedules: input.maintenance_schedules,
        use_off_peak_window: input.use_off_peak_window,
    }
}

/// The GUID a configuration change is addressed by.
///
/// `GUID` is `@length(36, 36)` with pattern
/// `^\p{XDigit}{8}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{12}$` —
/// exactly a hyphenated UUID — so the id comes from the `uuid` crate's v4 generator
/// rather than from a hand-rolled layout, and no two changes share one.
pub(super) fn new_change_id() -> String {
    new_guid()
}

/// A fresh `GUID`, for whichever resource this service addresses by one; see
/// [`new_change_id`] for why that is a v4 UUID.
pub(super) fn new_guid() -> String {
    ::uuid::Uuid::new_v4().to_string()
}

/// The `ReservedInstanceId` a purchase mints: the unique identifier for the
/// reservation, a `GUID` like every other id this service hands out
/// ([`new_guid`]).
pub fn new_reserved_instance_id() -> String {
    new_guid()
}

/// `UpgradeDomainResponse$UpgradeId`: "the unique identifier of the domain upgrade",
/// a `GUID` like every other id this service hands out ([`new_guid`]), so no two
/// runs share one.
pub fn new_upgrade_id() -> String {
    new_guid()
}

/// `DryRunProgressStatus$DryRunId`: "the unique identifier of the dry run", a `GUID`
/// like every other id this service hands out ([`new_guid`]), so no two runs share
/// one.
pub(super) fn new_dry_run_id() -> String {
    new_guid()
}

/// `DomainStatus$DomainId`: the account the domain belongs to and its name.
pub fn domain_id(domain_name: &str) -> String {
    format!("{}/{domain_name}", ::engine::emulator_account_id())
}

/// A domain's ARN: `arn:aws:es:<region>:<account-id>:domain/<domain-name>`.
pub fn domain_arn(domain_name: &str, region: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("domain/{domain_name}"))
}

/// A schema document mirroring the JSON value a REST request carried.
///
/// An index's schema is stored as the modelled document `CreateIndex` carries
/// ([`Index::schema`]), while the domain's own REST data plane reads and writes JSON, so
/// the two meet here — one index record whichever path created it, rather than two stores
/// that could report the same index differently.
///
/// Iterative rather than recursive, like every other walk over a document here
/// ([`index_schema_depth`], [`index_schema_values`], [`merged_index_schema`]): the
/// nesting of the value handed over is the CALLER's, and a stack this process overruns
/// takes the whole process down rather than failing the one request, so the walk holds
/// its state on the heap, where the size of the value already bounds it. A container is
/// assembled out of its converted children, so it is pushed BEHIND them.
#[must_use]
pub fn document_from_json(value: &::serde_json::Value) -> ::aws_smithy_types::Document {
    use ::aws_smithy_types::{Document, Number};

    let mut pending = vec![Step::Convert(value)];
    // The children converted so far whose container has yet to be assembled, in the
    // order they were converted.
    let mut converted: Vec<Document> = Vec::new();
    while let Some(step) = pending.pop() {
        match step {
            Step::Convert(::serde_json::Value::Null) => converted.push(Document::Null),
            Step::Convert(::serde_json::Value::Bool(flag)) => {
                converted.push(Document::Bool(*flag));
            }
            Step::Convert(::serde_json::Value::String(text)) => {
                converted.push(Document::String(text.clone()));
            }
            Step::Convert(::serde_json::Value::Number(number)) => {
                converted.push(Document::Number(
                    number
                        .as_i64()
                        .map(Number::NegInt)
                        .unwrap_or_else(|| Number::Float(number.as_f64().unwrap_or_default())),
                ));
            }
            Step::Convert(::serde_json::Value::Array(elements)) => {
                pending.push(Step::Array(elements.len()));
                // Pushed in reverse, so the elements come off the stack in order.
                pending.extend(elements.iter().rev().map(Step::Convert));
            }
            Step::Convert(::serde_json::Value::Object(members)) => {
                // The names and the children are taken from ONE walk of the members, so
                // a name cannot be paired with another member's value.
                let (names, children): (Vec<String>, Vec<&::serde_json::Value>) = members
                    .iter()
                    .map(|(name, child)| (name.clone(), child))
                    .unzip();
                pending.push(Step::Object(names));
                pending.extend(children.into_iter().rev().map(Step::Convert));
            }
            Step::Array(len) => {
                let elements = converted.split_off(converted.len().saturating_sub(len));
                converted.push(Document::Array(elements));
            }
            Step::Object(names) => {
                let values = converted.split_off(converted.len().saturating_sub(names.len()));
                converted.push(Document::Object(names.into_iter().zip(values).collect()));
            }
        }
    }
    // The walk started with ONE value, so what it leaves behind is that value's document.
    converted.pop().unwrap_or(Document::Null)
}

/// One step of a conversion walk: a value still to convert, or the container to assemble
/// out of the converted children that follow it — a list of that many elements, or a set
/// of members under those names.
///
/// Shared by both directions ([`document_from_json`] and [`json_from_document`]), so the
/// two cannot drift into different walks of the same tree.
pub(super) enum Step<'a, T> {
    Convert(&'a T),
    Array(usize),
    Object(Vec<String>),
}

/// [`document_from_json`] the other way: the JSON value a stored schema document is
/// reported as on the REST data plane.
///
/// Iterative for the same reason, and by the same walk: a document this service stored is
/// as deeply nested as the value a caller handed it.
#[must_use]
pub fn json_from_document(document: &::aws_smithy_types::Document) -> ::serde_json::Value {
    use ::aws_smithy_types::{Document, Number};
    use ::serde_json::Value;

    let mut pending = vec![Step::Convert(document)];
    let mut converted: Vec<Value> = Vec::new();
    while let Some(step) = pending.pop() {
        match step {
            Step::Convert(Document::Null) => converted.push(Value::Null),
            Step::Convert(Document::Bool(flag)) => converted.push(Value::Bool(*flag)),
            Step::Convert(Document::String(text)) => converted.push(Value::String(text.clone())),
            Step::Convert(Document::Number(Number::PosInt(value))) => {
                converted.push(Value::from(*value));
            }
            Step::Convert(Document::Number(Number::NegInt(value))) => {
                converted.push(Value::from(*value));
            }
            Step::Convert(Document::Number(Number::Float(value))) => {
                converted.push(
                    ::serde_json::Number::from_f64(*value)
                        .map(Value::Number)
                        .unwrap_or(Value::Null),
                );
            }
            Step::Convert(Document::Array(elements)) => {
                pending.push(Step::Array(elements.len()));
                pending.extend(elements.iter().rev().map(Step::Convert));
            }
            Step::Convert(Document::Object(members)) => {
                let (names, children): (Vec<String>, Vec<&Document>) = members
                    .iter()
                    .map(|(name, child)| (name.clone(), child))
                    .unzip();
                pending.push(Step::Object(names));
                pending.extend(children.into_iter().rev().map(Step::Convert));
            }
            Step::Array(len) => {
                let elements = converted.split_off(converted.len().saturating_sub(len));
                converted.push(Value::Array(elements));
            }
            Step::Object(names) => {
                let values = converted.split_off(converted.len().saturating_sub(names.len()));
                converted.push(Value::Object(names.into_iter().zip(values).collect()));
            }
        }
    }
    converted.pop().unwrap_or(Value::Null)
}

/// The domain-specific endpoint index, search and data-upload requests go to.
///
/// The shape is `ServiceUrl`'s own example,
/// `search-imdb-movies-oopcnjfn6ugo.eu-west-1.es.amazonaws.com`: the `search-`
/// prefix, the domain name, an opaque per-domain suffix, then the Region and this
/// service's endpoint prefix.
pub fn domain_endpoint(domain_name: &str, suffix: &str, region: &str) -> String {
    format!("search-{domain_name}-{suffix}.{region}.{ARN_NAMESPACE}.amazonaws.com")
}

/// The host a VPC-access domain is reached on over IPv4 — what
/// `DomainStatus$Endpoints` publishes under [`ENDPOINTS_KEY_VPC`].
///
/// The shape is that member's own example,
/// `vpc-endpoint-h2dsd34efgyghrtguk5gt6j2foh4.us-east-1.es.amazonaws.com`: the
/// `vpc-endpoint-` prefix, an opaque per-domain id, then the Region and this service's
/// endpoint prefix. Unlike [`domain_endpoint`] it carries no domain name: the host is
/// reached from inside the VPC and names only the endpoint.
pub fn vpc_access_endpoint(suffix: &str, region: &str) -> String {
    format!("vpc-endpoint-{suffix}.{region}.{ARN_NAMESPACE}.amazonaws.com")
}

/// The host a dual-stack VPC-access domain is reached on over IPv4 *or* IPv6 — what
/// `DomainStatus$Endpoints` publishes under [`ENDPOINTS_KEY_VPC_DUALSTACK`], beside
/// the IPv4 host of [`vpc_access_endpoint`].
///
/// The shape is that member's own example,
/// `vpc-endpoint-h2dsd34efgyghrtguk5gt6j2foh4.aos.us-east-1.on.aws`: the SAME opaque
/// id as the IPv4 host, under the service's dual-stack domain.
pub fn vpc_access_endpoint_v2(suffix: &str, region: &str) -> String {
    dual_stack_host(&format!("vpc-endpoint-{suffix}"), region)
}

/// One host of this service's DUAL-STACK domain: the label a domain is reached under over
/// IPv4, under `aos.<region>.on.aws` instead of the Region's `es` endpoint prefix.
///
/// The dual-stack domain is the one `DomainStatus$Endpoints`' own `vpcv2` example names
/// (`vpc-endpoint-<opaque-id>.aos.<region>.on.aws`), and the label is the IPv4 host's
/// because the version 2 endpoint IS that endpoint: "it functions like a normal endpoint,
/// except that it works with both IPv4 and IPv6 IP addresses"
/// (`DomainStatus$EndpointV2`). The one place that knows that domain, so the host a
/// VPC-access domain publishes in its map and the one a public domain publishes as its
/// `EndpointV2` cannot be built differently.
pub(super) fn dual_stack_host(label: &str, region: &str) -> String {
    format!("{label}.aos.{region}.on.aws")
}

/// The connection endpoint a service-managed VPC endpoint is reached on.
///
/// The same host shape as [`domain_endpoint`] — the domain, an opaque per-endpoint
/// suffix, the Region and this service's endpoint prefix — behind the `vpc-` prefix
/// the service gives an endpoint that proxies into a VPC rather than the `search-`
/// one a domain's own endpoint carries, so the two are never mistaken for each other.
pub fn vpc_endpoint_host(domain_name: &str, suffix: &str, region: &str) -> String {
    format!("vpc-{domain_name}-{suffix}.{region}.{ARN_NAMESPACE}.amazonaws.com")
}

/// The endpoint a request REACHED this emulator at when that endpoint is the EMULATOR'S
/// OWN — a loopback address, an IP literal or any other host that names no service — and
/// `None` when the host names a service, which is a client that resolves AWS's own hosts.
///
/// Both readings it composes are `services/common`'s, so no second rule for "which host is
/// a service's" or "which host did this request address" is written here:
/// `data_plane_addressing::addressed_host` (the `Host` header, else an absolute URI's
/// authority) and `data_plane_addressing::names_a_service`, which defers to the front
/// door's own host reader.
///
/// What it decides is the ADDRESS a domain publishes for its data plane
/// ([`announced_domain_endpoint`]): a caller that reached this emulator at its own endpoint
/// resolves nothing else, so an AWS-shaped host told to it names no address it can reach.
pub fn local_endpoint_host(parts: &::http::request::Parts) -> Option<String> {
    let host = ::service_common::data_plane_addressing::addressed_host(
        &parts.uri.to_string(),
        parts
            .headers
            .get(::http::header::HOST)
            .and_then(|value| value.to_str().ok()),
    )?;
    (!::service_common::data_plane_addressing::names_a_service(&host)).then_some(host)
}

/// The address a domain of this service PUBLISHES for index, search and data-upload
/// requests, as announced to a caller that reached this emulator at `local_endpoint`: that
/// same endpoint, with the reserved path this service's data plane serves the domain at
/// (`<endpoint>/_aws/es/<domain>`, the addressing `crate::service::domain_rest` resolves).
///
/// `aws_host` is the host AWS publishes for that domain ([`domain_endpoint`] and the
/// VPC-access hosts beside it), which is what a caller that resolves AWS's own hosts is
/// told — so one reading decides every endpoint member of a domain's status, and a member
/// that names an address the caller cannot reach is never published.
pub fn announced_domain_endpoint(
    aws_host: &str,
    domain_name: &str,
    local_endpoint: Option<&str>,
) -> String {
    match local_endpoint {
        Some(endpoint) => format!(
            "{endpoint}/{}/{ARN_NAMESPACE}/{domain_name}",
            ::service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE
        ),
        None => aws_host.to_string(),
    }
}
