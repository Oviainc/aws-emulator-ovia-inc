// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `TestHypervisorConfiguration`: the connection test a caller runs
//! to validate that a backup gateway can reach a hypervisor at a host.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the `MultiServiceRouter`,
//! reusing the harness of `gateway_lifecycle`, so what is asserted is the wire behaviour
//! and the only way a request can see what an earlier one wrote is the shared store.
//!
//! The rule under test: the test is answered from the records this service holds — it
//! passes with an empty 200 when the `GatewayArn` names a gateway in the store AND the
//! `Host` is one the store holds a hypervisor configuration for, it refuses a gateway the
//! store does not hold as an invalid REQUEST (`ValidationException` / `InvalidParameter`),
//! and it
//! refuses a host no record carries the way this service refuses a missing hypervisor. The
//! credentials decide nothing, the gateway's own association decides nothing, and the test
//! writes nothing, so the store is byte-for-byte what it was before every call.
//!
//! The tables below span that class — hosts differing in form and in length (both
//! published bounds of the modelled member), a host carried by one record and by several,
//! a host a record was MOVED onto by an update rather than imported at, credentials absent
//! and at each published bound, gateways associated with a server and with none, and
//! refusals differing in which end is missing, in the ARN's resource type, partition and
//! length (each published bound of the modelled `GatewayArn`) and in how a host stopped
//! being held — rather than one recorded request.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{
    service_state, HYPERVISOR_NOT_FOUND, HYPERVISOR_NOT_FOUND_MESSAGE, INVALID_PARAMETER,
    INVALID_PARAMETER_MESSAGE,
};

use crate::gateway_lifecycle::{
    build_engine, build_router, call, create, never_created_arn, Reply, REGION,
};

/// Hosts spanning the modelled member: `Host` is `@length(min: 3, max: 128)` with pattern
/// `^.+$` (`smithy_query(query="shape",shape="Host")`), so the pattern constrains nothing
/// beyond non-emptiness and the two extremes are the published bounds.
fn hosts_spanning_the_member() -> Vec<(&'static str, String)> {
    vec![
        ("an IPv4 host", "198.51.100.7".to_string()),
        ("an FQDN host", "vcenter.example.com".to_string()),
        ("the 3-character minimum", "a.b".to_string()),
        (
            "the 128-character maximum",
            format!("{}.example.com", "h".repeat(128 - ".example.com".len())),
        ),
        // `^.+$` accepts any non-empty run of characters, spaces included.
        ("a host carrying spaces", "the hypervisor box".to_string()),
    ]
}

/// A `GatewayArn`-shaped ARN of exactly `len` characters, for the published length bounds
/// of the modelled shape: `@length(min: 50, max: 180)`
/// (`smithy_query(query="shape",shape="GatewayArn")`). The id segment absorbs the length,
/// so every character stays inside the pattern's `[a-zA-Z-0-9]+`, and the resource-type
/// segment is a single character so that even the 50-character minimum fits.
fn gateway_arn_of_length(len: usize) -> String {
    let prefix = format!(
        "arn:aws:backup-gateway:{REGION}:{}:g/",
        engine::EMULATOR_ACCOUNT_ID
    );
    assert!(
        len > prefix.len(),
        "a {len}-character ARN cannot hold the {}-character prefix",
        prefix.len()
    );
    format!("{prefix}{}", "x".repeat(len - prefix.len()))
}

/// The refusal message, wherever the envelope carries it: a refusal the framework made
/// before the handler ran states it in `message`, a modelled error the handler returned in
/// the `Message` member.
fn message_of(reply: &Reply) -> String {
    reply
        .str_at("/message")
        .or_else(|| reply.str_at("/Message"))
        .unwrap_or_else(|| panic!("no message in {:?}", reply.body))
        .to_string()
}

/// The WHOLE store, rendered: every gateway, every hypervisor, every virtual machine and
/// every id counter. A call that writes nothing leaves this string untouched, so one
/// comparison covers every member of every record rather than the few a test thought to
/// name.
fn store_snapshot(engine: &Arc<engine::InMemoryEngine>) -> String {
    format!(
        "{:?}",
        *service_state(&**engine)
            .expect("the in-memory engine must expose this state")
            .lock()
    )
}

