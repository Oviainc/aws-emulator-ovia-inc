// SPDX-License-Identifier: CC0-1.0

//! What AWS Ground Station refuses, and the identity it refuses with — end to end over
//! HTTP through the same front door as the rest of this target's suites.
//!
//! Three rules, each asserted over a table that spans the class rather than the one
//! recorded request:
//!
//! * an antenna-uplink `Config` is created only for a frequency this account is
//!   approved to transmit on, and an account holds no uplink approval until one is
//!   granted outside the API — so by default every one of them is refused with
//!   `InvalidParameterException` (HTTP 431);
//! * a `Config` whose `configData` carries a numeric value outside the range the model
//!   documents for that member is refused the same way and nothing is stored — the
//!   value being compared after conversion from whichever `FrequencyUnits` the caller
//!   wrote it in;
//! * a request the model's own rules refuse (a missing required member, a value outside
//!   a declared bound) is answered with HTTP 400 `BadRequestException` and the service's
//!   one fixed sentence, whatever operation it was addressed to;
//! * an s3-recording `Config` is created only for a bucket whose location Ground
//!   Station can read — a bucket it has no record of, or an ARN that names no bucket,
//!   is refused.

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    parse_approved_uplink_bands, parse_recording_buckets, service_state, BAD_REQUEST_ERROR_CODE,
    INVALID_REQUEST_BODY, UNAPPROVED_UPLINK_FREQUENCY, UNRESOLVABLE_RECORDING_BUCKET,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::scheduling_loop::{harness, harness_with_engine, send, Reply};

/// A target EIRP inside the documented 20.0–50.0 dBW range, for a row that is varying
/// something else.
const VALID_EIRP: f64 = 30.0;

/// An uplink center frequency inside the documented 2025–2120 MHz band, for a row that
/// is varying something else. Inside [`UPLINK_APPROVAL_MHZ`] too, so a row that grants
/// that band is accepted on this frequency.
const VALID_UPLINK_MHZ: f64 = 2072.5;

/// The uplink band a test grants the account where it needs an uplink config to be
/// created at all: a sub-band of the 2025–2120 MHz the service operates, narrow enough
/// that a frequency inside the service's band but outside the grant is a row of its own.
const UPLINK_APPROVAL_MHZ: (f64, f64) = (2050.0, 2100.0);

/// A downlink center frequency inside the documented 7750–8400 MHz band.
const VALID_DOWNLINK_MHZ: f64 = 7812.5;

/// A bandwidth inside both documented bandwidth ranges (10 kHz–54 MHz for
/// `antennaDownlinkConfig`, 125 kHz–650 MHz for the demod-decode one).
const VALID_BANDWIDTH_MHZ: f64 = 20.0;

/// One row of a range table: what to send, and whether the service keeps it.
struct Case {
    /// Names the row in every assertion.
    label: &'static str,
    config_data: Value,
    /// `true` when the value is inside the documented range, so the create succeeds.
    accepted: bool,
}

/// `{"value": v, "units": u}` — the shape every `Frequency`, `FrequencyBandwidth` and
/// `Eirp` takes.
fn quantity(value: f64, units: &str) -> Value {
    json!({"value": value, "units": units})
}

fn uplink_config(center: Value, eirp: Value) -> Value {
    uplink_config_with_transmit(center, eirp, false)
}

fn uplink_config_with_transmit(center: Value, eirp: Value, transmit_disabled: bool) -> Value {
    json!({"antennaUplinkConfig": {
        "transmitDisabled": transmit_disabled,
        "spectrumConfig": {"centerFrequency": center, "polarization": "LEFT_HAND"},
        "targetEirp": eirp,
    }})
}

/// Grant this account an uplink approval for `band`, the way AWS does outside the API.
fn approve_uplink_band(engine: &engine::InMemoryEngine, band: (f64, f64)) {
    service_state(engine)
        .expect("this service's state store")
        .lock()
        .approved_uplink_bands_mhz
        .push(band);
}

fn downlink_config(center: Value, bandwidth: Value) -> Value {
    json!({"antennaDownlinkConfig": {"spectrumConfig": {
        "centerFrequency": center,
        "bandwidth": bandwidth,
        "polarization": "RIGHT_HAND",
    }}})
}

fn demod_decode_config(center: Value, bandwidth: Value) -> Value {
    json!({"antennaDownlinkDemodDecodeConfig": {
        "spectrumConfig": {
            "centerFrequency": center,
            "bandwidth": bandwidth,
            "polarization": "RIGHT_HAND",
        },
        "demodulationConfig": {"unvalidatedJSON": "{}"},
        "decodeConfig": {"unvalidatedJSON": "{}"},
    }})
}

/// `POST /config`, as these tests send it.
async fn create(router: &MultiServiceRouter, name: &str, config_data: &Value) -> Reply {
    send(
        router,
        "POST",
        "/config",
        Some(json!({"name": name, "configData": config_data})),
    )
    .await
}

