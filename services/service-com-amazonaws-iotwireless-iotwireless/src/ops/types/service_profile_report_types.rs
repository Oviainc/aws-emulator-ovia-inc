// SPDX-License-Identifier: CC0-1.0

//! The reported LoRaWAN service-profile settings, declared against this crate's LOCAL
//! model extension rather than the shipped service model.
//!
//! The service reports two members on a service-profile read — `AdrAlgorithm` and
//! `UplinkHistoryLen` — that the shipped `smithy-model/com-amazonaws-iotwireless-iotwireless.json`
//! snapshot does not declare on `LoRaWANGetServiceProfileInfo`, and the serializer is
//! schema-driven: it can only write a member the model behind the struct declares. That
//! snapshot is kept exactly as shipped, so this one shape is declared instead against
//! `smithy-model/local-reported-service-profile.json`, a small model this crate owns
//! that carries the snapshot's 23 members (each with the same target shape, type and
//! constraints) plus those two. Nothing else moves: `GetServiceProfileOutput` and every
//! other shape still come from the shipped snapshot, and a nested structure member is
//! serialized through its own type's schema, which is the one below.
//!
//! Keeping the two models side by side is deliberate: a later refresh of the shipped
//! snapshot replaces that file alone, and this shape keeps reporting what the service
//! reports. If a refresh declares the two members itself, this module and its model can
//! be deleted and the struct moved back to `get_service_profile_types`.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/local-reported-service-profile.json");

#[smithy_macros::smithy(shape = "com.amazonaws.iotwireless#LoRaWANGetServiceProfileInfo")]
#[derive(Debug, Clone, Default)]
pub struct LoRaWANGetServiceProfileInfo {
    pub ul_rate: Option<i32>,
    pub ul_bucket_size: Option<i32>,
    pub ul_rate_policy: Option<String>,
    pub dl_rate: Option<i32>,
    pub dl_bucket_size: Option<i32>,
    pub dl_rate_policy: Option<String>,
    pub add_gw_metadata: bool,
    pub dev_status_req_freq: Option<i32>,
    pub report_dev_status_battery: bool,
    pub report_dev_status_margin: bool,
    pub dr_min: i32,
    pub dr_max: i32,
    pub channel_mask: Option<String>,
    pub pr_allowed: bool,
    pub hr_allowed: bool,
    pub ra_allowed: bool,
    pub nwk_geo_loc: bool,
    pub target_per: i32,
    pub min_gw_diversity: Option<i32>,
    pub tx_power_index_min: Option<i32>,
    pub tx_power_index_max: Option<i32>,
    pub nb_trans_min: Option<i32>,
    pub nb_trans_max: Option<i32>,
    /// The two members the shipped snapshot does not declare, which the service reports.
    pub adr_algorithm: Option<String>,
    pub uplink_history_len: Option<i32>,
}