async fn import(router: &MultiServiceRouter, name: &str, host: &str) -> String {
    let reply = call(
        router,
        "ImportHypervisorConfiguration",
        &json!({ "Name": name, "Host": host }),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {name:?}: ImportHypervisorConfiguration must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply
        .str_at("/HypervisorArn")
        .unwrap_or_else(|| panic!("no HypervisorArn in {:?}", reply.body))
        .to_string()
}

/// One `TestHypervisorConfiguration` request. `credentials` are the `Username` /
/// `Password` members, omitted when `None`.
async fn test_configuration(
    router: &MultiServiceRouter,
    gateway_arn: &str,
    host: &str,
    credentials: Option<(&str, &str)>,
) -> Reply {
    let mut body = json!({ "GatewayArn": gateway_arn, "Host": host });
    if let Some((username, password)) = credentials {
        body["Username"] = json!(username);
        body["Password"] = json!(password);
    }
    call(router, "TestHypervisorConfiguration", &body).await
}

/// Credentials spanning the modelled members: `Username` is `@length(min: 1, max: 100)`
/// with pattern `^[ -\.0-\[\]-~]*[!-\.0-\[\]-~][ -\.0-\[\]-~]*$` and `Password` is
/// `@length(min: 1, max: 100)` with pattern `^[ -~]+$`
/// (`smithy_query(query="shape",shape="Username,Password")`), and both are optional
/// (`smithy_query(query="operation",operation="TestHypervisorConfiguration")`).
fn credentials_spanning_the_members() -> Vec<(&'static str, Option<(String, String)>)> {
    vec![
        ("no credentials at all", None),
        (
            "a vSphere-style username and a password",
            Some((
                "Administrator@vsphere.local".to_string(),
                "Pa55word!".to_string(),
            )),
        ),
        (
            "both at the 1-character minimum",
            Some(("u".to_string(), "p".to_string())),
        ),
        (
            "both at the 100-character maximum",
            Some(("u".repeat(100), "p".repeat(100))),
        ),
        (
            "a password of every printable character the pattern accepts",
            Some((
                "u".to_string(),
                // `^[ -~]+$` is the printable ASCII range, 95 characters — inside the
                // member's 100-character maximum.
                (0x20u8..=0x7eu8).map(char::from).collect::<String>(),
            )),
        ),
    ]
}

/// RULE: the test passes — an empty 200 — exactly when the gateway is one this store holds
/// and the host is one it holds a hypervisor configuration for, whatever the credentials,
/// whatever the host's form, however many records carry it, by whichever path it got
/// there, and whether or not the gateway serves a server; and it writes nothing.
#[tokio::test]
async fn a_test_of_a_configuration_this_service_holds_passes_and_writes_nothing() {
    for (host_case, host) in hosts_spanning_the_member() {
        // One store per host, so the records a case asserts over are exactly the ones it
        // wrote.
        let engine = build_engine();
        let router = build_router(&engine);

        // Two records at the host: one minted there by an import, one MOVED onto it by an
        // update — so the host the test resolves is read off the records as they stand and
        // not off what an import happened to carry.
        let imported = import(&router, "hv-at-host", &host).await;
        let moved = import(&router, "hv-moved", "10.0.0.9").await;
        let update = call(
            &router,
            "UpdateHypervisor",
            &json!({ "HypervisorArn": moved, "Host": host }),
        )
        .await;
        assert_eq!(
            update.status, 200,
            "case {host_case:?}: the record to move must update, got {} {:?}",
            update.status, update.body
        );
        // A record at another host entirely, to show the lookup is by host and not "any
        // hypervisor exists".
        let elsewhere = import(&router, "hv-elsewhere", "203.0.113.4").await;

        // One gateway associated with the server at the host, one with the server
        // elsewhere, and one associated with nothing: the association must decide nothing.
        let unassociated = create(&router, "gw-unassociated").await;
        let on_host = create(&router, "gw-on-host").await;
        let on_other = create(&router, "gw-on-other").await;
        for (gateway, server) in [(&on_host, &imported), (&on_other, &elsewhere)] {
            let associated = call(
                &router,
                "AssociateGatewayToServer",
                &json!({ "GatewayArn": gateway, "ServerArn": server }),
            )
            .await;
            assert_eq!(
                associated.status, 200,
                "case {host_case:?}: the association must answer 200, got {} {:?}",
                associated.status, associated.body
            );
        }

        let before = store_snapshot(&engine);

        for gateway in [&unassociated, &on_host, &on_other] {
            for (credential_case, credentials) in credentials_spanning_the_members() {
                let case = format!("{host_case} / {credential_case} / gateway {gateway}");
                let credentials = credentials
                    .as_ref()
                    .map(|(username, password)| (username.as_str(), password.as_str()));
                let reply = test_configuration(&router, gateway, &host, credentials).await;
                assert_eq!(
                    reply.status, 200,
                    "case {case:?}: a test of a configuration this service holds must \
                     answer 200, got {} {:?}",
                    reply.status, reply.body
                );
                // The modelled output carries no members, so a passing test answers an
                // empty document and nothing else.
                let members = reply
                    .body
                    .as_object()
                    .map(|members| members.len())
                    .unwrap_or_default();
                assert_eq!(
                    members, 0,
                    "case {case:?}: the modelled output carries no members, got {:?}",
                    reply.body
                );
                assert_eq!(
                    store_snapshot(&engine),
                    before,
                    "case {case:?}: a test must write nothing"
                );
            }
        }

        // And the records are still exactly what the reads report after every test: the
        // host both records carry, and the association each gateway holds.
        for arn in [&imported, &moved] {
            let read = call(&router, "GetHypervisor", &json!({ "HypervisorArn": arn })).await;
            assert_eq!(
                (read.status, read.str_at("/Hypervisor/Host")),
                (200, Some(host.as_str())),
                "case {host_case:?}: {arn} must still report its host, got {} {:?}",
                read.status,
                read.body
            );
        }
        let read = call(
            &router,
            "GetGateway",
            &json!({ "GatewayArn": unassociated }),
        )
        .await;
        assert_eq!(
            (read.status, read.str_at("/Gateway/HypervisorId")),
            (200, None),
            "case {host_case:?}: a test must not associate the gateway it tested with, got \
             {} {:?}",
            read.status,
            read.body
        );
    }
}

/// RULE (its refusal half, for the GATEWAY): a `GatewayArn` naming no gateway this store
/// holds makes the configuration under test an invalid REQUEST, so it is refused with the
/// common `ValidationException` at HTTP 400 carrying `ErrorCode` `InvalidParameter` and a
/// fixed sentence that echoes nothing of the request — and the refusal comes before the
/// host is looked at.
#[tokio::test]
async fn a_test_naming_a_gateway_this_service_does_not_hold_is_refused() {
    let engine = build_engine();
    let router = build_router(&engine);

    let host = "198.51.100.51";
    import(&router, "hv-held", host).await;
    let live = create(&router, "gw-live").await;
    let deleted = create(&router, "gw-deleted").await;
    let removed = call(&router, "DeleteGateway", &json!({ "GatewayArn": deleted })).await;
    assert_eq!(
        removed.status, 200,
        "DeleteGateway must answer 200, got {} {:?}",
        removed.status, removed.body
    );

    let before = store_snapshot(&engine);

    // (case, the GatewayArn, the Host). Every row is a well-formed `GatewayArn`, so it
    // reaches the handler rather than the boundary validator; the rows differ in why no
    // gateway is behind it, in the resource type and partition the ARN names, in its length
    // (each published bound of the modelled shape) and — the last row — in whether the host
    // is held either, which is how the ORDER of the two lookups is pinned.
    let rows: [(&str, String, &str); 7] = [
        (
            "a gateway that was never created",
            never_created_arn(),
            host,
        ),
        ("a gateway that was deleted", deleted.clone(), host),
        (
            "a GatewayArn naming a hypervisor resource",
            format!(
                "arn:aws:backup-gateway:{REGION}:{}:hypervisor/hv-0000000000000001",
                engine::EMULATOR_ACCOUNT_ID
            ),
            host,
        ),
        (
            "a GatewayArn in another partition",
            format!(
                "arn:aws-us-gov:backup-gateway:{REGION}:{}:gateway/bgw-0000000000000777",
                engine::EMULATOR_ACCOUNT_ID
            ),
            host,
        ),
        (
            "a GatewayArn at the modelled 50-character minimum",
            gateway_arn_of_length(50),
            host,
        ),
        (
            "a GatewayArn at the modelled 180-character maximum",
            gateway_arn_of_length(180),
            host,
        ),
        (
            "neither the gateway nor the host is held",
            never_created_arn(),
            "203.0.113.77",
        ),
    ];

    for (case, gateway_arn, host) in &rows {
        let refused = test_configuration(&router, gateway_arn, host, None).await;
        assert_eq!(
            (
                refused.status,
                refused.code.as_deref(),
                refused.str_at("/ErrorCode"),
            ),
            (
                400,
                Some("ValidationException"),
                Some(INVALID_PARAMETER),
            ),
            "case {case:?}: a GatewayArn with no gateway behind it must be refused with 400 \
             ValidationException / ErrorCode {INVALID_PARAMETER}, got {} {:?}",
            refused.status,
            refused.body
        );
        assert_eq!(
            message_of(&refused),
            INVALID_PARAMETER_MESSAGE,
            "case {case:?}: the refusal must state the fixed sentence for an invalid \
             request and echo nothing of it, got {:?}",
            refused.body
        );
        assert_eq!(
            store_snapshot(&engine),
            before,
            "case {case:?}: a refused test must write nothing"
        );
    }

    // And the same request succeeds for the gateway that does exist, so the refusals above
    // are about the ARN and not about the operation.
    let passed = test_configuration(&router, &live, host, None).await;
    assert_eq!(
        passed.status, 200,
        "the test of a gateway this store holds must answer 200, got {} {:?}",
        passed.status, passed.body
    );
}

/// RULE (its refusal half, for the HOST): a `Host` no record of this service carries is a
/// hypervisor the action requires and this service does not have, refused the way this
/// service refuses a missing hypervisor — the modelled `ResourceNotFoundException` at HTTP
/// 400 with `ErrorCode` `HypervisorNotFound` — and nothing is written.
#[tokio::test]
async fn a_test_of_a_host_this_service_holds_no_configuration_for_is_refused() {
    let engine = build_engine();
    let router = build_router(&engine);

    let gateway = create(&router, "gw-host-cases").await;

    // A record that is deleted, and a record that is MOVED off its host by an update: both
    // leave a host the store no longer carries, so neither path leaves a test passing on a
    // host that is gone.
    let deleted_host = "198.51.100.61";
    let doomed = import(&router, "hv-doomed", deleted_host).await;
    let removed = call(
        &router,
        "DeleteHypervisor",
        &json!({ "HypervisorArn": doomed }),
    )
    .await;
    assert_eq!(
        removed.status, 200,
        "DeleteHypervisor must answer 200, got {} {:?}",
        removed.status, removed.body
    );

    let vacated_host = "198.51.100.62";
    let mover = import(&router, "hv-mover", vacated_host).await;
    let moved = call(
        &router,
        "UpdateHypervisor",
        &json!({ "HypervisorArn": mover, "Host": "198.51.100.63" }),
    )
    .await;
    assert_eq!(
        moved.status, 200,
        "UpdateHypervisor must answer 200, got {} {:?}",
        moved.status, moved.body
    );

    let held = "vcenter.example.com";
    import(&router, "hv-held", held).await;

    let before = store_snapshot(&engine);

    // (case, the Host). Every row satisfies the modelled member, so it reaches the handler;
    // the rows differ in why the store holds no record for the host — never imported,
    // deleted, moved off — and in how NEARLY it matches a host that is held, because a host
    // is matched as the value the record carries and not approximately.
    let rows: [(&str, String); 8] = [
        ("a host that was never imported", "203.0.113.9".to_string()),
        ("the host of a deleted record", deleted_host.to_string()),
        ("the host a record was moved off", vacated_host.to_string()),
        (
            "a held host in a different case",
            held.to_ascii_uppercase(),
        ),
        ("a held host with a trailing dot", format!("{held}.")),
        (
            "a held host with surrounding spaces",
            format!(" {held} "),
        ),
        (
            "a prefix of a held host",
            held[..held.len() - 4].to_string(),
        ),
        (
            "a host at the 128-character maximum that is not held",
            format!("{}.example.com", "z".repeat(128 - ".example.com".len())),
        ),
    ];

    for (case, host) in &rows {
        let refused = test_configuration(&router, &gateway, host, None).await;
        assert_eq!(
            (
                refused.status,
                refused.code.as_deref(),
                refused.str_at("/ErrorCode"),
                message_of(&refused).as_str(),
            ),
            (
                400,
                Some("ResourceNotFoundException"),
                Some(HYPERVISOR_NOT_FOUND),
                HYPERVISOR_NOT_FOUND_MESSAGE,
            ),
            "case {case:?}: a host this service holds no configuration for must be refused \
             with 400 ResourceNotFoundException / ErrorCode {HYPERVISOR_NOT_FOUND}, got {} \
             {:?}",
            refused.status,
            refused.body
        );
        // The refusal states the condition and nothing the caller sent.
        assert!(
            !message_of(&refused).contains(host.as_str()),
            "case {case:?}: the refusal must not echo the host it was asked for, got {:?}",
            refused.body
        );
        assert_eq!(
            store_snapshot(&engine),
            before,
            "case {case:?}: a refused test must write nothing"
        );
    }

    // The host that IS held passes, so the refusals above are about the host and not about
    // the operation; and importing a host that was refused makes the same request pass,
    // which is the rule stated forwards.
    let passed = test_configuration(&router, &gateway, held, None).await;
    assert_eq!(
        passed.status, 200,
        "the test of a host this store holds must answer 200, got {} {:?}",
        passed.status, passed.body
    );
    for (case, host) in &rows {
        import(&router, "hv-late", host).await;
        let now_passes = test_configuration(&router, &gateway, host, None).await;
        assert_eq!(
            now_passes.status, 200,
            "case {case:?}: once a configuration for the host is imported the same test \
             must pass, got {} {:?}",
            now_passes.status, now_passes.body
        );
    }
}

/// A modelled constraint is refused at the BOUNDARY, before the handler runs: 400
/// `ValidationException` naming the member, and — for the `@sensitive` credentials — never
/// the value that violated it.
#[tokio::test]
async fn an_input_outside_a_modelled_constraint_is_refused_at_the_boundary() {
    let engine = build_engine();
    let router = build_router(&engine);

    let gateway = create(&router, "gw-constraints").await;
    let host = "198.51.100.71";
    import(&router, "hv-constraints", host).await;

    let before = store_snapshot(&engine);

    // (case, the request body, the member path the refusal must name). Each row is one
    // character past a published bound of the modelled member, plus the required members
    // absent.
    let rows: [(&str, Value, &str); 7] = [
        (
            "a Host one character below the 3-character minimum",
            json!({ "GatewayArn": gateway, "Host": "ab" }),
            "host",
        ),
        (
            "a Host one character past the 128-character maximum",
            json!({ "GatewayArn": gateway, "Host": "h".repeat(129) }),
            "host",
        ),
        (
            "no Host at all",
            json!({ "GatewayArn": gateway }),
            "host",
        ),
        (
            "no GatewayArn at all",
            json!({ "Host": host }),
            "gatewayArn",
        ),
        (
            "a GatewayArn one character past the 180-character maximum",
            json!({ "GatewayArn": gateway_arn_of_length(181), "Host": host }),
            "gatewayArn",
        ),
        (
            "a Username one character past the 100-character maximum",
            json!({
                "GatewayArn": gateway,
                "Host": host,
                "Username": "u".repeat(101),
            }),
            "username",
        ),
        (
            "a Password one character past the 100-character maximum",
            json!({
                "GatewayArn": gateway,
                "Host": host,
                "Password": "p".repeat(101),
            }),
            "password",
        ),
    ];

    for (case, body, member) in &rows {
        let refused = call(&router, "TestHypervisorConfiguration", body).await;
        assert_eq!(
            (refused.status, refused.code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: a value outside a modelled constraint must be refused with 400 \
             ValidationException, got {} {:?}",
            refused.status,
            refused.body
        );
        let message = message_of(&refused);
        assert!(
            message.contains(&format!("at '{member}' failed to satisfy constraint: ")),
            "case {case:?}: the refusal must name the member it refused, got {message:?}"
        );
        // A `@sensitive` member names itself and nothing of what it held.
        for secret in ["u".repeat(101), "p".repeat(101)] {
            assert!(
                !message.contains(&secret),
                "case {case:?}: the refusal must not echo a credential, got {message:?}"
            );
        }
        assert_eq!(
            store_snapshot(&engine),
            before,
            "case {case:?}: a refused test must write nothing"
        );
    }
}