/// Assert one range-table row: an accepted value is created and reads back unchanged, a
/// refused one is a 431 `InvalidParameterException` that stored nothing.
async fn assert_case(router: &MultiServiceRouter, index: usize, case: &Case) {
    let label = case.label;
    let name = format!("range-case-{index}");
    let reply = create(router, &name, &case.config_data).await;

    if !case.accepted {
        assert_eq!(
            reply.status, 431,
            "case {label}: expected CreateConfig 431, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidParameterException",
            "case {label}: expected InvalidParameterException, got {}",
            reply.body
        );
        assert!(
            reply
                .body
                .get("message")
                .and_then(Value::as_str)
                .is_some_and(|message| !message.is_empty()),
            "case {label}: the refusal must carry a message, got {}",
            reply.body
        );
        assert!(
            reply
                .body
                .get("parameterName")
                .and_then(Value::as_str)
                .is_some_and(|parameter| parameter.starts_with("configData.")),
            "case {label}: the refusal must name the configData member at fault, got {}",
            reply.body
        );
        return;
    }

    assert_eq!(
        reply.status, 200,
        "case {label}: expected CreateConfig 200, got {} {}",
        reply.status, reply.body
    );
    let config_id = reply.string("configId");
    let config_type = reply.string("configType");
    let read_back = send(
        router,
        "GET",
        &format!("/config/{config_type}/{config_id}"),
        None,
    )
    .await;
    assert_eq!(
        read_back.status, 200,
        "case {label}: expected GetConfig 200, got {} {}",
        read_back.status, read_back.body
    );
    assert_eq!(
        read_back.field("configData"),
        &case.config_data,
        "case {label}: GetConfig must return the configData that was created, got {}",
        read_back.field("configData")
    );
}

/// How many configs the store holds.
///
/// Read from the store rather than through `ListConfigs`, which is still a scaffolded
/// 501 (its own issue): what has to be observable here is that a refused create wrote
/// NOTHING, and the store is where that is visible.
fn config_count(engine: &engine::InMemoryEngine) -> usize {
    service_state(engine)
        .expect("this service's state store")
        .lock()
        .configs
        .len()
}

/// A unit outside the one its member's enum models is refused BEFORE the handler, by
/// rule 2's envelope: `FrequencyUnits`, `BandwidthUnits` and `EirpUnits` are modelled
/// value sets, so a token outside them fails the model's own rules and is answered with
/// HTTP 400 `BadRequestException` and the service's one fixed sentence. This is what
/// lets the range checks treat the unit as one of the modelled ones; nothing is stored.
#[tokio::test]
async fn a_unit_outside_the_modelled_enum_is_refused_before_the_handler() {
    let (router, engine) = harness_with_engine();
    approve_uplink_band(&engine, UPLINK_APPROVAL_MHZ);
    // Source for the value sets: `smithy_query(query="shape",
    // shape="FrequencyUnits,BandwidthUnits,EirpUnits")` — GHz/MHz/kHz for the two
    // frequency ones, dBW for EIRP. Each row writes a unit outside them.
    let cases: Vec<(&str, Value)> = vec![
        (
            "an uplink centre frequency in Hz",
            uplink_config(quantity(2.0725e9, "Hz"), quantity(VALID_EIRP, "dBW")),
        ),
        (
            "a target EIRP in dBm",
            uplink_config(quantity(VALID_UPLINK_MHZ, "MHz"), quantity(VALID_EIRP, "dBm")),
        ),
        (
            "a downlink centre frequency in THz",
            downlink_config(quantity(0.0078125, "THz"), quantity(VALID_BANDWIDTH_MHZ, "MHz")),
        ),
        (
            "a bandwidth in Hz",
            downlink_config(quantity(VALID_DOWNLINK_MHZ, "MHz"), quantity(20e6, "Hz")),
        ),
        (
            "a unit that is the empty string",
            downlink_config(quantity(VALID_DOWNLINK_MHZ, "MHz"), quantity(20.0, "")),
        ),
        (
            "a unit that differs only in case",
            downlink_config(quantity(VALID_DOWNLINK_MHZ, "MHz"), quantity(20.0, "mhz")),
        ),
    ];

    for (label, config_data) in &cases {
        let reply = create(&router, "unsupported-units", config_data).await;
        assert_eq!(
            reply.status, 400,
            "case {label}: expected 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            BAD_REQUEST_ERROR_CODE,
            "case {label}: expected {BAD_REQUEST_ERROR_CODE}, got {}",
            reply.body
        );
        assert_eq!(
            reply.string("message"),
            INVALID_REQUEST_BODY,
            "case {label}: wrong refusal text, got {}",
            reply.body
        );
    }
    assert_eq!(
        config_count(&engine),
        0,
        "a refused create must store nothing"
    );
}

