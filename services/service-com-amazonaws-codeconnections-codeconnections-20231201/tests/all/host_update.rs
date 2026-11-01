// SPDX-License-Identifier: CC0-1.0

//! `UpdateHost` over the wire: what it writes, and what it answers for a host that is
//! not there.
//!
//! THE RULE: an update addressed at a resource by ARN resolves that ARN in state FIRST —
//! an ARN resolving to no host is the modelled `ResourceNotFoundException` (404) stating
//! the one sentence every host-addressed call of this service answers for an absent host,
//! and nothing is written — and otherwise applies EVERY member the request carries and
//! only those, leaving a member the request omits exactly as it was. The empty output
//! reports nothing, so what the update left on the host is read back through `GetHost`.
//!
//! Each test asserts the rule over a TABLE whose rows differ along what the rule
//! generalises over: which host (every shape `host_association.rs` creates), which subset
//! of the updatable members the request names, which replacement value each carries, and
//! the ways an ARN the modelled `HostArn` pattern admits can resolve to nothing. The
//! recorded unit's case — an update addressed at a host that was deleted — is one row of
//! the second table.
//!
//! Every request goes through the real `MultiServiceRouter` dispatch, reusing the
//! `connection_lifecycle` suite's helpers, so these tests exercise the same path a signed
//! SDK request takes.

use smithy_server_runtime::MultiServiceRouter;

use super::connection_lifecycle::{REGION, call, router};
use super::git_sync::error_code;
use super::host_association::{
    VPC_LIST_MAX, create_host, create_plain_host, host_cases, host_not_found_message,
    vpc_configuration,
};

/// One `UpdateHost` request: which of the two updatable members it names, and what it
/// leaves the host carrying.
struct UpdateCase {
    /// What a failing assertion names this row by.
    label: &'static str,
    /// The replacement `ProviderEndpoint`, or `None` for a request that omits it.
    provider_endpoint: Option<String>,
    /// The replacement `VpcConfiguration`, or `None` for a request that omits it.
    vpc_configuration: Option<serde_json::Value>,
}

/// The update table: the four subsets of the two members `UpdateHost` can write, with a
/// replacement endpoint spanning the published bounds of the modelled `Url`
/// (`length 1..512`, from `smithy_query(query="operation", operation="UpdateHost")`) and a
/// replacement VPC configuration at each published list bound.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            label: "the endpoint alone",
            provider_endpoint: Some("https://ghe-updated.example.internal".to_string()),
            vpc_configuration: None,
        },
        UpdateCase {
            label: "the vpc configuration alone",
            provider_endpoint: None,
            vpc_configuration: Some(vpc_configuration(1, 1, None)),
        },
        UpdateCase {
            label: "both members, the endpoint at its published maximum length",
            provider_endpoint: Some(format!("https://{}", "u".repeat(512 - "https://".len()))),
            vpc_configuration: Some(vpc_configuration(
                VPC_LIST_MAX,
                VPC_LIST_MAX,
                Some("-----BEGIN CERTIFICATE-----\nYmFy\n-----END CERTIFICATE-----"),
            )),
        },
        UpdateCase {
            label: "both members, the endpoint at its published minimum length",
            provider_endpoint: Some("u".to_string()),
            vpc_configuration: Some(vpc_configuration(3, 2, None)),
        },
        UpdateCase {
            label: "neither member, so the request names no configuration at all",
            provider_endpoint: None,
            vpc_configuration: None,
        },
    ]
}

/// The `UpdateHost` body for one row, addressed at `host_arn`.
fn update_body(case: &UpdateCase, host_arn: &str) -> serde_json::Value {
    let mut body = serde_json::json!({ "HostArn": host_arn });
    if let Some(endpoint) = &case.provider_endpoint {
        body["ProviderEndpoint"] = serde_json::Value::String(endpoint.clone());
    }
    if let Some(vpc) = &case.vpc_configuration {
        body["VpcConfiguration"] = vpc.clone();
    }
    body
}

/// Read one host back, failing the test on any non-200.
async fn get_host(
    router: &MultiServiceRouter,
    case: &str,
    host_arn: &str,
) -> serde_json::Value {
    let (status, body) = call(router, "GetHost", serde_json::json!({ "HostArn": host_arn })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetHost, got {status} {body}"
    );
    body
}

