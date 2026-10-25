// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-groundstation-groundstation service.
//!
//! Every resource this service hands out lives here: configs keyed by their
//! `(configType, configId)` pair (the two labels `GET /config/{configType}/{configId}`
//! carries), mission profiles, contacts, ephemerides and AWS Ground Station Agents keyed
//! by their uuid, plus one ARN index so a `trackingConfigArn` / `missionProfileArn`
//! reference resolves without scanning a map.
//!
//! Beside them are the facts about the account and the ground station network that AWS
//! establishes OUTSIDE this API, which the model therefore only reports: the uplink
//! approval, the reserved-minutes plan, the on-boarded satellites and the antennas AWS has
//! taken out of service for maintenance ([`MaintenanceReservation`]). Each is read from an
//! operator setting when this store is created.
//!
//! The helpers beside the struct are the pieces every handler of this service needs —
//! the request's region, the union-member-to-`ConfigCapabilityType` mapping, the
//! capability's user-facing spelling, the `ConfigArn` reader, uuid minting and the
//! opaque pagination token codec — so they are written once here rather than in each
//! `src/ops/<operation>.rs`.
//!
//! Error TEXT, on the other hand, is per operation: this service answers a
//! `ResourceNotFoundException` with the text of the operation that raised it, so each
//! `src/ops/<operation>.rs` carries its own (see [`MISSION_PROFILE_NOT_FOUND`] for the
//! one literal two operations genuinely share).

use std::collections::hash_map::RandomState;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    AgentDetails, AggregateStatus, ComponentStatusData, ConfigTypeData, ContactVersion,
    AzElSegmentsData, DiscoveryData, Eirp, EndpointDetails, EphemerisData, EphemerisMetaData,
    Frequency, FrequencyBandwidth, InvalidParameterException, KmsKey, S3Object, SpectrumConfig,
    TrackingOverrides,
};

/// Region used in minted ARNs when the request carries no usable SigV4 credential
/// scope (an unsigned request still routes by endpoint host, so this is reachable).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN namespace (and sigv4 signing name) of this service, per `OPERATIONS.md`.
pub const ARN_SERVICE: &str = "groundstation";

/// `ReserveContact` admits a contact in `SCHEDULING`; the scheduler moves it to
/// `SCHEDULED` on its own. CHOSEN span (AWS documents no duration for it), inside the
/// 250–750 ms band this project gives a clock-owed transition, so a caller that reads
/// the contact straight after reserving it still observes the transient status.
pub const SCHEDULING_SETTLES_AFTER: Duration = Duration::from_millis(300);

/// `CreateDataflowEndpointGroup` admits a dataflow endpoint in `creating`; the service
/// finishes attaching it and it becomes `created` on its own. CHOSEN span (AWS documents
/// no duration for it — the real service takes minutes to place the stream's network
/// interfaces), inside the 250–750 ms band this project gives a clock-owed transition, so
/// a caller that reads the group straight after creating it still observes `creating`.
pub const ENDPOINT_CREATION_SETTLES_AFTER: Duration = Duration::from_millis(300);

/// `UpdateContact` appends a contact version in `UPDATING` ("the version is being applied
/// to the contact" — `smithy_query(query="shape", shape="VersionStatus")`); the service
/// applies it and it becomes `ACTIVE` on its own. CHOSEN span (AWS documents no duration
/// for it), inside the 250–750 ms band this project gives a clock-owed transition, so a
/// caller that reads the contact straight after updating it still observes the version
/// being applied and the configuration still in force.
pub const VERSION_ACTIVATES_AFTER: Duration = Duration::from_millis(300);

/// `CancelContact` on a contact whose pass is already under way STOPS it: the contact
/// reports `CANCELLING` while the stop is being carried out and reaches `CANCELLED` on its
/// own. CHOSEN span (AWS documents no duration for it), inside the 250–750 ms band this
/// project gives a clock-owed transition, so a caller that reads the contact straight after
/// stopping it still observes the release under way.
pub const CANCELLATION_SETTLES_AFTER: Duration = Duration::from_millis(300);

/// `CreateEphemeris` admits an ephemeris in `VALIDATING`; the service validates the
/// supplied data and the ephemeris reaches `ENABLED`, `DISABLED` or `INVALID` on its own.
/// CHOSEN span (AWS documents no duration for it — the real service takes up to minutes to
/// validate an ephemeris), inside the 250–750 ms band this project gives a clock-owed
/// transition, so a caller that lists the ephemeris straight after creating it still
/// observes `VALIDATING`.
pub const EPHEMERIS_VALIDATION_SETTLES_AFTER: Duration = Duration::from_millis(300);

/// Which `ConfigCapabilityType` a `ConfigTypeData` union member selects.
///
/// The config's type is NOT a request field: it is implied by whichever member of the
/// union the caller set, and it is what both the `configArn`'s `config/<type>/<id>`
/// segment and the `{configType}` label of `GetConfig` carry. The eight pairs are the
/// model's own: `smithy_query(query="shape", shape="ConfigCapabilityType")` lists the
/// enum values and `ConfigTypeData` the members.
pub fn config_type_of(config_data: &ConfigTypeData) -> &'static str {
    match config_data {
        ConfigTypeData::antennaDownlinkConfig(_) => "antenna-downlink",
        ConfigTypeData::antennaDownlinkDemodDecodeConfig(_) => "antenna-downlink-demod-decode",
        ConfigTypeData::antennaUplinkConfig(_) => "antenna-uplink",
        ConfigTypeData::dataflowEndpointConfig(_) => "dataflow-endpoint",
        ConfigTypeData::s3RecordingConfig(_) => "s3-recording",
        ConfigTypeData::telemetrySinkConfig(_) => "telemetry-sink",
        ConfigTypeData::trackingConfig(_) => "tracking",
        ConfigTypeData::uplinkEchoConfig(_) => "uplink-echo",
    }
}

/// The error code this service names when its request validator refuses a request that
/// failed the model's own rules: a missing required member, or a value outside a
/// declared bound. Recorded against AWS, which answers HTTP 400 with
/// `x-amzn-errortype: BadRequestException` — NOT the `ValidationException` the framework
/// defaults to, which this service's model does not declare for any operation
/// (`smithy_query(query="service_info")` lists its common errors).
pub const BAD_REQUEST_ERROR_CODE: &str = "BadRequestException";

/// The message that goes with [`BAD_REQUEST_ERROR_CODE`]: one fixed sentence for the
/// whole class, not a rendering of the constraint that failed.
pub const INVALID_REQUEST_BODY: &str = "Invalid request body";

/// The frequency bands an uplink `Config` may be tuned to, in MHz, as closed ranges.
///
/// Source: the model's own documentation of `UplinkSpectrumConfig$centerFrequency` —
/// "Center frequency of an uplink spectral Config. Valid values are between 2025 to
/// 2120 MHz" (`smithy_query(query="doc", shape="UplinkSpectrumConfig",
/// member="centerFrequency")`). A frequency outside them is not one the service will
/// radiate on, so a `Config` naming it is refused rather than stored. It is the band
/// the SERVICE operates; which part of it a given account may transmit on is
/// [`ComAmazonawsGroundstationGroundstationState::approved_uplink_bands_mhz`].
pub const UPLINK_CENTER_FREQUENCY_BANDS_MHZ: &[(f64, f64)] = &[(2025.0, 2120.0)];

/// The frequency bands a downlink `Config` may be tuned to, in MHz.
///
/// Source: `SpectrumConfig$centerFrequency`'s documentation — "Valid values are between
/// 2200 to 2300 MHz and 7750 to 8400 MHz for downlink and 2025 to 2120 MHz for uplink"
/// (`smithy_query(query="doc", shape="SpectrumConfig", member="centerFrequency")`).
/// `SpectrumConfig` is the DOWNLINK spectrum shape — the uplink one is
/// `UplinkSpectrumConfig` above — so the two downlink bands are the ones that apply.
pub const DOWNLINK_CENTER_FREQUENCY_BANDS_MHZ: &[(f64, f64)] = &[(2200.0, 2300.0), (7750.0, 8400.0)];

/// The bandwidth an `antennaDownlinkConfig` may ask for, in MHz.
///
/// Source: `FrequencyBandwidth$value`'s documentation — "For AntennaDownlinkconfig,
/// valid values are between 10 kHz to 54 MHz" (`smithy_query(query="doc",
/// shape="FrequencyBandwidth", member="value")`), with the lower bound converted to the
/// MHz every bound here is expressed in.
pub const ANTENNA_DOWNLINK_BANDWIDTH_MHZ: (f64, f64) = (0.01, 54.0);

/// The bandwidth an `antennaDownlinkDemodDecodeConfig` may ask for, in MHz.
///
/// Source: the same `FrequencyBandwidth$value` documentation — "For
/// AntennaDownlinkDemodDecodeconfig, valid values are between 125 kHz to 650 MHz".
pub const DEMOD_DECODE_BANDWIDTH_MHZ: (f64, f64) = (0.125, 650.0);

/// The target EIRP an `antennaUplinkConfig` may ask for, in dBW.
///
/// Source: `Eirp$value`'s documentation — "Value of an EIRP. Valid values are between
/// 20.0 to 50.0 dBW" (`smithy_query(query="doc", shape="Eirp", member="value")`).
pub const TARGET_EIRP_DBW: (f64, f64) = (20.0, 50.0);

/// What the service answers for an uplink center frequency this account may not
/// transmit on: its fixed refusal text for this one condition, as recorded against AWS.
/// The model documents the error (`InvalidParameterException`, HTTP 431) but not the
/// sentence, so the status and the error code are the contract and this text is the
/// wording that goes with them.
pub const UNAPPROVED_UPLINK_FREQUENCY: &str =
    "The specified Uplink Frequency has not been approved.";

/// The values `FrequencyUnits` and `BandwidthUnits` accept (`smithy_query(query="shape",
/// shape="FrequencyUnits")` — `GHz`, `MHz`, `kHz`).
///
/// A token outside them never reaches a handler: it fails the model's own value set, so
/// the boundary validator refuses the request with this service's 400
/// `BadRequestException` (pinned by
/// `tests/all/request_validation.rs::a_unit_outside_the_modelled_enum_is_refused_before_the_handler`).
/// That is why [`in_megahertz`] needs no refusal of its own — a unit it cannot name is a
/// value in no band, which is how the checks below treat it.
pub const FREQUENCY_UNITS: &[&str] = &["GHz", "MHz", "kHz"];

/// The one value `EirpUnits` accepts (`smithy_query(query="shape", shape="EirpUnits")`),
/// refused outside it the same way as [`FREQUENCY_UNITS`].
pub const EIRP_UNITS: &[&str] = &["dBW"];

/// How far outside a documented bound a value may land and still be inside it: one
/// millihertz, expressed in MHz.
///
/// A bound is a quantity, not a bit pattern, and the same quantity written in another
/// unit converts through a binary multiply — `0.054 GHz` is the 54 MHz ceiling but
/// lands an ulp past it. The tolerance is far below `kHz`, the finest unit the model
/// offers, so it cannot widen a bound into a value a caller could use.
const BOUND_TOLERANCE_MHZ: f64 = 1e-9;

/// `value` expressed in MHz, from one of the [`FREQUENCY_UNITS`]; `None` for any other
/// unit, which the boundary validator has already refused (see [`FREQUENCY_UNITS`]).
fn in_megahertz(value: f64, units: &str) -> Option<f64> {
    match units {
        "GHz" => Some(value * 1_000.0),
        "MHz" => Some(value),
        "kHz" => Some(value / 1_000.0),
        _ => None,
    }
}

/// `value` as the request wrote it, unit and all: `2024.9 MHz`.
///
/// The refusal quotes the request rather than the converted number, so a caller reading
/// it sees the value it sent beside the bound it missed.
fn as_written(value: f64, units: &str) -> String {
    format!("{value} {units}")
}

/// The modelled `InvalidParameterException` (HTTP 431) naming the member at fault.
fn invalid_parameter(parameter_name: &str, message: String) -> InvalidParameterException {
    InvalidParameterException {
        message: Some(message),
        parameter_name: Some(parameter_name.to_string()),
    }
}

/// `bands` as a sentence fragment: `2200 to 2300 MHz or 7750 to 8400 MHz`.
fn render_bands(bands: &[(f64, f64)]) -> String {
    bands
        .iter()
        .map(|(low, high)| format!("{low} to {high} MHz"))
        .collect::<Vec<_>>()
        .join(" or ")
}

/// Whether `megahertz` falls inside one of `bands`, each a closed range.
fn in_any_band(megahertz: f64, bands: &[(f64, f64)]) -> bool {
    bands.iter().any(|(low, high)| {
        megahertz >= low - BOUND_TOLERANCE_MHZ && megahertz <= high + BOUND_TOLERANCE_MHZ
    })
}

/// Check a center frequency against the bands its link direction allows, wording the
/// refusal from the request and the documented bound: the member, the value as written,
/// and the range it fell outside.
fn check_center_frequency(
    path: &str,
    frequency: &Frequency,
    bands: &[(f64, f64)],
) -> Result<(), InvalidParameterException> {
    if in_megahertz(frequency.value, &frequency.units)
        .is_some_and(|megahertz| in_any_band(megahertz, bands))
    {
        return Ok(());
    }
    Err(invalid_parameter(
        path,
        format!(
            "Value '{}' at '{path}' is outside the valid range of {}.",
            as_written(frequency.value, &frequency.units),
            render_bands(bands)
        ),
    ))
}

/// Check a bandwidth against the closed range its config type allows.
fn check_bandwidth(
    path: &str,
    bandwidth: &FrequencyBandwidth,
    bound: (f64, f64),
) -> Result<(), InvalidParameterException> {
    if in_megahertz(bandwidth.value, &bandwidth.units)
        .is_some_and(|megahertz| in_any_band(megahertz, &[bound]))
    {
        return Ok(());
    }
    let (low, high) = bound;
    Err(invalid_parameter(
        path,
        format!(
            "Value '{}' at '{path}' is outside the valid range of {low} to {high} MHz.",
            as_written(bandwidth.value, &bandwidth.units)
        ),
    ))
}

/// Check a downlink spectrum: its center frequency against the downlink bands, its
/// bandwidth against the range the owning config type allows.
fn check_downlink_spectrum(
    path: &str,
    spectrum: &SpectrumConfig,
    bandwidth: (f64, f64),
) -> Result<(), InvalidParameterException> {
    check_center_frequency(
        &format!("{path}.centerFrequency"),
        &spectrum.center_frequency,
        DOWNLINK_CENTER_FREQUENCY_BANDS_MHZ,
    )?;
    check_bandwidth(&format!("{path}.bandwidth"), &spectrum.bandwidth, bandwidth)
}

/// Check an uplink center frequency against what this account may transmit on: inside
/// the band the service operates, AND inside one of the bands the account is approved
/// for. Both are the same refusal to a caller — the frequency is not one this account
/// may uplink on — so both answer the service's one fixed sentence for it.
fn check_uplink_center_frequency(
    path: &str,
    frequency: &Frequency,
    approved_bands: &[(f64, f64)],
) -> Result<(), InvalidParameterException> {
    let megahertz = in_megahertz(frequency.value, &frequency.units);
    if megahertz.is_some_and(|megahertz| {
        in_any_band(megahertz, UPLINK_CENTER_FREQUENCY_BANDS_MHZ)
            && in_any_band(megahertz, approved_bands)
    }) {
        return Ok(());
    }
    Err(invalid_parameter(
        path,
        UNAPPROVED_UPLINK_FREQUENCY.to_string(),
    ))
}

/// Check a target EIRP against [`TARGET_EIRP_DBW`]. A unit outside [`EIRP_UNITS`] is a
/// value in no range, for the reason given on that constant.
fn check_target_eirp(path: &str, eirp: &Eirp) -> Result<(), InvalidParameterException> {
    let (low, high) = TARGET_EIRP_DBW;
    if EIRP_UNITS.contains(&eirp.units.as_str()) && eirp.value >= low && eirp.value <= high {
        return Ok(());
    }
    Err(invalid_parameter(
        &format!("{path}.value"),
        format!(
            "Value '{}' at '{path}.value' is outside the valid range of \
             {low} to {high} dBW.",
            as_written(eirp.value, &eirp.units)
        ),
    ))
}

/// Refuse a `ConfigTypeData` the service would not accept: a numeric member outside the
/// range the model documents for it, or an uplink frequency `approved_uplink_bands` does
/// not cover. The refusal is the service's modelled `InvalidParameterException` (HTTP
/// 431), naming the member at fault.
///
/// Every bound is the model's own, cited on its constant above, and the value is
/// compared after conversion from whichever `FrequencyUnits` the caller wrote it in —
/// `2.05 GHz`, `2050 MHz` and `2050000 kHz` are the same frequency and are judged alike.
/// The union members with no documented numeric range (`trackingConfig`,
/// `dataflowEndpointConfig`, `uplinkEchoConfig`, `s3RecordingConfig`,
/// `telemetrySinkConfig`) carry no bound for this to check: what they carry are
/// references, resolved elsewhere — `uplinkEchoConfig`'s against this service's own
/// store, `s3RecordingConfig`'s bucket by [`check_recording_bucket`].
fn validate_config_data(
    config_data: &ConfigTypeData,
    approved_uplink_bands: &[(f64, f64)],
) -> Result<(), InvalidParameterException> {
    match config_data {
        ConfigTypeData::antennaUplinkConfig(uplink) => {
            check_uplink_center_frequency(
                "configData.antennaUplinkConfig.spectrumConfig.centerFrequency",
                &uplink.spectrum_config.center_frequency,
                approved_uplink_bands,
            )?;
            check_target_eirp(
                "configData.antennaUplinkConfig.targetEirp",
                &uplink.target_eirp,
            )
        }
        ConfigTypeData::antennaDownlinkConfig(downlink) => check_downlink_spectrum(
            "configData.antennaDownlinkConfig.spectrumConfig",
            &downlink.spectrum_config,
            ANTENNA_DOWNLINK_BANDWIDTH_MHZ,
        ),
        ConfigTypeData::antennaDownlinkDemodDecodeConfig(downlink) => check_downlink_spectrum(
            "configData.antennaDownlinkDemodDecodeConfig.spectrumConfig",
            &downlink.spectrum_config,
            DEMOD_DECODE_BANDWIDTH_MHZ,
        ),
        ConfigTypeData::trackingConfig(_)
        | ConfigTypeData::dataflowEndpointConfig(_)
        | ConfigTypeData::uplinkEchoConfig(_)
        | ConfigTypeData::s3RecordingConfig(_)
        | ConfigTypeData::telemetrySinkConfig(_) => Ok(()),
    }
}

/// The value the model gives `SpectrumConfig$polarization` when a request omits it.
///
/// Source: `smithy_query(query="shape", shape="SpectrumConfig")` — the member targets
/// `Polarization` (values `RIGHT_HAND`, `LEFT_HAND`, `NONE`) and carries
/// `default: "NONE"`, with `addedDefault` and `clientOptional`.
pub const SPECTRUM_CONFIG_POLARIZATION_DEFAULT: &str = "NONE";

/// Materialise the member defaults the model declares, on a `configData` about to be
/// stored.
///
/// A member the model gives a `default` is one the SERVICE always has a value for: a
/// request that leaves it out is stored carrying that default, so every later read
/// answers the member populated and a client that round-trips a resource back through a
/// create or an update loses nothing. An explicitly supplied value is never touched.
///
/// The one member with a modelled default anywhere in a `Config`'s shape closure is
/// `SpectrumConfig$polarization` (`smithy_query(query="closure",
/// operation="CreateConfig")` — no other member of the closure carries `default`), and
/// it is applied wherever that shape is reached: `antennaDownlinkConfig.spectrumConfig`
/// and `antennaDownlinkDemodDecodeConfig.spectrumConfig`. The spectrum shape reached
/// through `antennaUplinkConfig` is `UplinkSpectrumConfig`, whose own `polarization`
/// carries NO default, so an omitted uplink polarization stays absent — the default
/// materialised here is the model's, not this service's guess.
pub fn apply_modelled_defaults(config_data: &mut ConfigTypeData) {
    let spectrum: Option<&mut SpectrumConfig> = match config_data {
        ConfigTypeData::antennaDownlinkConfig(downlink) => Some(&mut downlink.spectrum_config),
        ConfigTypeData::antennaDownlinkDemodDecodeConfig(downlink) => {
            Some(&mut downlink.spectrum_config)
        }
        ConfigTypeData::antennaUplinkConfig(_)
        | ConfigTypeData::trackingConfig(_)
        | ConfigTypeData::dataflowEndpointConfig(_)
        | ConfigTypeData::uplinkEchoConfig(_)
        | ConfigTypeData::s3RecordingConfig(_)
        | ConfigTypeData::telemetrySinkConfig(_) => None,
    };
    if let Some(spectrum) = spectrum {
        if spectrum.polarization.is_none() {
            spectrum.polarization = Some(SPECTRUM_CONFIG_POLARIZATION_DEFAULT.to_string());
        }
    }
}