/// An account that has been granted no uplink approval cannot create an antenna-uplink
/// `Config` AT ALL: whatever the request asks for, the answer is HTTP 431
/// `InvalidParameterException` with the service's fixed sentence for the condition, and
/// nothing is stored. The rows vary everything the request can vary — the frequency
/// (including the two bounds of the band the service itself operates), the unit it is
/// written in, whether transmit is disabled, and the target EIRP — because none of them
/// is what the refusal turns on.
#[tokio::test]
async fn an_account_with_no_uplink_approval_cannot_create_an_antenna_uplink_config() {
    let (router, engine) = harness_with_engine();
    let cases: Vec<(&str, Value)> = vec![
        (
            "2025 MHz — the lower bound of the band the service operates",
            uplink_config(quantity(2025.0, "MHz"), quantity(VALID_EIRP, "dBW")),
        ),
        (
            "2120 MHz — the upper bound of that band",
            uplink_config(quantity(2120.0, "MHz"), quantity(VALID_EIRP, "dBW")),
        ),
        (
            "2072.5 MHz — mid band",
            uplink_config(
                quantity(VALID_UPLINK_MHZ, "MHz"),
                quantity(VALID_EIRP, "dBW"),
            ),
        ),
        (
            "2.0725 GHz — the same frequency in GHz",
            uplink_config(quantity(2.0725, "GHz"), quantity(VALID_EIRP, "dBW")),
        ),
        (
            "2072500 kHz — the same frequency in kHz",
            uplink_config(quantity(2_072_500.0, "kHz"), quantity(VALID_EIRP, "dBW")),
        ),
        (
            "2024.9 MHz — one step below the band",
            uplink_config(quantity(2024.9, "MHz"), quantity(VALID_EIRP, "dBW")),
        ),
        (
            "2120.1 MHz — one step above the band",
            uplink_config(quantity(2120.1, "MHz"), quantity(VALID_EIRP, "dBW")),
        ),
        (
            "2250 MHz — the DOWNLINK S band, never an uplink one",
            uplink_config(quantity(2250.0, "MHz"), quantity(VALID_EIRP, "dBW")),
        ),
        (
            "8.4 GHz — the downlink X band",
            uplink_config(quantity(8.4, "GHz"), quantity(VALID_EIRP, "dBW")),
        ),
        (
            "transmit disabled, in band",
            uplink_config_with_transmit(
                quantity(VALID_UPLINK_MHZ, "MHz"),
                quantity(VALID_EIRP, "dBW"),
                true,
            ),
        ),
        (
            "the lowest EIRP the model documents",
            uplink_config(quantity(VALID_UPLINK_MHZ, "MHz"), quantity(20.0, "dBW")),
        ),
    ];

    for (label, config_data) in &cases {
        let reply = create(&router, "unapproved-uplink", config_data).await;
        assert_eq!(
            reply.status, 431,
            "case {label}: expected 431, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidParameterException",
            "case {label}: expected InvalidParameterException, got {}",
            reply.body
        );
        assert_eq!(
            reply.string("message"),
            UNAPPROVED_UPLINK_FREQUENCY,
            "case {label}: wrong refusal text, got {}",
            reply.body
        );
        assert!(
            reply
                .body
                .get("parameterName")
                .and_then(Value::as_str)
                .is_some_and(|parameter| parameter.ends_with("centerFrequency")),
            "case {label}: the refusal must name the frequency member, got {}",
            reply.body
        );
    }
    assert_eq!(
        config_count(&engine),
        0,
        "a refused create must store nothing"
    );
}

