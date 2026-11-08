// SPDX-License-Identifier: CC0-1.0

//! End-to-end behaviour of the Device Farm **network profile** operations:
//! `CreateNetworkProfile` and `GetNetworkProfile`.
//!
//! Every case drives the real wire path through the shared harness in
//! [`crate::projects`] — `POST /` with `X-Amz-Target: DeviceFarm_20150623.<Operation>`
//! through the multi-service router — so what is asserted is what an SDK client sees.
//!
//! The table below spans the class each rule generalises over: which shaping members the
//! request carries (none, all, each direction differently), their values at both
//! documented bounds, the type the request asks for, the name and description at their
//! modelled length bounds, and the region the request is signed for.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::projects::{arn_of, error_code, foreign_arn, resource_id, Emulator, DEFAULT_REGION, REGION};

/// The members a `NetworkProfile` response carries besides its ARN, so a row asserts the
/// WHOLE reported profile rather than the one member it varies.
const PROFILE_MEMBERS: [&str; 11] = [
    "name",
    "description",
    "type",
    "uplinkBandwidthBits",
    "downlinkBandwidthBits",
    "uplinkDelayMs",
    "downlinkDelayMs",
    "uplinkJitterMs",
    "downlinkJitterMs",
    "uplinkLossPercent",
    "downlinkLossPercent",
];

/// The profile a `CreateNetworkProfile` request resolves to when it shapes nothing,
/// patched with whatever the row expects differently.
///
/// The bounds come from the model's own member documentation, read with
/// `smithy_query(query="shape",shape="NetworkProfile")`: bandwidth "an integer from 0 to
/// 104857600", delay and jitter "milliseconds as an integer from 0 to 2000", and
/// `PercentInteger` range 0..100 with default 0. An unshaped direction is the full
/// 104857600 bits with no delay, jitter or loss, and a created profile's type is `PRIVATE`
/// — one of the two `NetworkProfileType` values, the other (`CURATED`) being the profiles
/// the service itself publishes.
fn expected_profile(name: &str, patches: Value) -> Value {
    let mut profile = json!({
        "name": name,
        "type": "PRIVATE",
        "uplinkBandwidthBits": 104_857_600_i64,
        "downlinkBandwidthBits": 104_857_600_i64,
        "uplinkDelayMs": 0,
        "downlinkDelayMs": 0,
        "uplinkJitterMs": 0,
        "downlinkJitterMs": 0,
        "uplinkLossPercent": 0,
        "downlinkLossPercent": 0,
    });
    for (member, value) in patches
        .as_object()
        .unwrap_or_else(|| panic!("the patch is an object: {patches}"))
    {
        profile[member] = value.clone();
    }
    profile
}

