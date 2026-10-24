// SPDX-License-Identifier: CC0-1.0

//! This service keeps NO endpoint registry of its own.
//!
//! An endpoint is the SageMaker CONTROL PLANE's resource: this data plane models no
//! operation that creates, updates or describes one, so every invocation is answered from
//! the control plane's store through the one shared engine. The rule this suite pins is
//! that consequence — what the data plane can invoke is exactly what the control plane
//! holds AT THAT MOMENT, never a copy taken earlier or a seed of its own:
//!
//! * before the control plane creates an endpoint, no route can invoke it;
//! * after it creates one, every route resolves it — without this service being told;
//! * what an endpoint hosts comes from ITS OWN configuration, so two endpoints created
//!   with different configurations are invoked differently (the data plane following a
//!   configuration CHANGE cannot be exercised yet: `UpdateEndpoint` is still a stub, so
//!   no call can change what an existing endpoint hosts);
//! * after the control plane deletes it, every route refuses it again.

use serde_json::json;

use crate::harness::{
    async_inference_config, async_invocations, invocations, post, routes, Emulator,
    INPUT_LOCATION as INPUT,
};

/// Where a seeded deployment writes its asynchronous results.
const OUTPUT_PATH: &str = "s3://aws-emulator-sagemaker/async/output";

/// RULE: the data plane holds no endpoint of its own, so a name is invokable exactly
/// while the control plane holds an endpoint record for it — not before the record is
/// created, and not after it is deleted.
#[tokio::test]
async fn a_name_is_invokable_exactly_while_the_control_plane_holds_it() {
    let emulator = Emulator::new();

    // Rows differ along the name's shape — short, hyphenated and long — so the rule is
    // asserted over the name space rather than over one name. The longest row stops short
    // of `EndpointName`'s 63-character maximum because the harness derives the control
    // plane's `ModelName` from it (`<name>-model`), and that member is `len 0..63` too.
    let names = [
        "lifecycle",
        "a",
        "ep-with-hyphens-1",
        "a0123456789012345678901234567890123456789b",
    ];
    for name in names {
        // 1. Nothing was created yet: a booted emulator publishes NO endpoint.
        for (operation, path, headers) in routes() {
            let call = post(&emulator, &path(name), &headers, b"").await;
            assert_eq!(
                call.status,
                400,
                "case {name} / {operation} before creation: a booted emulator must hold no \
                 endpoint, got {} / {}",
                call.status,
                String::from_utf8_lossy(&call.body)
            );
        }

        // 2. The CONTROL PLANE creates it; nothing tells the data plane.
        emulator
            .deploy(name, &["AllTraffic"], async_inference_config(OUTPUT_PATH, None))
            .await;
        for (operation, path, headers) in routes() {
            let call = post(&emulator, &path(name), &headers, b"").await;
            assert!(
                call.status == 202 || call.status == 424,
                "case {name} / {operation} after creation: the data plane must resolve the \
                 control plane's endpoint, got {} / {}",
                call.status,
                String::from_utf8_lossy(&call.body)
            );
        }

        // 3. The CONTROL PLANE deletes it: the name resolves to nothing again.
        emulator
            .control("DeleteEndpoint", &json!({ "EndpointName": name }))
            .await;
        for (operation, path, headers) in routes() {
            let call = post(&emulator, &path(name), &headers, b"").await;
            assert_eq!(
                call.status,
                400,
                "case {name} / {operation} after deletion: a deleted endpoint must resolve to \
                 nothing, got {} / {}",
                call.status,
                String::from_utf8_lossy(&call.body)
            );
        }
    }
}

/// RULE: what the data plane reports about an endpoint comes from the endpoint's own
/// configuration, so two endpoints configured differently answer differently — the data
/// plane holds no defaults of its own to fall back on.
#[tokio::test]
async fn what_an_endpoint_hosts_comes_from_its_own_configuration() {
    let emulator = Emulator::new();
    // One variant, no asynchronous configuration.
    emulator
        .deploy("single", &["AllTraffic"], json!({}))
        .await;
    // Two variants and an asynchronous configuration.
    emulator
        .deploy(
            "double",
            &["Blue", "Green"],
            async_inference_config(OUTPUT_PATH, None),
        )
        .await;

    // `TargetVariant` must name a variant the endpoint's OWN configuration lists.
    let variant_cases = [
        ("single", "AllTraffic", true),
        ("single", "Blue", false),
        ("double", "Blue", true),
        ("double", "Green", true),
        ("double", "AllTraffic", false),
    ];
    for (endpoint, variant, hosted) in variant_cases {
        let call = post(
            &emulator,
            &invocations(endpoint),
            &[("x-amzn-sagemaker-target-variant", variant)],
            b"1,2,3",
        )
        .await;
        if hosted {
            assert_eq!(
                call.status, 424,
                "case {endpoint} / {variant}: a hosted variant must get past resolution, got {}",
                call.status
            );
        } else {
            assert_eq!(
                call.status, 400,
                "case {endpoint} / {variant}: a variant the endpoint does not host must be \
                 refused, got {}",
                call.status
            );
            assert!(
                call.message().contains(variant),
                "case {endpoint} / {variant}: the refusal must name the variant, got {:?}",
                call.message()
            );
        }
    }

    // The asynchronous route is open exactly where the endpoint's configuration carries
    // an `AsyncInferenceConfig`.
    let async_cases = [("single", 400), ("double", 202)];
    for (endpoint, expected) in async_cases {
        let call = post(&emulator, &async_invocations(endpoint), &[INPUT], b"").await;
        assert_eq!(
            call.status,
            expected,
            "case {endpoint}: the asynchronous route follows the endpoint's own configuration, \
             got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
    }
}