/// Once the account holds an approval, an uplink `Config` is created exactly for the
/// frequencies that approval covers — judged after conversion from the unit the caller
/// used, and never outside the 2025–2120 MHz the service itself operates
/// (`smithy_query(query="doc", shape="UplinkSpectrumConfig", member="centerFrequency")`:
/// "Valid values are between 2025 to 2120 MHz"), which an approval cannot widen.
#[tokio::test]
async fn an_uplink_config_is_created_only_for_an_approved_frequency() {
    let (router, engine) = harness_with_engine();
    // Granted: 2050–2100 MHz, plus a band that runs below the 2025 MHz floor of what
    // the service operates, so the two limits can be told apart.
    approve_uplink_band(&engine, UPLINK_APPROVAL_MHZ);
    approve_uplink_band(&engine, (2000.0, 2030.0));

    let cases: Vec<Case> = vec![
        Case {
            label: "2050 MHz — the lower bound of the approved band",
            config_data: uplink_config(quantity(2050.0, "MHz"), quantity(VALID_EIRP, "dBW")),
            accepted: true,
        },
        Case {
            label: "2100 MHz — the upper bound of the approved band",
            config_data: uplink_config(quantity(2100.0, "MHz"), quantity(VALID_EIRP, "dBW")),
            accepted: true,
        },
        Case {
            label: "2072.5 MHz — inside the approved band",
            config_data: uplink_config(
                quantity(VALID_UPLINK_MHZ, "MHz"),
                quantity(VALID_EIRP, "dBW"),
            ),
            accepted: true,
        },
        Case {
            label: "2.0725 GHz — the same frequency in GHz",
            config_data: uplink_config(quantity(2.0725, "GHz"), quantity(VALID_EIRP, "dBW")),
            accepted: true,
        },
        Case {
            label: "2072500 kHz — the same frequency in kHz",
            config_data: uplink_config(quantity(2_072_500.0, "kHz"), quantity(VALID_EIRP, "dBW")),
            accepted: true,
        },
        Case {
            label: "transmit disabled, inside the approved band",
            config_data: uplink_config_with_transmit(
                quantity(VALID_UPLINK_MHZ, "MHz"),
                quantity(VALID_EIRP, "dBW"),
                true,
            ),
            accepted: true,
        },
        Case {
            label: "2025 MHz — approved, and the floor of what the service operates",
            config_data: uplink_config(quantity(2025.0, "MHz"), quantity(VALID_EIRP, "dBW")),
            accepted: true,
        },
        Case {
            label: "2049.9 MHz — one step below the approved band, and unapproved",
            config_data: uplink_config(quantity(2049.9, "MHz"), quantity(VALID_EIRP, "dBW")),
            accepted: false,
        },
        Case {
            label: "2100.1 MHz — one step above the approved band",
            config_data: uplink_config(quantity(2100.1, "MHz"), quantity(VALID_EIRP, "dBW")),
            accepted: false,
        },
        Case {
            label: "2110 MHz — inside the service's band, outside every approval",
            config_data: uplink_config(quantity(2110.0, "MHz"), quantity(VALID_EIRP, "dBW")),
            accepted: false,
        },
        Case {
            label: "2024.9 MHz — approved, but below the band the service operates",
            config_data: uplink_config(quantity(2024.9, "MHz"), quantity(VALID_EIRP, "dBW")),
            accepted: false,
        },
        Case {
            label: "2.0 GHz — approved, below the service's band, in GHz",
            config_data: uplink_config(quantity(2.0, "GHz"), quantity(VALID_EIRP, "dBW")),
            accepted: false,
        },
    ];

    let accepted = cases.iter().filter(|case| case.accepted).count();
    for (index, case) in cases.iter().enumerate() {
        assert_case(&router, index, case).await;
    }
    assert_eq!(
        config_count(&engine),
        accepted,
        "only the accepted rows may be stored"
    );
}

/// A downlink `Config`'s center frequency must fall in one of the two bands
/// `SpectrumConfig$centerFrequency` documents — "between 2200 to 2300 MHz and 7750 to
/// 8400 MHz for downlink" (`smithy_query(query="doc", shape="SpectrumConfig",
/// member="centerFrequency")`) — including the gap between them, which is in neither.
#[tokio::test]
async fn downlink_center_frequency_must_be_inside_one_of_the_documented_bands() {
    let (router, engine) = harness_with_engine();
    let bandwidth = quantity(VALID_BANDWIDTH_MHZ, "MHz");
    let cases = vec![
        Case {
            label: "2200 MHz — lower bound of the S band",
            config_data: downlink_config(quantity(2200.0, "MHz"), bandwidth.clone()),
            accepted: true,
        },
        Case {
            label: "2300 MHz — upper bound of the S band",
            config_data: downlink_config(quantity(2300.0, "MHz"), bandwidth.clone()),
            accepted: true,
        },
        Case {
            label: "7750 MHz — lower bound of the X band",
            config_data: downlink_config(quantity(7750.0, "MHz"), bandwidth.clone()),
            accepted: true,
        },
        Case {
            label: "8.4 GHz — upper bound of the X band, in GHz",
            config_data: downlink_config(quantity(8.4, "GHz"), bandwidth.clone()),
            accepted: true,
        },
        Case {
            label: "2199.9 MHz — one step below the S band",
            config_data: downlink_config(quantity(2199.9, "MHz"), bandwidth.clone()),
            accepted: false,
        },
        Case {
            label: "2300.1 MHz — one step above the S band",
            config_data: downlink_config(quantity(2300.1, "MHz"), bandwidth.clone()),
            accepted: false,
        },
        Case {
            label: "5000 MHz — the gap between the two bands",
            config_data: downlink_config(quantity(5000.0, "MHz"), bandwidth.clone()),
            accepted: false,
        },
        Case {
            label: "7749.9 MHz — one step below the X band",
            config_data: downlink_config(quantity(7749.9, "MHz"), bandwidth.clone()),
            accepted: false,
        },
        Case {
            label: "8400.1 MHz — one step above the X band",
            config_data: downlink_config(quantity(8400.1, "MHz"), bandwidth.clone()),
            accepted: false,
        },
        Case {
            label: "2050 MHz — the UPLINK band, not a downlink one",
            config_data: downlink_config(quantity(VALID_UPLINK_MHZ, "MHz"), bandwidth.clone()),
            accepted: false,
        },
        Case {
            label: "demod-decode at 2250 MHz — the same bands apply",
            config_data: demod_decode_config(quantity(2250.0, "MHz"), bandwidth.clone()),
            accepted: true,
        },
        Case {
            label: "demod-decode at 9000 MHz — above every downlink band",
            config_data: demod_decode_config(quantity(9000.0, "MHz"), bandwidth),
            accepted: false,
        },
    ];

    let accepted = cases.iter().filter(|case| case.accepted).count();
    for (index, case) in cases.iter().enumerate() {
        assert_case(&router, index, case).await;
    }
    assert_eq!(
        config_count(&engine),
        accepted,
        "only the accepted rows may be stored"
    );
}