/// CreateNetworkProfile mints a profile under an existing project, resolves every shaping
/// member (keeping what the request shaped, defaulting what it did not), classifies the
/// profile as PRIVATE whatever type was asked for, and GetNetworkProfile answers with the
/// IDENTICAL stored profile.
#[tokio::test]
async fn created_network_profiles_are_persisted_and_read_back_identically() {
    let emu = Emulator::new();

    // (case, signing region, CreateNetworkProfile input, expected reported profile)
    let cases: Vec<(String, Option<&str>, Value, Value)> = vec![
        (
            "only the required members; every shaping member defaults".to_string(),
            REGION,
            json!({ "name": "defaults-everywhere" }),
            expected_profile("defaults-everywhere", json!({})),
        ),
        (
            "type CURATED asked for: the created profile is still PRIVATE".to_string(),
            REGION,
            json!({ "name": "asked-for-curated", "type": "CURATED" }),
            expected_profile("asked-for-curated", json!({})),
        ),
        (
            "type PRIVATE asked for".to_string(),
            REGION,
            json!({ "name": "asked-for-private", "type": "PRIVATE" }),
            expected_profile("asked-for-private", json!({})),
        ),
        (
            "every shaping member at its documented maximum".to_string(),
            REGION,
            json!({
                "name": "fully-throttled",
                "uplinkBandwidthBits": 104_857_600_i64,
                "downlinkBandwidthBits": 104_857_600_i64,
                "uplinkDelayMs": 2000,
                "downlinkDelayMs": 2000,
                "uplinkJitterMs": 2000,
                "downlinkJitterMs": 2000,
                "uplinkLossPercent": 100,
                "downlinkLossPercent": 100
            }),
            expected_profile(
                "fully-throttled",
                json!({
                    "uplinkDelayMs": 2000,
                    "downlinkDelayMs": 2000,
                    "uplinkJitterMs": 2000,
                    "downlinkJitterMs": 2000,
                    "uplinkLossPercent": 100,
                    "downlinkLossPercent": 100
                }),
            ),
        ),
        (
            "every shaping member explicitly zero: an explicit 0 bandwidth is kept, not \
             replaced by the unshaped default"
                .to_string(),
            REGION,
            json!({
                "name": "all-zero",
                "uplinkBandwidthBits": 0,
                "downlinkBandwidthBits": 0,
                "uplinkDelayMs": 0,
                "downlinkDelayMs": 0,
                "uplinkJitterMs": 0,
                "downlinkJitterMs": 0,
                "uplinkLossPercent": 0,
                "downlinkLossPercent": 0
            }),
            expected_profile(
                "all-zero",
                json!({ "uplinkBandwidthBits": 0, "downlinkBandwidthBits": 0 }),
            ),
        ),
        (
            "each direction shaped differently".to_string(),
            Some("eu-west-1"),
            json!({
                "name": "asymmetric",
                "type": "CURATED",
                "uplinkBandwidthBits": 1_048_576_i64,
                "downlinkBandwidthBits": 10_485_760_i64,
                "uplinkDelayMs": 10,
                "downlinkDelayMs": 20,
                "uplinkJitterMs": 1,
                "downlinkJitterMs": 2,
                "uplinkLossPercent": 3,
                "downlinkLossPercent": 4
            }),
            expected_profile(
                "asymmetric",
                json!({
                    "uplinkBandwidthBits": 1_048_576_i64,
                    "downlinkBandwidthBits": 10_485_760_i64,
                    "uplinkDelayMs": 10,
                    "downlinkDelayMs": 20,
                    "uplinkJitterMs": 1,
                    "downlinkJitterMs": 2,
                    "uplinkLossPercent": 3,
                    "downlinkLossPercent": 4
                }),
            ),
        ),
        (
            "a description, and an empty name (modelled Name length minimum of 0)".to_string(),
            Some("ap-south-1"),
            json!({ "name": "", "description": "a shaped 3G link" }),
            expected_profile("", json!({ "description": "a shaped 3G link" })),
        ),
        (
            "name and description at their modelled length maxima (256 and 16384)".to_string(),
            REGION,
            json!({ "name": "n".repeat(256), "description": "d".repeat(16384) }),
            expected_profile(&"n".repeat(256), json!({ "description": "d".repeat(16384) })),
        ),
        (
            "unsigned request: the ARN carries the default region".to_string(),
            None,
            json!({ "name": "unsigned-caller" }),
            expected_profile("unsigned-caller", json!({})),
        ),
    ];

    let mut arns: BTreeSet<String> = BTreeSet::new();
    for (case, region, input, expected) in &cases {
        // The owning project is created by the SAME caller, so its ARN and the profile's
        // share a region.
        let (status, response) = emu
            .call("CreateProject", *region, &json!({ "name": "network-profile-owner" }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateProject to answer 200, got {status:?} with {response}"
        );
        let project_arn = arn_of(&response["project"]);

        let mut request = input.clone();
        request["projectArn"] = json!(project_arn);
        let (status, response) = emu.call("CreateNetworkProfile", *region, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateNetworkProfile to answer 200, got {status:?} with \
             {response}"
        );

        let profile = response["networkProfile"].clone();
        let arn = arn_of(&profile);
        let expected_prefix = format!(
            "arn:aws:devicefarm:{}:{}:networkprofile:{}/",
            region.unwrap_or(DEFAULT_REGION),
            engine::EMULATOR_ACCOUNT_ID,
            resource_id(&project_arn)
        );
        assert!(
            arn.starts_with(&expected_prefix),
            "case {case:?}: expected an ARN starting {expected_prefix:?}, got {arn:?}"
        );
        assert!(
            arn.len() >= 32 && arn.len() <= 1011,
            "case {case:?}: expected the ARN within the modelled length 32..1011, got {:?}",
            arn.len()
        );
        assert!(
            arns.insert(arn.clone()),
            "case {case:?}: expected a fresh ARN, got one already minted: {arn:?}"
        );

        for member in PROFILE_MEMBERS {
            assert_eq!(
                profile[member], expected[member],
                "case {case:?}: expected {member} {:?}, got {:?}",
                expected[member], profile[member]
            );
        }

        // The round trip: GetNetworkProfile reports the persisted record, byte for byte.
        let (status, response) = emu
            .call("GetNetworkProfile", REGION, &json!({ "arn": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetNetworkProfile to answer 200, got {status:?} with \
             {response}"
        );
        assert_eq!(
            response["networkProfile"], profile,
            "case {case:?}: expected GetNetworkProfile to return {profile}, got {}",
            response["networkProfile"]
        );
    }
}

/// Two profiles created with the same name in the same project are different resources:
/// each gets its own ARN and reports its own shaping, and reading one never changes what a
/// later read sees.
#[tokio::test]
async fn profiles_sharing_a_name_are_distinct_resources() {
    let emu = Emulator::new();
    let project = emu.create(&json!({ "name": "shared-names" })).await;
    let project_arn = arn_of(&project);

    let mut created: Vec<(i64, String, Value)> = Vec::new();
    for delay in [25_i64, 250] {
        let (status, response) = emu
            .call(
                "CreateNetworkProfile",
                REGION,
                &json!({
                    "projectArn": project_arn,
                    "name": "same-name",
                    "uplinkDelayMs": delay,
                    "downlinkDelayMs": delay
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case delay {delay}: expected 200, got {status:?} with {response}"
        );
        let profile = response["networkProfile"].clone();
        created.push((delay, arn_of(&profile), profile));
    }

    assert_ne!(
        created[0].1, created[1].1,
        "expected two distinct ARNs for two profiles sharing a name, got {:?} twice",
        created[0].1
    );

    for (delay, arn, profile) in &created {
        // Read twice: a reporting operation must not change what the next caller sees.
        for attempt in 1..=2 {
            let (status, response) = emu
                .call("GetNetworkProfile", REGION, &json!({ "arn": arn }))
                .await;
            assert_eq!(
                status, 200,
                "case delay {delay} read {attempt}: expected 200, got {status:?} with {response}"
            );
            assert_eq!(
                response["networkProfile"], *profile,
                "case delay {delay} read {attempt}: expected {profile}, got {}",
                response["networkProfile"]
            );
            assert_eq!(
                response["networkProfile"]["uplinkDelayMs"].as_i64(),
                Some(*delay),
                "case delay {delay} read {attempt}: expected uplinkDelayMs {delay}, got {:?}",
                response["networkProfile"]["uplinkDelayMs"]
            );
        }
    }
}

/// Every way a network-profile request can name a resource that is not there is refused
/// with the modelled HTTP 400 — never a 501 and never a 200: `ArgumentException` for an
/// ARN that does not name the resource type the member expects, and `NotFoundException`
/// for a well-formed ARN the store has no record for.
#[tokio::test]
async fn network_profile_requests_naming_absent_resources_are_refused() {
    let emu = Emulator::new();
    let project_arn = arn_of(&emu.create(&json!({ "name": "refusals" })).await);
    let profile_arn = {
        let (status, response) = emu
            .call(
                "CreateNetworkProfile",
                REGION,
                &json!({ "projectArn": project_arn, "name": "present" }),
            )
            .await;
        assert_eq!(status, 200, "CreateNetworkProfile answered {status}: {response}");
        arn_of(&response["networkProfile"])
    };

    // (case, operation, request, expected error code)
    let cases: Vec<(String, &str, Value, &str)> = vec![
        (
            "CreateNetworkProfile for a project ARN no project has".to_string(),
            "CreateNetworkProfile",
            json!({ "projectArn": foreign_arn("project"), "name": "orphan" }),
            "NotFoundException",
        ),
        (
            "CreateNetworkProfile given an upload ARN".to_string(),
            "CreateNetworkProfile",
            json!({ "projectArn": foreign_arn("upload"), "name": "wrong-type" }),
            "ArgumentException",
        ),
        (
            "CreateNetworkProfile given a network-profile ARN".to_string(),
            "CreateNetworkProfile",
            json!({ "projectArn": profile_arn.clone(), "name": "wrong-type" }),
            "ArgumentException",
        ),
        (
            "GetNetworkProfile for a network-profile ARN no profile has".to_string(),
            "GetNetworkProfile",
            json!({ "arn": foreign_arn("networkprofile") }),
            "NotFoundException",
        ),
        (
            "GetNetworkProfile given the owning project's ARN".to_string(),
            "GetNetworkProfile",
            json!({ "arn": project_arn.clone() }),
            "ArgumentException",
        ),
        (
            "GetNetworkProfile given a device-pool ARN".to_string(),
            "GetNetworkProfile",
            json!({ "arn": foreign_arn("devicepool") }),
            "ArgumentException",
        ),
    ];

    for (case, op, request, expected_code) in &cases {
        let (status, response) = emu.call(op, REGION, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 refusal, got {status:?} with {response}"
        );
        assert_eq!(
            error_code(&response),
            *expected_code,
            "case {case:?}: expected {expected_code}, got {response}"
        );
    }

    // The refused calls changed nothing: the one profile that exists still reads back.
    let (status, response) = emu
        .call("GetNetworkProfile", REGION, &json!({ "arn": profile_arn }))
        .await;
    assert_eq!(
        status, 200,
        "case the surviving profile after six refusals: expected 200, got {status:?} with \
         {response}"
    );
    assert_eq!(
        response["networkProfile"]["name"],
        json!("present"),
        "case the surviving profile after six refusals: expected name \"present\", got {}",
        response["networkProfile"]["name"]
    );
}
