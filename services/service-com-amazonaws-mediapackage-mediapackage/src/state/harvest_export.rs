// SPDX-License-Identifier: CC0-1.0

//! Carrying a submitted HarvestJob OUT: what it exports, where it writes it, and which
//! modelled terminal status the outcome of that write decides.
//!
//! A harvest exports a window of an origin endpoint's packaged content to the caller's
//! bucket, and the model says what it leaves there: `S3Destination$BucketName` is "The
//! name of an S3 bucket within which harvested content will be exported" and
//! `S3Destination$ManifestKey` "The key in the specified S3 bucket where the harvested
//! top-level manifest will be placed"
//! (`smithy_query(query="shape",shape="com.amazonaws.mediapackage#S3Destination",model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json")`).
//! So a job this service carries out PRODUCES an object: the top-level manifest, at that
//! key of that bucket, which a client can then read back with `GetObject`.
//!
//! ## What decides the terminal status
//!
//! `Status` is `IN_PROGRESS | SUCCEEDED | FAILED` and the submitted job starts
//! `IN_PROGRESS`, so the service owes it a terminal status without anybody calling again.
//! The one thing that status reports is whether the export happened: a job whose manifest
//! the object store ACCEPTED reaches `SUCCEEDED`, and a job whose manifest is not in the
//! destination reaches `FAILED` — the bucket the submission named is one the store does
//! not hold, the write was refused, or nothing answered the write at all (an assembly with
//! no object store). Every one of those is the same fact about the harvest: its output was
//! not placed where the submission said it would be.
//!
//! ## Where the destination is reached
//!
//! Amazon S3 is reached the way any emulated service reaches another: the AWS SDK driven
//! over the operation callback ([`smithy_server_runtime::OperationContext::callback`]),
//! which routes IN PROCESS — no socket, no outbound request, no host resolution. The
//! object a harvest writes is therefore the object a client's own `GetObject` reads, and
//! the per-bucket rules of the write (does the bucket exist, what does it retain) stay the
//! object store's own, asked through its front door rather than re-decided here. The
//! sibling VOD model reads an asset's source object through exactly this seam.
//!
//! ## What the manifest says
//!
//! The document is the top-level manifest of the endpoint's OWN packaging format
//! ([`PackagingKind`]: an HLS master playlist for HLS and CMAF, an MPD for DASH, a server
//! manifest for Microsoft Smooth), and it states only what this emulator knows: the
//! harvested window, read off the submitted job, and nothing about renditions or segments.
//! A MediaPackage channel here ingests nothing — this emulator serves no MediaPackage data
//! plane — so the window a harvest reads back holds no published media, and a manifest
//! naming segments or bitrates would describe content that is not there.

use std::time::SystemTime;

use super::{
    harvest_window, iso8601_of_secs, ComAmazonawsMediapackageMediapackageState, HarvestJobRecord,
    OriginEndpointRecord, PackagingKind, ResourceKey, DEFAULT_REGION,
};

/// One harvest the clock has made due, with the export it owes: the destination the
/// submission named, the manifest document, and the media type it is stored under.
///
/// Taken out of the store under its guard ([`ComAmazonawsMediapackageMediapackageState::take_due_harvests`])
/// and carried out with no guard held, which is why it owns everything it needs.
#[derive(Debug, Clone)]
pub struct DueHarvest {
    /// The stored job this export belongs to, so its outcome is recorded against it.
    pub job: ResourceKey,
    /// The Region the job was submitted in, which is the signing scope of the write.
    pub region: String,
    /// `S3Destination$BucketName`.
    pub bucket: String,
    /// `S3Destination$ManifestKey`.
    pub manifest_key: String,
    /// The top-level manifest's bytes.
    pub manifest: Vec<u8>,
    /// The media type that manifest is written as.
    pub media_type: &'static str,
}

/// Materialise every harvest the clock has already made due: carry the export out and
/// record the terminal status its outcome decides.
///
/// Called FIRST by every harvest-job handler, a read included. It applies only what was
/// owed at `now` whether or not anybody called, so what a reader sees is decided by the
/// clock and never by the read itself. A harvest stays owed until its outcome is recorded
/// ([`ComAmazonawsMediapackageMediapackageState::take_due_harvests`]), so a call that
/// never finishes leaves it for the next one instead of leaving the job reporting work
/// nothing will finish.
///
/// The store's guard is taken to collect the due harvests and again to record each
/// outcome, and is NOT held across the write: the export goes out through the object
/// store's own front door, which must not wait on this service's lock.
pub async fn settle_due_harvests(
    state: &::engine::ServiceState<ComAmazonawsMediapackageMediapackageState>,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    now: SystemTime,
) {
    let due = state.lock().take_due_harvests(now);
    for harvest in due {
        let exported = export(ctx, &harvest).await;
        state.lock().record_harvest_outcome(&harvest.job, exported);
    }
}