/// Bandwidth is bounded per config type, from `FrequencyBandwidth$value`'s own
/// documentation (`smithy_query(query="doc", shape="FrequencyBandwidth",
/// member="value")`): "For AntennaDownlinkDemodDecodeconfig, valid values are between
/// 125 kHz to 650 MHz. For AntennaDownlinkconfig, valid values are between 10 kHz to 54
/// MHz." So one bandwidth can be valid for one config type and not for the other.
#[tokio::test]
async fn bandwidth_must_be_inside_the_range_its_config_type_documents() {
    let (router, engine) = harness_with_engine();
    let center = quantity(VALID_DOWNLINK_MHZ, "MHz");
    let cases = vec![
        Case {
            label: "antenna-downlink, 10 kHz — the documented lower bound",
            config_data: downlink_config(center.clone(), quantity(10.0, "kHz")),
            accepted: true,
        },
        Case {
            label: "antenna-downlink, 54 MHz — the documented upper bound",
            config_data: downlink_config(center.clone(), quantity(54.0, "MHz")),
            accepted: true,
        },
        Case {
            label: "antenna-downlink, 9 kHz — below the lower bound",
            config_data: downlink_config(center.clone(), quantity(9.0, "kHz")),
            accepted: false,
        },
        Case {
            label: "antenna-downlink, 54.1 MHz — above the upper bound",
            config_data: downlink_config(center.clone(), quantity(54.1, "MHz")),
            accepted: false,
        },
        Case {
            label: "antenna-downlink, 0.1 GHz — 100 MHz, above the upper bound in GHz",
            config_data: downlink_config(center.clone(), quantity(0.1, "GHz")),
            accepted: false,
        },
        Case {
            label: "demod-decode, 125 kHz — the documented lower bound",
            config_data: demod_decode_config(center.clone(), quantity(125.0, "kHz")),
            accepted: true,
        },
        Case {
            label: "demod-decode, 650 MHz — the documented upper bound",
            config_data: demod_decode_config(center.clone(), quantity(650.0, "MHz")),
            accepted: true,
        },
        Case {
            label: "demod-decode, 100 kHz — valid for antenna-downlink, not for this type",
            config_data: demod_decode_config(center.clone(), quantity(100.0, "kHz")),
            accepted: false,
        },
        Case {
            label: "demod-decode, 650.5 MHz — above the upper bound",
            config_data: demod_decode_config(center.clone(), quantity(650.5, "MHz")),
            accepted: false,
        },
        Case {
            label: "antenna-downlink, 100 kHz — valid for this type, not for demod-decode",
            config_data: downlink_config(center, quantity(100.0, "kHz")),
            accepted: true,
        },
    ];

    let accepted = cases.iter().filter(|case| case.accepted).count();
    for (index, case) in cases.iter().enumerate() {
        assert_case(&router, index, case).await;
    }
    assert_eq!(
        config_count(&engine),
        accepted,
        "only the accepted rows may be stored"
    );
}

/// An uplink `Config`'s target EIRP must fall in the range `Eirp$value` documents —
/// "Valid values are between 20.0 to 50.0 dBW" (`smithy_query(query="doc", shape="Eirp",
/// member="value")`).
#[tokio::test]
async fn target_eirp_must_be_inside_the_documented_range() {
    let (router, engine) = harness_with_engine();
    // The frequency is not what these rows vary, so it is one this account may use.
    approve_uplink_band(&engine, UPLINK_APPROVAL_MHZ);
    let center = quantity(VALID_UPLINK_MHZ, "MHz");
    let cases: Vec<Case> = vec![
        Case {
            label: "20.0 dBW — the documented lower bound",
            config_data: uplink_config(center.clone(), quantity(20.0, "dBW")),
            accepted: true,
        },
        Case {
            label: "50.0 dBW — the documented upper bound",
            config_data: uplink_config(center.clone(), quantity(50.0, "dBW")),
            accepted: true,
        },
        Case {
            label: "35.5 dBW — mid range",
            config_data: uplink_config(center.clone(), quantity(35.5, "dBW")),
            accepted: true,
        },
        Case {
            label: "19.9 dBW — one step below the lower bound",
            config_data: uplink_config(center.clone(), quantity(19.9, "dBW")),
            accepted: false,
        },
        Case {
            label: "0.0 dBW — far below the lower bound",
            config_data: uplink_config(center.clone(), quantity(0.0, "dBW")),
            accepted: false,
        },
        Case {
            label: "50.1 dBW — one step above the upper bound",
            config_data: uplink_config(center.clone(), quantity(50.1, "dBW")),
            accepted: false,
        },
        Case {
            label: "500.0 dBW — far above the upper bound",
            config_data: uplink_config(center, quantity(500.0, "dBW")),
            accepted: false,
        },
    ];

    let accepted = cases.iter().filter(|case| case.accepted).count();
    for (index, case) in cases.iter().enumerate() {
        assert_case(&router, index, case).await;
    }
    assert_eq!(
        config_count(&engine),
        accepted,
        "only the accepted rows may be stored"
    );
}

