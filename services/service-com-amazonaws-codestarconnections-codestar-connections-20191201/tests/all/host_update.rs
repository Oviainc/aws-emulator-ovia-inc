// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateHost`, and the read that proves what it wrote, `GetHost`.
//!
//! Every call goes over the wire through the [`Emulator`] harness the sibling
//! `connection_lifecycle` suite builds (`POST /` + `X-Amz-Target:
//! CodeStar_connections_20191201.<Operation>`, awsJson1_0 body), so what is asserted here
//! is the service's HTTP behaviour and not a handler called directly.
//!
//! The rules pinned, each over a TABLE rather than one request:
//!
//! * an update REPLACES exactly the configurations its request carries and leaves every
//!   member it omits as stored — over the published `Url` bounds, both published
//!   `VpcId` / `SubnetId` / `SecurityGroupId` widths, 1, 2 and 10 subnets and security
//!   groups, a `TlsCertificate` present and absent, and each combination of the two
//!   optional members;
//! * an update addresses exactly the host its `HostArn` names: every other host is
//!   untouched;
//! * a `HostArn` naming no host is reported ABSENT (`ResourceNotFoundException`, at the
//!   status the recorded service answers, with a message naming the supplied ARN) and the
//!   refused call writes nothing;
//! * a request outside a modeled constraint is refused before anything is stored.

use serde_json::{json, Value};

use super::connection_lifecycle::{error_code, error_message, Emulator, ACCOUNT_ID, REGION};

/// The `ProviderType` the hosts below are created with — a host's provider is not
/// updatable, so one value serves every row.
const PROVIDER_TYPE: &str = "GitHubEnterpriseServer";

/// The status the model's `CreateHost` doc gives a host created through the CLI or the
/// SDK ("in `PENDING` status by default"), which `UpdateHost` models no member to change.
const PENDING: &str = "PENDING";

/// `CreateHost`, asserting success, and return the minted host ARN — the shared
/// harness's own create-and-assert path, so this suite holds no copy of it.
async fn create_host(emulator: &Emulator, case: &str, body: Value) -> String {
    emulator.create_host_from(case, body).await
}

/// `GetHost`, asserting success, and return the response body — likewise the shared
/// harness's read.
async fn get_host(emulator: &Emulator, case: &str, host_arn: &str) -> Value {
    emulator.get_host(case, host_arn).await
}

/// A `VpcConfiguration` inside the modeled constraints: `VpcId` pattern
/// `^vpc-\w{8}(\w{9})?$`, `SubnetIds` / `SecurityGroupIds` `len 1..10` over
/// `^subnet-\w{8}(\w{9})?$` / `^sg-\w{8}(\w{9})?$`, `TlsCertificate` `len 1..16384`
/// (from `smithy_query(query="shape", shape="VpcConfiguration")`).
///
/// `wide` picks the SECOND published width of each id — 17 word characters after the
/// prefix instead of 8 — so a table can walk both forms the patterns allow.
fn vpc_configuration(wide: bool, count: usize, tls: Option<&str>) -> Value {
    let tail = |index: usize| {
        if wide {
            format!("0a1b2c3d4e5f6789{index}")
        } else {
            format!("0a1b2c3{index}")
        }
    };
    let subnets: Vec<String> = (0..count).map(|i| format!("subnet-{}", tail(i))).collect();
    let groups: Vec<String> = (0..count).map(|i| format!("sg-{}", tail(i))).collect();
    let mut configuration = json!({
        "VpcId": format!("vpc-{}", tail(0)),
        "SubnetIds": subnets,
        "SecurityGroupIds": groups,
    });
    if let Some(tls) = tls {
        configuration["TlsCertificate"] = json!(tls);
    }
    configuration
}