/// Write one harvest's top-level manifest to its destination, answering whether the object
/// store took it.
///
/// `false` for every answer that is not the store reporting the object stored, because they
/// all say the same thing about the harvest: the manifest is not at the key the submission
/// named.
async fn export(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    harvest: &DueHarvest,
) -> bool {
    // The Region ends up in the AUTHORITY of the endpoint URL the client below is
    // addressed by, so it is held to the Region-label grammar first (the shared reading
    // every service that puts a stored Region in an authority uses); anything outside it
    // addresses this service's default Region instead. The transport is the in-process
    // callback either way.
    let region = ::service_common::arn::region_label(&harvest.region).unwrap_or(DEFAULT_REGION);
    let answer = s3_client(ctx, region)
        .put_object()
        .bucket(&harvest.bucket)
        .key(&harvest.manifest_key)
        .content_type(harvest.media_type)
        .body(::aws_sdk_s3::primitives::ByteStream::from(
            harvest.manifest.clone(),
        ))
        .send()
        .await;
    match answer {
        Ok(_) => true,
        Err(error) => {
            // The destination's own values are rendered in their escaped `Debug` form, so
            // a value carrying a newline cannot forge a log line.
            ::tracing::debug!(
                bucket = ?harvest.bucket,
                key = ?harvest.manifest_key,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a harvest job's top-level manifest was not written to its destination, so \
                 the job failed"
            );
            false
        }
    }
}

/// The Amazon S3 client a harvest writes its output with, addressed to `region`.
///
/// Shared with [`super::harvest_destination`], which asks the same object store, through the
/// same seam, whether the destination a submission names is one it holds: the destination a
/// job is pre-flighted against and the one its export writes to are then reached by one
/// client construction rather than two.
///
/// The transport is the emulator's own in-process callback, so the `endpoint_url` below is
/// an ADDRESS the in-process router reads rather than a host any resolver looks up: this
/// client reaches the emulator's own Amazon S3 and nothing else, whatever bucket or key a
/// submission named.
///
/// There is nothing to retry, time out or watch for a stalled stream, each of which would
/// need a sleep implementation this client is not given. The same goes for caching the
/// identity: the credentials the callback hands over are used directly, and they only sign
/// this operation's in-process calls.
pub(super) fn s3_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        // The in-process transport routes on the request's own path, and a virtual-host
        // bucket name is not a host this emulator resolves, so the request addresses its
        // bucket in the path.
        .force_path_style(true)
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

/// The media type each packaging format's top-level manifest is served as: the type
/// registered for an HLS playlist, for an MPEG-DASH MPD, and for a Microsoft Smooth
/// Streaming server manifest.
const HLS_MANIFEST_MEDIA_TYPE: &str = "application/vnd.apple.mpegurl";
/// See [`HLS_MANIFEST_MEDIA_TYPE`].
const DASH_MANIFEST_MEDIA_TYPE: &str = "application/dash+xml";
/// See [`HLS_MANIFEST_MEDIA_TYPE`].
const MSS_MANIFEST_MEDIA_TYPE: &str = "application/vnd.ms-sstr+xml";

/// How many of a Microsoft Smooth manifest's ticks make a second: its `TimeScale` is in
/// 100-nanosecond units, which is the scale that format's manifests carry.
const MSS_TICKS_PER_SECOND: i128 = 10_000_000;