/// Every request this service's own rules refuse — a required member absent, a value
/// outside a declared bound — is answered with HTTP 400, the error code
/// `BadRequestException` (which is what an SDK resolves the error shape from) and one
/// fixed sentence, whatever operation it was addressed to and whichever member failed.
///
/// `GetMinuteUsage` is the operation the recording covers: its `month` is `Month`
/// (`@range` 1..12) and its `year` is `Year` (`@range` 2018..3000)
/// (`smithy_query(query="operation", operation="GetMinuteUsage")`).
#[tokio::test]
async fn a_request_the_model_refuses_is_a_bad_request_exception() {
    let router = harness();
    let cases: Vec<(&str, &str, &str, Value)> = vec![
        (
            "month above the modelled maximum",
            "POST",
            "/minute-usage",
            json!({"month": 13, "year": 2024}),
        ),
        (
            "month below the modelled minimum",
            "POST",
            "/minute-usage",
            json!({"month": 0, "year": 2024}),
        ),
        (
            "year below the modelled minimum",
            "POST",
            "/minute-usage",
            json!({"month": 6, "year": 2017}),
        ),
        (
            "year above the modelled maximum",
            "POST",
            "/minute-usage",
            json!({"month": 6, "year": 3001}),
        ),
        (
            "month absent",
            "POST",
            "/minute-usage",
            json!({"year": 2024}),
        ),
        ("year absent", "POST", "/minute-usage", json!({"month": 6})),
        (
            "a config name outside the modelled pattern",
            "POST",
            "/config",
            json!({
                "name": "not/a/safe/name",
                "configData": {"trackingConfig": {"autotrack": "REQUIRED"}},
            }),
        ),
        (
            "a config name longer than the modelled maximum",
            "POST",
            "/config",
            json!({
                "name": "x".repeat(257),
                "configData": {"trackingConfig": {"autotrack": "REQUIRED"}},
            }),
        ),
        (
            "configData absent",
            "POST",
            "/config",
            json!({"name": "no-config-data"}),
        ),
    ];

    for (label, method, path, body) in cases {
        let reply = send(&router, method, path, Some(body)).await;
        assert_eq!(
            reply.status, 400,
            "case {label}: expected 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            BAD_REQUEST_ERROR_CODE,
            "case {label}: expected {BAD_REQUEST_ERROR_CODE}, got {}",
            reply.body
        );
        assert_eq!(
            reply.string("message"),
            INVALID_REQUEST_BODY,
            "case {label}: wrong refusal text, got {}",
            reply.body
        );
    }
}

/// The same refusal must not swallow a request the model ACCEPTS. Each bound of
/// `GetMinuteUsage`'s two ranges is inside them, so none of these is a 400 — the answer
/// is this operation's 501 `NotImplemented`, which is its own scaffolded-operation
/// issue, not this one.
#[tokio::test]
async fn a_request_inside_the_modelled_bounds_is_not_refused_as_a_bad_request() {
    let router = harness();
    // Each published bound of `Month` (1..12) and `Year` (2018..3000), plus a mid value.
    for (label, body) in [
        ("month 1, year 2018 — both lower bounds", json!({"month": 1, "year": 2018})),
        ("month 12, year 3000 — both upper bounds", json!({"month": 12, "year": 3000})),
        ("month 6, year 2024 — mid range", json!({"month": 6, "year": 2024})),
    ] {
        let reply = send(&router, "POST", "/minute-usage", Some(body)).await;
        assert_ne!(
            reply.status, 400,
            "case {label}: a request inside the modelled bounds must not be refused, got {}",
            reply.body
        );
        assert_ne!(
            reply.error_code(),
            BAD_REQUEST_ERROR_CODE,
            "case {label}: expected no BadRequestException, got {}",
            reply.body
        );
    }
}

