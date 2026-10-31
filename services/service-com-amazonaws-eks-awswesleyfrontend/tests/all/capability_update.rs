// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `UpdateCapability`: the asynchronous change of one managed
//! capability, driven over the wire through the same `MultiServiceRouter` the
//! production binary mounts.
//!
//! Every suite asserts a RULE over a TABLE of inputs rather than one recorded
//! request: the rows differ along which member the change carries, which capability
//! type it is carried against, how a role mapping is added, replaced or removed, and
//! which resource of the pair the request addresses is missing. Each assertion names
//! its case and prints the actual value.
//!
//! The wire harness (`Answer`, `emulator`, `call`, `encode_label`) is the shared one
//! in `tests/all/wire.rs`.

use std::time::Duration;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    CAPABILITY_ACTIVATION_DELAY, CLUSTER_ACTIVATION_DELAY, CROSS_ACCOUNT_PASS_ROLE_REFUSAL,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, encode_label, error_code, Answer};

const REGION: &str = "us-west-2";

/// The IAM role a capability "uses to interact with Amazon Web Services services",
/// in the one account this emulator serves.
const CAPABILITY_ROLE: &str = "arn:aws:iam::000000000000:role/eksCapabilityRole";

/// A little longer than the span the clock owes a transition, so a read after it is
/// certainly past the due instant.
fn past(delay: Duration) -> Duration {
    delay + Duration::from_millis(250)
}

async fn create_cluster(router: &MultiServiceRouter, name: &str) {
    let body = serde_json::json!({
        "name": name,
        "roleArn": "arn:aws:iam::000000000000:role/eksClusterRole",
        "resourcesVpcConfig": { "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"] },
    });
    let created = call(router, REGION, "POST", "/clusters", &body.to_string()).await;
    assert_eq!(
        created.status, 200,
        "setup: expected 200 from CreateCluster for {name:?}, got {created:?}"
    );
    tokio::time::sleep(past(CLUSTER_ACTIVATION_DELAY)).await;
}

async fn create_capability(
    router: &MultiServiceRouter,
    cluster: &str,
    body: &serde_json::Value,
) -> Answer {
    call(
        router,
        REGION,
        "POST",
        &format!("/clusters/{}/capabilities", encode_label(cluster)),
        &body.to_string(),
    )
    .await
}

async fn update_capability(
    router: &MultiServiceRouter,
    cluster: &str,
    name: &str,
    body: &serde_json::Value,
) -> Answer {
    call(
        router,
        REGION,
        "POST",
        &format!(
            "/clusters/{}/capabilities/{}",
            encode_label(cluster),
            encode_label(name)
        ),
        &body.to_string(),
    )
    .await
}

async fn describe_capability(router: &MultiServiceRouter, cluster: &str, name: &str) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!(
            "/clusters/{}/capabilities/{}",
            encode_label(cluster),
            encode_label(name)
        ),
        "",
    )
    .await
}

async fn describe_update(router: &MultiServiceRouter, cluster: &str, update_id: &str) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!(
            "/clusters/{}/updates/{update_id}?capabilityName=cap",
            encode_label(cluster)
        ),
        "",
    )
    .await
}

/// Stand up one `ACTIVE` capability named `cap` of `capability_type` on its own
/// cluster, and answer the cluster's name.
async fn active_capability(
    router: &MultiServiceRouter,
    cluster: &str,
    capability_type: &str,
    configuration: Option<serde_json::Value>,
) {
    create_cluster(router, cluster).await;
    let mut body = serde_json::json!({
        "capabilityName": "cap",
        "type": capability_type,
        "roleArn": CAPABILITY_ROLE,
        "deletePropagationPolicy": "RETAIN",
    });
    if let Some(configuration) = configuration {
        body["configuration"] = configuration;
    }
    let created = create_capability(router, cluster, &body).await;
    assert_eq!(
        created.status, 200,
        "setup: expected 200 from CreateCapability on {cluster:?}, got {created:?}"
    );
    tokio::time::sleep(past(CAPABILITY_ACTIVATION_DELAY)).await;
}

// ---------------------------------------------------------------------------
// RULE: an update is asynchronous — the capability keeps every value it had until
// the clock completes the update, and then reports exactly what was asked for.
// ---------------------------------------------------------------------------