/// The top-level manifest a harvest of `endpoint` writes, with the media type it is stored
/// under — or `None` when there is nothing to export, which is a harvest that produced no
/// output and so a job that FAILED.
///
/// `None` has two causes: an endpoint that stores no packaging configuration at all, so
/// there is no manifest format to write, and a job whose bounds name no interval of media
/// to export ([`harvest_window`]). `CreateHarvestJob` refuses a submission with either, and
/// every endpoint this service holds stores exactly one packaging configuration, so a
/// stored job reaching here has both.
pub fn harvested_manifest(
    endpoint: &OriginEndpointRecord,
    job: &HarvestJobRecord,
) -> Option<(Vec<u8>, &'static str)> {
    let (start, length) = harvest_window(job)?;
    let kind = endpoint.packaging_kind()?;
    let document = match kind {
        PackagingKind::Hls | PackagingKind::Cmaf => hls_master_playlist(),
        PackagingKind::Dash => dash_manifest(start, length),
        PackagingKind::Mss => smooth_manifest(length),
    };
    let media_type = match kind {
        PackagingKind::Hls | PackagingKind::Cmaf => HLS_MANIFEST_MEDIA_TYPE,
        PackagingKind::Dash => DASH_MANIFEST_MEDIA_TYPE,
        PackagingKind::Mss => MSS_MANIFEST_MEDIA_TYPE,
    };
    Some((document.into_bytes(), media_type))
}

/// The HLS master playlist a harvest of an HLS or CMAF endpoint writes: the one tag the
/// format requires, and no variant.
///
/// A master playlist's entries are the renditions of the harvested content, and a
/// MediaPackage endpoint here has none to export: nothing is ingested into a channel of
/// this emulator, so the startover window a harvest reads back holds no published
/// rendition. Naming one — with a bandwidth or a child playlist — would describe content
/// the harvest did not write.
fn hls_master_playlist() -> String {
    "#EXTM3U\n".to_string()
}

/// The MPD a harvest of a DASH endpoint writes.
///
/// A harvested presentation is complete rather than live, which is what `type="static"`
/// says, and its length is the harvested window: `mediaPresentationDuration` and the one
/// period's `duration` are that window, and `availabilityStartTime` is the instant it
/// starts — both read off the submitted job's own bounds ([`harvest_window`]) rather than
/// the clock. It carries
/// no adaptation set for the same reason the HLS playlist carries no variant (see
/// [`hls_master_playlist`]).
fn dash_manifest(start: i128, length: i128) -> String {
    let duration = iso8601_duration(length);
    // Rendered from the instant the submitted bound NAMES rather than from its text, so
    // the document states the window rather than echoing what the caller wrote.
    let start = iso8601_of_secs(i64::try_from(start.div_euclid(1_000_000_000)).unwrap_or(0));
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
         <MPD xmlns=\"urn:mpeg:dash:schema:mpd:2011\" \
         profiles=\"urn:mpeg:dash:profile:isoff-main:2011\" type=\"static\" \
         availabilityStartTime=\"{start}\" mediaPresentationDuration=\"{duration}\">\n\
         \x20 <Period id=\"1\" start=\"PT0S\" duration=\"{duration}\"/>\n\
         </MPD>\n"
    )
}

/// The Microsoft Smooth server manifest a harvest of an MSS endpoint writes.
///
/// `Duration` is the harvested window in the manifest's own 100-nanosecond ticks
/// ([`MSS_TICKS_PER_SECOND`]), and it carries no stream index for the same reason the HLS
/// playlist carries no variant (see [`hls_master_playlist`]).
fn smooth_manifest(length: i128) -> String {
    let ticks = length / (1_000_000_000 / MSS_TICKS_PER_SECOND);
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
         <SmoothStreamingMedia MajorVersion=\"2\" MinorVersion=\"2\" \
         TimeScale=\"{MSS_TICKS_PER_SECOND}\" Duration=\"{ticks}\"/>\n"
    )
}