/// An UPDATE is held to the same rule as a create: `UpdateConfig` replaces a config's
/// parameters, so parameters the service and this account may not use are refused with
/// HTTP 431 `InvalidParameterException` and the stored config is left as it was.
#[tokio::test]
async fn update_config_is_held_to_the_same_configdata_rule_as_create() {
    let (router, engine) = harness_with_engine();
    approve_uplink_band(&engine, UPLINK_APPROVAL_MHZ);

    // The config the updates below address: valid parameters, created and read back.
    let original = downlink_config(
        quantity(VALID_DOWNLINK_MHZ, "MHz"),
        quantity(VALID_BANDWIDTH_MHZ, "MHz"),
    );
    let created = create(&router, "update-subject", &original).await;
    assert_eq!(
        created.status, 200,
        "the config to update must be created, got {} {}",
        created.status, created.body
    );
    let config_id = created.string("configId");
    let path = format!("/config/antenna-downlink/{config_id}");

    // Each row is a configData the create path refuses, for a different reason: the
    // centre frequency, the bandwidth, and (under the config's own type) an uplink
    // frequency this account may not transmit on.
    for (label, config_data, expected_message) in [
        (
            "a centre frequency outside every downlink band",
            downlink_config(quantity(5000.0, "MHz"), quantity(VALID_BANDWIDTH_MHZ, "MHz")),
            None,
        ),
        (
            "a bandwidth above the documented maximum",
            downlink_config(quantity(VALID_DOWNLINK_MHZ, "MHz"), quantity(54.1, "MHz")),
            None,
        ),
        (
            "an unapproved uplink frequency, under the uplink type",
            uplink_config(quantity(2110.0, "MHz"), quantity(VALID_EIRP, "dBW")),
            Some(UNAPPROVED_UPLINK_FREQUENCY),
        ),
    ] {
        let uplink = config_data.get("antennaUplinkConfig").is_some();
        let target = if uplink {
            // The uplink row addresses an uplink config of its own, since an update may
            // not change a config's type: created here, now that the account is approved.
            let created = create(&router, "update-subject-uplink", &uplink_config(
                quantity(VALID_UPLINK_MHZ, "MHz"),
                quantity(VALID_EIRP, "dBW"),
            ))
            .await;
            assert_eq!(
                created.status, 200,
                "case {label}: the uplink config to update must be created, got {} {}",
                created.status, created.body
            );
            format!("/config/antenna-uplink/{}", created.string("configId"))
        } else {
            path.clone()
        };
        let reply = send(
            &router,
            "PUT",
            &target,
            Some(json!({"name": "updated", "configData": config_data})),
        )
        .await;
        assert_eq!(
            reply.status, 431,
            "case {label}: expected UpdateConfig 431, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidParameterException",
            "case {label}: expected InvalidParameterException, got {}",
            reply.body
        );
        if let Some(message) = expected_message {
            assert_eq!(
                reply.string("message"),
                message,
                "case {label}: wrong refusal text, got {}",
                reply.body
            );
        }
    }

    // The refused updates left the config exactly as it was created.
    let read_back = send(&router, "GET", &path, None).await;
    assert_eq!(
        read_back.status, 200,
        "GetConfig after the refused updates: got {} {}",
        read_back.status, read_back.body
    );
    assert_eq!(
        read_back.string("name"),
        "update-subject",
        "a refused update must not have replaced the name, got {}",
        read_back.body
    );
    assert_eq!(
        read_back.field("configData"),
        &original,
        "a refused update must not have replaced the parameters, got {}",
        read_back.field("configData")
    );

    // An update INSIDE the ranges still replaces them.
    let replacement = downlink_config(quantity(2250.0, "MHz"), quantity(0.01, "MHz"));
    let updated = send(
        &router,
        "PUT",
        &path,
        Some(json!({"name": "update-subject-2", "configData": replacement})),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "an update inside the documented ranges must succeed, got {} {}",
        updated.status, updated.body
    );
    let read_back = send(&router, "GET", &path, None).await;
    assert_eq!(
        read_back.field("configData"),
        &replacement,
        "the accepted update must have replaced the parameters, got {}",
        read_back.field("configData")
    );
}

/// The uplink approval an operator grants at start-up through
/// `state::APPROVED_UPLINK_BANDS_ENV` reads as documented: `<low>-<high>` MHz pairs,
/// comma-separated, and an entry that is not one of those grants nothing rather than
/// being guessed at. The rows differ along count, spacing, decimals, and each way an
/// entry can be malformed.
#[test]
fn the_uplink_approval_setting_parses_as_documented() {
    for (setting, expected) in [
        ("", vec![]),
        ("2025-2120", vec![(2025.0, 2120.0)]),
        ("2050-2100, 2110-2120", vec![(2050.0, 2100.0), (2110.0, 2120.0)]),
        (" 2050.5 - 2060.5 ", vec![(2050.5, 2060.5)]),
        ("2025-2025", vec![(2025.0, 2025.0)]),
        ("2120-2025", vec![]),
        ("2025", vec![]),
        ("2025-", vec![]),
        ("-2120", vec![]),
        ("not a band", vec![]),
        ("2050-2100,oops", vec![(2050.0, 2100.0)]),
    ] {
        let parsed = parse_approved_uplink_bands(setting);
        assert_eq!(
            parsed, expected,
            "case {setting:?}: expected {expected:?}, got {parsed:?}"
        );
    }
}

/// The recording buckets an operator names at start-up through
/// `state::RECORDING_BUCKETS_ENV` read as documented: comma-separated names, surrounding
/// space trimmed, empty entries dropped.
#[test]
fn the_recording_buckets_setting_parses_as_documented() {
    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("", vec![]),
        ("aws-groundstation-downlink", vec!["aws-groundstation-downlink"]),
        (" gs , my.recording-bucket-01 ", vec!["gs", "my.recording-bucket-01"]),
        (",,", vec![]),
        ("gs,,other", vec!["gs", "other"]),
    ];
    for (setting, expected) in cases {
        let parsed = parse_recording_buckets(setting);
        assert_eq!(
            parsed, expected,
            "case {setting:?}: expected {expected:?}, got {parsed:?}"
        );
    }
}