/// The bucket name an S3 BUCKET ARN names, or `None` when the string is not one.
///
/// An S3 bucket ARN is `arn:<partition>:s3:::<bucket>`: the service segment is `s3` and
/// the region and account segments are empty, because a bucket name is global rather
/// than regional or per account (the model's `BucketArn` carries an `arnReference` to
/// `AWS::S3::Bucket` and no pattern, so the shape of the ARN is checked here). The
/// bucket is the first path segment of the resource part, so an ARN that carries a key
/// or a prefix after it still names its bucket — and an ARN of any OTHER service names
/// no bucket at all, however its resource part is spelled.
pub fn bucket_name_of(bucket_arn: &str) -> Option<&str> {
    let mut segments = bucket_arn.splitn(6, ':');
    if segments.next()? != "arn" {
        return None;
    }
    let _partition = segments.next()?;
    if segments.next()? != "s3" {
        return None;
    }
    // Region and account: empty on a bucket ARN. An `arn:aws:s3:us-east-1:123…:…` is
    // some other S3 resource (an access point, say), not a bucket.
    if !segments.next()?.is_empty() || !segments.next()?.is_empty() {
        return None;
    }
    let resource = segments.next()?;
    let name = resource.split('/').next().unwrap_or(resource);
    (!name.is_empty()).then_some(name)
}

/// What `s3RecordingConfig`'s bucket resolution answers when the `bucketArn` names no
/// bucket: the service's fixed refusal text for this condition, as recorded against AWS.
/// As with every other refusal here the status (431) and the error code
/// (`InvalidParameterException`) are the contract, and this is the wording that goes
/// with them.
pub const RECORDING_BUCKET_DOES_NOT_EXIST: &str =
    "Unable to create new S3 endpoint config - bucket does not exist.";

/// Refuse a `configData` whose recording bucket does not exist — and only then.
///
/// An `s3RecordingConfig` names the bucket to record to by ARN, and Ground Station
/// resolves that bucket before it will store the config: a bucket S3 reports this account
/// does not hold, and a `bucketArn` that names no bucket at all, are refused with the
/// modelled `InvalidParameterException` (HTTP 431) instead of being stored, so a caller
/// never holds a recording config that cannot record. A bucket that EXISTS is accepted.
///
/// S3 is reached the way the framework documents a handler reaching another emulated
/// service ([`smithy_server_runtime::OperationContext::callback`]): an AWS SDK client
/// over the in-process callback's HTTP client and credentials, so this crate names no
/// other service crate and the bucket is resolved against the emulator's OWN S3 state.
/// The question is put with `HeadBucket`, the operation S3 documents for it ("You can use
/// this operation to determine if a bucket exists").
///
/// `None` means the create or update may proceed. That INCLUDES every answer that is not
/// S3 reporting the bucket absent — a router with no S3 mounted, or any other failure to
/// ask: nothing is then known about the bucket, and a refusal has to be grounded in what
/// S3 answered rather than assumed (worth a warning, because that is a deployment where
/// this gate does not run). Every other `ConfigTypeData` member names no bucket and is
/// nothing for this to resolve.
pub async fn unresolvable_recording_bucket(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    config_data: &ConfigTypeData,
) -> Option<InvalidParameterException> {
    let ConfigTypeData::s3RecordingConfig(recording) = config_data else {
        return None;
    };
    let refusal = || {
        invalid_parameter(
            "configData.s3RecordingConfig.bucketArn",
            RECORDING_BUCKET_DOES_NOT_EXIST.to_string(),
        )
    };
    // A value that is not an S3 bucket ARN names no bucket, and that is settled without
    // asking S3: there is nothing to ask about. An ARN of any other service names no
    // bucket either, however its resource part is spelled (see [`bucket_name_of`]).
    let Some(bucket) = bucket_name_of(&recording.bucket_arn) else {
        return Some(refusal());
    };
    let answer = recording_bucket_client(ctx, region)
        .head_bucket()
        .bucket(bucket)
        .send()
        .await;
    match answer {
        // S3 holds the bucket, so the config can record to it.
        Ok(_) => None,
        // S3 says this account holds no such bucket: the refusal is grounded.
        Err(error) if bucket_reported_absent(&error) => Some(refusal()),
        Err(error) => {
            ::tracing::warn!(
                bucket_arn = %recording.bucket_arn,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the recording config's bucket could not be resolved against S3"
            );
            None
        }
    }
}

/// The S3 client this service resolves a recording bucket through: the AWS SDK driven
/// over the emulator's own in-process transport.
fn recording_bucket_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        // The emulator serves one endpoint for every service, so the bucket travels in
        // the path rather than in a per-bucket host the router could not resolve.
        .force_path_style(true)
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client
        // is not given. The same goes for caching the identity: the credentials are
        // handed over directly.
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

/// Whether an S3 `HeadBucket` failure is S3 reporting that the bucket does not exist
/// (`NotFound`, the only error that operation models), as opposed to the call not
/// reaching S3 at all.
fn bucket_reported_absent(
    error: &::aws_sdk_s3::error::SdkError<
        ::aws_sdk_s3::operation::head_bucket::HeadBucketError,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
) -> bool {
    error
        .as_service_error()
        .is_some_and(::aws_sdk_s3::operation::head_bucket::HeadBucketError::is_not_found)
}