/// RULE: `UpdateHost` replaces exactly the configurations the request supplies and leaves
/// every other member of the stored host as it was — an omitted member is not a request to
/// clear it, and a host's name, provider type and status are not updatable at all.
#[tokio::test]
async fn update_host_replaces_only_the_configurations_the_request_supplies() {
    let emulator = Emulator::new();

    let created_endpoint = "https://ghe.example.com".to_string();
    let created_vpc = vpc_configuration(false, 1, None);
    let arn = create_host(
        &emulator,
        "seed",
        json!({
            "Name": "updatable",
            "ProviderType": PROVIDER_TYPE,
            "ProviderEndpoint": created_endpoint,
            "VpcConfiguration": created_vpc,
        }),
    )
    .await;

    // Each row: what the update supplies. `Url` is modeled `len 1..512`, so the endpoint
    // rows walk both published bounds, and the VPC rows walk both published id widths,
    // 1, 2 and 10 subnets/security groups, and a `TlsCertificate` present and absent.
    let rows: Vec<(&str, Option<String>, Option<Value>)> = vec![
        ("nothing supplied", None, None),
        ("endpoint only, shortest Url", Some("a".to_string()), None),
        (
            "endpoint only, longest Url",
            Some(format!("https://{}", "e".repeat(512 - "https://".len()))),
            None,
        ),
        (
            "endpoint only, a port and a path",
            Some("http://ghe.internal:8443/enterprise".to_string()),
            None,
        ),
        (
            "vpc only, widest ids and 10 of each",
            None,
            Some(vpc_configuration(true, 10, None)),
        ),
        (
            "vpc only, with a TLS certificate",
            None,
            Some(vpc_configuration(
                false,
                2,
                Some("-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----"),
            )),
        ),
        (
            "both endpoint and vpc",
            Some("https://ghe.example.net/v3".to_string()),
            Some(vpc_configuration(true, 3, None)),
        ),
        (
            "endpoint again, vpc left alone",
            Some("https://ghe.example.org".to_string()),
            None,
        ),
    ];

    // What state must hold after each row: the last value each member was given.
    let mut expected_endpoint = created_endpoint;
    let mut expected_vpc = created_vpc;
    for (case, endpoint, configuration) in rows {
        let mut body = json!({ "HostArn": arn });
        if let Some(endpoint) = &endpoint {
            body["ProviderEndpoint"] = json!(endpoint);
            expected_endpoint = endpoint.clone();
        }
        if let Some(configuration) = &configuration {
            body["VpcConfiguration"] = configuration.clone();
            expected_vpc = configuration.clone();
        }

        let (status, response) = emulator.call("UpdateHost", body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateHost to answer 200, got {status} {response:?}"
        );

        let host = get_host(&emulator, case, &arn).await;
        assert_eq!(
            host["ProviderEndpoint"].as_str(),
            Some(expected_endpoint.as_str()),
            "case {case:?}: wrong ProviderEndpoint in {host:?}"
        );
        assert_eq!(
            host["VpcConfiguration"], expected_vpc,
            "case {case:?}: wrong VpcConfiguration in {host:?}"
        );
        // Not updatable by this operation, so unchanged by every row.
        assert_eq!(
            host["Name"].as_str(),
            Some("updatable"),
            "case {case:?}: UpdateHost changed Name in {host:?}"
        );
        assert_eq!(
            host["ProviderType"].as_str(),
            Some(PROVIDER_TYPE),
            "case {case:?}: UpdateHost changed ProviderType in {host:?}"
        );
        assert_eq!(
            host["Status"].as_str(),
            Some(PENDING),
            "case {case:?}: UpdateHost changed Status in {host:?}"
        );
    }
}

/// RULE: an update addresses exactly the host its `HostArn` names; every other stored host
/// is untouched.
#[tokio::test]
async fn updating_one_host_leaves_every_other_host_as_it_was() {
    let emulator = Emulator::new();

    let mut arns = Vec::new();
    for index in 0..4 {
        let case = format!("host-{index}");
        arns.push(
            create_host(
                &emulator,
                &case,
                json!({
                    "Name": case,
                    "ProviderType": PROVIDER_TYPE,
                    "ProviderEndpoint": format!("https://host-{index}.example.com"),
                }),
            )
            .await,
        );
    }

    // Update each host in turn; after every update only that host has moved.
    for (target, arn) in arns.iter().enumerate() {
        let case = format!("updating host-{target}");
        let endpoint = format!("https://updated-{target}.example.com");
        let (status, response) = emulator
            .call(
                "UpdateHost",
                json!({ "HostArn": arn, "ProviderEndpoint": endpoint }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateHost to answer 200, got {status} {response:?}"
        );

        for (index, other) in arns.iter().enumerate() {
            let expected = if index <= target {
                format!("https://updated-{index}.example.com")
            } else {
                format!("https://host-{index}.example.com")
            };
            let host = get_host(&emulator, &case, other).await;
            assert_eq!(
                host["ProviderEndpoint"].as_str(),
                Some(expected.as_str()),
                "case {case:?}: host-{index} reported the wrong ProviderEndpoint in {host:?}"
            );
        }
    }
}

/// RULE: a `HostArn` that resolves to no host this service holds is reported ABSENT by an
/// update, whatever the ARN looks like, and the refused call writes nothing.
#[tokio::test]
async fn an_update_of_an_arn_naming_no_host_is_reported_absent() {
    let emulator = Emulator::new();

    // One real host, so a refusal comes from the lookup and not from an empty store.
    let kept = create_host(
        &emulator,
        "seed",
        json!({
            "Name": "seed",
            "ProviderType": PROVIDER_TYPE,
            "ProviderEndpoint": "https://seed.example.com",
        }),
    )
    .await;

    // Each ARN satisfies `HostArn`'s modeled pattern
    // `^arn:aws(-[\w]+)*:codestar-connections:.+:[0-9]{12}:host\/.+$` and names no host:
    // a UUID-shaped id, a named id, another region, another account, another partition.
    let arns = [
        format!(
            "arn:aws:codestar-connections:{REGION}:{ACCOUNT_ID}\
             :host/00000000-0000-0000-0000-000000000000"
        ),
        format!("arn:aws:codestar-connections:{REGION}:{ACCOUNT_ID}:host/enterprise-one"),
        format!(
            "arn:aws:codestar-connections:eu-central-1:{ACCOUNT_ID}\
             :host/11111111-2222-3333-4444-555555555555"
        ),
        format!("arn:aws:codestar-connections:{REGION}:111122223333:host/another-account"),
        format!("arn:aws-cn:codestar-connections:cn-north-1:{ACCOUNT_ID}:host/beijing"),
    ];

    for arn in arns {
        // The bodies differ along which optional members the update carries, since each
        // one has to be refused before anything is written.
        for body in [
            json!({ "HostArn": &arn }),
            json!({ "HostArn": &arn, "ProviderEndpoint": "https://nowhere.example.com" }),
            json!({ "HostArn": &arn, "VpcConfiguration": vpc_configuration(false, 1, None) }),
            json!({
                "HostArn": &arn,
                "ProviderEndpoint": "https://nowhere.example.com",
                "VpcConfiguration": vpc_configuration(true, 2, None),
            }),
        ] {
            let case = format!("UpdateHost {arn} {body}");
            let (status, response) = emulator.call("UpdateHost", body.clone()).await;
            // The model puts `@httpError 404` on `ResourceNotFoundException`, but the
            // recorded wire response for a host ARN that resolves to nothing is HTTP 400
            // with that same shape and message — the status a client of the real service
            // branches on; see `RESOURCE_NOT_FOUND_STATUS` in `src/state.rs`, the one
            // status this service answers every absent resource with — `GetHost` and
            // `DeleteHost` answer an unresolvable `HostArn` the same way.
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} {response:?}"
            );
            assert_eq!(
                error_code(&response),
                "ResourceNotFoundException",
                "case {case:?}: wrong error code in {response:?}"
            );
            // The message is built from the request: it names the ARN that was supplied.
            assert_eq!(
                error_message(&response),
                format!("No host exists with the ARN: {arn}"),
                "case {case:?}: wrong message in {response:?}"
            );

            // The refused update created nothing: the ARN still resolves to no host.
            let (status, read) = emulator.call("GetHost", json!({ "HostArn": &arn })).await;
            assert_eq!(
                status, 400,
                "case {case:?}: a refused UpdateHost created a host, got {status} {read:?}"
            );
        }
    }

    // The one real host is as it was created.
    let host = get_host(&emulator, "surviving host", &kept).await;
    assert_eq!(
        host["ProviderEndpoint"].as_str(),
        Some("https://seed.example.com"),
        "a refused UpdateHost changed the surviving host: {host:?}"
    );
}