/// Tell this account's Ground Station that it can read the location of `buckets`, the
/// way [`state::RECORDING_BUCKETS_ENV`] does at start-up.
fn make_buckets_readable(engine: &engine::InMemoryEngine, buckets: &[&str]) {
    service_state(engine)
        .expect("this service's state store")
        .lock()
        .readable_recording_buckets
        .extend(buckets.iter().map(|bucket| (*bucket).to_string()));
}

fn s3_recording_config(bucket: &str) -> Value {
    json!({"s3RecordingConfig": {
        "bucketArn": format!("arn:aws:s3:::{bucket}"),
        "roleArn": "arn:aws:iam::000000000000:role/GroundStationS3Role",
        "prefix": "data/{satellite_id}/{year}/{month}/{day}",
    }})
}

/// An `s3RecordingConfig` is created only for a bucket whose location Ground Station can
/// read: one it can is created and reads back, and one it cannot — a bucket it has no
/// record of, or an ARN that names no bucket at all — is refused with HTTP 431
/// `InvalidParameterException` and the service's fixed sentence for the condition,
/// whatever the bucket is named and however the ARN is written.
#[tokio::test]
async fn an_s3_recording_config_needs_a_bucket_whose_location_can_be_read() {
    let (router, engine) = harness_with_engine();

    // Bucket names that differ along what the rule generalises over: length, the
    // characters S3 allows in a name, and whether anything about the name looks like a
    // Ground Station bucket. Each is one this service can locate, so each is accepted.
    // "role" and "instance" are locatable too, which is what makes the ARN-shape rows
    // below fail on the ARN rather than on the name being unknown.
    let existing = ["gs", "aws-groundstation-downlink", "my.recording-bucket-01"];
    make_buckets_readable(&engine, &existing);
    make_buckets_readable(&engine, &["role", "instance"]);
    for (index, bucket) in existing.iter().enumerate() {
        let config_data = s3_recording_config(bucket);
        let reply = create(&router, &format!("recording-{index}"), &config_data).await;
        assert_eq!(
            reply.status, 200,
            "bucket {bucket}: expected CreateConfig 200, got {} {}",
            reply.status, reply.body
        );
        let config_id = reply.string("configId");
        let read_back = send(
            &router,
            "GET",
            &format!("/config/s3-recording/{config_id}"),
            None,
        )
        .await;
        assert_eq!(
            read_back.status, 200,
            "bucket {bucket}: expected GetConfig 200, got {} {}",
            read_back.status, read_back.body
        );
        assert_eq!(
            read_back.field("configData"),
            &config_data,
            "bucket {bucket}: GetConfig must return the configData that was created, got {}",
            read_back.field("configData")
        );
    }
    assert_eq!(
        config_count(&engine),
        existing.len(),
        "exactly the configs whose bucket can be located may be stored"
    );

    // A bucket this service cannot locate, ARNs of other services, and malformed ARNs:
    // the location cannot be read in any of these, so none of them is created. The rows
    // naming another service's resource matter because `BucketArn` carries no model
    // pattern, so the boundary validator passes any string: a `role/<name>` or
    // `instance/<id>` resource part must not be read as a bucket name, even when a bucket
    // of that name IS locatable — "role" and "instance" are, above.
    for (label, bucket_arn) in [
        ("a bucket this service cannot locate", "arn:aws:s3:::absent-bucket"),
        (
            "a bucket name that differs from a locatable one",
            "arn:aws:s3:::aws-groundstation-downlinks",
        ),
        ("an object ARN under a bucket it cannot locate", "arn:aws:s3:::absent-bucket/key"),
        (
            "an IAM role ARN whose resource part names a locatable bucket",
            "arn:aws:iam::000000000000:role/GroundStationS3Role",
        ),
        (
            "an EC2 instance ARN whose resource part names a locatable bucket",
            "arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0",
        ),
        (
            "an S3 access point ARN, which is not a bucket ARN",
            "arn:aws:s3:us-east-1:000000000000:accesspoint/gs",
        ),
        ("an ARN naming no bucket at all", "arn:aws:s3:::"),
        ("a string that is not an ARN", "not-an-arn"),
        ("an ARN of no service at all", "arn:aws:::::gs"),
    ] {
        let config_data = json!({"s3RecordingConfig": {
            "bucketArn": bucket_arn,
            "roleArn": "arn:aws:iam::000000000000:role/GroundStationS3Role",
        }});
        let reply = create(&router, "recording-refused", &config_data).await;
        assert_eq!(
            reply.status, 431,
            "case {label}: expected 431, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidParameterException",
            "case {label}: expected InvalidParameterException, got {}",
            reply.body
        );
        assert_eq!(
            reply.string("message"),
            UNRESOLVABLE_RECORDING_BUCKET,
            "case {label}: wrong refusal text, got {}",
            reply.body
        );
    }
    assert_eq!(
        config_count(&engine),
        existing.len(),
        "a refused create must store nothing"
    );
}