/// The capability's name as this service spells it in user-facing text: the
/// UpperCamelCase form of a `ConfigCapabilityType` token.
///
/// That form is not a second naming scheme — it is exactly the name of the shape the
/// matching `ConfigTypeData` member targets, minus its `Config` suffix:
/// `antenna-uplink` -> `AntennaUplinkConfig` -> `AntennaUplink`, `s3-recording` ->
/// `S3RecordingConfig` -> `S3Recording`, `antenna-downlink-demod-decode` ->
/// `AntennaDownlinkDemodDecodeConfig` -> `AntennaDownlinkDemodDecode`. Source:
/// `smithy_query(query="shape", shape="ConfigTypeData,ConfigCapabilityType")` lists the
/// eight member targets beside the eight tokens, and the two agree segment for segment —
/// so the spelling is DERIVED from the token rather than tabulated here, and holds for
/// every capability type including any the model gains later.
pub fn capability_display_name(config_type: &str) -> String {
    config_type
        .split('-')
        .map(|segment| {
            let mut characters = segment.chars();
            match characters.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + characters.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// The `(configType, configId)` pair a `ConfigArn` addresses, or `None` when `arn` is
/// not one of this service's config ARNs.
///
/// The modelled `ConfigArn` pattern ends in `config/<type>/<uuid>(/<suffix>)?` (it is on
/// `configArn` in `smithy_query(query="operation", operation="GetConfig")`), so the two
/// segments after `:config/` are the very pair [`ComAmazonawsGroundstationGroundstationState::configs`]
/// is keyed on — which is what lets an ARN-valued reference be reported by the id and
/// type it names rather than echoed back whole.
pub fn config_arn_parts(arn: &str) -> Option<(String, String)> {
    let (_, resource) = arn.split_once(":config/")?;
    let mut segments = resource.split('/');
    let config_type = segments.next()?;
    let config_id = segments.next()?;
    (!config_type.is_empty() && !config_id.is_empty())
        .then(|| (config_type.to_string(), config_id.to_string()))
}

/// The NODE of the config `arn` addresses, when the ARN names one: the optional segment
/// the modelled `ConfigArn` pattern allows after the config's uuid (`config/<type>/<uuid>`
/// followed by `(/.{1,256})?` — the pattern on `GetConfig`'s `configArn`).
///
/// A config can carry several outputs, and that trailing segment is how a reference picks
/// ONE of them: it is what an `antenna-downlink-demod-decode` edge endpoint names when it
/// is wired to a particular output node of that config, which is the `outputNode` a
/// contact's dataflow details report for it.
pub fn config_arn_node(arn: &str) -> Option<String> {
    let (_, resource) = arn.split_once(":config/")?;
    let mut segments = resource.splitn(3, '/');
    segments.next()?;
    segments.next()?;
    let node = segments.next()?;
    (!node.is_empty()).then(|| node.to_string())
}

/// Whether the `ConfigArn` `reference` addresses the config whose own ARN is
/// `config_arn`: that ARN itself, or that ARN followed by one of the config's NODES.
///
/// The modelled `ConfigArn` pattern ends in `config/<type>/<uuid>(/.{1,256})?`, so a
/// reference may address either the config as a whole or one named output of it (see
/// [`config_arn_node`]) — both are references to that one config, which is what makes
/// them resolve to it and what makes it in use.
pub fn addresses_config(reference: &str, config_arn: &str) -> bool {
    reference == config_arn
        || reference
            .strip_prefix(config_arn)
            .is_some_and(|node| node.starts_with('/'))
}

/// What this service answers when a mission profile a request names does not exist: a
/// fixed literal that discloses neither the id nor the ARN that was asked for.
///
/// It lives here rather than in one handler because BOTH operations that resolve a
/// mission profile answer it — `GetMissionProfile`, which resolves one by
/// `{missionProfileId}`, and `ReserveContact`, which resolves one by `missionProfileArn`
/// — so it is one text of the service, not one text per operation.
pub const MISSION_PROFILE_NOT_FOUND: &str = "MissionProfile not found";

/// The region to mint this request's ARNs in: the SigV4 credential scope's region,
/// which is the region the caller addressed, falling back to [`DEFAULT_REGION`].
///
/// A scope region that could not appear in an ARN (the model's `ConfigArn` pattern
/// allows `[-a-z0-9]{1,50}` there) falls back too, so a minted ARN always satisfies the
/// modelled pattern.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_arn_region(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `region` is spellable in the region segment of this service's ARNs
/// (`[-a-z0-9]{1,50}`, from the `ConfigArn`/`MissionProfileArn` patterns).
fn is_arn_region(region: &str) -> bool {
    !region.is_empty()
        && region.len() <= 50
        && region
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// How many ids this process has minted: the part of an id that makes it UNIQUE.
///
/// A counter cannot repeat within a process, so no two ids this service hands out can
/// collide however fast they are minted — which a clock reading alone cannot promise.
static IDS_MINTED: AtomicU64 = AtomicU64::new(0);

/// A per-process random value, the part of an id that makes it UNGUESSABLE.
///
/// `RandomState` seeds each of its hashers with keys the standard library draws from the
/// OS once per process, so hashing a fixed value through one yields a value that differs
/// between runs and is not derivable from the request.
fn process_seed() -> u64 {
    static SEED: OnceLock<u64> = OnceLock::new();
    *SEED.get_or_init(|| {
        let mut hasher = ::std::hash::BuildHasher::build_hasher(&RandomState::new());
        ::std::hash::Hash::hash(&"com.amazonaws.groundstation", &mut hasher);
        ::std::hash::Hasher::finish(&hasher)
    })
}

/// A fresh lowercase, hyphenated uuid — the `Uuid` shape every id member of this
/// service targets (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`),
/// in the version-4 layout (version nibble `4`, variant bits `10`).
///
/// The 16 bytes are a SHA-256 digest — from the maintained `sha2` crate, not written
/// here — of this process's random seed, the number of ids minted before this one and
/// the current clock reading, rendered by `hex`. The counter is what makes an id unique
/// and the seed is what makes it unguessable; digesting them is what spreads both across
/// all 16 bytes, so an id reveals neither.
///
/// It does NOT use the `uuid` crate, which would otherwise be the obvious choice: taking
/// a new dependency rewrites the workspace `Cargo.lock`, the one file every concurrently
/// landing change in this repository also rewrites, and this change has to merge beside
/// them. `sha2` and `hex` are already in this crate's dependency closure, so minting the
/// id from them leaves the lockfile untouched.
pub fn new_uuid() -> String {
    use ::sha2::Digest as _;

    let minted = IDS_MINTED.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());

    let mut digest = ::sha2::Sha256::new();
    digest.update(process_seed().to_be_bytes());
    digest.update(minted.to_be_bytes());
    digest.update(nanos.to_be_bytes());
    let mut bytes = <[u8; 16]>::try_from(&digest.finalize()[..16]).expect("16 of 32 digest bytes");

    // The version-4 layout: `4` in the high nibble of byte 6, `10` in the top two bits
    // of byte 8. Everything else stays as digested.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    let hex = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// `now` as a Smithy timestamp.
pub fn now_timestamp() -> DateTime {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64());
    DateTime::from_secs_f64(secs)
}

/// `timestamp`, shifted by `seconds` (negative shifts earlier).
pub fn shift_seconds(timestamp: &DateTime, seconds: i64) -> DateTime {
    DateTime::from_secs_f64(timestamp.as_secs_f64() + seconds as f64)
}

/// The scope a listing's pagination tokens are issued under: the listing's own name,
/// plus a fingerprint of the filters the offset counts WITHIN when it has any.
///
/// An offset is only meaningful against the sequence it was taken from, so a token
/// carries its scope: a token from another listing, or from the same listing under
/// different filters, is then refused rather than silently resuming at an offset into a
/// different sequence (skipping or repeating records). The fingerprint is a
/// non-cryptographic hash — it only has to change when the filters do.
///
/// `DefaultHasher`'s output is NOT stable across process runs or std versions, which is
/// fine and deliberate: a token only ever addresses an offset into this process's
/// in-memory store, so it is not meant to survive a restart (after one there is no
/// sequence left to resume), and a token from an earlier run is refused rather than
/// resumed.
pub fn page_scope(listing: &str, filters: &[String]) -> String {
    if filters.is_empty() {
        return listing.to_string();
    }
    let mut hasher = ::std::collections::hash_map::DefaultHasher::new();
    // Hashed as ONE string joined on US (0x1f), a byte none of this service's filter
    // values can contain (every one is an ARN, a ground-station name, an enum value or a
    // timestamp), so no two different filter sets can run together into the same input.
    ::std::hash::Hash::hash(&filters.join("\u{1f}"), &mut hasher);
    format!("{listing}.{:016x}", ::std::hash::Hasher::finish(&hasher))
}

/// A pagination token for "resume `scope` at item `offset`".
///
/// Spelled `<scope>.<n>`: every character is inside the model's `PaginationToken`
/// pattern (`^[A-Za-z0-9-/+_.=]+$`) and needs no escaping in the query string
/// `ListMissionProfiles` binds its `nextToken` to, and the shortest form clears the
/// shape's 3-character minimum.
pub fn encode_page_token(scope: &str, offset: usize) -> String {
    format!("{scope}.{offset}")
}

/// The offset a token issued by [`encode_page_token`] for `scope` names, or `None` when
/// the token was not issued by this service for that scope.
pub fn decode_page_token(scope: &str, token: &str) -> Option<usize> {
    token.strip_prefix(scope)?.strip_prefix('.')?.parse().ok()
}

/// Largest page any listing returns, and the ceiling on a caller's `maxResults`: the
/// modelled maximum of `PaginationMaxResults` (`smithy_query(query="shape",
/// shape="PaginationMaxResults")` — range 1..100). A request that names no page size
/// gets this bounded page and a `nextToken`, rather than the whole store in one answer.
pub const MAX_PAGE_SIZE: usize = 100;

/// Largest number of records this emulator keeps in any one of its stores.
///
/// NOT an AWS quota — AWS Ground Station's own limits are per-account and adjustable.
/// It is a bound on the shared in-process store, so a caller cannot grow the emulator's
/// memory without limit; a create that would pass it is refused with the service's
/// modelled `ResourceLimitExceededException` (HTTP 429, "Account limits for this
/// resource have been exceeded"), which is what the service answers when a create runs
/// past an account limit.
pub const MAX_RECORDS_PER_STORE: usize = 1_000;

/// One page of `items`, plus the token that resumes after it.
///
/// The page is at most [`MAX_PAGE_SIZE`] items whatever the caller asked for, so no
/// single request can draw the whole store out in one answer.
pub fn paginate<T: Clone>(
    scope: &str,
    items: &[T],
    max_results: Option<i32>,
    offset: usize,
) -> (Vec<T>, Option<String>) {
    let start = offset.min(items.len());
    let limit = match max_results {
        Some(max) if max > 0 => (max as usize).min(MAX_PAGE_SIZE),
        _ => MAX_PAGE_SIZE,
    };
    let end = start.saturating_add(limit).min(items.len());
    let page = items[start..end].to_vec();
    let next = (end < items.len()).then(|| encode_page_token(scope, end));
    (page, next)
}

/// What an ARN this service minted points at, so an ARN-valued reference resolves in
/// one lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceRef {
    /// A config, by the `(configType, configId)` pair its map is keyed on.
    Config {
        config_type: String,
        config_id: String,
    },
    /// A mission profile, by `missionProfileId`.
    MissionProfile { mission_profile_id: String },
    /// A contact, by `contactId`.
    Contact { contact_id: String },
    /// A dataflow endpoint group, by `dataflowEndpointGroupId`.
    DataflowEndpointGroup { dataflow_endpoint_group_id: String },
    /// An AWS Ground Station Agent, by `agentId`.
    Agent { agent_id: String },
    /// An ephemeris, by `ephemerisId`.
    Ephemeris { ephemeris_id: String },
}

/// A config as `CreateConfig` stored it.
#[derive(Debug, Clone)]
pub struct Config {
    pub config_id: String,
    pub config_arn: String,
    pub name: String,
    pub config_type: String,
    pub config_data: ConfigTypeData,
    pub tags: HashMap<String, String>,
}

/// A mission profile as `CreateMissionProfile` stored it.
#[derive(Debug, Clone)]
pub struct MissionProfile {
    pub mission_profile_id: String,
    pub mission_profile_arn: String,
    pub name: String,
    pub region: String,
    pub contact_pre_pass_duration_seconds: Option<i32>,
    pub contact_post_pass_duration_seconds: Option<i32>,
    pub minimum_viable_contact_duration_seconds: i32,
    pub dataflow_edges: Vec<Vec<String>>,
    pub tracking_config_arn: String,
    pub telemetry_sink_config_arn: Option<String>,
    pub tags: HashMap<String, String>,
    pub streams_kms_key: Option<KmsKey>,
    pub streams_kms_role: Option<String>,
}

impl MissionProfile {
    /// Whether this profile references the config `config_arn` names, through ANY member
    /// that carries a `ConfigArn`: its tracking config, its telemetry sink config, or
    /// either end of any of its dataflow edges.
    ///
    /// This is what makes the config "in use" for [`DeleteConfig`]'s referential
    /// integrity check, so the question is asked of the whole profile rather than of one
    /// member.
    ///
    /// [`DeleteConfig`]: crate::ops::delete_config
    pub fn references_config(&self, config_arn: &str) -> bool {
        // Each reference is matched with [`addresses_config`], so an edge wired to one
        // NODE of a config is as much a reference to it as one naming the config whole.
        addresses_config(&self.tracking_config_arn, config_arn)
            || self
                .telemetry_sink_config_arn
                .as_deref()
                .is_some_and(|arn| addresses_config(arn, config_arn))
            || self.dataflow_edges.iter().any(|edge| {
                edge.iter()
                    .any(|arn| addresses_config(arn, config_arn))
            })
    }
}

/// A contact as `ReserveContact` stored it, plus every version `UpdateContact` has
/// appended to it.
///
/// The satellite the contact is flown against and the tracking overrides the antenna
/// uses are NOT fields here: they belong to a VERSION of the contact
/// ([`ContactVersionRecord`]), and the contact is flown with whichever version is active
/// — [`Contact::satellite_arn`] and [`Contact::tracking_overrides`] read it. That keeps
/// one record of each value, rather than a contact-level copy that can drift from the
/// version that produced it.
#[derive(Debug, Clone)]
pub struct Contact {
    pub contact_id: String,
    pub mission_profile_arn: String,
    pub start_time: DateTime,
    pub end_time: DateTime,
    pub pre_pass_start_time: DateTime,
    pub post_pass_end_time: DateTime,
    pub ground_station: String,
    /// The antenna at that ground station this contact's pass is placed on, as
    /// [`antenna_name`] spells it. Assigned when the contact is reserved
    /// ([`ComAmazonawsGroundstationGroundstationState::free_antenna`]) and never changed
    /// afterwards — the antenna is what the reservation holds, and `UpdateContact` changes
    /// what the antenna does during the pass, not which antenna flies it. It is what
    /// `ListGroundStationReservations` reports as the reservation's `antennaName`.
    pub antenna_name: String,
    pub contact_status: String,
    pub region: String,
    pub tags: HashMap<String, String>,
    /// Every version of this contact, oldest first and never empty: `ReserveContact`
    /// mints version [`FIRST_VERSION_ID`] and each accepted `UpdateContact` appends the
    /// next one.
    pub versions: Vec<ContactVersionRecord>,
    /// When the reservation was accepted.
    pub created: DateTime,
    /// When the contact last changed.
    pub last_updated: DateTime,
    /// The instant the scheduler owes this contact its `SCHEDULING` -> `SCHEDULED`
    /// move; `None` once the move has been materialised or the contact left
    /// `SCHEDULING`.
    pub scheduling_settles_at: Option<SystemTime>,
    /// The instant the service owes this contact its `CANCELLING` -> `CANCELLED` move —
    /// the end of the stop a cancel of a pass already under way started; `None` when no
    /// stop is being carried out.
    pub cancellation_settles_at: Option<SystemTime>,
    /// Whether this contact's pass has already held the antenna on the account's behalf,
    /// so those minutes stay spent however the status moves on afterwards: stopping a
    /// pass under way lands the contact on `CANCELLED`, and a release cannot give back
    /// antenna time the pass has already used (see [`Contact::holds_antenna_time`]).
    pub antenna_time_spent: bool,
}

/// The model's `VersionStatus` values, whose full set is `UPDATING`, `ACTIVE`,
/// `SUPERSEDED` and `FAILED_TO_UPDATE` (`smithy_query(query="shape",
/// shape="VersionStatus")`).
///
/// A version this service holds carries one of the first three: it is `UPDATING` while
/// the service is applying it ("the version is being applied to the contact"), `ACTIVE`
/// once it is the one in force ("the current active version of the contact") and
/// `SUPERSEDED` once a newer version replaced it ("replaced by a newer version").
/// `FAILED_TO_UPDATE` is the status of an update this emulator never produces: nothing
/// here fails applying a version, so no version is ever left in it.
pub const ACTIVE_VERSION_STATUS: &str = "ACTIVE";

/// See [`ACTIVE_VERSION_STATUS`].
pub const UPDATING_VERSION_STATUS: &str = "UPDATING";

/// See [`ACTIVE_VERSION_STATUS`].
pub const SUPERSEDED_VERSION_STATUS: &str = "SUPERSEDED";

/// The version a reserved contact starts at, and the smallest `versionId` the model
/// admits (`VersionId`, range 1..128 — `smithy_query(query="shape",
/// shape="VersionId")`).
pub const FIRST_VERSION_ID: i32 = 1;

/// How many versions one contact can have: the LARGEST `versionId` the model admits
/// (`VersionId`, range 1..128), since versions are numbered from [`FIRST_VERSION_ID`]
/// without a gap. An `UpdateContact` that would need a version past it cannot be
/// answered with a `versionId` the model can carry, so it is refused with the modelled
/// `ResourceLimitExceededException`.
pub const MAX_CONTACT_VERSIONS: i32 = 128;

/// The `ContactStatus` values in which a contact still takes a new version.
///
/// `UpdateContact` changes what the antenna does during the pass — which satellite it
/// tracks (`satelliteArn`) and how (`trackingOverrides`) — so it can only be answered
/// while the pass is still ahead of the contact. Of the model's `ContactStatus` values
/// (`smithy_query(query="shape", shape="ContactStatus")`), these two are the ones before
/// a pass begins: from `PREPASS` on the ground station is already working the contact,
/// `CANCELLING` is a release under way, `CANCELLED`, `AWS_CANCELLED`, `COMPLETED`,
/// `FAILED`, `FAILED_TO_SCHEDULE` and `AWS_FAILED` are contacts that will never be
/// flown again, and `AVAILABLE` is a window that was offered and never reserved, so it
/// is not a contact this store holds.
///
/// CHOSEN gate: AWS documents `UpdateContact` only as "Updates a specific contact", so
/// which statuses still take an update is this emulator's rule, stated once here rather
/// than per operation.
pub const STATUSES_OPEN_TO_UPDATE: &[&str] = &["SCHEDULED", "SCHEDULING"];

/// The `ContactStatus` a released contact holds, and the one it reports while a release
/// of a pass already under way is being carried out — both the model's own
/// (`smithy_query(query="shape", shape="ContactStatus")`).
pub const CONTACT_CANCELLED: &str = "CANCELLED";

/// See [`CONTACT_CANCELLED`].
pub const CONTACT_CANCELLING: &str = "CANCELLING";

/// The `ContactStatus` a reservation is admitted in, and the one the scheduler moves it
/// to once the contact holds its place in the schedule — both the model's own
/// (`smithy_query(query="shape", shape="ContactStatus")`).
pub const CONTACT_SCHEDULING: &str = "SCHEDULING";

/// See [`CONTACT_SCHEDULING`].
pub const CONTACT_SCHEDULED: &str = "SCHEDULED";

/// The `ContactStatus` of a contact whose pre-pass period has begun, of one whose pass is
/// running, of one in its post-pass period and of one whose pass has finished — the
/// model's own values again, in the order a contact reaches them.
pub const CONTACT_PREPASS: &str = "PREPASS";

/// See [`CONTACT_PREPASS`].
pub const CONTACT_PASS: &str = "PASS";

/// See [`CONTACT_PREPASS`].
pub const CONTACT_POSTPASS: &str = "POSTPASS";

/// See [`CONTACT_PREPASS`].
pub const CONTACT_COMPLETED: &str = "COMPLETED";

/// The statuses a contact moves through as the clock crosses its OWN timestamps, in
/// lifecycle order; it is also the set of statuses that move along it at all.
///
/// Every boundary is a timestamp the contact already carries and every read already
/// reports: the pre-pass period starts at `prePassStartTime`, "at which you receive a
/// CloudWatch event indicating an upcoming pass", the pass itself runs `startTime` to
/// `endTime`, and the post-pass period ends at `postPassEndTime`, "at which you receive a
/// CloudWatch event indicating the pass has finished" (`smithy_query(query="operation",
/// operation="DescribeContact")`). The lifecycle is therefore owed by the contact's own
/// window rather than by a span this emulator chooses, and a contact that is somewhere
/// else entirely — still [`CONTACT_SCHEDULING`], being released, or already in one of
/// [`TERMINAL_CONTACT_STATUSES`] — is not on it and is not carried along it.
pub const CONTACT_PASS_LIFECYCLE: &[&str] = &[
    CONTACT_SCHEDULED,
    CONTACT_PREPASS,
    CONTACT_PASS,
    CONTACT_POSTPASS,
    CONTACT_COMPLETED,
];

/// How far along [`CONTACT_PASS_LIFECYCLE`] `status` stands, or `None` when it is not on
/// that lifecycle at all.
pub fn pass_lifecycle_position(status: &str) -> Option<usize> {
    CONTACT_PASS_LIFECYCLE
        .iter()
        .position(|stage| *stage == status)
}

/// The `ContactStatus` values in which a cancel releases a reservation whose pass has not
/// begun, so the contact is `CANCELLED` outright.
///
/// `CancelContact` "cancels or stops a contact with a specified contact ID based on its
/// position in the contact lifecycle. For contacts that: Have yet to start, the contact
/// will be cancelled" (`smithy_query(query="operation", operation="CancelContact")`).
/// These are the statuses of a contact whose pass has yet to start: the reservation is
/// held and the ground station has not begun working it.
pub const STATUSES_CANCELLABLE_BEFORE_PASS: &[&str] = &["SCHEDULING", "SCHEDULED"];

/// The `ContactStatus` values in which a cancel STOPS a contact whose pass is under way:
/// "[for contacts that] have started but have yet to finish, the contact will be stopped"
/// (same doc as [`STATUSES_CANCELLABLE_BEFORE_PASS`]). From `PREPASS` on the ground
/// station is already working the contact, and through `POSTPASS` it has not finished, so
/// a cancel here is a stop — reported as [`CONTACT_CANCELLING`] while it is carried out.
pub const STATUSES_CANCELLABLE_DURING_PASS: &[&str] = &["PREPASS", "PASS", "POSTPASS"];

/// The `ContactStatus` values a contact can no longer leave: its pass is over, it was
/// released, or it will never be flown.
///
/// Source: the model's own enum (`smithy_query(query="shape", shape="ContactStatus")`).
/// Of its thirteen values these six are the ends of the lifecycle — a contact released by
/// the customer or by AWS (`CANCELLED`, `AWS_CANCELLED`), one whose pass finished
/// (`COMPLETED`), one that failed or that the service itself failed (`FAILED`,
/// `AWS_FAILED`) and a reservation that never became a contact (`FAILED_TO_SCHEDULE`).
/// Every other value is a contact still going somewhere: `SCHEDULING` and `SCHEDULED`
/// have their pass ahead of them, `PREPASS`, `PASS` and `POSTPASS` are in it,
/// `CANCELLING` is a release under way, and `AVAILABLE` is a window still on offer.
///
/// It is what makes a contact NON-terminal, which is the state the model's
/// `ResourceInUseException` is about: "the specified resource is in use by non-terminal
/// state contacts and cannot be modified or deleted" (`smithy_query(query="shape",
/// shape="ResourceInUseException")`).
pub const TERMINAL_CONTACT_STATUSES: &[&str] = &[
    "AWS_CANCELLED",
    "AWS_FAILED",
    "CANCELLED",
    "COMPLETED",
    "FAILED",
    "FAILED_TO_SCHEDULE",
];

/// Every `ContactStatus` in which a cancel still has a reservation to release, in
/// lifecycle order, as a refusal naming them spells them.
pub fn cancellable_statuses() -> String {
    STATUSES_CANCELLABLE_BEFORE_PASS
        .iter()
        .chain(STATUSES_CANCELLABLE_DURING_PASS)
        .copied()
        .collect::<Vec<_>>()
        .join(", ")
}

/// One version of a contact: the configuration that version carries, and where the
/// version stands in the contact's history.
///
/// A contact is versioned because `UpdateContact` POSTs to
/// `/contact/{contactId}/versions` and answers a `versionId`
/// (`smithy_query(query="operation", operation="UpdateContact")`): an update does not
/// overwrite the contact's configuration, it appends a version which the service then
/// applies, superseding the version before it.
#[derive(Debug, Clone)]
pub struct ContactVersionRecord {
    pub version_id: i32,
    /// The satellite this version flies the contact against; `None` when neither this
    /// version nor the one it was based on named one.
    pub satellite_arn: Option<String>,
    /// The tracking overrides this version flies the contact with.
    pub tracking_overrides: Option<TrackingOverrides>,
    /// When the version was created ("time the contact version was created in UTC" —
    /// `ContactVersion$created`).
    pub created: DateTime,
    /// When it became the contact's active version ("a version is activated when it
    /// becomes the current active version of the contact"); `None` while it is still
    /// being applied, or if a newer version replaced it before it was.
    pub activated: Option<DateTime>,
    /// When a newer version replaced it ("a version is superseded when a newer version
    /// becomes active").
    pub superseded: Option<DateTime>,
    /// This version's [`ACTIVE_VERSION_STATUS`]-family status.
    pub status: String,
    /// The `clientToken` of the `UpdateContact` that appended this version, when it
    /// carried one. `clientToken` is `@idempotencyToken` on an `@idempotent` operation,
    /// so a retry carrying the same token answers with THIS version instead of
    /// appending another.
    pub client_token: Option<String>,
    /// The instant the service owes this version its `UPDATING` -> `ACTIVE` move; `None`
    /// once the move has been materialised, or for a version that was never pending.
    pub activates_at: Option<SystemTime>,
}

impl ContactVersionRecord {
    /// Version [`FIRST_VERSION_ID`], as `ReserveContact` mints it: the configuration the
    /// contact is reserved with is in force from the instant the reservation is
    /// accepted, so this version is `ACTIVE` and activated right away — nothing is still
    /// applying it and nothing precedes it.
    pub fn initial(
        created: DateTime,
        satellite_arn: Option<String>,
        tracking_overrides: Option<TrackingOverrides>,
    ) -> Self {
        Self {
            version_id: FIRST_VERSION_ID,
            satellite_arn,
            tracking_overrides,
            created,
            activated: Some(created),
            superseded: None,
            status: ACTIVE_VERSION_STATUS.to_string(),
            client_token: None,
            activates_at: None,
        }
    }
}

/// The `ContactStatus` values in which a contact holds NO antenna time for the account,
/// so its window counts no minutes against the account's monthly usage.
///
/// Source: the model's own enum, whose values are `SCHEDULING`, `FAILED_TO_SCHEDULE`,
/// `SCHEDULED`, `CANCELLED`, `AWS_CANCELLED`, `PREPASS`, `PASS`, `POSTPASS`, `COMPLETED`,
/// `FAILED`, `AVAILABLE`, `CANCELLING` and `AWS_FAILED` (`smithy_query(query="shape",
/// shape="ContactStatus")`). The five listed here are the ones where the antenna was
/// never held on the account's behalf: a window merely offered and never reserved
/// (`AVAILABLE`), a reservation that never became a contact (`FAILED_TO_SCHEDULE`), one
/// released before the pass by the customer or by AWS (`CANCELLED`, `AWS_CANCELLED`) and
/// one the service itself failed (`AWS_FAILED`). Every other value is a contact the
/// account holds or held the antenna for — including `CANCELLING`, whose release has not
/// completed, and `FAILED`, whose antenna time was still reserved.
///
/// A status in this list releases the minutes only when the antenna was never actually
/// held: a contact STOPPED mid-pass lands on `CANCELLED` with its antenna time already
/// spent, which [`Contact::antenna_time_spent`] records and [`Contact::holds_antenna_time`]
/// honours.
///
/// CHOSEN classification: the API documents the member only as "total scheduled minutes",
/// so which statuses release their minutes is this emulator's rule, applied in one place
/// rather than per operation.
pub const STATUSES_WITHOUT_ANTENNA_TIME: &[&str] = &[
    "AVAILABLE",
    "AWS_CANCELLED",
    "AWS_FAILED",
    "CANCELLED",
    "FAILED_TO_SCHEDULE",
];

impl Contact {
    /// Whether this contact holds (or held) antenna time for the account, so its minutes
    /// count against the usage of the month it starts in: its status is not one of
    /// [`STATUSES_WITHOUT_ANTENNA_TIME`], or its pass already spent the antenna time
    /// ([`Contact::antenna_time_spent`]) and no later status move gives it back.
    pub fn holds_antenna_time(&self) -> bool {
        self.antenna_time_spent
            || !STATUSES_WITHOUT_ANTENNA_TIME.contains(&self.contact_status.as_str())
    }

    /// Whether a cancel still has something to release here: the reservation is live and
    /// the pass has not finished, so the contact is either cancelled before its pass or
    /// stopped during it.
    ///
    /// Everything else holds no reservation a cancel could release — a contact already
    /// released (`CANCELLED`, `AWS_CANCELLED`), one whose pass is over or will never be
    /// flown (`COMPLETED`, `FAILED`, `FAILED_TO_SCHEDULE`, `AWS_FAILED`) and a window
    /// merely offered (`AVAILABLE`). `CANCELLING` is not here either, but for the
    /// opposite reason: a release is already under way, which `CancelContact` answers
    /// without starting a second one.
    pub fn is_cancellable(&self) -> bool {
        let status = self.contact_status.as_str();
        STATUSES_CANCELLABLE_BEFORE_PASS.contains(&status)
            || STATUSES_CANCELLABLE_DURING_PASS.contains(&status)
    }

    /// Whether this contact is still going somewhere: its status is not one of
    /// [`TERMINAL_CONTACT_STATUSES`], so what it is flown with is still a live dependency.
    pub fn is_non_terminal(&self) -> bool {
        !TERMINAL_CONTACT_STATUSES.contains(&self.contact_status.as_str())
    }

    /// Whether this contact is flown against the satellite `satellite_arn` names.
    ///
    /// The version in force answers it, and so does the version still being applied: an
    /// `UPDATING` version is the configuration the contact is about to be flown with, so a
    /// satellite either version names is one this contact depends on.
    pub fn flies_satellite(&self, satellite_arn: &str) -> bool {
        [self.active_version(), self.latest_version()]
            .into_iter()
            .flatten()
            .any(|version| version.satellite_arn.as_deref() == Some(satellite_arn))
    }

    /// Release this contact's reservation, from whichever side of its pass the cancel
    /// arrived on. Only called for a contact [`Contact::is_cancellable`] admits.
    ///
    /// A contact whose pass has yet to start is `CANCELLED` outright: there is nothing to
    /// stop, and the scheduler no longer owes it its `SCHEDULING` -> `SCHEDULED` move.
    /// One whose pass is under way is STOPPED instead — it reports [`CONTACT_CANCELLING`]
    /// while the stop is carried out and the service owes it its move to
    /// [`CONTACT_CANCELLED`] [`CANCELLATION_SETTLES_AFTER`] from now (materialised by
    /// `settle_due_contacts`). That pass already had the antenna on the account's behalf,
    /// so the stop also records the antenna time as spent: a release cannot give back
    /// time the pass has already used.
    pub fn begin_cancellation(&mut self, now: DateTime) {
        if STATUSES_CANCELLABLE_DURING_PASS.contains(&self.contact_status.as_str()) {
            self.antenna_time_spent = true;
            self.contact_status = CONTACT_CANCELLING.to_string();
            self.cancellation_settles_at = Some(SystemTime::now() + CANCELLATION_SETTLES_AFTER);
        } else {
            self.contact_status = CONTACT_CANCELLED.to_string();
            self.cancellation_settles_at = None;
        }
        self.scheduling_settles_at = None;
        self.last_updated = now;
    }

    /// The stage of its own pass the clock has carried this contact to at `now` (epoch
    /// seconds): [`CONTACT_SCHEDULED`] while its pre-pass period is still ahead,
    /// [`CONTACT_PREPASS`] from `prePassStartTime`, [`CONTACT_PASS`] from `startTime`,
    /// [`CONTACT_POSTPASS`] from `endTime` and [`CONTACT_COMPLETED`] from
    /// `postPassEndTime`.
    ///
    /// A boundary two of the contact's timestamps share is a stage of no duration, which
    /// the contact therefore never reports: a mission profile that widens the window by
    /// nothing gives the contact no pre-pass period, so such a contact goes from
    /// scheduled straight into its pass.
    pub fn pass_stage(&self, now: f64) -> &'static str {
        if now >= self.post_pass_end_time.as_secs_f64() {
            CONTACT_COMPLETED
        } else if now >= self.end_time.as_secs_f64() {
            CONTACT_POSTPASS
        } else if now >= self.start_time.as_secs_f64() {
            CONTACT_PASS
        } else if now >= self.pre_pass_start_time.as_secs_f64() {
            CONTACT_PREPASS
        } else {
            CONTACT_SCHEDULED
        }
    }

    /// Carry this contact to the stage of [`CONTACT_PASS_LIFECYCLE`] its own timestamps
    /// have already made due at `now` (epoch seconds), and answer whether that moved it.
    ///
    /// The move only ever goes FORWARD along the lifecycle, and only for a contact that is
    /// on it: a contact whose status the clock cannot explain — one still being scheduled
    /// or released, one already terminal, or a window merely offered — keeps the status it
    /// holds. A contact that has reached [`CONTACT_PREPASS`] or beyond has had the antenna
    /// on the account's behalf, so the move also records the antenna time as spent
    /// ([`Contact::antenna_time_spent`]), which no later status move gives back.
    pub fn advance_pass_stage(&mut self, now: f64) -> bool {
        let Some(reached) = pass_lifecycle_position(&self.contact_status) else {
            return false;
        };
        let stage = self.pass_stage(now);
        let Some(due) = pass_lifecycle_position(stage) else {
            return false;
        };
        if due <= reached {
            return false;
        }
        self.contact_status = stage.to_string();
        self.antenna_time_spent = true;
        true
    }

    /// The whole minutes of antenna time this contact is scheduled for: its own window,
    /// `startTime` to `endTime`, rounded UP to the next whole minute.
    ///
    /// The pre- and post-pass widening is deliberately not part of it — those spans are
    /// the set-up and tear-down around the pass, while the antenna time is the window the
    /// reservation itself names. CHOSEN rounding: minute usage is reported as an
    /// `Integer` of minutes (`smithy_query(query="operation",
    /// operation="GetMinuteUsage")`), and a part minute of a pass is still a minute the
    /// account has the antenna for, so it counts as a whole one.
    pub fn scheduled_minutes(&self) -> i32 {
        let seconds = (self.end_time.as_secs_f64() - self.start_time.as_secs_f64()).max(0.0);
        (seconds / 60.0).ceil().min(f64::from(i32::MAX)) as i32
    }

    /// The version in force: the newest one that has been activated, which is the
    /// configuration the contact is flown with. A version still being applied
    /// (`UPDATING`) is not in force yet, so it is not this one.
    pub fn active_version(&self) -> Option<&ContactVersionRecord> {
        self.versions
            .iter()
            .rev()
            .find(|version| version.status == ACTIVE_VERSION_STATUS)
    }

    /// The newest version of this contact, whatever its status — the one an update is
    /// based on and the one a `versionId` answer names.
    pub fn latest_version(&self) -> Option<&ContactVersionRecord> {
        self.versions.last()
    }

    /// The `versionId` this contact's operations answer with: its newest version's.
    pub fn latest_version_id(&self) -> i32 {
        self.latest_version()
            .map_or(FIRST_VERSION_ID, |version| version.version_id)
    }

    /// The satellite the contact is flown against: the active version's.
    ///
    /// The ACTIVE version, not the latest: a version still being applied is not in force
    /// yet, so a read reports the configuration the contact would fly with right now.
    /// [`Contact::append_version`] bases its merge on the LATEST version instead, so
    /// chained updates compose rather than each one starting from the version in force.
    pub fn satellite_arn(&self) -> Option<&str> {
        self.active_version()
            .and_then(|version| version.satellite_arn.as_deref())
    }

    /// The tracking overrides the contact is flown with: the active version's (see
    /// [`Contact::satellite_arn`] on active versus latest).
    pub fn tracking_overrides(&self) -> Option<&TrackingOverrides> {
        self.active_version()
            .and_then(|version| version.tracking_overrides.as_ref())
    }

    /// This contact's version record, as `DescribeContact` and `ListContacts` report
    /// it: its NEWEST version — the one in force, or the one being applied while an
    /// update settles, which is what a caller polls to watch an update land.
    ///
    /// INFALLIBLE, because a contact always has at least one version: a reservation
    /// mints [`FIRST_VERSION_ID`] and nothing removes a version. Were that invariant
    /// ever broken the answer falls back to the contact's own first version rather than
    /// reporting no version at all.
    pub fn version(&self) -> ContactVersion {
        match self.latest_version() {
            Some(record) => self.version_record(record),
            None => self.version_record(&ContactVersionRecord::initial(self.created, None, None)),
        }
    }

    /// `record` as a `ContactVersion` response member.
    pub fn version_record(&self, record: &ContactVersionRecord) -> ContactVersion {
        ContactVersion {
            version_id: Some(record.version_id),
            created: Some(record.created),
            activated: record.activated,
            superseded: record.superseded,
            // A version that has been superseded was last touched when it was
            // superseded; the newest version is the one every later change to the
            // contact lands on, so its lastUpdated is the contact's own.
            last_updated: Some(record.superseded.unwrap_or(self.last_updated)),
            status: Some(record.status.clone()),
            // Nothing in this emulator fails applying a version, so the members
            // reporting a failed update carry nothing.
            failure_codes: None,
            failure_message: None,
        }
    }

    /// This contact's whole version HISTORY, as `ListContactVersions` reports it: every
    /// version the store holds, oldest first.
    ///
    /// The stored sequence is append-only and each appended version takes the next
    /// `versionId` ([`Contact::append_version`]), so its own order IS ascending
    /// `versionId` — which is the order the listing pages over, and the order a version
    /// history reads in.
    pub fn version_history(&self) -> Vec<ContactVersion> {
        self.versions
            .iter()
            .map(|record| self.version_record(record))
            .collect()
    }

    /// Append a version carrying `satellite_arn` and `tracking_overrides`, and answer
    /// its `versionId`.
    ///
    /// A member the update does not name keeps the value the newest version already
    /// carries, so an update that names only one of them does not drop the other. The
    /// version is admitted `UPDATING`: the service owes it its move to `ACTIVE`
    /// [`VERSION_ACTIVATES_AFTER`] from now, which is when it takes force and supersedes
    /// the version before it (see [`Contact::apply_due_versions`]).
    pub fn append_version(
        &mut self,
        satellite_arn: Option<String>,
        tracking_overrides: Option<TrackingOverrides>,
        client_token: Option<String>,
        now: DateTime,
    ) -> i32 {
        let base = self.latest_version();
        let version_id = base.map_or(FIRST_VERSION_ID, |version| version.version_id + 1);
        let satellite_arn =
            satellite_arn.or_else(|| base.and_then(|version| version.satellite_arn.clone()));
        let tracking_overrides = tracking_overrides
            .or_else(|| base.and_then(|version| version.tracking_overrides.clone()));
        self.versions.push(ContactVersionRecord {
            version_id,
            satellite_arn,
            tracking_overrides,
            created: now,
            activated: None,
            superseded: None,
            status: UPDATING_VERSION_STATUS.to_string(),
            client_token,
            activates_at: Some(SystemTime::now() + VERSION_ACTIVATES_AFTER),
        });
        self.last_updated = now;
        version_id
    }

    /// The version `client_token` already appended to this contact, when that token has
    /// been seen: `clientToken` is `@idempotencyToken`, so a retry carrying it answers
    /// with the version it minted the first time rather than appending another.
    pub fn version_minted_by(&self, client_token: &str) -> Option<&ContactVersionRecord> {
        self.versions
            .iter()
            .find(|version| version.client_token.as_deref() == Some(client_token))
    }

    /// Materialise the `UPDATING` -> `ACTIVE` move of every version the clock has
    /// already made due: the newest due version takes force, every version before it is
    /// superseded, and the contact is flown with the new one from here on.
    ///
    /// The move was owed at this instant whether or not anybody called, so applying it
    /// on a read changes nothing a later caller could otherwise have seen.
    pub fn apply_due_versions(&mut self) {
        let now = SystemTime::now();
        let Some(activating) = self
            .versions
            .iter()
            .rposition(|version| version.activates_at.is_some_and(|due| due <= now))
        else {
            return;
        };

        let stamp = now_timestamp();
        for (position, version) in self.versions.iter_mut().enumerate() {
            if position == activating {
                version.status = ACTIVE_VERSION_STATUS.to_string();
                version.activated = Some(stamp);
                version.activates_at = None;
            } else if position < activating && version.superseded.is_none() {
                // Every older version is replaced — including one still pending, which
                // a newer version overtook before it could take force.
                version.status = SUPERSEDED_VERSION_STATUS.to_string();
                version.superseded = Some(stamp);
                version.activates_at = None;
            }
        }
        self.last_updated = stamp;
    }

    /// `tags` the way a response carries them: always the map, empty when the
    /// resource carries no tag.
    pub fn response_tags(&self) -> Option<HashMap<String, String>> {
        response_tags(&self.tags)
    }
}

/// `tags` the way a response of this service carries them: the resource's tag map,
/// SERIALIZED even when the resource carries no tag — `{}` then, never an omitted
/// member.
///
/// A `TagsMap`-targeting response member of this service reports how a resource is
/// tagged, and "no tags" is a map with no entry, not the absence of an answer: a client
/// that diffs or iterates tags reads a map in both cases. Every read that reports tags
/// goes through here, so the whole service answers the same way.
pub fn response_tags(tags: &HashMap<String, String>) -> Option<HashMap<String, String>> {
    Some(tags.clone())
}

/// The request member every tag map of this service arrives in, and the member a refusal
/// of one names — `tags` on each operation that takes a `TagsMap`
/// (`smithy_query(query="operation", operation="CreateConfig,CreateMissionProfile,\
/// CreateDataflowEndpointGroup,ReserveContact")`).
pub const TAGS_PARAMETER: &str = "tags";

/// The tag-key prefix AWS reserves for itself: a caller may neither set nor remove a key
/// that starts with it, on any AWS resource.
///
/// Source: the AWS tagging rules every taggable service shares ("the `aws:` prefix is
/// reserved for AWS use … you can't edit or delete a tag with the `aws:` prefix"). The
/// model cannot express it — `TagsMap` is a bare `map<String,String>` with no `length`
/// or `pattern` trait (`smithy_query(query="shape", shape="TagsMap")`) — so the rule
/// lives here, where every create of this service reaches it.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` is in the reserved [`RESERVED_TAG_KEY_PREFIX`] namespace, matched in
/// ANY letter case: the namespace is AWS's whichever way a caller spells it, so
/// `AWS:owner` is as reserved as `aws:owner`. The same reading the other services of
/// this emulator enforce, so one tag map is judged alike wherever it is sent.
pub fn is_reserved_tag_key(key: &str) -> bool {
    // `str::get` answers `None` rather than panicking when the key's first four bytes
    // are not a character boundary, so a key that cannot carry the prefix is simply
    // not one.
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// The inclusive length bounds of a tag KEY, in characters, from the same AWS tagging
/// rules: a key is 1 to 128 characters.
pub const TAG_KEY_CHARS: (usize, usize) = (1, 128);

/// The largest tag VALUE, in characters, from the same rules. A value may be empty, so
/// only the ceiling is a bound.
pub const MAX_TAG_VALUE_CHARS: usize = 256;

/// The most tags one resource may carry, from the same rules.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// Refuse a `tags` map AWS's platform-wide tagging rules do not allow, with the modelled
/// `InvalidParameterException` (HTTP 431) naming the [`TAGS_PARAMETER`] member.
///
/// `TagsMap` carries no constraint in the model, so the boundary validator lets every map
/// through and this is the only gate: a tag map is checked HERE, before any id, ARN or
/// record is minted, so a caller never holds a resource AWS would have refused to tag
/// that way. Every create of this service that accepts tags calls it, as does
/// `TagResource` (which additionally judges the MERGED map against the per-resource
/// bound — see [`validate_tag_merge`]), so one illegal map is refused identically
/// whichever resource and whichever operation it was sent to.
///
/// The conditions, each from a constant above: more than [`MAX_TAGS_PER_RESOURCE`]
/// entries (a property of the map as a whole, so it is judged first — the request is
/// refused however legal each entry is), a key in the reserved
/// [`RESERVED_TAG_KEY_PREFIX`] namespace in any letter case ([`is_reserved_tag_key`]),
/// a key outside [`TAG_KEY_CHARS`], or a value
/// longer than [`MAX_TAG_VALUE_CHARS`]. Lengths are counted in CHARACTERS rather than
/// bytes, because that is the unit the rules are written in, and the entries are walked
/// in key order so the same bad map is always refused for the same reason.
///
/// The status (431) and the error code are the contract; the text names the member and
/// the constraint it failed, built from the request.
pub fn validate_tags(
    tags: Option<&HashMap<String, String>>,
) -> Result<(), InvalidParameterException> {
    let Some(tags) = tags else {
        return Ok(());
    };

    if tags.len() > MAX_TAGS_PER_RESOURCE {
        return Err(invalid_parameter(
            TAGS_PARAMETER,
            format!(
                "Value at '{TAGS_PARAMETER}' carries {} tags; a resource may carry at most \
                 {MAX_TAGS_PER_RESOURCE} tags.",
                tags.len()
            ),
        ));
    }

    let (min_key, max_key) = TAG_KEY_CHARS;
    let mut entries: Vec<(&String, &String)> = tags.iter().collect();
    entries.sort_by_key(|(key, _)| *key);
    for (key, value) in entries {
        if is_reserved_tag_key(key) {
            return Err(invalid_parameter(
                TAGS_PARAMETER,
                format!(
                    "Tag key '{key}' at '{TAGS_PARAMETER}' uses the reserved \
                     '{RESERVED_TAG_KEY_PREFIX}' prefix, which only AWS may set."
                ),
            ));
        }
        let key_chars = key.chars().count();
        if key_chars < min_key || key_chars > max_key {
            return Err(invalid_parameter(
                TAGS_PARAMETER,
                format!(
                    "Tag key '{key}' at '{TAGS_PARAMETER}' is {key_chars} characters long; a \
                     tag key must be {min_key} to {max_key} characters."
                ),
            ));
        }
        let value_chars = value.chars().count();
        if value_chars > MAX_TAG_VALUE_CHARS {
            return Err(invalid_parameter(
                TAGS_PARAMETER,
                format!(
                    "Tag value at '{TAGS_PARAMETER}.{key}' is {value_chars} characters long; a \
                     tag value must be at most {MAX_TAG_VALUE_CHARS} characters."
                ),
            ));
        }
    }

    Ok(())
}

/// Refuse a tag ASSIGNMENT whose merge would leave the resource carrying more than
/// [`MAX_TAGS_PER_RESOURCE`] tags, with the modelled `InvalidParameterException`
/// (HTTP 431) naming the [`TAGS_PARAMETER`] member.
///
/// [`validate_tags`] judges the map a request CARRIES; this judges the map the resource
/// would be left with, which is the quantity the rule is actually about ("the most tags
/// one resource may carry"): assigning tags merges them into what the resource already
/// holds, so a legal request map can still take the resource past the bound. A key the
/// resource already holds is an OVERWRITE and adds nothing to the count, so only the
/// genuinely new keys are counted.
///
/// Checked before anything is written, so a refused assignment leaves the resource
/// exactly as it was rather than half-tagged.
pub fn validate_tag_merge(
    stored: &HashMap<String, String>,
    assigned: &HashMap<String, String>,
) -> Result<(), InvalidParameterException> {
    let added = assigned
        .keys()
        .filter(|key| !stored.contains_key(*key))
        .count();
    let merged = stored.len() + added;
    if merged > MAX_TAGS_PER_RESOURCE {
        return Err(invalid_parameter(
            TAGS_PARAMETER,
            format!(
                "Value at '{TAGS_PARAMETER}' would leave the resource carrying {merged} tags; a \
                 resource may carry at most {MAX_TAGS_PER_RESOURCE} tags."
            ),
        ));
    }
    Ok(())
}

/// The request member a tag REMOVAL names its keys in — `tagKeys` on `UntagResource`
/// (`smithy_query(query="operation", operation="UntagResource")`), the removal's
/// counterpart to [`TAGS_PARAMETER`].
pub const TAG_KEYS_PARAMETER: &str = "tagKeys";

/// Refuse a tag-key list that names a key AWS's platform-wide tagging rules do not let a
/// caller hold, with the modelled `InvalidParameterException` (HTTP 431) naming the
/// [`TAG_KEYS_PARAMETER`] member.
///
/// The REMOVAL counterpart of [`validate_tags`]: one set of rules decides which keys are
/// a caller's, so a key a caller may not set is a key it may not remove either — a key in
/// the reserved [`RESERVED_TAG_KEY_PREFIX`] namespace in any letter case
/// ([`is_reserved_tag_key`]), which names a tag AWS itself maintains, or a key past
/// [`TAG_KEY_CHARS`]'s ceiling, which no tag of a resource can be spelled with. The floor
/// on a key and the length of the list are MODELLED (`smithy_query(query="shape",
/// shape="TagKeys")` — a `0..500` list of `length min 1` members), so the boundary
/// validator has already refused an empty key and an over-long list before this runs.
///
/// Judged from the request alone, before any ARN is resolved and before anything is
/// removed, so one illegal key costs the whole call and every tag stays as it was —
/// including the keys of the same request that ARE the caller's to remove. Keys are
/// walked in the order the request wrote them, so the same list is always refused for the
/// same reason.
///
/// The status (431) and the error code are the contract; the text names the member and the
/// constraint it failed, built from the request.
pub fn validate_tag_keys(keys: &[String]) -> Result<(), InvalidParameterException> {
    let (_, max_key) = TAG_KEY_CHARS;
    for key in keys {
        if is_reserved_tag_key(key) {
            return Err(invalid_parameter(
                TAG_KEYS_PARAMETER,
                format!(
                    "Tag key '{key}' at '{TAG_KEYS_PARAMETER}' uses the reserved \
                     '{RESERVED_TAG_KEY_PREFIX}' prefix, whose tags only AWS may change."
                ),
            ));
        }
        let key_chars = key.chars().count();
        if key_chars > max_key {
            return Err(invalid_parameter(
                TAG_KEYS_PARAMETER,
                format!(
                    "Tag key '{key}' at '{TAG_KEYS_PARAMETER}' is {key_chars} characters long; a \
                     tag key must be at most {max_key} characters."
                ),
            ));
        }
    }

    Ok(())
}

/// The message every operation addressed at a config by its `(configType, configId)`
/// pair carries when that pair names no config — built from the two `@httpLabel`
/// members of the request, id before type, which is the rule the recorded corpus shows
/// for this service's config reads.
///
/// It lives here rather than in one handler because the two operations that LOOK UP a
/// config by that pair answer it — `GetConfig` and `DeleteConfig`. The operation that
/// WRITES one does not: see [`config_mutation_not_found_message`].
pub fn config_not_found_message(config_type: &str, config_id: &str) -> String {
    format!("No config found with id {config_id} and type {config_type}")
}

/// The message `UpdateConfig` carries when the pair it addressed names no config.
///
/// The MUTATION path composes its own text, from the same two `@httpLabel` members plus
/// the account the write was scoped to: `No config found with ID <configId> and type
/// <configType> for customer <accountId>` — capital `ID` and an account-qualified tail,
/// where the read path ([`config_not_found_message`]) names only the pair. The
/// composition rule is the requirement; the two are separate functions because the read
/// and the write genuinely answer different texts, so neither can be derived from the
/// other.
pub fn config_mutation_not_found_message(config_type: &str, config_id: &str) -> String {
    format!(
        "No config found with ID {config_id} and type {config_type} for customer {}",
        ::engine::emulator_account_id()
    )
}

/// The message `UpdateMissionProfile` carries when `{missionProfileId}` names no
/// profile: the id the request addressed plus the account the write was scoped to, where
/// the read path answers this service's bare [`MISSION_PROFILE_NOT_FOUND`] literal.
pub fn mission_profile_mutation_not_found_message(mission_profile_id: &str) -> String {
    format!(
        "No mission profile found with ID {mission_profile_id} for customer {}",
        ::engine::emulator_account_id()
    )
}

/// The message `UpdateContact` carries when `{contactId}` names no contact: the id the
/// request addressed plus the account the write was scoped to — the composition every
/// MUTATION path of this service uses ([`config_mutation_not_found_message`],
/// [`mission_profile_mutation_not_found_message`]), where the READ path
/// (`DescribeContact`) answers a bare literal and `CancelContact` quotes the id on its
/// own.
pub fn contact_mutation_not_found_message(contact_id: &str) -> String {
    format!(
        "No contact found with ID {contact_id} for customer {}",
        ::engine::emulator_account_id()
    )
}

/// What `DeleteConfig` answers while a live mission profile still references the config:
/// the refusal names the referencing mission-profile ARNs, which is what tells a caller
/// what to delete first.
///
/// The status (431) and the error code (`InvalidParameterException`) are the contract;
/// this is the wording that goes with them, and the ARNs in it are the ones the store
/// holds rather than anything echoed from the request.
pub fn config_in_use_message(mission_profile_arns: &[String]) -> String {
    format!(
        "Config cannot be deleted; it is currently being used by the following Mission \
         Profiles: {}",
        mission_profile_arns.join(", ")
    )
}

/// Why an ARN-valued reference that admits exactly ONE config capability type cannot be
/// used, when it cannot.
///
/// The two are distinct conditions and this service answers them differently: see
/// [`ComAmazonawsGroundstationGroundstationState::typed_reference_fault`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypedReferenceFault {
    /// The ARN resolves to no config this account holds: there is nothing to reference.
    Absent,
    /// It resolves to a config that EXISTS, under another capability type.
    WrongType,
}

/// What this service answers for such a reference that resolved to NO config: the
/// capability the member required, spelled as the model spells that capability's shape
/// ([`capability_display_name`]), and the ARN the request gave.
pub fn typed_reference_absent_message(required_type: &str, arn: &str) -> String {
    format!(
        "Unable to find {} Config with ARN: {arn}",
        capability_display_name(required_type)
    )
}

/// What this service answers for such a reference that resolved to a config of ANOTHER
/// capability type: the ARN named a config that exists, so nothing is missing — what is
/// wrong is the type it points at, and the text states the type the member requires as
/// the `ConfigCapabilityType` token a `ConfigArn`'s type segment carries.
pub fn typed_reference_wrong_type_message(required_type: &str) -> String {
    wrong_config_type_message(&[required_type])
}

/// `types` as the sentence fragment a refusal names them in, each quoted as the
/// `ConfigCapabilityType` token it is: `"antenna-uplink"`, or `"dataflow-endpoint" or
/// "s3-recording"`, or `"a", "b" or "c"`.
fn quoted_config_types(types: &[&str]) -> String {
    let quoted: Vec<String> = types.iter().map(|token| format!("\"{token}\"")).collect();
    match quoted.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, leading)) => format!("{} or {last}", leading.join(", ")),
    }
}

