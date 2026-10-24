// SPDX-License-Identifier: CC0-1.0

//! The resolution rule the three invocation routes SHARE.
//!
//! `InvokeEndpoint`'s own behaviour is covered in [`crate::endpoint_invocation`]; what
//! this suite pins is that the other two routes answer the same question the same way,
//! because all three ask it through one resolver
//! (`service_com_amazonaws_sagemakerruntime_amazonsagemakerruntime::state::resolve_endpoint`).
//! A rule stated once must hold everywhere it applies, so it is asserted over the CROSS
//! PRODUCT of the three modelled routes and the endpoint states they can meet.

use serde_json::json;

use crate::harness::{async_inference_config, post, routes, Emulator, ACCOUNT};

/// RULE: for every one of the three routes, an endpoint the account does not host and an
/// endpoint with no deployment behind it are both refused with the modelled
/// `ValidationError` (HTTP 400) naming the endpoint and the account — no route answers a
/// hollow success, and no route refuses with a different status than the others.
#[tokio::test]
async fn every_route_resolves_the_endpoint_the_same_way() {
    let emulator = Emulator::new();
    emulator
        .deploy(
            "serving",
            &["AllTraffic"],
            async_inference_config("s3://aws-emulator-sagemaker/async/output", None),
        )
        .await;
    emulator
        .deploy(
            "deleted",
            &["AllTraffic"],
            async_inference_config("s3://aws-emulator-sagemaker/async/output", None),
        )
        .await;
    emulator
        .control("DeleteEndpoint", &json!({ "EndpointName": "deleted" }))
        .await;

    // The three modelled routes, each with the headers it needs to get as far as
    // resolving the endpoint.
    let routes = routes();
    // The endpoint states a resolution can meet, other than "serving". A still-provisioning
    // endpoint is created per row, immediately before it is invoked: its `Creating` status
    // is transient by design (the clock owes it `InService` shortly after `CreateEndpoint`),
    // so a shared one would settle part-way through the table.
    let endpoints = ["nosuchendpoint", "deleted"];

    for (operation, path, headers) in routes {
        for endpoint in endpoints {
            let call = post(&emulator, &path(endpoint), &headers, b"").await;
            assert_eq!(
                call.status,
                400,
                "case {operation} / {endpoint}: expected HTTP 400, got {} / {}",
                call.status,
                String::from_utf8_lossy(&call.body)
            );
            assert_eq!(
                call.error_code(),
                "ValidationError",
                "case {operation} / {endpoint}: expected ValidationError, got {:?}",
                call.error_code()
            );
            assert!(
                call.message().contains(endpoint) && call.message().contains(ACCOUNT),
                "case {operation} / {endpoint}: the refusal must name the endpoint and the \
                 account, got {:?}",
                call.message()
            );
        }

        let provisioning = format!("provisioning-{}", operation.to_lowercase());
        emulator
            .deploy_without_waiting(
                &provisioning,
                &["AllTraffic"],
                async_inference_config("s3://aws-emulator-sagemaker/async/output", None),
            )
            .await;
        let call = post(&emulator, &path(&provisioning), &headers, b"").await;
        assert_eq!(
            call.status,
            400,
            "case {operation} / still provisioning: an endpoint with no deployment behind it \
             must be refused, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert!(
            call.message().contains(&provisioning) && call.message().contains(ACCOUNT),
            "case {operation} / still provisioning: the refusal must name the endpoint and the \
             account, got {:?}",
            call.message()
        );

        // The same route against the SERVING endpoint gets past resolution: the rows
        // above are refused for the endpoint's state, not because the route never works.
        let served = post(&emulator, &path("serving"), &headers, b"").await;
        assert!(
            served.status == 202 || served.status == 424,
            "case {operation} / serving: resolution must succeed (202 queued, or 424 because no \
             container ran the inference), got {} / {}",
            served.status,
            String::from_utf8_lossy(&served.body)
        );
    }
}

/// RULE: every call the data plane answers — refused or not — is logged in this service's
/// own state, so the three routes leave one comparable access trail.
#[tokio::test]
async fn every_route_logs_the_call_it_answered() {
    let emulator = Emulator::new();
    emulator
        .deploy(
            "logged",
            &["AllTraffic"],
            async_inference_config("s3://aws-emulator-sagemaker/async/output", None),
        )
        .await;

    let routes = routes();
    for (round, (operation, path, headers)) in routes.into_iter().enumerate() {
        let before = emulator.invocations_of("logged").len();
        post(&emulator, &path("logged"), &headers, b"").await;
        let after = emulator.invocations_of("logged").len();
        assert_eq!(
            after,
            before + 1,
            "case {operation}: the call must be logged exactly once (round {round})"
        );
        let logged = emulator
            .last_logged()
            .unwrap_or_else(|| panic!("case {operation}: the call must be logged"));
        assert_eq!(
            logged.endpoint_name, "logged",
            "case {operation}: the log must name the endpoint the request named, got {:?}",
            logged.endpoint_name
        );
    }
}