/// The table spans what the rule generalises over: which member the change carries
/// (the role, the ACK configuration's two members, the Argo CD network access, the
/// delete propagation policy) and which capability type it is carried against.
#[tokio::test]
async fn a_capability_reports_a_change_only_once_the_clock_completes_the_update() {
    let argo_cd_configuration = serde_json::json!({
        "argoCd": {
            "namespace": "argocd",
            "awsIdc": {
                "idcInstanceArn": "arn:aws:sso:::instance/ssoins-1234567890abcdef",
                "idcRegion": "us-east-1",
            },
            "rbacRoleMappings": [
                { "role": "ADMIN", "identities": [{ "id": "group-1", "type": "SSO_GROUP" }] },
            ],
        },
    });

    struct Case {
        name: &'static str,
        capability_type: &'static str,
        configuration: Option<serde_json::Value>,
        request: serde_json::Value,
        /// The JSON pointer into the capability document the change shows up at, and
        /// the value it must report once the update completes.
        expected: Vec<(&'static str, serde_json::Value)>,
    }

    let cases = [
        Case {
            name: "the role alone, on a KRO capability",
            capability_type: "KRO",
            configuration: None,
            request: serde_json::json!({
                "roleArn": "arn:aws:iam::000000000000:role/newCapabilityRole",
            }),
            expected: vec![(
                "/roleArn",
                serde_json::json!("arn:aws:iam::000000000000:role/newCapabilityRole"),
            )],
        },
        Case {
            name: "both ACK configuration members, on an ACK capability",
            capability_type: "ACK",
            configuration: None,
            request: serde_json::json!({
                "configuration": {
                    "ack": { "enableCrossNamespace": true, "disabledServices": ["s3", "sqs"] },
                },
            }),
            expected: vec![
                ("/configuration/ack/enableCrossNamespace", serde_json::json!(true)),
                (
                    "/configuration/ack/disabledServices",
                    serde_json::json!(["s3", "sqs"]),
                ),
            ],
        },
        Case {
            name: "the Argo CD network access, on an ARGOCD capability",
            capability_type: "ARGOCD",
            configuration: Some(argo_cd_configuration.clone()),
            request: serde_json::json!({
                "configuration": {
                    "argoCd": { "networkAccess": { "vpceIds": ["vpce-0123456789abcdef0"] } },
                },
            }),
            expected: vec![(
                "/configuration/argoCd/networkAccess/vpceIds",
                serde_json::json!(["vpce-0123456789abcdef0"]),
            )],
        },
        Case {
            name: "a role mapping added beside the one the capability already carries",
            capability_type: "ARGOCD",
            configuration: Some(argo_cd_configuration.clone()),
            request: serde_json::json!({
                "configuration": {
                    "argoCd": {
                        "rbacRoleMappings": {
                            "addOrUpdateRoleMappings": [
                                { "role": "VIEWER", "identities": [{ "id": "user-1", "type": "SSO_USER" }] },
                            ],
                        },
                    },
                },
            }),
            expected: vec![(
                "/configuration/argoCd/rbacRoleMappings",
                serde_json::json!([
                    { "role": "ADMIN", "identities": [{ "id": "group-1", "type": "SSO_GROUP" }] },
                    { "role": "VIEWER", "identities": [{ "id": "user-1", "type": "SSO_USER" }] },
                ]),
            )],
        },
        Case {
            name: "a role mapping that REPLACES the identities of a role already mapped",
            capability_type: "ARGOCD",
            configuration: Some(argo_cd_configuration.clone()),
            request: serde_json::json!({
                "configuration": {
                    "argoCd": {
                        "rbacRoleMappings": {
                            "addOrUpdateRoleMappings": [
                                { "role": "ADMIN", "identities": [{ "id": "group-2", "type": "SSO_GROUP" }] },
                            ],
                        },
                    },
                },
            }),
            expected: vec![(
                "/configuration/argoCd/rbacRoleMappings",
                serde_json::json!([
                    { "role": "ADMIN", "identities": [{ "id": "group-2", "type": "SSO_GROUP" }] },
                ]),
            )],
        },
        Case {
            name: "a role mapping removed, leaving the capability with none",
            capability_type: "ARGOCD",
            configuration: Some(argo_cd_configuration.clone()),
            request: serde_json::json!({
                "configuration": {
                    "argoCd": {
                        "rbacRoleMappings": {
                            "removeRoleMappings": [
                                { "role": "ADMIN", "identities": [{ "id": "group-1", "type": "SSO_GROUP" }] },
                            ],
                        },
                    },
                },
            }),
            expected: vec![(
                "/configuration/argoCd/rbacRoleMappings",
                serde_json::json!([]),
            )],
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let router = emulator();
        let cluster = format!("capability-update-{index}");
        active_capability(
            &router,
            &cluster,
            case.capability_type,
            case.configuration.clone(),
        )
        .await;

        let before = describe_capability(&router, &cluster, "cap").await;
        let started = update_capability(&router, &cluster, "cap", &case.request).await;
        assert_eq!(
            started.status, 200,
            "case {:?}: expected 200 from UpdateCapability, got {started:?}",
            case.name
        );
        assert_eq!(
            started.body["update"]["type"],
            serde_json::json!("CapabilityUpdate"),
            "case {:?}: expected update type CapabilityUpdate, got {:?}",
            case.name,
            started.body["update"]["type"]
        );
        assert_eq!(
            started.body["update"]["status"],
            serde_json::json!("InProgress"),
            "case {:?}: expected update status InProgress, got {:?}",
            case.name,
            started.body["update"]["status"]
        );

        // The change is NOT visible while the update is in flight, and the capability
        // says so: it is `UPDATING`.
        let during = describe_capability(&router, &cluster, "cap").await;
        assert_eq!(
            during.body["capability"]["status"],
            serde_json::json!("UPDATING"),
            "case {:?}: expected status UPDATING while the update runs, got {:?}",
            case.name,
            during.body["capability"]["status"]
        );
        for (pointer, _) in &case.expected {
            assert_eq!(
                during.body["capability"].pointer(pointer),
                before.body["capability"].pointer(pointer),
                "case {:?}: {pointer} must not change before the update completes, got {:?}",
                case.name,
                during.body["capability"].pointer(pointer)
            );
        }

        tokio::time::sleep(past(CAPABILITY_ACTIVATION_DELAY)).await;

        let after = describe_capability(&router, &cluster, "cap").await;
        assert_eq!(
            after.body["capability"]["status"],
            serde_json::json!("ACTIVE"),
            "case {:?}: expected status ACTIVE once the update completes, got {:?}",
            case.name,
            after.body["capability"]["status"]
        );
        for (pointer, expected) in &case.expected {
            let actual = after.body["capability"].pointer(pointer);
            assert_eq!(
                actual,
                Some(expected),
                "case {:?}: expected {pointer} {expected:?}, got {actual:?}",
                case.name
            );
        }

        // The update the call answered with is the one the cluster recorded, and the
        // clock has completed it.
        let update_id = started.body["update"]["id"]
            .as_str()
            .expect("UpdateCapability answers with the id of the update it started");
        let described = describe_update(&router, &cluster, update_id).await;
        assert_eq!(
            described.body["update"]["status"],
            serde_json::json!("Successful"),
            "case {:?}: expected the recorded update to be Successful, got {described:?}",
            case.name
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: the request is refused before the change is recorded — the pass-role
// authorization of the role it names first, then the resources it addresses, then
// the body.
// ---------------------------------------------------------------------------

/// The table spans the refusals the operation can give, by what the row breaks: the
/// account the role belongs to, which half of the addressed pair is missing, the
/// capability type a configuration member belongs to, and a request that asks for
/// nothing at all.
#[tokio::test]
async fn an_update_is_refused_before_anything_is_recorded() {
    let router = emulator();
    active_capability(&router, "refusals", "ACK", None).await;

    let cases: Vec<(&str, &str, &str, serde_json::Value, u16, &str)> = vec![
        (
            "a role of another account",
            "refusals",
            "cap",
            serde_json::json!({ "roleArn": "arn:aws:iam::111122223333:role/elsewhere" }),
            403,
            "AccessDeniedException",
        ),
        (
            "a roleArn that is not an ARN at all",
            "refusals",
            "cap",
            serde_json::json!({ "roleArn": "not-an-arn" }),
            403,
            "AccessDeniedException",
        ),
        (
            "a cluster that was never created",
            "no-such-cluster",
            "cap",
            serde_json::json!({ "roleArn": CAPABILITY_ROLE }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a capability the cluster does not carry",
            "refusals",
            "no-such-capability",
            serde_json::json!({ "roleArn": CAPABILITY_ROLE }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an argoCd configuration on an ACK capability",
            "refusals",
            "cap",
            serde_json::json!({
                "configuration": { "argoCd": { "networkAccess": { "vpceIds": [] } } },
            }),
            400,
            "InvalidParameterException",
        ),
        (
            "a request that asks for no change at all",
            "refusals",
            "cap",
            serde_json::json!({}),
            400,
            "InvalidParameterException",
        ),
    ];

    for (case, cluster, capability, request, status, expected_code) in &cases {
        let answer = update_capability(&router, cluster, capability, request).await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected HTTP {status}, got {answer:?}"
        );
        let actual = error_code(&answer);
        assert_eq!(
            &actual, expected_code,
            "case {case:?}: expected {expected_code}, got {actual:?}"
        );
        if *expected_code == "AccessDeniedException" {
            assert_eq!(
                answer.body["message"],
                serde_json::json!(CROSS_ACCOUNT_PASS_ROLE_REFUSAL),
                "case {case:?}: expected the cross-account pass-role sentence, got {:?}",
                answer.body["message"]
            );
        }
        // Nothing was recorded: the capability is still the one that was created.
        let described = describe_capability(&router, "refusals", "cap").await;
        assert_eq!(
            described.body["capability"]["roleArn"],
            serde_json::json!(CAPABILITY_ROLE),
            "case {case:?}: a refused update must change nothing, got {described:?}"
        );
        assert_eq!(
            described.body["capability"]["status"],
            serde_json::json!("ACTIVE"),
            "case {case:?}: a refused update must leave the capability settled, got {described:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: one capability takes one update at a time, and a retry of the call that
// started one is answered with that same update.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_second_update_conflicts_and_a_retry_is_absorbed() {
    let router = emulator();
    active_capability(&router, "one-at-a-time", "ACK", None).await;

    let request = serde_json::json!({
        "clientRequestToken": "token-one",
        "configuration": { "ack": { "enableCrossNamespace": true } },
    });
    let first = update_capability(&router, "one-at-a-time", "cap", &request).await;
    assert_eq!(
        first.status, 200,
        "case first update: expected 200, got {first:?}"
    );
    let update_id = first.body["update"]["id"].clone();

    // The SAME token is the SDK's retry of that call: it is answered with the update
    // the first call started, not with a second one.
    let retried = update_capability(&router, "one-at-a-time", "cap", &request).await;
    assert_eq!(
        retried.status, 200,
        "case retry: expected 200, got {retried:?}"
    );
    assert_eq!(
        retried.body["update"]["id"], update_id,
        "case retry: expected the first update's id {update_id:?}, got {:?}",
        retried.body["update"]["id"]
    );

    // A DIFFERENT token while the first update is still in flight is a second
    // update of a capability that is already taking one.
    let conflicting = update_capability(
        &router,
        "one-at-a-time",
        "cap",
        &serde_json::json!({
            "clientRequestToken": "token-two",
            "configuration": { "ack": { "enableCrossNamespace": false } },
        }),
    )
    .await;
    assert_eq!(
        conflicting.status, 409,
        "case second update: expected HTTP 409, got {conflicting:?}"
    );
    let actual = error_code(&conflicting);
    assert_eq!(
        actual, "ResourceInUseException",
        "case second update: expected ResourceInUseException, got {actual:?}"
    );

    // Once the first update completes, the capability takes another.
    tokio::time::sleep(past(CAPABILITY_ACTIVATION_DELAY)).await;
    let next = update_capability(
        &router,
        "one-at-a-time",
        "cap",
        &serde_json::json!({
            "clientRequestToken": "token-three",
            "configuration": { "ack": { "enableCrossNamespace": false } },
        }),
    )
    .await;
    assert_eq!(
        next.status, 200,
        "case update after the first completed: expected 200, got {next:?}"
    );
}