/// What this service answers for a config reference that resolved to a config whose
/// capability type the POSITION it was named in does not admit: the reference resolved,
/// so nothing is missing — what is wrong is the type it points at, and the text states
/// the capability types that position does admit.
///
/// One text for the whole class: a position admitting exactly one type
/// ([`typed_reference_wrong_type_message`], the recorded wording for
/// `uplinkEchoConfig$antennaUplinkConfigArn`) and one admitting several (either end of a
/// mission profile's dataflow edge) differ only in how many types the sentence lists.
pub fn wrong_config_type_message(admissible: &[&str]) -> String {
    format!(
        "Invalid ARN provided. Config Type must be of type {}.",
        quoted_config_types(admissible)
    )
}

/// The member a refusal of a mission profile's dataflow graph names, shared by the create
/// and the update so both answer one parameter name.
pub const DATAFLOW_EDGES_PARAMETER: &str = "dataflowEdges";

/// The ordered `(source, destination)` capability pairs a dataflow edge may join.
///
/// A dataflow edge is one dataflow of a contact, so its two ends are the capability that
/// ORIGINATES the data and the capability that RECEIVES it: data an antenna downlinks
/// (`antenna-downlink`, or `antenna-downlink-demod-decode` when the service demodulates
/// and decodes it first) is carried to a dataflow endpoint of the customer's VPC or
/// written to their bucket; data the customer hands to a dataflow endpoint is radiated by
/// the antenna (`antenna-uplink`); and the echo of an uplink (`uplink-echo`) is carried
/// back to a dataflow endpoint. The capability types that take part are the model's own:
/// `Source$configDetails` is "Additional details for a Config, if type is
/// dataflow-endpoint or antenna-downlink-demod-decode", `Destination$configDetails` the
/// same for a destination, and `ConfigDetails` carries `s3RecordingDetails`, "Details for
/// an S3 recording Config in a contact" (`smithy_query(query="shape",
/// shape="Source,Destination,ConfigDetails")`).
///
/// The two capability types that appear in NO pair are the two a mission profile wires up
/// through a member of its own rather than through the graph — `trackingConfigArn` is "ARN
/// of a tracking Config" and `telemetrySinkConfigArn` that of a telemetry sink
/// (`smithy_query(query="operation", operation="CreateMissionProfile")`) — so neither is a
/// node of a dataflow.
pub const DATAFLOW_EDGE_CAPABILITY_PAIRS: &[(&str, &str)] = &[
    ("antenna-downlink", "dataflow-endpoint"),
    ("antenna-downlink", "s3-recording"),
    ("antenna-downlink-demod-decode", "dataflow-endpoint"),
    ("antenna-downlink-demod-decode", "s3-recording"),
    ("dataflow-endpoint", "antenna-uplink"),
    ("uplink-echo", "dataflow-endpoint"),
];