/// Every member an `UpdateHost` request names replaces the stored one, every member it
/// omits is left alone, and the members no update can address (the host's name, provider
/// type and status) never move.
///
/// Rows are every host shape `host_association.rs` creates crossed with every subset of
/// the updatable members, applied in sequence to the SAME host — so each row is also an
/// assertion that the previous row's values survived into it.
#[tokio::test]
async fn update_host_writes_every_member_the_request_names_and_only_those() {
    let router = router();

    for host_case in host_cases() {
        let host = create_host(&router, &host_case).await;
        let before = get_host(&router, host_case.label, &host).await;
        let mut expected_endpoint = host_case.provider_endpoint.clone();
        let mut expected_vpc = before["VpcConfiguration"].clone();

        for update in update_cases() {
            let case = format!("{} / {}", host_case.label, update.label);
            if let Some(endpoint) = &update.provider_endpoint {
                expected_endpoint = endpoint.clone();
            }
            if let Some(vpc) = &update.vpc_configuration {
                expected_vpc = vpc.clone();
            }

            let (status, body) = call(&router, "UpdateHost", update_body(&update, &host)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from UpdateHost, got {status} {body}"
            );
            assert!(
                body.as_object().is_none_or(serde_json::Map::is_empty),
                "case {case:?}: UpdateHost reports no members, got {body}"
            );

            let got = get_host(&router, &case, &host).await;
            assert_eq!(
                got["ProviderEndpoint"],
                serde_json::json!(expected_endpoint),
                "case {case:?}: expected the endpoint {expected_endpoint:?}, got {got}"
            );
            assert_eq!(
                got["VpcConfiguration"], expected_vpc,
                "case {case:?}: expected the vpc configuration {expected_vpc}, got {got}"
            );
            // Nothing this operation cannot address moved.
            for member in ["Name", "ProviderType", "Status"] {
                assert_eq!(
                    got[member], before[member],
                    "case {case:?}: {member} is no member of an update, got {got}"
                );
            }
        }

        // The host's tags are on the same row as its configuration, so an update leaves
        // them where they were.
        let (status, tags) = call(
            &router,
            "ListTagsForResource",
            serde_json::json!({ "ResourceArn": host }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from ListTagsForResource, got {status} {tags}",
            host_case.label
        );
        let reported = tags["Tags"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            reported.len(),
            host_case.tags.len(),
            "case {:?}: expected the host's {} tags, got {tags}",
            host_case.label,
            host_case.tags.len()
        );
    }
}

/// An `UpdateHost` addressed at an ARN that resolves to no host is refused 404
/// `ResourceNotFoundException` with the service's fixed sentence naming the ARN from the
/// request, and writes nothing.
///
/// Rows are the ways an ARN the modelled `HostArn` pattern admits can resolve to nothing:
/// an id this store never minted, the ARN of a host that was DELETED (the recorded unit's
/// case), and a live host's ARN restated in another region, account, partition or in the
/// legacy service namespace the pattern also admits.
#[tokio::test]
async fn update_host_refuses_an_arn_that_resolves_to_no_host() {
    let router = router();
    let live = create_plain_host(&router, "live-host", "GitHubEnterpriseServer").await;
    let live_endpoint = get_host(&router, "the live host", &live).await["ProviderEndpoint"].clone();
    let doomed = create_plain_host(&router, "doomed-host", "GitLabSelfManaged").await;
    let (status, body) = call(
        &router,
        "DeleteHost",
        serde_json::json!({ "HostArn": doomed }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the first DeleteHost must succeed, got {status} {body}"
    );

    let account = engine::emulator_account_id();
    let rows: Vec<(&str, String)> = vec![
        (
            "a host id this store never minted",
            format!(
                "arn:aws:codeconnections:{REGION}:{account}:host/00000000-1111-2222-3333-444444444444"
            ),
        ),
        ("the ARN of a host that was deleted", doomed.clone()),
        (
            "a live host's id in another region",
            live.replace(REGION, "ca-central-1"),
        ),
        (
            "a live host's id in another account",
            live.replace(account, "314159265358"),
        ),
        (
            "a live host's id in the aws-us-gov partition",
            live.replace("arn:aws:", "arn:aws-us-gov:"),
        ),
        (
            "a live host's id in the legacy codestar-connections namespace",
            live.replace("codeconnections", "codestar-connections"),
        ),
    ];

    for (label, arn) in &rows {
        for update in update_cases() {
            let case = format!("{label} / {}", update.label);
            let (status, body) = call(&router, "UpdateHost", update_body(&update, arn)).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404 for an ARN that resolves to no host, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {body}"
            );
            let message = body["Message"]
                .as_str()
                .or_else(|| body["message"].as_str())
                .unwrap_or_else(|| panic!("case {case:?}: the refusal carries no message: {body}"));
            assert_eq!(
                message,
                host_not_found_message(arn),
                "case {case:?}: expected the fixed sentence naming the requested ARN, \
                 got {message:?}"
            );
        }
    }

    // Nothing above reached the live host: it still reports the endpoint it was created
    // with, and the deleted host is still gone.
    let got = get_host(&router, "the live host after the refusals", &live).await;
    assert_eq!(
        got["ProviderEndpoint"], live_endpoint,
        "a refused UpdateHost writes nothing, got {got}"
    );
    let (status, body) = call(
        &router,
        "GetHost",
        serde_json::json!({ "HostArn": doomed }),
    )
    .await;
    assert_eq!(
        status, 404,
        "the deleted host must stay deleted, got {status} {body}"
    );
}
