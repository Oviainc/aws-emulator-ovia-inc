// SPDX-License-Identifier: CC0-1.0

//! The OFFERING slice of this service's data model: the reserved resources that
//! are available for purchase.
//!
//! An offering is NOT a resource a caller creates — `Offering` is documented as
//! "Reserved resources available for purchase" and the only operations that
//! touch one read it (`ListOfferings`, `DescribeOffering`) or buy a reservation
//! against it (`PurchaseOffering`). So this slice is a CATALOG rather than a
//! store: it holds no per-request state, every reader derives the same offerings
//! from it, and reading it changes nothing a later caller sees. It lives beside
//! the stores, in the same module tree, so the operations that report an offering
//! all answer from this one catalog instead of each building its own.
//!
//! ## What the catalog holds
//!
//! One offering per RESOURCE CONFIGURATION that can be reserved. A reservation's
//! configuration is `ReservationResourceSpecification`
//! (`smithy_query(query="shape", shape="ReservationResourceSpecification")`), and
//! its members are enums, so the configurations that exist are the product of
//! those enums' values: `ReservationResourceType` x `ChannelClass` x
//! `ReservationCodec` x `ReservationResolution` x `ReservationMaximumBitrate`,
//! times the members the model scopes to ONE resource type —
//! `MaximumFramerate` "(Outputs only)", `VideoQuality` "(Outputs only)" and
//! `SpecialFeature` "(Channels only)", each carried only by the offerings of the
//! type its own documentation names.
//!
//! The value sets themselves are the model's, taken from the generated
//! `*_VALUES` constants in `crate::ops::types`, so adding a value to the model
//! adds the offerings that carry it instead of leaving a hand-written list
//! behind.
//!
//! ## What an offering reports
//!
//! Every member is derived from the catalog entry and the region the request was
//! signed for: the id from the entry's position in the catalog (so an id names
//! the same configuration on every call and in every process), the ARN from that
//! id, the resource specification from the entry itself, and the prices from the
//! entry by [`OfferingSpec::usage_price`]. Nothing is built out of the request in
//! flight beyond the region the caller addressed.
//!
//! The two members AWS does not give a machine-readable rule for — the recurring
//! price and the description's prose — are this EMULATOR's, not the real service's
//! data: each is a pure function of the catalog entry (see
//! [`OfferingSpec::usage_price`] and [`OfferingSpec::description`]), so a read of
//! one offering is stable and comparable with another's, but neither is AWS's
//! published price list or console wording and nothing should treat them as such.

use std::sync::OnceLock;

use crate::ops::types::{
    Offering, ReservationResourceSpecification, CHANNEL_CLASS_VALUES, RESERVATION_CODEC_VALUES,
    RESERVATION_MAXIMUM_BITRATE_VALUES, RESERVATION_MAXIMUM_FRAMERATE_VALUES,
    RESERVATION_RESOLUTION_VALUES, RESERVATION_RESOURCE_TYPE_VALUES,
    RESERVATION_SPECIAL_FEATURE_VALUES, RESERVATION_VIDEO_QUALITY_VALUES,
};

use super::{ChannelRecord, ComAmazonawsMedialiveMedialiveState};

/// The `ReservationResourceType` values this slice names, spelled exactly as the
/// model declares them (`smithy_query(query="shape",
/// shape="ReservationResourceType")`: INPUT | OUTPUT | MULTIPLEX | CHANNEL).
///
/// Named here because two of them scope a specification member: the model
/// documents `MaximumFramerate` and `VideoQuality` as "(Outputs only)" and
/// `SpecialFeature` as "(Channels only)".
pub mod reservation_resource_type {
    pub const CHANNEL: &str = "CHANNEL";
    pub const INPUT: &str = "INPUT";
    pub const MULTIPLEX: &str = "MULTIPLEX";
    pub const OUTPUT: &str = "OUTPUT";
}

/// The one unit an offering's lease duration is measured in
/// (`smithy_query(query="shape", shape="OfferingDurationUnits")` declares MONTHS
/// and nothing else).
pub const OFFERING_DURATION_UNITS: &str = "MONTHS";

/// The lease term, in [`OFFERING_DURATION_UNITS`], every offering in this catalog
/// is for: the one the `Duration` member documents by example ("Lease duration,
/// e.g. '12'").
///
/// The catalog offers ONE term, so `ListOfferings`'s `Duration` filter ("Filter
/// by offering duration, e.g. '12'") admits every offering for this term and
/// none for any other.
pub const OFFERING_DURATION: i32 = 12;