/// The capability types a dataflow edge may name as its SOURCE: every distinct first
/// element of [`DATAFLOW_EDGE_CAPABILITY_PAIRS`], in the order the table lists them, so
/// the set is derived from the table rather than tabulated twice.
pub fn dataflow_edge_source_types() -> Vec<&'static str> {
    let mut types: Vec<&'static str> = Vec::new();
    for (source, _) in DATAFLOW_EDGE_CAPABILITY_PAIRS {
        if !types.contains(source) {
            types.push(source);
        }
    }
    types
}

/// The capability types a dataflow edge whose source is `source_type` may name as its
/// DESTINATION; empty when no dataflow starts at that capability.
pub fn dataflow_edge_destination_types(source_type: &str) -> Vec<&'static str> {
    DATAFLOW_EDGE_CAPABILITY_PAIRS
        .iter()
        .filter(|(source, _)| *source == source_type)
        .map(|(_, destination)| *destination)
        .collect()
}

/// The message an operation that resolves a config BY ARN — a mission profile's
/// `trackingConfigArn`, `telemetrySinkConfigArn` or a `dataflowEdges` endpoint — carries
/// when that reference resolves to nothing.
///
/// It identifies the config the lookup looked FOR: its id, the config type it searched
/// under, and the account the search was scoped to, rather than echoing the ARN back.
/// The id and the fallback type come from the ARN the request gave
/// ([`config_arn_parts`]); `required_type` is the type the member itself requires, which
/// is `None` for a dataflow edge, whose endpoints may be of any type. An ARN that is not
/// one of this service's config ARNs names no id to report, so the ARN is all there is
/// to say.
///
/// Shared by every operation that takes such a reference, so a create and an update of
/// the same profile answer one text.
pub fn config_reference_not_found_message(arn: &str, required_type: Option<&str>) -> String {
    let from_arn = config_arn_parts(arn);
    let searched_type = required_type
        .map(str::to_string)
        .or_else(|| from_arn.as_ref().map(|(config_type, _)| config_type.clone()));
    match (&from_arn, &searched_type) {
        (Some((_, config_id)), Some(config_type)) => format!(
            "No config found with ID {config_id} and config type: {config_type} for customer {}",
            ::engine::emulator_account_id()
        ),
        _ => format!("Unable to find Config with ARN: {arn}"),
    }
}

/// What this service answers when a dataflow endpoint group a request names does not
/// exist: a fixed literal that discloses neither the id nor the ARN that was asked for,
/// spelled the way this service already answers for a missing mission profile (see
/// [`MISSION_PROFILE_NOT_FOUND`]).
///
/// It lives here rather than in one handler because every operation addressed at a group
/// by its `{dataflowEndpointGroupId}` label answers it — `GetDataflowEndpointGroup` and
/// `DeleteDataflowEndpointGroup` — so it is one text of the service.
pub const DATAFLOW_ENDPOINT_GROUP_NOT_FOUND: &str = "DataflowEndpointGroup not found";

/// Which kind of endpoint an `EndpointDetails` describes.
///
/// The operation's own documentation partitions the endpoints exactly this way — "All
/// dataflow endpoints within a single dataflow endpoint group must be of the same type.
/// You cannot mix AWS Ground Station Agent endpoints with Dataflow endpoints in the same
/// group" (`smithy_query(query="operation", operation="CreateDataflowEndpointGroup")`) —
/// so the kind is what a group's endpoints have to agree on, and it is also what decides
/// which members the service derives for an endpoint rather than reads from the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointKind {
    /// The `endpoint` member: a `DataflowEndpoint` Ground Station itself connects the
    /// stream to, inside the customer's VPC.
    Dataflow,
    /// One of the three AWS Ground Station Agent members: the stream is carried by an
    /// agent the customer runs on an instance of their own.
    Agent,
}

impl EndpointKind {
    /// How a refusal names this kind, so a mixed group's message says what disagrees.
    pub fn description(self) -> &'static str {
        match self {
            Self::Dataflow => "a dataflow endpoint",
            Self::Agent => "an AWS Ground Station Agent endpoint",
        }
    }

    /// Whether an endpoint of this kind can be attached only with the `securityDetails`
    /// triple, i.e. whether a request that omits it describes nothing the service can
    /// create.
    ///
    /// `EndpointDetails` leaves `securityDetails` optional (`smithy_query(query="shape",
    /// shape="EndpointDetails")`), and what it carries is "a list of subnets, a list of
    /// security groups and a role to connect streams to instances"
    /// (`smithy_query(query="doc", shape="SecurityDetails")`) — the elastic network
    /// interfaces GROUND STATION itself places to deliver a [`Self::Dataflow`]
    /// endpoint's stream. A [`Self::Agent`] endpoint's stream is carried by the agent the
    /// customer runs on their own instance, so its shape marks everything it needs
    /// `@required` of itself (`name`, `egressAddress`, `ingressAddress` —
    /// `smithy_query(query="shape", shape="AwsGroundStationAgentEndpoint")`) and the VPC
    /// placement does not apply to it; one is accepted with or without the member.
    pub fn requires_security_details(self) -> bool {
        match self {
            Self::Dataflow => true,
            Self::Agent => false,
        }
    }
}

/// The endpoint members `details` sets, by the modelled member name and the kind of
/// endpoint each one describes, in the order `EndpointDetails` declares them
/// (`smithy_query(query="shape", shape="EndpointDetails")`).
///
/// An `EndpointDetails` describes ONE endpoint, so exactly one of these members belongs
/// to a well-formed request; this is what lets `CreateDataflowEndpointGroup` refuse an
/// item that names none or several, and what the read path classifies a stored endpoint
/// with.
pub fn endpoint_members(details: &EndpointDetails) -> Vec<(&'static str, EndpointKind)> {
    let mut members = Vec::new();
    if details.endpoint.is_some() {
        members.push(("endpoint", EndpointKind::Dataflow));
    }
    if details.aws_ground_station_agent_endpoint.is_some() {
        members.push(("awsGroundStationAgentEndpoint", EndpointKind::Agent));
    }
    if details.uplink_aws_ground_station_agent_endpoint.is_some() {
        members.push(("uplinkAwsGroundStationAgentEndpoint", EndpointKind::Agent));
    }
    if details.downlink_aws_ground_station_agent_endpoint.is_some() {
        members.push(("downlinkAwsGroundStationAgentEndpoint", EndpointKind::Agent));
    }
    members
}

/// The NAME of the endpoint `details` describes, whichever member describes it.
///
/// Every endpoint member of `EndpointDetails` carries a name of its own (`name` on
/// `DataflowEndpoint` and on each of the three agent-endpoint shapes —
/// `smithy_query(query="shape", shape="EndpointDetails")`), and that name is what a
/// `dataflowEndpointConfig` references an endpoint of this account by
/// (`dataflowEndpointName`), so one reader answers it for all four.
pub fn endpoint_name(details: &EndpointDetails) -> Option<&str> {
    if let Some(endpoint) = details.endpoint.as_ref() {
        return endpoint.name.as_deref();
    }
    if let Some(endpoint) = details.aws_ground_station_agent_endpoint.as_ref() {
        return Some(endpoint.name.as_str());
    }
    if let Some(endpoint) = details.uplink_aws_ground_station_agent_endpoint.as_ref() {
        return Some(endpoint.name.as_str());
    }
    details
        .downlink_aws_ground_station_agent_endpoint
        .as_ref()
        .map(|endpoint| endpoint.name.as_str())
}

/// The `EndpointStatus` a dataflow endpoint carries while the service is still attaching
/// it, and the one it carries once attached (`smithy_query(query="shape",
/// shape="EndpointStatus")` — `created`, `creating`, `deleted`, `deleting`, `failed`).
pub const ENDPOINT_CREATING: &str = "creating";
/// See [`ENDPOINT_CREATING`].
pub const ENDPOINT_CREATED: &str = "created";

/// The `CapabilityHealth` values an endpoint's `healthStatus` reports
/// (`smithy_query(query="shape", shape="CapabilityHealth")`).
pub const HEALTH_HEALTHY: &str = "HEALTHY";
/// See [`HEALTH_HEALTHY`].
pub const HEALTH_UNHEALTHY: &str = "UNHEALTHY";

/// The `CapabilityHealthReason` an endpoint still being attached reports, the one a
/// finished endpoint reports, and the one an AWS Ground Station Agent endpoint no agent
/// has registered against reports (`smithy_query(query="shape",
/// shape="CapabilityHealthReason")` — `NO_REGISTERED_AGENT`, `INVALID_IP_OWNERSHIP`,
/// `NOT_AUTHORIZED_TO_CREATE_SLR`, `UNVERIFIED_IP_OWNERSHIP`, `INITIALIZING_DATAPLANE`,
/// `DATAPLANE_FAILURE`, `HEALTHY`).
pub const REASON_INITIALIZING_DATAPLANE: &str = "INITIALIZING_DATAPLANE";
/// See [`REASON_INITIALIZING_DATAPLANE`].
pub const REASON_HEALTHY: &str = "HEALTHY";
/// See [`REASON_INITIALIZING_DATAPLANE`].
pub const REASON_NO_REGISTERED_AGENT: &str = "NO_REGISTERED_AGENT";

/// The `AgentStatus` an AWS Ground Station Agent endpoint carries while no agent has
/// registered itself for that endpoint (`smithy_query(query="shape",
/// shape="AgentStatus")` — `SUCCESS`, `FAILED`, `ACTIVE`, `INACTIVE`).
///
/// It is also the aggregate status a freshly registered [`Agent`] carries: the status of
/// an agent is what the agent itself reports through `UpdateAgentStatus`, so until it has
/// reported one the service holds it inactive rather than claiming a status nothing
/// observed.
pub const AGENT_INACTIVE: &str = "INACTIVE";

/// A dataflow endpoint group as `CreateDataflowEndpointGroup` stored it.
#[derive(Debug, Clone)]
pub struct DataflowEndpointGroup {
    pub dataflow_endpoint_group_id: String,
    pub dataflow_endpoint_group_arn: String,
    /// The endpoints of the group, as the request described them plus the members the
    /// SERVICE owns: each endpoint's `status`, and the `healthStatus`/`healthReasons`
    /// the model documents as "ignored when calling `CreateDataflowEndpointGroup`"
    /// (`smithy_query(query="doc", shape="EndpointDetails", member="healthStatus")`).
    pub endpoint_details: Vec<EndpointDetails>,
    pub contact_pre_pass_duration_seconds: Option<i32>,
    pub contact_post_pass_duration_seconds: Option<i32>,
    pub tags: HashMap<String, String>,
    /// The instant the service owes this group's dataflow endpoints their `creating` ->
    /// `created` move; `None` once the move has been materialised.
    pub endpoints_settle_at: Option<SystemTime>,
}

/// An AWS Ground Station Agent as `RegisterAgent` registered it, with the status its
/// last `UpdateAgentStatus` report carried.
///
/// The model makes `Agent` a resource of this service with the lifecycle
/// `RegisterAgent` (create), `GetAgentConfiguration` (read) and `UpdateAgentStatus`
/// (update), identified by `agentId` and ARN-addressed as `agent/{agentId}`
/// (`smithy_query(query="shape", shape="Agent")`), so one record per agent carries
/// everything those three operations read and write.
#[derive(Debug, Clone)]
pub struct Agent {
    pub agent_id: String,
    pub agent_arn: String,
    /// What the agent said about itself when it registered: its version, the EC2
    /// instance it runs on and the versions of its components.
    pub agent_details: AgentDetails,
    /// The addresses and capability ARNs the agent registered with — "data for
    /// associating an agent with the capabilities it is managing".
    pub discovery_data: DiscoveryData,
    pub tags: HashMap<String, String>,
    /// The aggregate status the agent last reported, [`AGENT_INACTIVE`] until it has
    /// reported one.
    pub aggregate_status: AggregateStatus,
    /// The per-component statuses the agent last reported: the whole list of the latest
    /// report, since each report carries the agent's components as they stand.
    pub component_statuses: Vec<ComponentStatusData>,
    /// The agent task the last report was about; `None` before the first report.
    pub task_id: Option<String>,
    /// The tasking document `GetAgentConfiguration` hands the agent.
    ///
    /// `None` until the service has tasked the agent: the document describes the
    /// contact the agent is to carry, and no operation this model declares produces one
    /// (`smithy_query(query="digest")` carries no operation that tasks an agent), so an
    /// agent that nothing has tasked is reported with no document rather than with an
    /// invented one.
    pub tasking_document: Option<String>,
    /// When the agent registered.
    pub registered: DateTime,
    /// When the agent last reported its status, equal to [`Agent::registered`] until it
    /// has reported once.
    pub last_updated: DateTime,
}

/// What this service answers when an `{agentId}` label names no registered agent.
///
/// Shared by the two operations that address an agent by that label —
/// `GetAgentConfiguration` and `UpdateAgentStatus` — so a read and an update of an
/// agent that is not there answer one text.
pub fn agent_not_found_message(agent_id: &str) -> String {
    format!("No agent found with id {agent_id}")
}

/// What an operation addressed at a resource by its `resourceArn` answers when that ARN
/// names no resource of this account.
///
/// Shared by every tag operation, so a read and a write of the tags of a resource that is
/// not there answer one text. The CONTRACT is the error shape and its status —
/// `ResourceNotFoundException`, HTTP 434 (`smithy_query(query="operation",
/// operation="TagResource")`); AWS documents no wording for it, so this composition — the
/// ARN the request asked about, which is the only thing these operations address a
/// resource by — is this service's.
pub fn tagged_resource_not_found_message(arn: &str) -> String {
    format!("No resource found with ARN {arn}")
}

/// The `ReservationType` of the block of antenna time one of this account's contacts
/// holds at a ground station.
///
/// This and [`RESERVATION_TYPE_MAINTENANCE`] are the model's own enum values
/// (`smithy_query(query="shape", shape="ReservationType")` — `MAINTENANCE` and
/// `CONTACT`), which is both what `ListGroundStationReservations` reports as an item's
/// `reservationType` and what its `reservationTypes` filters on.
pub const RESERVATION_TYPE_CONTACT: &str = "CONTACT";

/// The `ReservationType` of a block of antenna time AWS holds for its own maintenance of
/// a ground station's antenna. See [`RESERVATION_TYPE_CONTACT`].
pub const RESERVATION_TYPE_MAINTENANCE: &str = "MAINTENANCE";

/// The `MaintenanceType` values a maintenance reservation can carry: the model's own enum
/// (`smithy_query(query="shape", shape="MaintenanceType")` — `PLANNED` and `UNPLANNED`).
pub const MAINTENANCE_TYPES: &[&str] = &["PLANNED", "UNPLANNED"];

/// The name of the `number`th antenna of a ground station, counted from one.
///
/// CHOSEN spelling: the model constrains an `AntennaName` only to 4..256 characters of
/// `[ a-zA-Z0-9-._:=]` (`smithy_query(query="shape", shape="AntennaName")`), documents it
/// as no more than "Name of the antenna", and no request of this service names one — AWS
/// places a pass on an antenna itself. So the name is this emulator's, spelled once here
/// so the antenna a reservation reports and the antenna [`MAINTENANCE_ENV`] takes out of
/// service read the same way. The ground station is reported beside it (a
/// `GroundStationReservationListItem` carries both), so the name does not repeat it.
pub fn antenna_name(number: usize) -> String {
    format!("Antenna {number}")
}

/// Whether two windows, each `(start, end)` in epoch seconds, share any instant.
///
/// CLOSED at both ends, which is how a listing's window filter asks the question — a
/// reservation is in the window when its own window overlaps it — so one that starts
/// exactly as the window ends is in it. The SAME reading serves the antenna placement, so
/// two passes that merely touch are placed on different antennas; the two call sites
/// ([`ComAmazonawsGroundstationGroundstationState::antennas_held`] and the listings) read
/// one rule, and a half-open reading would have to replace it in both rather than in one.
pub fn windows_overlap(first: (f64, f64), second: (f64, f64)) -> bool {
    first.1 >= second.0 && first.0 <= second.1
}

/// A ground station antenna AWS has taken out of service for a span, as
/// [`MAINTENANCE_ENV`] described it — the `MAINTENANCE` half of what
/// `ListGroundStationReservations` reports.
///
/// Nothing in THIS API creates one: the model declares no operation that schedules
/// maintenance and none that reports it other than `ListGroundStationReservations` itself
/// (`smithy_query(query="digest")`), because AWS maintains its own antennas. So a
/// maintenance window is minted when this service's store is created, from the operator
/// setting, and the API reports it — and, like a contact's reservation, it holds the
/// antenna, so no pass is placed on an antenna while it is under maintenance.
#[derive(Debug, Clone)]
pub struct MaintenanceReservation {
    /// The ground station whose antenna is out of service, a `GroundStationName`.
    pub ground_station: String,
    /// The antenna that is out of service, an `AntennaName` ([`antenna_name`]).
    pub antenna_name: String,
    /// Whether this maintenance was planned, a `MaintenanceType` ([`MAINTENANCE_TYPES`]).
    pub maintenance_type: String,
    /// The first instant the antenna is out of service.
    pub start_time: DateTime,
    /// The instant it is back in service.
    pub end_time: DateTime,
}