/// `nanos` as an ISO 8601 duration of seconds, which is the form a DASH manifest states a
/// duration in. The fractional part is written only when the span carries one, to the
/// millisecond — the finest a `@timestampFormat`-free ISO 8601 bound is read to here.
fn iso8601_duration(nanos: i128) -> String {
    let seconds = nanos / 1_000_000_000;
    let millis = (nanos % 1_000_000_000) / 1_000_000;
    if millis == 0 {
        format!("PT{seconds}S")
    } else {
        format!("PT{seconds}.{millis:03}S")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stored job over the window `[start, end]`, as `CreateHarvestJob` would have
    /// stored one: the bounds are the caller's own text, which is what the manifest's
    /// window is read from.
    fn job(start: &str, end: &str) -> HarvestJobRecord {
        HarvestJobRecord {
            start_time: start.to_string(),
            end_time: end.to_string(),
            ..HarvestJobRecord::default()
        }
    }

    /// An endpoint storing one packaging configuration, so the manifest format under test
    /// is the one the endpoint serves.
    fn endpoint(kind: PackagingKind) -> OriginEndpointRecord {
        let mut record = OriginEndpointRecord::default();
        match kind {
            PackagingKind::Hls => {
                record.hls_package = Some(crate::ops::types::HlsPackage::default());
            }
            PackagingKind::Dash => {
                record.dash_package = Some(crate::ops::types::DashPackage::default());
            }
            PackagingKind::Cmaf => {
                record.cmaf_package = Some(crate::ops::types::CmafPackage::default());
            }
            PackagingKind::Mss => {
                record.mss_package = Some(crate::ops::types::MssPackage::default());
            }
        }
        record
    }

    /// The manifest written is the one the endpoint's packaging format serves, and what it
    /// states about the window is the window the job was submitted with.
    ///
    /// The rows span what the rule generalises over: every packaging format the service
    /// stores, and windows differing in length and in the FORM of their bounds (a `Z`
    /// instant, a numeric UTC offset, a fractional second).
    #[test]
    fn the_harvested_manifest_is_the_endpoints_own_format_over_the_submitted_window() {
        let rows: &[(&str, PackagingKind, &str, &str, &str, &str)] = &[
            (
                "HLS, one hour",
                PackagingKind::Hls,
                "2026-01-01T00:00:00Z",
                "2026-01-01T01:00:00Z",
                HLS_MANIFEST_MEDIA_TYPE,
                "#EXTM3U",
            ),
            (
                "CMAF, one minute",
                PackagingKind::Cmaf,
                "2026-04-05T06:07:00Z",
                "2026-04-05T06:08:00Z",
                HLS_MANIFEST_MEDIA_TYPE,
                "#EXTM3U",
            ),
            (
                "DASH, half an hour, numeric offset",
                PackagingKind::Dash,
                "2026-02-03T10:15:00+00:00",
                "2026-02-03T10:45:00+00:00",
                DASH_MANIFEST_MEDIA_TYPE,
                "mediaPresentationDuration=\"PT1800S\"",
            ),
            (
                "DASH, a fractional window",
                PackagingKind::Dash,
                "2026-03-04T23:59:59.250Z",
                "2026-03-05T00:00:09.500Z",
                DASH_MANIFEST_MEDIA_TYPE,
                "mediaPresentationDuration=\"PT10.250S\"",
            ),
            (
                "Microsoft Smooth, two seconds",
                PackagingKind::Mss,
                "2026-05-06T07:08:09Z",
                "2026-05-06T07:08:11Z",
                MSS_MANIFEST_MEDIA_TYPE,
                "Duration=\"20000000\"",
            ),
        ];
        for (case, kind, start, end, expected_type, expected_text) in rows {
            let written = harvested_manifest(&endpoint(*kind), &job(start, end));
            let (document, media_type) = written
                .unwrap_or_else(|| panic!("case {case:?}: a harvest of an endpoint that stores a packaging configuration must write a manifest"));
            let document = String::from_utf8(document)
                .unwrap_or_else(|_| panic!("case {case:?}: a manifest is text"));
            assert_eq!(
                media_type, *expected_type,
                "case {case:?}: expected the media type {expected_type:?}, got {media_type:?}"
            );
            assert!(
                document.contains(expected_text),
                "case {case:?}: the manifest must carry {expected_text:?}, got {document:?}"
            );
        }
    }

    /// An endpoint storing no packaging configuration has no manifest format, so a harvest
    /// of it exports nothing — and neither does one whose window is not an interval.
    #[test]
    fn a_harvest_with_nothing_to_export_writes_no_manifest() {
        let rows: &[(&str, OriginEndpointRecord, HarvestJobRecord)] = &[
            (
                "no packaging configuration",
                OriginEndpointRecord::default(),
                job("2026-01-01T00:00:00Z", "2026-01-01T01:00:00Z"),
            ),
            (
                "bounds that are not instants",
                endpoint(PackagingKind::Hls),
                job("not-an-instant", "neither-is-this"),
            ),
            (
                "an inverted window",
                endpoint(PackagingKind::Hls),
                job("2026-01-01T01:00:00Z", "2026-01-01T00:00:00Z"),
            ),
        ];
        for (case, endpoint, job) in rows {
            let written = harvested_manifest(endpoint, job);
            assert!(
                written.is_none(),
                "case {case:?}: expected no manifest, got {written:?}"
            );
        }
    }
}