/// The one offering type the model declares (`smithy_query(query="shape",
/// shape="OfferingType")`: NO_UPFRONT), so every offering in this catalog is of
/// it.
pub const OFFERING_TYPE: &str = "NO_UPFRONT";

/// The currency the catalog's prices are in, in the ISO-4217 format
/// `CurrencyCode` documents ("Currency code for usagePrice and fixedPrice in
/// ISO-4217 format, e.g. 'USD'").
pub const OFFERING_CURRENCY_CODE: &str = "USD";

/// The one-time charge every offering in this catalog carries. `FixedPrice` is
/// "One-time charge for each reserved resource, e.g. '0.0' for a NO_UPFRONT
/// offering" and every offering here is [`OFFERING_TYPE`], so each carries that
/// zero.
pub const OFFERING_FIXED_PRICE: f64 = 0.0;

/// The id of the catalog's FIRST offering; the one at position `index` is this
/// plus `index`.
///
/// `OfferingId` is documented "Unique offering ID, e.g. '87654321'" — an
/// eight-digit numeric string — and the catalog is far shorter than the eight
/// digits allow, so every offering's id has that shape. Deriving the id from the
/// position makes it stable (the same configuration keeps the same id on every
/// call) and resolvable both ways, which is what lets `DescribeOffering` and
/// `PurchaseOffering` find a configuration from an id alone.
const FIRST_OFFERING_ID: u64 = 10_000_000;

/// One catalog entry: the resource configuration that offering reserves.
///
/// Every other member of the reported `Offering` is derived from this, so two
/// readers of the same entry cannot disagree about what it costs or describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OfferingSpec {
    /// Which kind of reserved resource this offering is for.
    pub resource_type: &'static str,
    pub channel_class: &'static str,
    pub codec: &'static str,
    pub maximum_bitrate: &'static str,
    pub resolution: &'static str,
    /// `MaximumFramerate`, "(Outputs only)": absent on every offering whose
    /// resource type is not `OUTPUT`.
    pub maximum_framerate: Option<&'static str>,
    /// `VideoQuality`, "(Outputs only)": absent for the same reason.
    pub video_quality: Option<&'static str>,
    /// `SpecialFeature`, "(Channels only)": absent on every offering whose
    /// resource type is not `CHANNEL`, and absent on the plain channel offerings
    /// too, since a reserved channel either carries one of the special features
    /// or carries none.
    pub special_feature: Option<&'static str>,
}

/// The whole catalog, in the one order every reader walks it in.
///
/// Built once per process and then shared: the entries depend on nothing but the
/// model's own value sets, so there is nothing per-request to rebuild, and a
/// fixed order is what makes an offering's id (its position) stable.
pub fn catalog() -> &'static [OfferingSpec] {
    static CATALOG: OnceLock<Vec<OfferingSpec>> = OnceLock::new();
    CATALOG.get_or_init(build_catalog)
}