/// A satellite this account is on-boarded to, as [`SATELLITES_ENV`] described it.
///
/// Nothing in THIS API creates one: the model declares no operation that onboards a
/// satellite — `ListSatellites` and `GetSatellite` only report them
/// (`smithy_query(query="digest")`) — because AWS onboards a spacecraft to an account
/// outside the API, once its licensing and configuration have been reviewed. So a
/// satellite record is minted when this service's store is created, from the operator
/// setting, and the API reports it.
#[derive(Debug, Clone)]
pub struct Satellite {
    /// The `Uuid` this onboarding is addressed by, minted by [`new_uuid`] when the store
    /// is created and stable for the life of that store.
    pub satellite_id: String,
    /// This satellite's ARN. Its REGION segment is empty: the modelled `satelliteArn`
    /// pattern makes the region optional
    /// (`^arn:aws:groundstation:([-a-z0-9]{1,50})?:[0-9]{12}:satellite/…`,
    /// `smithy_query(query="shape", shape="satelliteArn")`) where every other ARN of this
    /// service requires one, because an on-boarded satellite belongs to the account
    /// rather than to one region.
    pub satellite_arn: String,
    /// The spacecraft's NORAD catalogue number ([`NORAD_SATELLITE_ID_RANGE`]).
    pub norad_satellite_id: i32,
    /// The ground stations this satellite is on-boarded to, each a `GroundStationName`.
    pub ground_stations: Vec<String>,
    /// The ephemeris currently used to compute this satellite's trajectory, once one is
    /// known for it. An onboarding carries none until an ephemeris is stored.
    pub current_ephemeris: Option<EphemerisMetaData>,
}

/// The range a `noradSatelliteID` may hold: `smithy_query(query="shape",
/// shape="noradSatelliteID")` — 0..339999, inclusive of both bounds.
pub const NORAD_SATELLITE_ID_RANGE: ::std::ops::RangeInclusive<i32> = 0..=339_999;

/// Whether `value` is inside [`NORAD_SATELLITE_ID_RANGE`], so the bound is stated once
/// and read the same way everywhere.
pub fn is_norad_satellite_id(value: i32) -> bool {
    NORAD_SATELLITE_ID_RANGE.contains(&value)
}

/// Most ground stations one satellite can be on-boarded to: the modelled maximum length
/// of `GroundStationIdList` (`smithy_query(query="shape", shape="GroundStationIdList")` —
/// len 0..500), which is what `GetSatellite`/`ListSatellites` report.
pub const MAX_GROUND_STATIONS_PER_SATELLITE: usize = 500;

/// Whether `byte` is inside the character set a `GroundStationName` and an `AntennaName`
/// are spelled from: `[ a-zA-Z0-9-._:=]`, which both shapes' patterns name
/// (`smithy_query(query="shape", shape="GroundStationName,AntennaName")`).
fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b' ' | b'-' | b'.' | b'_' | b':' | b'=')
}

/// Whether `name` is spellable as a `GroundStationName`: length 4..97 over the character
/// set `[ a-zA-Z0-9-._:=]` (`smithy_query(query="shape", shape="GroundStationName")`).
pub fn is_ground_station_name(name: &str) -> bool {
    (4..=97).contains(&name.len()) && name.bytes().all(is_name_byte)
}

/// Whether `name` is spellable as an `AntennaName`: length 4..256 over the same character
/// set as a `GroundStationName` (`smithy_query(query="shape", shape="AntennaName")`).
pub fn is_antenna_name(name: &str) -> bool {
    (4..=256).contains(&name.len()) && name.bytes().all(is_name_byte)
}

/// Most antennas one ground station can report: the modelled maximum length of
/// `AntennaList` (`smithy_query(query="shape", shape="AntennaList")` — len 0..100), which
/// is the list `ListAntennas` answers from.
pub const MAX_ANTENNAS_PER_GROUND_STATION: usize = 100;

/// A ground station serving this account, and the antennas at it.
///
/// Nothing in THIS API builds one: the model declares no operation that creates a ground
/// station or an antenna — the listings only report them (`smithy_query(query="digest")`)
/// — because AWS builds and operates the antennas itself and an account reaches whichever
/// ones it has been granted. So a ground station record is written when this service's
/// store is created, from the operator setting, and the API reports it.
#[derive(Debug, Clone)]
pub struct GroundStation {
    /// Its `GroundStationName`: both the `groundStationId` an operation addresses it by
    /// and the `groundStationName` a listing reports, which are ONE name in this API —
    /// the two members target the same shape (`smithy_query(query="shape",
    /// shape="GroundStationData")`).
    pub ground_station_name: String,
    /// The antennas at it, each an `AntennaName`, held in name order and bounded by
    /// [`MAX_ANTENNAS_PER_GROUND_STATION`]. A ground station whose antennas this account
    /// has not been told about has none here, which is a ground station with nothing to
    /// list rather than an unknown one.
    pub antennas: Vec<String>,
}

/// The status an ephemeris carries while the service is still validating the data it was
/// created with.
///
/// This and the four constants below are values of the model's own `EphemerisStatus` enum,
/// whose members are `VALIDATING`, `INVALID`, `ERROR`, `ENABLED`, `DISABLED` and `EXPIRED`
/// (`smithy_query(query="shape", shape="EphemerisStatus")`). `ERROR` is not among them:
/// the model documents it for an ephemeris the service itself failed to process, and
/// nothing in this emulator fails that way, so no ephemeris here reaches it.
pub const EPHEMERIS_VALIDATING: &str = "VALIDATING";

/// The status of an ephemeris whose data validated and which was asked to be enabled.
pub const EPHEMERIS_ENABLED: &str = "ENABLED";

/// The status of an ephemeris whose data validated and which was not asked to be enabled.
pub const EPHEMERIS_DISABLED: &str = "DISABLED";

/// The status of an ephemeris whose supplied data did not validate.
pub const EPHEMERIS_INVALID: &str = "INVALID";

/// The status of an ephemeris whose expiration time has passed: `CreateEphemeris`
/// documents `expirationTime` as "an overall expiration time for the ephemeris in UTC,
/// after which it will become `EXPIRED`" (`smithy_query(query="doc",
/// shape="CreateEphemerisRequest", member="expirationTime")`).
pub const EPHEMERIS_EXPIRED: &str = "EXPIRED";

/// What this service answers when an ephemeris a request addresses by its `{ephemerisId}`
/// does not exist: a fixed literal that discloses neither the id nor the ARN that was
/// asked for, spelled the way this service already answers for a missing mission profile
/// or dataflow endpoint group (see [`MISSION_PROFILE_NOT_FOUND`],
/// [`DATAFLOW_ENDPOINT_GROUP_NOT_FOUND`]).
///
/// It lives here rather than in one handler because every operation addressed at an
/// ephemeris by that label answers it, so it is one text of the service.
pub const EPHEMERIS_NOT_FOUND: &str = "Ephemeris not found";

/// What this service answers while the ephemeris a delete addressed is still in use: the
/// refusal names the contacts that hold it, which is what tells a caller what has to be
/// flown or released first.
///
/// The status (409) and the error code (`ResourceInUseException`) are the contract; this is
/// the wording that goes with them, and the ids in it are the ones the store holds rather
/// than anything echoed from the request — the same composition this service already uses
/// for a config a mission profile still references ([`config_in_use_message`]).
pub fn ephemeris_in_use_message(contact_ids: &[String]) -> String {
    format!(
        "Ephemeris cannot be deleted; it is currently being used by the following \
         contacts: {}",
        contact_ids.join(", ")
    )
}

/// Why an ephemeris whose supplied data describes a backwards-running validity range did
/// not validate: a value of the model's `EphemerisInvalidReason` enum, whose members are
/// `METADATA_INVALID`, `TIME_RANGE_INVALID`, `TRAJECTORY_INVALID`, `KMS_KEY_INVALID` and
/// `VALIDATION_ERROR` (`smithy_query(query="shape", shape="EphemerisInvalidReason")`).
pub const EPHEMERIS_TIME_RANGE_INVALID: &str = "TIME_RANGE_INVALID";

/// The priority an ephemeris is created with when the request names none: the model's own
/// documentation of `CreateEphemerisRequest$priority` — "Higher numbers take precedence.
/// The default is 1. Must be 1 or greater." (`smithy_query(query="doc",
/// shape="CreateEphemerisRequest", member="priority")`). The member carries no `@default`
/// trait, so the documented default is applied here instead.
pub const DEFAULT_EPHEMERIS_PRIORITY: i32 = 1;

/// Whether an ephemeris created without an `enabled` member is enabled.
///
/// FALSE: the member exists to ask for enabling — "Set to `true` to enable the ephemeris
/// after validation. Set to `false` to keep it disabled." (`smithy_query(query="doc",
/// shape="CreateEphemerisRequest", member="enabled")`) — so a request that asks for
/// neither has not asked to enable it, and the ephemeris validates to
/// [`EPHEMERIS_DISABLED`]. `UpdateEphemeris` then enables it, where `enabled` is required.
pub const EPHEMERIS_ENABLED_BY_DEFAULT: bool = false;

/// Which `EphemerisType` an `EphemerisData` union member selects.
///
/// An ephemeris's type is NOT a request field: it is implied by whichever member of the
/// union the caller set, and it is what `ListEphemerides` reports as `ephemerisType` and
/// filters on. The three pairs are the model's own: `smithy_query(query="shape",
/// shape="EphemerisData")` lists the members and `shape="EphemerisType"` the enum values.
/// The fourth value, `SERVICE_MANAGED`, belongs to an ephemeris AWS supplies itself (the
/// Space Track default), which no customer request creates.
pub fn ephemeris_type_of(data: &EphemerisData) -> &'static str {
    match data {
        EphemerisData::Tle(_) => "TLE",
        EphemerisData::Oem(_) => "OEM",
        EphemerisData::azEl(_) => "AZ_EL",
    }
}

/// The Amazon S3 object `data` was supplied from, when it names one rather than carrying
/// the data inline — what `ListEphemerides` reports as an item's `sourceS3Object`.
pub fn ephemeris_source_s3_object(data: &EphemerisData) -> Option<S3Object> {
    match data {
        EphemerisData::Tle(tle) => tle.s3_object.clone(),
        EphemerisData::Oem(oem) => oem.s3_object.clone(),
        EphemerisData::azEl(az_el) => match &az_el.data {
            AzElSegmentsData::s3Object(object) => Some(object.clone()),
            AzElSegmentsData::azElData(_) => None,
        },
    }
}

/// The validity ranges the data `data` carries, as `(start, end)` epoch seconds.
///
/// A TLE set declares one `validTimeRange` per entry and an az/el set one per segment
/// (`smithy_query(query="shape", shape="TLEData")` / `shape="AzElSegment"`). An OEM body
/// carries its validity inside the CCSDS document this emulator stores verbatim rather
/// than parses, and data supplied from an Amazon S3 object is not fetched here, so both
/// declare none.
fn ephemeris_validity_ranges(data: &EphemerisData) -> Vec<(f64, f64)> {
    match data {
        EphemerisData::Tle(tle) => tle
            .tle_data
            .iter()
            .flatten()
            .map(|entry| {
                (
                    entry.valid_time_range.start_time.as_secs_f64(),
                    entry.valid_time_range.end_time.as_secs_f64(),
                )
            })
            .collect(),
        EphemerisData::Oem(_) => Vec::new(),
        EphemerisData::azEl(az_el) => match &az_el.data {
            AzElSegmentsData::azElData(segments) => segments
                .az_el_segment_list
                .iter()
                .map(|segment| {
                    (
                        segment.valid_time_range.start_time.as_secs_f64(),
                        segment.valid_time_range.end_time.as_secs_f64(),
                    )
                })
                .collect(),
            AzElSegmentsData::s3Object(_) => Vec::new(),
        },
    }
}

/// The last instant the data `data` supplies is valid for, when the data declares its own
/// validity ([`ephemeris_validity_ranges`]); `None` when it declares none.
///
/// This is the expiration an ephemeris created without an explicit `expirationTime`
/// takes: the data stops being usable when its own validity ends.
pub fn ephemeris_validity_end(data: &EphemerisData) -> Option<DateTime> {
    ephemeris_validity_ranges(data)
        .into_iter()
        .map(|(_, end)| end)
        .reduce(f64::max)
        .map(DateTime::from_secs_f64)
}

/// Why the data `data` supplies cannot be used, as an `EphemerisInvalidReason`, or `None`
/// when nothing in it is wrong.
///
/// A validity range must run forwards: one whose end does not follow its start describes
/// no interval at all, so the data is refused as [`EPHEMERIS_TIME_RANGE_INVALID`] rather
/// than enabled. This is the verdict of the validation `CreateEphemeris` starts, applied
/// when that validation falls due ([`EPHEMERIS_VALIDATION_SETTLES_AFTER`]).
pub fn ephemeris_invalid_reason(data: &EphemerisData) -> Option<&'static str> {
    ephemeris_validity_ranges(data)
        .iter()
        .any(|(start, end)| end <= start)
        .then_some(EPHEMERIS_TIME_RANGE_INVALID)
}

/// An ephemeris as `CreateEphemeris` stored it.
#[derive(Debug, Clone)]
pub struct Ephemeris {
    pub ephemeris_id: String,
    /// This ephemeris's ARN — what the tagging operations address it by, and what the one
    /// ARN index resolves to this record.
    pub ephemeris_arn: String,
    /// The satellite this ephemeris applies to, when the request named one.
    pub satellite_id: Option<String>,
    pub name: String,
    /// The `EphemerisType` the supplied data selected ([`ephemeris_type_of`]).
    pub ephemeris_type: String,
    pub priority: i32,
    /// Whether the caller asked for this ephemeris to be enabled once it validates.
    pub enabled: bool,
    pub creation_time: DateTime,
    /// When this ephemeris expires, after which it is [`EPHEMERIS_EXPIRED`]: the
    /// request's `expirationTime`, or the end of the supplied data's own validity when
    /// the request named none. `None` when neither is known, in which case the ephemeris
    /// does not expire.
    pub expiration_time: Option<DateTime>,
    /// The Amazon S3 object the data was supplied from, when it was not supplied inline.
    pub source_s3_object: Option<S3Object>,
    /// The KMS key the request asked for the ephemeris to be encrypted with.
    pub kms_key_arn: Option<String>,
    /// The ephemeris data as supplied, which `DescribeEphemeris` reports as
    /// `suppliedData`.
    pub supplied_data: EphemerisData,
    pub tags: HashMap<String, String>,
    /// This ephemeris's `EphemerisStatus` as it stands now.
    pub status: String,
    /// The verdict of the validation this ephemeris's data failed, as an
    /// `EphemerisInvalidReason`; `None` when the data validated. Set when the ephemeris
    /// is created ([`ephemeris_invalid_reason`]) and reported only once the validation has
    /// fallen due and the status is [`EPHEMERIS_INVALID`].
    pub invalid_reason: Option<&'static str>,
    /// The instant the service owes this ephemeris the move out of
    /// [`EPHEMERIS_VALIDATING`]; `None` once that move has been materialised.
    pub validation_settles_at: Option<SystemTime>,
}

/// What this service answers when a `satelliteId` names no satellite this account is
/// on-boarded to.
///
/// Shared by every operation that takes one, so a create that references an unknown
/// satellite and a listing filtered on one answer the same text.
pub fn satellite_not_found_message(satellite_id: &str) -> String {
    format!("No satellite found with id {satellite_id}")
}

/// Per-service state for com-amazonaws-groundstation-groundstation. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsGroundstationGroundstationState {
    /// Configs, keyed by the `(configType, configId)` pair `GetConfig` addresses.
    pub configs: HashMap<(String, String), Config>,
    /// Mission profiles, keyed by `missionProfileId`.
    pub mission_profiles: HashMap<String, MissionProfile>,
    /// Contacts, keyed by `contactId`.
    pub contacts: HashMap<String, Contact>,
    /// Dataflow endpoint groups, keyed by `dataflowEndpointGroupId`.
    pub dataflow_endpoint_groups: HashMap<String, DataflowEndpointGroup>,
    /// Registered AWS Ground Station Agents, keyed by `agentId`.
    pub agents: HashMap<String, Agent>,
    /// Ephemerides, keyed by `ephemerisId` — what `ListEphemerides` reports.
    pub ephemerides: HashMap<String, Ephemeris>,
    /// Every ARN this service minted, and what it points at.
    pub arn_index: HashMap<String, ResourceRef>,
    /// The uplink frequency bands, in MHz, this account has been approved to transmit
    /// on — the account-level authorisation `CreateConfig` checks an
    /// `antennaUplinkConfig`'s center frequency against.
    ///
    /// EMPTY unless [`APPROVED_UPLINK_BANDS_ENV`] grants a band, and that is this
    /// account's real authorisation rather than a gap in the emulator: AWS Ground
    /// Station radiates only on frequencies an account has been approved for, that
    /// approval is granted outside the API (by AWS, after spectrum coordination), and
    /// this service's model carries no operation that grants one and no response member
    /// that reports one (`smithy_query(query="digest")`). So an account starts with no
    /// uplink approval, exactly as a new AWS account does, and every
    /// `antennaUplinkConfig` is refused with [`UNAPPROVED_UPLINK_FREQUENCY`] until a
    /// band lands here — from the environment variable at start-up, or from the grant
    /// the day the API models one. It is a field rather than a constant because the
    /// authorisation is per account.
    pub approved_uplink_bands_mhz: Vec<(f64, f64)>,
    /// The reserved-minutes pricing plan this account holds — what `GetMinuteUsage`
    /// reports as `isReservedMinutesCustomer` and `totalReservedMinuteAllocation`, and
    /// what it measures `estimatedMinutesRemaining` against.
    ///
    /// EMPTY unless [`RESERVED_MINUTES_ENV`] grants an allocation, and that is this
    /// account's real pricing plan rather than a gap in the emulator: an account signs up
    /// for reserved minutes outside this API (AWS sells the commitment; the model carries
    /// no operation that signs an account up and none that reports the signup other than
    /// `GetMinuteUsage` itself — `smithy_query(query="digest")`). So an account starts on
    /// on-demand pricing, exactly as a new AWS account does, and `GetMinuteUsage` answers
    /// `isReservedMinutesCustomer: false` with a zero allocation until a plan lands here.
    pub reserved_minutes: ReservedMinutesPlan,
    /// The satellites this account is on-boarded to, keyed by `satelliteId` — what
    /// `ListSatellites` reports.
    ///
    /// EMPTY unless [`SATELLITES_ENV`] onboards one, and that is this account's real
    /// onboarding rather than a gap in the emulator: AWS onboards a spacecraft to an
    /// account outside this API (after reviewing its licensing and configuration), and
    /// the model carries no operation that onboards one and no response member that
    /// reports one other than the two satellite readers themselves
    /// (`smithy_query(query="digest")`). So an account starts on-boarded to no satellite,
    /// exactly as a new AWS account does, and `ListSatellites` answers an empty list until
    /// an onboarding lands here.
    pub satellites: HashMap<String, Satellite>,
    /// The ground station antennas AWS has taken out of service for maintenance — the
    /// `MAINTENANCE` reservations `ListGroundStationReservations` reports, and blocks of
    /// antenna time no pass is placed on.
    ///
    /// EMPTY unless [`MAINTENANCE_ENV`] schedules one, and that is the real state of the
    /// ground station network rather than a gap in the emulator: AWS maintains its own
    /// antennas, and the model carries no operation that schedules maintenance and none
    /// that reports it other than `ListGroundStationReservations` itself
    /// (`smithy_query(query="digest")`). So a ground station starts with nothing out of
    /// service and the listing reports only this account's own contact reservations until
    /// a maintenance window lands here.
    pub maintenance_reservations: Vec<MaintenanceReservation>,
    /// The ground stations this account is served by, keyed by their `GroundStationName`,
    /// and the antennas at each — what `ListAntennas` reports.
    ///
    /// EMPTY unless [`GROUND_STATIONS_ENV`] describes one, and that is this account's real
    /// reach rather than a gap in the emulator: AWS builds and operates the antennas and
    /// grants an account access to them outside this API, and the model carries no
    /// operation that builds a ground station or an antenna and no response member that
    /// reports one other than the listings themselves (`smithy_query(query="digest")`). So
    /// an account starts knowing only the ground stations its satellite onboardings name
    /// ([`Self::knows_ground_station`]), and none of them lists an antenna until an
    /// inventory lands here.
    pub ground_stations: HashMap<String, GroundStation>,
}