/// RULE: an update outside a modeled constraint is refused before anything is written —
/// `ProviderEndpoint` is `Url len 1..512`, so each published bound holds and one past it
/// does not, and a `HostArn` outside the modeled pattern never reaches the lookup.
#[tokio::test]
async fn an_update_outside_the_modeled_constraints_is_refused() {
    let emulator = Emulator::new();

    let arn = create_host(
        &emulator,
        "seed",
        json!({
            "Name": "bounded",
            "ProviderType": PROVIDER_TYPE,
            "ProviderEndpoint": "https://bounded.example.com",
        }),
    )
    .await;

    // (endpoint length, must be accepted) — `Url` is modeled `len 1..512`: both published
    // bounds, one inside each, and one past the maximum.
    let lengths = [
        (0usize, false),
        (1, true),
        (511, true),
        (512, true),
        (513, false),
    ];
    let mut expected = "https://bounded.example.com".to_string();
    for (length, accepted) in lengths {
        let case = format!("endpoint-len-{length}");
        let endpoint = "u".repeat(length);
        let (status, response) = emulator
            .call(
                "UpdateHost",
                json!({ "HostArn": arn, "ProviderEndpoint": endpoint }),
            )
            .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: an endpoint inside len 1..512 was refused: {status} \
                 {response:?}"
            );
            expected = endpoint;
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: an endpoint outside len 1..512 was accepted: {status} \
                 {response:?}"
            );
        }

        // Only an accepted update reached state.
        let host = get_host(&emulator, &case, &arn).await;
        assert_eq!(
            host["ProviderEndpoint"].as_str(),
            Some(expected.as_str()),
            "case {case:?}: wrong ProviderEndpoint in {host:?}"
        );
    }

    // A `HostArn` the modeled pattern rejects never reaches the lookup: not an ARN at
    // all, another kind of resource, and an account field that is not twelve digits.
    for host_arn in [
        "not-an-arn",
        "arn:aws:codestar-connections:us-west-2:000000000000:connection/abc",
        &format!("arn:aws:codestar-connections:{REGION}:12345:host/short-account"),
    ] {
        let case = format!("HostArn={host_arn:?}");
        let (status, response) = emulator
            .call("UpdateHost", json!({ "HostArn": host_arn }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: an ARN outside the modeled pattern was accepted: {status} \
             {response:?}"
        );
    }

    // An update missing the required `HostArn` is refused by the request walk.
    let (status, response) = emulator
        .call(
            "UpdateHost",
            json!({ "ProviderEndpoint": "https://nowhere.example.com" }),
        )
        .await;
    assert_eq!(
        status, 400,
        "an update with no HostArn was accepted: {status} {response:?}"
    );
}