/// The product described in this module's docs, in the order the model's own
/// value sets list its factors.
fn build_catalog() -> Vec<OfferingSpec> {
    let mut catalog = Vec::new();
    for resource_type in RESERVATION_RESOURCE_TYPE_VALUES.iter().copied() {
        // The three members the model scopes to one resource type. An offering
        // of another type carries none of them, which is the single `None` the
        // loop below then runs once.
        let outputs_only = resource_type == reservation_resource_type::OUTPUT;
        let channels_only = resource_type == reservation_resource_type::CHANNEL;
        let framerates = scoped_values(outputs_only, RESERVATION_MAXIMUM_FRAMERATE_VALUES, false);
        let video_qualities = scoped_values(outputs_only, RESERVATION_VIDEO_QUALITY_VALUES, false);
        // A reserved channel either carries one of the special features or
        // carries none at all, so the channel offerings span the values AND
        // their absence.
        let special_features =
            scoped_values(channels_only, RESERVATION_SPECIAL_FEATURE_VALUES, true);
        for channel_class in CHANNEL_CLASS_VALUES.iter().copied() {
            for codec in RESERVATION_CODEC_VALUES.iter().copied() {
                for resolution in RESERVATION_RESOLUTION_VALUES.iter().copied() {
                    for maximum_bitrate in RESERVATION_MAXIMUM_BITRATE_VALUES.iter().copied() {
                        for maximum_framerate in framerates.iter().copied() {
                            for video_quality in video_qualities.iter().copied() {
                                for special_feature in special_features.iter().copied() {
                                    catalog.push(OfferingSpec {
                                        resource_type,
                                        channel_class,
                                        codec,
                                        maximum_bitrate,
                                        resolution,
                                        maximum_framerate,
                                        video_quality,
                                        special_feature,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    catalog
}

/// The values a specification member takes across the offerings of one resource
/// type: each modelled value when the member applies to that type, plus its
/// absence when `optional` says the member is one an offering of that type may
/// also leave out, and nothing but that absence when it does not apply at all.
fn scoped_values(
    applies: bool,
    values: &'static [&'static str],
    optional: bool,
) -> Vec<Option<&'static str>> {
    if !applies {
        return vec![None];
    }
    let absent = if optional { Some(None) } else { None };
    absent
        .into_iter()
        .chain(values.iter().copied().map(Some))
        .collect()
}

/// The id of the offering at `index` in the catalog.
pub fn offering_id(index: usize) -> String {
    (FIRST_OFFERING_ID + index as u64).to_string()
}

/// The position in the catalog an `OfferingId` names, or `None` when the id
/// names no offering this catalog holds — including every value that is not one
/// of its ids at all.
pub fn offering_index(offering_id: &str) -> Option<usize> {
    let id: u64 = offering_id.parse().ok()?;
    let index = usize::try_from(id.checked_sub(FIRST_OFFERING_ID)?).ok()?;
    (index < catalog().len()).then_some(index)
}

/// The offering an `OfferingId` names, as the region it was asked for reports
/// it, or `None` when the id names no offering.
///
/// The ONE projection every operation that reports an offering answers from, so
/// a listed offering cannot drift from a described one.
pub fn offering_by_id(offering_id: &str, region: &str) -> Option<Offering> {
    let index = offering_index(offering_id)?;
    Some(catalog()[index].offering(index, region))
}

impl OfferingSpec {
    /// This entry as the modelled `Offering` a reader reports, at position
    /// `index` of the catalog and in `region`.
    pub fn offering(&self, index: usize, region: &str) -> Offering {
        let id = offering_id(index);
        Offering {
            // The ARN form the `Arn` member documents by example
            // ("arn:aws:medialive:us-west-2:123456789012:offering:87654321").
            arn: Some(::engine::emulator_arn(
                "medialive",
                region,
                &format!("offering:{id}"),
            )),
            currency_code: Some(OFFERING_CURRENCY_CODE.to_string()),
            duration: Some(OFFERING_DURATION),
            duration_units: Some(OFFERING_DURATION_UNITS.to_string()),
            fixed_price: Some(OFFERING_FIXED_PRICE),
            offering_description: Some(self.description(region)),
            offering_id: Some(id),
            offering_type: Some(OFFERING_TYPE.to_string()),
            region: Some(region.to_string()),
            resource_specification: Some(self.resource_specification()),
            usage_price: Some(self.usage_price()),
        }
    }

    /// This entry as the modelled `ReservationResourceSpecification`: the
    /// configuration the reserved resource has.
    pub fn resource_specification(&self) -> ReservationResourceSpecification {
        ReservationResourceSpecification {
            channel_class: Some(self.channel_class.to_string()),
            codec: Some(self.codec.to_string()),
            maximum_bitrate: Some(self.maximum_bitrate.to_string()),
            maximum_framerate: self.maximum_framerate.map(ToString::to_string),
            resolution: Some(self.resolution.to_string()),
            resource_type: Some(self.resource_type.to_string()),
            special_feature: self.special_feature.map(ToString::to_string),
            video_quality: self.video_quality.map(ToString::to_string),
        }
    }

    /// The recurring charge for one reserved resource of this configuration.
    ///
    /// CHOSEN, not AWS's price list: no AWS document gives a machine-readable
    /// price for a MediaLive offering, and inventing one caller's recorded
    /// number would be worse than a rule. So the price is DERIVED from the
    /// configuration — a base per resource type scaled by each member that costs
    /// capacity — which keeps the two properties a caller can rely on: the same
    /// configuration always costs the same (a read is stable), and a
    /// configuration that reserves more never costs less than one that reserves
    /// less.
    ///
    /// Rounded to whole cents, because `UsagePrice` is a charge in
    /// [`OFFERING_CURRENCY_CODE`].
    pub fn usage_price(&self) -> f64 {
        let base = match self.resource_type {
            reservation_resource_type::INPUT => 25.0,
            reservation_resource_type::OUTPUT => 50.0,
            reservation_resource_type::CHANNEL => 75.0,
            // MULTIPLEX, and anything a later model adds: a multiplex reserves
            // the transport for many programs at once.
            _ => 100.0,
        };
        // A standard channel has two encoding pipelines and a single pipeline
        // channel only has one (the `ChannelClass` doc), so it reserves twice
        // the capacity.
        let channel_class = match self.channel_class {
            "SINGLE_PIPELINE" => 1.0,
            _ => 2.0,
        };
        let codec = match self.codec {
            "AUDIO" => 0.25,
            "LINK" => 0.5,
            "MPEG2" => 1.0,
            "AVC" => 1.25,
            "HEVC" => 1.5,
            // AV1, and anything a later model adds.
            _ => 1.75,
        };
        // Lines of vertical resolution, as `ReservationResolution` defines the
        // four steps.
        let resolution = match self.resolution {
            "SD" => 1.0,
            "HD" => 2.0,
            "FHD" => 3.0,
            _ => 4.0,
        };
        let maximum_bitrate = match self.maximum_bitrate {
            "MAX_10_MBPS" => 1.0,
            "MAX_20_MBPS" => 1.5,
            _ => 2.5,
        };
        let maximum_framerate = match self.maximum_framerate {
            Some("MAX_60_FPS") => 1.5,
            _ => 1.0,
        };
        let video_quality = match self.video_quality {
            Some("ENHANCED") => 1.25,
            Some("PREMIUM") => 1.5,
            _ => 1.0,
        };
        let special_feature = match self.special_feature {
            None => 1.0,
            _ => 1.1,
        };
        let price: f64 = base
            * channel_class
            * codec
            * resolution
            * maximum_bitrate
            * maximum_framerate
            * video_quality
            * special_feature;
        (price * 100.0).round() / 100.0
    }

    /// The sentence `OfferingDescription` carries: the configuration this
    /// offering reserves and the region it reserves it in, the way the member's
    /// own example reads one out ("HD AVC output at 10-20 Mbps, 30 fps, and
    /// standard VQ in US West (Oregon)") — resolution, codec and what is being
    /// reserved, then the rest of the configuration, then the region.
    ///
    /// Built from the entry and the region, never from a request, and it names
    /// the configuration in the model's own vocabulary rather than in the
    /// console's prose, which no document gives a rule for.
    pub fn description(&self, region: &str) -> String {
        let mut description = format!(
            "{} {} {} at {}",
            self.resolution, self.codec, self.resource_type, self.maximum_bitrate
        );
        if let Some(maximum_framerate) = self.maximum_framerate {
            description.push_str(&format!(", {maximum_framerate}"));
        }
        if let Some(video_quality) = self.video_quality {
            description.push_str(&format!(", {video_quality} video quality"));
        }
        if let Some(special_feature) = self.special_feature {
            description.push_str(&format!(", {special_feature}"));
        }
        description.push_str(&format!(
            ", {} channel class in {region}",
            self.channel_class
        ));
        description
    }

    /// Whether this offering is one `filters` asks for: it matches only when
    /// EVERY filter the request named holds of it.
    pub fn matches(&self, filters: &OfferingFilters<'_>) -> bool {
        member_matches(filters.resource_type, Some(self.resource_type))
            && member_matches(filters.channel_class, Some(self.channel_class))
            && member_matches(filters.codec, Some(self.codec))
            && member_matches(filters.maximum_bitrate, Some(self.maximum_bitrate))
            && member_matches(filters.resolution, Some(self.resolution))
            && member_matches(filters.maximum_framerate, self.maximum_framerate)
            && member_matches(filters.video_quality, self.video_quality)
            && member_matches(filters.special_feature, self.special_feature)
            && filters
                .duration
                .is_none_or(|duration| duration == OFFERING_DURATION)
            && filters
                .channel_configuration
                .as_ref()
                .is_none_or(|configuration| self.serves(configuration))
    }

    /// Whether this offering reserves a resource configured the way the channel
    /// `configuration` describes — what `ChannelConfiguration` ("Filter to
    /// offerings that match the configuration of an existing channel") admits.
    ///
    /// A channel's configuration is its class and its `InputSpecification`, and
    /// the three members of that specification are the same `Codec`,
    /// `MaximumBitrate` and `Resolution` a reservation names (their enums are
    /// narrower, but spelled identically), so matching is per member. A member
    /// the channel does not state constrains nothing: the channel's
    /// configuration says nothing about it.
    fn serves(&self, configuration: &ChannelOfferingConfiguration) -> bool {
        member_matches(
            configuration.channel_class.as_deref(),
            Some(self.channel_class),
        ) && member_matches(configuration.codec.as_deref(), Some(self.codec))
            && member_matches(
                configuration.maximum_bitrate.as_deref(),
                Some(self.maximum_bitrate),
            )
            && member_matches(configuration.resolution.as_deref(), Some(self.resolution))
    }
}

/// Whether a filter on one specification member admits an offering that carries
/// `offered` for it: a filter the request did not name admits every offering,
/// and a named one admits exactly the offerings carrying that value — so an
/// offering that does not carry the member AT ALL is not one of them.
fn member_matches(filter: Option<&str>, offered: Option<&str>) -> bool {
    match filter {
        None => true,
        Some(wanted) => offered == Some(wanted),
    }
}

/// The filters one `ListOfferings` request named, each already checked against
/// the value set its member accepts.
///
/// Every member of `ReservationResourceSpecification` has a filter of the same
/// name on the request, plus `Duration` on the offering itself and
/// `ChannelConfiguration`, which is resolved to the configuration of a stored
/// channel before it gets here.
#[derive(Debug, Clone, Default)]
pub struct OfferingFilters<'request> {
    pub channel_class: Option<&'request str>,
    pub codec: Option<&'request str>,
    pub duration: Option<i32>,
    pub maximum_bitrate: Option<&'request str>,
    pub maximum_framerate: Option<&'request str>,
    pub resolution: Option<&'request str>,
    pub resource_type: Option<&'request str>,
    pub special_feature: Option<&'request str>,
    pub video_quality: Option<&'request str>,
    pub channel_configuration: Option<ChannelOfferingConfiguration>,
}

/// The configuration of one stored channel, as the `ChannelConfiguration` filter
/// matches offerings against it.
#[derive(Debug, Clone, Default)]
pub struct ChannelOfferingConfiguration {
    pub channel_class: Option<String>,
    pub codec: Option<String>,
    pub maximum_bitrate: Option<String>,
    pub resolution: Option<String>,
}

/// One page of the offerings `filters` admits: the offerings from position
/// `start` on, at most `page_size` of them, and the id the next page starts at —
/// absent on the last page, since a token is handed out only when an admitted
/// offering is left over.
///
/// The catalog is walked in its one fixed order, so a token means the same thing
/// on the next call as it did on this one.
pub fn offering_page(
    filters: &OfferingFilters<'_>,
    region: &str,
    start: usize,
    page_size: usize,
) -> (Vec<Offering>, Option<String>) {
    let mut admitted = catalog()
        .iter()
        .enumerate()
        .skip(start)
        .filter(|(_, spec)| spec.matches(filters));
    let offerings: Vec<Offering> = admitted
        .by_ref()
        .take(page_size)
        .map(|(index, spec)| spec.offering(index, region))
        .collect();
    let next_token = admitted.next().map(|(index, _)| offering_id(index));
    (offerings, next_token)
}

impl ComAmazonawsMedialiveMedialiveState {
    /// The configuration of the channel `channel_id`, for the
    /// `ChannelConfiguration` filter to match offerings against, or `None` when
    /// the account has no channel with that id.
    ///
    /// A READ, so it resolves a channel whose delete has been accepted exactly
    /// as `DescribeChannel` goes on reporting it: the record stays in the
    /// account, and what it was configured as does not change when it is
    /// deleted.
    pub fn channel_offering_configuration(
        &self,
        channel_id: &str,
    ) -> Option<ChannelOfferingConfiguration> {
        self.channels
            .get(channel_id)
            .map(channel_offering_configuration)
    }
}

/// One channel record's configuration: its class, and the three members of its
/// `InputSpecification`.
fn channel_offering_configuration(channel: &ChannelRecord) -> ChannelOfferingConfiguration {
    let specification = channel.input_specification.as_ref();
    ChannelOfferingConfiguration {
        channel_class: Some(channel.channel_class.clone()),
        codec: specification.and_then(|specification| specification.codec.clone()),
        maximum_bitrate: specification
            .and_then(|specification| specification.maximum_bitrate.clone()),
        resolution: specification.and_then(|specification| specification.resolution.clone()),
    }
}