/// The reserved-minutes pricing plan an account holds: which calendar months it covers
/// and how many minutes it allocates in each.
///
/// The allocation is PER MONTH because `GetMinuteUsage` reports both the signup and the
/// allocation "specific to the month being requested"
/// (`smithy_query(query="operation", operation="GetMinuteUsage")`), so a plan that
/// started, changed size or lapsed partway through a year answers differently month by
/// month.
#[derive(Debug, Clone, Default)]
pub struct ReservedMinutesPlan {
    /// Minutes allocated in every month the plan covers, when it is a standing monthly
    /// commitment; `None` when the account has no standing commitment at all.
    pub monthly_allocation: Option<i32>,
    /// The allocation of one calendar month, keyed `(year, month)`. An entry here
    /// OVERRIDES [`monthly_allocation`](Self::monthly_allocation) for that month, which
    /// is how a plan of a different size — or one covering a single month — is expressed.
    pub by_month: HashMap<(i32, i32), i32>,
}

impl ReservedMinutesPlan {
    /// The minutes this plan allocates for `(year, month)`, or `None` when the plan does
    /// not cover that month — which is exactly what makes the account a reserved-minutes
    /// customer *for the month being requested*.
    pub fn allocation_for(&self, year: i32, month: i32) -> Option<i32> {
        self.by_month
            .get(&(year, month))
            .copied()
            .or(self.monthly_allocation)
    }
}

/// The environment variable an operator uses to put THIS emulated account on the reserved
/// minutes pricing plan, since the API models no operation that signs one up: a
/// comma-separated list of entries, each either a bare minute count (a standing monthly
/// commitment, `GROUNDSTATION_RESERVED_MINUTES="500"`) or `<year>-<month>=<minutes>` for
/// one calendar month (`"500,2026-02=750"` commits 500 minutes a month and 750 in
/// February 2026). Read once when this service's store is created. Unset means on-demand
/// pricing, which is what a new AWS account has.
pub const RESERVED_MINUTES_ENV: &str = "GROUNDSTATION_RESERVED_MINUTES";

/// The plan `setting` grants. An entry that is not a non-negative minute count, optionally
/// keyed by a `<year>-<month>` inside the calendar's 1–12 months, is skipped rather than
/// guessed at, so a typo commits nothing instead of committing something unintended.
pub fn parse_reserved_minutes(setting: &str) -> ReservedMinutesPlan {
    let mut plan = ReservedMinutesPlan::default();
    for entry in setting.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        match entry.split_once('=') {
            // A bare count: the standing monthly commitment.
            None => {
                if let Ok(minutes) = entry.parse::<i32>() {
                    if minutes >= 0 {
                        plan.monthly_allocation = Some(minutes);
                    }
                }
            }
            // `<year>-<month>=<minutes>`: one month's own allocation.
            Some((month_key, minutes)) => {
                let Some((year, month)) = month_key.trim().split_once('-') else {
                    continue;
                };
                let (Ok(year), Ok(month), Ok(minutes)) = (
                    year.trim().parse::<i32>(),
                    month.trim().parse::<i32>(),
                    minutes.trim().parse::<i32>(),
                ) else {
                    continue;
                };
                if (1..=12).contains(&month) && minutes >= 0 {
                    plan.by_month.insert((year, month), minutes);
                }
            }
        }
    }
    plan
}

/// The environment variable an operator uses to grant THIS emulated account its uplink
/// approval, since the API models no operation that grants one: a comma-separated list
/// of `<low>-<high>` bands in MHz, read once when this service's store is created
/// (`GROUNDSTATION_APPROVED_UPLINK_BANDS_MHZ="2025-2120"` approves the whole band the
/// service operates). Unset means no approval, which is what a new AWS account has and
/// what AWS answers for.
pub const APPROVED_UPLINK_BANDS_ENV: &str = "GROUNDSTATION_APPROVED_UPLINK_BANDS_MHZ";

/// The bands `setting` grants: `<low>-<high>` pairs in MHz, comma-separated. An entry
/// that is not a pair of numbers low-to-high is skipped rather than guessed at, so a
/// typo grants nothing instead of granting something unintended.
pub fn parse_approved_uplink_bands(setting: &str) -> Vec<(f64, f64)> {
    setting
        .split(',')
        .filter_map(|entry| {
            let (low, high) = entry.trim().split_once('-')?;
            let low: f64 = low.trim().parse().ok()?;
            let high: f64 = high.trim().parse().ok()?;
            (low <= high).then_some((low, high))
        })
        .collect()
}

/// The environment variable an operator uses to onboard a satellite to THIS emulated
/// account, since the API models no operation that onboards one: a comma-separated list of
/// entries, each either a bare NORAD catalogue number (`25994`) or
/// `<norad>=<ground station>+<ground station>…` naming the ground stations the satellite is
/// on-boarded to (`25994=Ohio 1+Oregon 1`). Read once when this service's store is created.
/// Unset means no satellite, which is what a new AWS account is on-boarded to.
pub const SATELLITES_ENV: &str = "GROUNDSTATION_SATELLITES";

/// The satellites `setting` onboards, keyed by the `satelliteId` each is minted with.
///
/// `,` separates entries and `+` separates one entry's ground stations because neither is
/// spellable in a `GroundStationName`, so no name can be split in two; the first `=` of an
/// entry separates the NORAD number from the names, which may contain further `=`.
///
/// An entry whose NORAD number is not an integer inside [`NORAD_SATELLITE_ID_RANGE`] is
/// skipped, as is one repeating a NORAD number already on-boarded (one spacecraft is one
/// satellite) and any ground station not spellable as a `GroundStationName`, so a typo
/// onboards nothing instead of something unintended. The list is bounded by
/// [`MAX_RECORDS_PER_STORE`] and each satellite's ground stations by
/// [`MAX_GROUND_STATIONS_PER_SATELLITE`].
pub fn parse_satellites(setting: &str) -> HashMap<String, Satellite> {
    let mut satellites: HashMap<String, Satellite> = HashMap::new();
    let mut onboarded: Vec<i32> = Vec::new();
    for entry in setting.split(',') {
        let entry = entry.trim();
        if entry.is_empty() || satellites.len() >= MAX_RECORDS_PER_STORE {
            continue;
        }
        let (norad, names) = match entry.split_once('=') {
            None => (entry, ""),
            Some((norad, names)) => (norad.trim(), names),
        };
        let Ok(norad_satellite_id) = norad.parse::<i32>() else {
            continue;
        };
        if !is_norad_satellite_id(norad_satellite_id) || onboarded.contains(&norad_satellite_id) {
            continue;
        }
        let ground_stations: Vec<String> = names
            .split('+')
            .map(str::trim)
            .filter(|name| is_ground_station_name(name))
            .map(str::to_string)
            .take(MAX_GROUND_STATIONS_PER_SATELLITE)
            .collect();
        let satellite_id = new_uuid();
        let satellite_arn =
            ::engine::emulator_arn(ARN_SERVICE, "", &format!("satellite/{satellite_id}"));
        onboarded.push(norad_satellite_id);
        satellites.insert(
            satellite_id.clone(),
            Satellite {
                satellite_id,
                satellite_arn,
                norad_satellite_id,
                ground_stations,
                current_ephemeris: None,
            },
        );
    }
    satellites
}

/// The environment variable an operator uses to take a ground station's antenna out of
/// service for maintenance, since the API models no operation that schedules one: a
/// comma-separated list of entries, each
/// `<ground station>/<antenna>/<maintenance type>/<start>/<end>` with the two instants in
/// epoch seconds (`"Ohio 1/Antenna 1/PLANNED/1763000000/1763003600"`). Read once when this
/// service's store is created. Unset means nothing is out of service, which is what a
/// ground station network normally is.
///
/// `,` separates entries and `/` the fields of one, because neither is spellable in a
/// `GroundStationName` or an `AntennaName`, so no name can be split in two.
pub const MAINTENANCE_ENV: &str = "GROUNDSTATION_MAINTENANCE";

/// The maintenance windows `setting` schedules, in the order it lists them.
///
/// An entry that is not five `/`-separated fields — a ground station and an antenna
/// spellable as their shapes require, a `MaintenanceType` of [`MAINTENANCE_TYPES`], and a
/// window of two epoch-seconds instants running forwards — is skipped rather than guessed
/// at, so a typo takes nothing out of service instead of something unintended. The list is
/// bounded by [`MAX_RECORDS_PER_STORE`].
pub fn parse_maintenance(setting: &str) -> Vec<MaintenanceReservation> {
    let mut windows: Vec<MaintenanceReservation> = Vec::new();
    for entry in setting.split(',') {
        if windows.len() >= MAX_RECORDS_PER_STORE {
            break;
        }
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let fields: Vec<&str> = entry.split('/').map(str::trim).collect();
        let [ground_station, antenna, maintenance_type, start, end] = fields[..] else {
            continue;
        };
        if !is_ground_station_name(ground_station)
            || !is_antenna_name(antenna)
            || !MAINTENANCE_TYPES.contains(&maintenance_type)
        {
            continue;
        }
        let (Ok(start), Ok(end)) = (start.parse::<i64>(), end.parse::<i64>()) else {
            continue;
        };
        if end <= start {
            continue;
        }
        windows.push(MaintenanceReservation {
            ground_station: ground_station.to_string(),
            antenna_name: antenna.to_string(),
            maintenance_type: maintenance_type.to_string(),
            start_time: DateTime::from_secs(start),
            end_time: DateTime::from_secs(end),
        });
    }
    windows
}

/// The environment variable an operator uses to describe the ground stations THIS
/// emulated account is served by and the antennas at each, since the API models no
/// operation that builds one: a comma-separated list of entries, each either a bare
/// `GroundStationName` (a ground station whose antennas this account has not been told
/// about) or `<ground station>=<antenna>+<antenna>…` naming the antennas at it
/// (`Ohio 1=Antenna 1+Antenna 2,Oregon 1`). Read once when this service's store is
/// created. Unset means no inventory, in which case the only ground stations this account
/// knows are the ones its satellite onboardings name, and none of them lists an antenna.
pub const GROUND_STATIONS_ENV: &str = "GROUNDSTATION_GROUND_STATIONS";

/// The ground stations `setting` describes, keyed by the `GroundStationName` each is
/// addressed by.
///
/// `,` separates entries and `+` separates one entry's antennas because neither is
/// spellable in a `GroundStationName` or an `AntennaName`, so no name can be split in
/// two; the first `=` of an entry separates the ground station from its antennas, which
/// may contain further `=`.
///
/// An entry whose ground station is not spellable as a `GroundStationName` is skipped, as
/// is one repeating a ground station already described (one ground station is one entry)
/// and any antenna not spellable as an `AntennaName` or repeating one already at that
/// ground station, so a typo describes nothing instead of something unintended. The
/// inventory is bounded by [`MAX_RECORDS_PER_STORE`] and each ground station's antennas by
/// [`MAX_ANTENNAS_PER_GROUND_STATION`].
pub fn parse_ground_stations(setting: &str) -> HashMap<String, GroundStation> {
    let mut inventory: HashMap<String, GroundStation> = HashMap::new();
    for entry in setting.split(',') {
        let entry = entry.trim();
        if entry.is_empty() || inventory.len() >= MAX_RECORDS_PER_STORE {
            continue;
        }
        let (name, antennas) = match entry.split_once('=') {
            None => (entry, ""),
            Some((name, antennas)) => (name.trim(), antennas),
        };
        if !is_ground_station_name(name) || inventory.contains_key(name) {
            continue;
        }
        let mut at_station: Vec<String> = Vec::new();
        for antenna in antennas.split('+').map(str::trim) {
            if !is_antenna_name(antenna)
                || at_station.iter().any(|held| held == antenna)
                || at_station.len() >= MAX_ANTENNAS_PER_GROUND_STATION
            {
                continue;
            }
            at_station.push(antenna.to_string());
        }
        // Held in NAME order, so the order a listing reports them in is a property of the
        // store rather than of each read. The bound above keeps the first names the
        // setting named; the sort only arranges what it kept.
        at_station.sort();
        inventory.insert(
            name.to_string(),
            GroundStation {
                ground_station_name: name.to_string(),
                antennas: at_station,
            },
        );
    }
    inventory
}

/// The value `variable` holds, parsed by `parse`, or nothing when it is not set.
fn from_env<T: Default>(variable: &str, parse: impl Fn(&str) -> T) -> T {
    ::std::env::var(variable)
        .map(|setting| parse(&setting))
        .unwrap_or_default()
}

impl Default for ComAmazonawsGroundstationGroundstationState {
    /// An account with no resources, the uplink approval [`APPROVED_UPLINK_BANDS_ENV`]
    /// grants it, the reserved minutes [`RESERVED_MINUTES_ENV`] commits, the satellites
    /// [`SATELLITES_ENV`] onboards, the ground station maintenance [`MAINTENANCE_ENV`]
    /// schedules and the ground stations [`GROUND_STATIONS_ENV`] describes (none of them,
    /// unless they are set).
    fn default() -> Self {
        Self {
            configs: HashMap::new(),
            mission_profiles: HashMap::new(),
            contacts: HashMap::new(),
            dataflow_endpoint_groups: HashMap::new(),
            agents: HashMap::new(),
            ephemerides: HashMap::new(),
            arn_index: HashMap::new(),
            approved_uplink_bands_mhz: from_env(
                APPROVED_UPLINK_BANDS_ENV,
                parse_approved_uplink_bands,
            ),
            reserved_minutes: from_env(RESERVED_MINUTES_ENV, parse_reserved_minutes),
            satellites: from_env(SATELLITES_ENV, parse_satellites),
            maintenance_reservations: from_env(MAINTENANCE_ENV, parse_maintenance),
            ground_stations: from_env(GROUND_STATIONS_ENV, parse_ground_stations),
        }
    }
}

impl ComAmazonawsGroundstationGroundstationState {
    /// Refuse a `ConfigTypeData` this account's authorisation or the model's own
    /// documented ranges do not allow, with the modelled `InvalidParameterException`
    /// (HTTP 431). See [`validate_config_data`] for the bounds.
    ///
    /// The one part of a `configData` this cannot judge from the store is an
    /// `s3RecordingConfig`'s bucket, which lives in another service:
    /// [`unresolvable_recording_bucket`] resolves it against S3 before the handler takes
    /// this lock.
    pub fn validate_config_data(
        &self,
        config_data: &ConfigTypeData,
    ) -> Result<(), InvalidParameterException> {
        validate_config_data(config_data, &self.approved_uplink_bands_mhz)
    }

    /// Every ground station this account reaches, as the store records it and with
    /// repeats: its own inventory ([`GROUND_STATIONS_ENV`]) and the stations the
    /// satellites it is on-boarded to are on-boarded to — the two places a ground station
    /// this account reaches is recorded. Written once here so "the ground stations of this
    /// account" is one rule, whether a caller asks whether one exists or asks for all of
    /// them.
    fn ground_station_names(&self) -> impl Iterator<Item = &str> + '_ {
        self.ground_stations.keys().map(String::as_str).chain(
            self.satellites
                .values()
                .flat_map(|satellite| satellite.ground_stations.iter().map(String::as_str)),
        )
    }

    /// Whether this account knows a ground station called `name`. A name neither place
    /// records is a ground station this account cannot address.
    pub fn knows_ground_station(&self, name: &str) -> bool {
        self.ground_station_names().any(|station| station == name)
    }

    /// The ground stations this account is served by, each a `GroundStationName`, in name
    /// order and without repeats — so one station recorded in both places, or named twice
    /// in one, is one ground station, and a page boundary is stable whatever order the
    /// store holds them in. Empty for an account told about none.
    pub fn known_ground_stations(&self) -> Vec<String> {
        let mut names: Vec<String> = self.ground_station_names().map(str::to_string).collect();
        names.sort();
        names.dedup();
        names
    }

    /// The ground stations the satellite `satellite_id` is on-boarded to, in the same
    /// order and with the same de-duplication as [`Self::known_ground_stations`]; empty
    /// when this account holds no onboarding under that id.
    pub fn ground_stations_of_satellite(&self, satellite_id: &str) -> Vec<String> {
        let mut names: Vec<String> = self
            .satellites
            .get(satellite_id)
            .map(|satellite| satellite.ground_stations.clone())
            .unwrap_or_default();
        names.sort();
        names.dedup();
        names
    }

    /// The antennas at the ground station `name`, each an `AntennaName`, in the order the
    /// inventory holds them — by name, which [`parse_ground_stations`] is what arranges —
    /// so a page boundary is stable whatever order the setting named them in; empty when
    /// that ground station has none and when this account knows no station by that name.
    pub fn antennas_at(&self, name: &str) -> Vec<String> {
        self.ground_stations
            .get(name)
            .map(|station| station.antennas.clone())
            .unwrap_or_default()
    }

    /// The config `arn` names, if it names one.
    ///
    /// An ARN naming one NODE of a config resolves to that config too: the index holds
    /// the ARN of each config this service minted, and the modelled `ConfigArn` pattern
    /// lets a reference append a node to it ([`addresses_config`]), so such a reference is
    /// resolved through the config it addresses rather than looked up as an ARN of its
    /// own.
    pub fn config_by_arn(&self, arn: &str) -> Option<&Config> {
        match self.arn_index.get(arn) {
            Some(ResourceRef::Config {
                config_type,
                config_id,
            }) => self.configs.get(&(config_type.clone(), config_id.clone())),
            Some(_) => None,
            None => {
                let (config_type, config_id) = config_arn_parts(arn)?;
                let config = self.configs.get(&(config_type, config_id))?;
                addresses_config(arn, &config.config_arn).then_some(config)
            }
        }
    }

    /// Whether `arn` resolves to a config this service still holds, of `required_type`
    /// when the member that carries the reference is for one config type only.
    ///
    /// Shared by every operation that takes a config reference, so a reference to a
    /// config that was never created and one to a config that has since been DELETED
    /// are refused the same way.
    pub fn resolves_config(&self, arn: &str, required_type: Option<&str>) -> bool {
        self.config_by_arn(arn)
            .is_some_and(|config| required_type.is_none_or(|wanted| config.config_type == wanted))
    }

    /// Why `arn` cannot be used where only a config of `required_type` may be named, or
    /// `None` when it names one.
    ///
    /// The two faults are different conditions and this service answers them
    /// differently: an ARN resolving to no config of this account is a MISSING resource
    /// (`ResourceNotFoundException`, HTTP 434), while one resolving to a config that
    /// exists under another capability type is a BAD PARAMETER
    /// (`InvalidParameterException`, HTTP 431) — the reference resolved, and what is
    /// wrong with it is the type it points at, which no amount of waiting will change.
    /// See [`typed_reference_absent_message`] and
    /// [`typed_reference_wrong_type_message`] for the two texts.
    ///
    /// Used by every reference member whose resolution is type-checked against the ARN it
    /// was given — `uplinkEchoConfig$antennaUplinkConfigArn`, whose refusal quotes that
    /// ARN. A mission profile's config references are resolved differently: their
    /// recorded text names the id from the ARN under the type the MEMBER requires ("No
    /// config found with ID … and config type: … for customer …"), i.e. the lookup is by
    /// that pair, so a config of another type is genuinely nothing found there — see
    /// [`config_reference_not_found_message`] and [`resolves_config`].
    ///
    /// [`resolves_config`]: Self::resolves_config
    pub fn typed_reference_fault(
        &self,
        arn: &str,
        required_type: &str,
    ) -> Option<TypedReferenceFault> {
        match self.config_by_arn(arn) {
            None => Some(TypedReferenceFault::Absent),
            Some(config) if config.config_type != required_type => {
                Some(TypedReferenceFault::WrongType)
            }
            Some(_) => None,
        }
    }

    /// Why the two configs an `edge` of a mission profile's `dataflowEdges` names cannot
    /// form a dataflow, as the modelled `InvalidParameterException` (HTTP 431) to answer
    /// with; `None` when they can.
    ///
    /// An edge is one dataflow of a contact, so its ends are type-checked and not merely
    /// resolved: the source's capability must be one that originates a dataflow and the
    /// destination's one that receives data from THAT source
    /// ([`DATAFLOW_EDGE_CAPABILITY_PAIRS`]). The refusal names whichever end is at fault —
    /// the source when its capability starts no dataflow at all, otherwise the destination
    /// — and the capability types that end does admit, in the wording this service already
    /// uses for a config reference pointing at a config of a type its position does not
    /// take ([`wrong_config_type_message`]).
    ///
    /// An end whose ARN resolves to no config is NOT this fault: that is a missing
    /// resource, answered by [`resolves_config`] before an edge reaches here.
    ///
    /// Shared by the create and the update, so a profile cannot be left holding a graph a
    /// create would have refused.
    ///
    /// [`resolves_config`]: Self::resolves_config
    pub fn dataflow_edge_fault(&self, edge: &[String]) -> Option<InvalidParameterException> {
        // The model makes an edge a 2..2 list (`smithy_query(query="shape",
        // shape="DataflowEdge")`), so the boundary validator has already refused any other
        // length and there is no pair here to judge.
        let [source, destination] = edge else {
            return None;
        };
        let source_type = &self.config_by_arn(source)?.config_type;
        let destination_type = &self.config_by_arn(destination)?.config_type;

        let admissible_destinations = dataflow_edge_destination_types(source_type);
        if admissible_destinations.contains(&destination_type.as_str()) {
            return None;
        }
        let admissible = if admissible_destinations.is_empty() {
            dataflow_edge_source_types()
        } else {
            admissible_destinations
        };
        Some(invalid_parameter(
            DATAFLOW_EDGES_PARAMETER,
            wrong_config_type_message(&admissible),
        ))
    }

    /// The ARNs of the mission profiles that reference the config `config_arn` names, in
    /// a stable order; empty when nothing references it.
    ///
    /// Every member of a mission profile that carries a `ConfigArn` is an inbound
    /// reference — `trackingConfigArn`, `telemetrySinkConfigArn` and both ends of every
    /// entry of `dataflowEdges` (`smithy_query(query="shape",
    /// shape="CreateMissionProfileRequest")`) — so a config is in use when ANY of them
    /// names it, not only the tracking one. Sorted, so a refusal listing them reads the
    /// same on every call whatever order the map iterates in.
    pub fn mission_profiles_referencing(&self, config_arn: &str) -> Vec<String> {
        let mut referencing: Vec<String> = self
            .mission_profiles
            .values()
            .filter(|profile| profile.references_config(config_arn))
            .map(|profile| profile.mission_profile_arn.clone())
            .collect();
        referencing.sort();
        referencing
    }

    /// Remove the config the `(configType, configId)` pair addresses, along with the ARN
    /// index entry that resolved to it, and answer it; `None` when the pair addresses no
    /// config.
    ///
    /// Dropping the index entry is what makes the deletion complete: an ARN-valued
    /// reference to a deleted config then resolves to nothing, exactly as a reference to
    /// one that never existed does.
    pub fn remove_config(&mut self, config_type: &str, config_id: &str) -> Option<Config> {
        let removed = self
            .configs
            .remove(&(config_type.to_string(), config_id.to_string()))?;
        self.arn_index.remove(&removed.config_arn);
        Some(removed)
    }

    /// Remove the mission profile `mission_profile_id` addresses, along with its ARN
    /// index entry, and answer it; `None` when no profile has that id.
    pub fn remove_mission_profile(&mut self, mission_profile_id: &str) -> Option<MissionProfile> {
        let removed = self.mission_profiles.remove(mission_profile_id)?;
        self.arn_index.remove(&removed.mission_profile_arn);
        Some(removed)
    }

    /// The mission profile `arn` names, if it names one.
    pub fn mission_profile_by_arn(&self, arn: &str) -> Option<&MissionProfile> {
        match self.arn_index.get(arn)? {
            ResourceRef::MissionProfile { mission_profile_id } => {
                self.mission_profiles.get(mission_profile_id)
            }
            _ => None,
        }
    }

    /// The dataflow endpoint of this account named `name`, as the group that holds it
    /// stored it; `None` when no group of this account holds one by that name.
    ///
    /// A `dataflowEndpointConfig` names an endpoint by `dataflowEndpointName` rather than
    /// by the group it belongs to, so the lookup is over every group's endpoints
    /// ([`endpoint_name`] reads the name whichever member describes the endpoint). Groups
    /// are taken in id order, so two endpoints sharing a name resolve to the same one on
    /// every call whatever order the map iterates in.
    pub fn dataflow_endpoint(&self, name: &str) -> Option<&EndpointDetails> {
        let mut group_ids: Vec<&String> = self.dataflow_endpoint_groups.keys().collect();
        group_ids.sort();
        group_ids
            .into_iter()
            .filter_map(|id| self.dataflow_endpoint_groups.get(id))
            .flat_map(|group| group.endpoint_details.iter())
            .find(|details| endpoint_name(details) == Some(name))
    }

    /// The tags of the resource `arn` names, when this service holds one.
    ///
    /// EVERY resource this service hands out is taggable — a config, a mission profile, a
    /// contact, a dataflow endpoint group, an agent and an ephemeris each carry their own
    /// `tags` map, which is what their creates store and their reads report — so the
    /// lookup is by ARN through the one
    /// [`ComAmazonawsGroundstationGroundstationState::arn_index`] rather than per resource
    /// kind: that index is what makes an ARN-addressed operation resolve without knowing
    /// which map the resource lives in. An ARN this service never minted, or one whose
    /// resource has since been DELETED (a delete drops the index entry), resolves to
    /// nothing.
    ///
    /// This is the READ half, for the operation that reports a resource's tags;
    /// [`Self::tags_mut`] is the write half.
    pub fn tags_of(&self, arn: &str) -> Option<&HashMap<String, String>> {
        match self.arn_index.get(arn)? {
            ResourceRef::Config {
                config_type,
                config_id,
            } => self
                .configs
                .get(&(config_type.clone(), config_id.clone()))
                .map(|config| &config.tags),
            ResourceRef::MissionProfile { mission_profile_id } => self
                .mission_profiles
                .get(mission_profile_id)
                .map(|profile| &profile.tags),
            ResourceRef::Contact { contact_id } => {
                self.contacts.get(contact_id).map(|contact| &contact.tags)
            }
            ResourceRef::DataflowEndpointGroup {
                dataflow_endpoint_group_id,
            } => self
                .dataflow_endpoint_groups
                .get(dataflow_endpoint_group_id)
                .map(|group| &group.tags),
            ResourceRef::Agent { agent_id } => self.agents.get(agent_id).map(|agent| &agent.tags),
            ResourceRef::Ephemeris { ephemeris_id } => self
                .ephemerides
                .get(ephemeris_id)
                .map(|ephemeris| &ephemeris.tags),
        }
    }

    /// The tags of the resource `arn` names, for WRITING, when this service holds one.
    ///
    /// The write half of [`Self::tags_of`]: the same ARN resolution, handing back the
    /// stored map itself so a tag write is a read-modify-write of the ONE map the
    /// resource's reads report, under the single lock the caller already holds. Two
    /// writers that each add a key therefore both persist, whatever order they arrive in.
    pub fn tags_mut(&mut self, arn: &str) -> Option<&mut HashMap<String, String>> {
        // Resolved to an owned reference first: the index borrow ends here, so the
        // resource's own map can be borrowed mutably below.
        match self.arn_index.get(arn)?.clone() {
            ResourceRef::Config {
                config_type,
                config_id,
            } => self
                .configs
                .get_mut(&(config_type, config_id))
                .map(|config| &mut config.tags),
            ResourceRef::MissionProfile { mission_profile_id } => self
                .mission_profiles
                .get_mut(&mission_profile_id)
                .map(|profile| &mut profile.tags),
            ResourceRef::Contact { contact_id } => self
                .contacts
                .get_mut(&contact_id)
                .map(|contact| &mut contact.tags),
            ResourceRef::DataflowEndpointGroup {
                dataflow_endpoint_group_id,
            } => self
                .dataflow_endpoint_groups
                .get_mut(&dataflow_endpoint_group_id)
                .map(|group| &mut group.tags),
            ResourceRef::Agent { agent_id } => self
                .agents
                .get_mut(&agent_id)
                .map(|agent| &mut agent.tags),
            ResourceRef::Ephemeris { ephemeris_id } => self
                .ephemerides
                .get_mut(&ephemeris_id)
                .map(|ephemeris| &mut ephemeris.tags),
        }
    }

    /// Remove the dataflow endpoint group `dataflow_endpoint_group_id` addresses, along
    /// with its ARN index entry, and answer it; `None` when no group has that id.
    pub fn remove_dataflow_endpoint_group(
        &mut self,
        dataflow_endpoint_group_id: &str,
    ) -> Option<DataflowEndpointGroup> {
        let removed = self
            .dataflow_endpoint_groups
            .remove(dataflow_endpoint_group_id)?;
        self.arn_index.remove(&removed.dataflow_endpoint_group_arn);
        Some(removed)
    }

    /// The contacts that still depend on the ephemeris `ephemeris_id` names, as the
    /// refusal of a delete lists them; empty when nothing depends on it.
    ///
    /// An ephemeris is what a contact's antenna is pointed by, through the satellite the
    /// ephemeris applies to: a contact names its satellite by ARN
    /// ([`Contact::flies_satellite`]), and the ephemeris in force for that satellite is the
    /// trajectory its pass is flown on. So the ephemerides a contact depends on are the
    /// ones that apply to its satellite, and a contact depends on them for as long as it is
    /// not terminal ([`Contact::is_non_terminal`]) — once its pass is over or its
    /// reservation released, nothing of it is still to be flown.
    ///
    /// Only an ephemeris whose status is [`EPHEMERIS_ENABLED`] is in force: that status is
    /// what "whether or not the ephemeris is enabled" (`smithy_query(query="operation",
    /// operation="DescribeEphemeris")`) reports, and one still validating, disabled,
    /// invalid or expired is not the trajectory anything is flown on. CHOSEN coupling: AWS
    /// documents only that such a resource "is in use by non-terminal state contacts", so
    /// which contacts hold which ephemeris is this emulator's rule, stated once here rather
    /// than per operation.
    ///
    /// Sorted, so a refusal listing them reads the same on every call whatever order the
    /// map iterates in.
    pub fn contacts_using_ephemeris(&self, ephemeris_id: &str) -> Vec<String> {
        let Some(ephemeris) = self.ephemerides.get(ephemeris_id) else {
            return Vec::new();
        };
        if ephemeris.status != EPHEMERIS_ENABLED {
            return Vec::new();
        }
        // An ephemeris that applies to no satellite is the trajectory of nothing, so no
        // contact is flown on it.
        let Some(satellite_arn) = ephemeris
            .satellite_id
            .as_deref()
            .and_then(|satellite_id| self.satellites.get(satellite_id))
            .map(|satellite| satellite.satellite_arn.clone())
        else {
            return Vec::new();
        };
        let mut holding: Vec<String> = self
            .contacts
            .values()
            .filter(|contact| contact.is_non_terminal() && contact.flies_satellite(&satellite_arn))
            .map(|contact| contact.contact_id.clone())
            .collect();
        holding.sort();
        holding
    }

    /// Remove the ephemeris `ephemeris_id` addresses, along with the ARN index entry that
    /// resolved to it and any satellite reference that named it, and answer it; `None` when
    /// no ephemeris has that id.
    ///
    /// Dropping the index entry is what makes the deletion complete: the tagging
    /// operations address an ephemeris by ARN, and after this that ARN resolves to nothing,
    /// exactly as the ARN of one that never existed does.
    pub fn remove_ephemeris(&mut self, ephemeris_id: &str) -> Option<Ephemeris> {
        let removed = self.ephemerides.remove(ephemeris_id)?;
        self.arn_index.remove(&removed.ephemeris_arn);
        // A satellite reports the ephemeris its trajectory is computed from; a deleted one
        // is no longer that, so the reference goes with it rather than being left dangling.
        for satellite in self.satellites.values_mut() {
            if satellite
                .current_ephemeris
                .as_ref()
                .and_then(|metadata| metadata.ephemeris_id.as_deref())
                == Some(ephemeris_id)
            {
                satellite.current_ephemeris = None;
            }
        }
        Some(removed)
    }

    /// Materialise every `creating` -> `created` endpoint move the clock has already made
    /// due. A reporting operation calls this before reading: the move was owed at this
    /// instant whether or not anybody called, so applying it changes nothing a later
    /// caller could otherwise have seen.
    ///
    /// An endpoint that is `created` is attached, so its health is the dataplane's own:
    /// `HEALTHY`. An AWS Ground Station Agent endpoint carries no `status` of its own and
    /// keeps the health it was stored with, which stays `NO_REGISTERED_AGENT` until an
    /// agent registers for it.
    pub fn settle_due_endpoints(&mut self) {
        let now = SystemTime::now();
        for group in self.dataflow_endpoint_groups.values_mut() {
            let Some(due) = group.endpoints_settle_at else {
                continue;
            };
            if due > now {
                continue;
            }
            for details in &mut group.endpoint_details {
                let Some(endpoint) = details.endpoint.as_mut() else {
                    continue;
                };
                if endpoint.status.as_deref() == Some(ENDPOINT_CREATING) {
                    endpoint.status = Some(ENDPOINT_CREATED.to_string());
                    details.health_status = Some(HEALTH_HEALTHY.to_string());
                    details.health_reasons = Some(vec![REASON_HEALTHY.to_string()]);
                }
            }
            group.endpoints_settle_at = None;
        }
    }

    /// Materialise every ephemeris status move the clock has already made due: the end of
    /// the validation `CreateEphemeris` started, and the expiration of an ephemeris whose
    /// `expirationTime` has passed. A reporting operation calls this before reading: both
    /// moves were owed at this instant whether or not anybody called, so applying them
    /// changes nothing a later caller could otherwise have seen.
    ///
    /// Validation lands the ephemeris on the verdict taken when it was created
    /// ([`ephemeris_invalid_reason`]): [`EPHEMERIS_INVALID`] when its data describes no
    /// usable validity range, else [`EPHEMERIS_ENABLED`] or [`EPHEMERIS_DISABLED`]
    /// according to what the caller asked for. Expiration then overrides either of those
    /// two, and an ephemeris that did not validate stays `INVALID`: there is nothing of
    /// it to expire.
    pub fn settle_due_ephemerides(&mut self) {
        let now = SystemTime::now();
        let now_seconds = now
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |since| since.as_secs_f64());
        for ephemeris in self.ephemerides.values_mut() {
            if let Some(due) = ephemeris.validation_settles_at {
                if due > now {
                    continue;
                }
                ephemeris.status = match (ephemeris.invalid_reason, ephemeris.enabled) {
                    (Some(_), _) => EPHEMERIS_INVALID.to_string(),
                    (None, true) => EPHEMERIS_ENABLED.to_string(),
                    (None, false) => EPHEMERIS_DISABLED.to_string(),
                };
                ephemeris.validation_settles_at = None;
            }
            let expired = ephemeris
                .expiration_time
                .is_some_and(|expiration| expiration.as_secs_f64() <= now_seconds);
            if expired
                && matches!(
                    ephemeris.status.as_str(),
                    EPHEMERIS_ENABLED | EPHEMERIS_DISABLED
                )
            {
                ephemeris.status = EPHEMERIS_EXPIRED.to_string();
            }
        }
    }

    /// Materialise every `SCHEDULING` -> `SCHEDULED` move, every `CANCELLING` ->
    /// `CANCELLED` move, every `UPDATING` -> `ACTIVE` contact-version move and every step
    /// of the pass lifecycle ([`CONTACT_PASS_LIFECYCLE`]) the clock has already made due.
    /// A reporting operation calls this before reading: the moves were owed at this
    /// instant whether or not anybody called, so applying them changes nothing a later
    /// caller could otherwise have seen.
    pub fn settle_due_contacts(&mut self) {
        let now = SystemTime::now();
        let now_seconds = now
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |since| since.as_secs_f64());
        for contact in self.contacts.values_mut() {
            contact.apply_due_versions();
            if contact
                .cancellation_settles_at
                .is_some_and(|due| due <= now)
            {
                if contact.contact_status == CONTACT_CANCELLING {
                    contact.contact_status = CONTACT_CANCELLED.to_string();
                    contact.last_updated = now_timestamp();
                }
                contact.cancellation_settles_at = None;
            }
            if contact.scheduling_settles_at.is_some_and(|due| due <= now) {
                if contact.contact_status == CONTACT_SCHEDULING {
                    contact.contact_status = CONTACT_SCHEDULED.to_string();
                    contact.last_updated = now_timestamp();
                }
                contact.scheduling_settles_at = None;
            }
            // And then the pass itself: the stage of its window the clock has reached.
            // Last, so a contact the scheduler has just confirmed is carried on to the
            // stage its own timestamps owe it in the same settle.
            if contact.advance_pass_stage(now_seconds) {
                contact.last_updated = now_timestamp();
            }
        }
    }

    /// The antennas at `ground_station` that something already holds for any instant of
    /// `window` (`(start, end)` in epoch seconds): the ones a maintenance window AWS
    /// scheduled has taken out of service, and the ones a contact of this account whose
    /// reservation still holds antenna time ([`Contact::holds_antenna_time`]) is placed on.
    ///
    /// A released reservation holds nothing, so the antenna it was on is free again. Read
    /// in ONE pass over each store, so placing a pass does not rescan them per candidate
    /// antenna.
    pub fn antennas_held(&self, ground_station: &str, window: (f64, f64)) -> HashSet<&str> {
        let under_maintenance = self
            .maintenance_reservations
            .iter()
            .filter(|maintenance| maintenance.ground_station == ground_station)
            .filter(|maintenance| {
                windows_overlap(
                    (
                        maintenance.start_time.as_secs_f64(),
                        maintenance.end_time.as_secs_f64(),
                    ),
                    window,
                )
            })
            .map(|maintenance| maintenance.antenna_name.as_str());
        let reserved = self
            .contacts
            .values()
            .filter(|contact| contact.ground_station == ground_station)
            .filter(|contact| contact.holds_antenna_time())
            .filter(|contact| {
                windows_overlap(
                    (
                        contact.start_time.as_secs_f64(),
                        contact.end_time.as_secs_f64(),
                    ),
                    window,
                )
            })
            .map(|contact| contact.antenna_name.as_str());
        under_maintenance.chain(reserved).collect()
    }

    /// The antennas a pass at `ground_station` can be placed on, in the order the
    /// placement takes them: the antennas this account has been told are AT that ground
    /// station ([`Self::antennas_at`], in the name order the inventory holds them) when it
    /// records any, and otherwise the one antenna this emulator numbers for a ground
    /// station it was told nothing about ([`antenna_name`]).
    ///
    /// A ground station's antennas are FINITE — AWS builds a fixed number of them at a
    /// site — so this list is also what bounds how much antenna time can be held there at
    /// once. CHOSEN count for a ground station with no recorded inventory: ONE antenna, the
    /// smallest a ground station can be operated with, so the one-pass-per-antenna rule
    /// still bounds it rather than that station taking unboundedly many overlapping passes;
    /// an operator that wants more says so with [`GROUND_STATIONS_ENV`].
    pub fn placement_antennas(&self, ground_station: &str) -> Vec<String> {
        let recorded = self.antennas_at(ground_station);
        if recorded.is_empty() {
            vec![antenna_name(1)]
        } else {
            recorded
        }
    }

    /// The antenna at `ground_station` a pass over `window` is placed on: the first of
    /// [`Self::placement_antennas`] nothing holds for any instant of it, or `None` when
    /// every one of them is held — the ground station has no antenna time left over that
    /// window, which is what a reservation of it is refused on.
    ///
    /// An antenna serves ONE pass at a time, so two reservations whose windows overlap at
    /// a ground station are on different antennas, and a reservation that overlaps none
    /// takes the first antenna. Overlap is [`windows_overlap`], which is inclusive at both
    /// ends, so two passes that merely TOUCH — one ending exactly as the next starts — are
    /// placed on different antennas too; the band between a pass and the next on one
    /// antenna is this emulator's reading, not a documented one, and it is the same reading
    /// the listing's window filter uses.
    ///
    /// CHOSEN placement: AWS assigns the antenna itself and documents no rule for which,
    /// and no request of this service names one, so the emulator places a pass on the first
    /// free antenna. A maintenance window on an antenna the placement never names
    /// ([`MAINTENANCE_ENV`] accepts any `AntennaName`) is an antenna no pass is placed on,
    /// so it holds nothing from a contact — it is still reported as the reservation it is.
    pub fn free_antenna(&self, ground_station: &str, window: (f64, f64)) -> Option<String> {
        let held = self.antennas_held(ground_station, window);
        self.placement_antennas(ground_station)
            .into_iter()
            .find(|antenna| !held.contains(antenna.as_str()))
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsGroundstationGroundstationState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsGroundstationGroundstationState>)
}
