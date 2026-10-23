// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetPasswordData`, driven over the wire the service actually
//! speaks — `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. the operation REPORTS THE PASSWORD DATA THE SERVICE HOLDS for the instance the
//!    request names, read out of the instance store: the instance's own id, and an
//!    EMPTY `passwordData` for an instance whose password is not available ("Returns an
//!    empty string if the password is not available",
//!    `GetPasswordDataResult$PasswordData`) — which is every instance here, since no key
//!    pair material exists for a launch to have named, so EC2 would hold no password
//!    encrypted under one. `timestamp` is "the time the data was last updated", so it is
//!    reported for data that exists and omitted when there is none;
//! 2. an instance id OUTSIDE THE FORM EC2 ISSUES is refused by syntax, before any
//!    lookup, with `InvalidInstanceID.Malformed`, and only an id inside that form which
//!    names no instance of the account is `InvalidInstanceID.NotFound`;
//! 3. `DryRun` answers whether the call WOULD have succeeded — `DryRunOperation` for a
//!    request that would have been served, and the refusal itself for one that would
//!    have been refused;
//! 4. the operation is a READ: two calls about one instance answer the same thing and
//!    neither moves the instance.
//!
//! Each table row is a distinct input along what the rule generalises over — the image's
//! platform, whether the launch named a key pair, which state the instance is parked in,
//! and for the id rule every way an id can fall outside the form (prefix, length, digit
//! case, a non-hex digit, the retired short form, a leading digit above zero, the null
//! id) beside two ids inside it — so no row is the specification.

use std::time::Duration;

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, IMAGE_PLATFORM_WINDOWS, ImageSpec,
    PLATFORM_DETAILS_WINDOWS, TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, error_code};

/// The Region every call here is signed for; it only selects the ec2Query envelope,
/// since password data carries no Region member.
const REGION: &str = "us-west-2";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One ec2Query call through the shared harness.
async fn api(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// Wait until every clock-owed transition started before now has certainly fallen due.
/// Derived from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

/// A Windows AMI, as a register operation would store one: "this value is set to
/// `windows` for Windows AMIs; otherwise, it is blank" (`Image$Platform`) is what makes
/// an image a Windows one — the platform whose boot agent generates an administrator
/// password at all.
fn windows_spec(name: &str) -> ImageSpec {
    ImageSpec {
        name: name.to_string(),
        platform: Some(IMAGE_PLATFORM_WINDOWS.to_string()),
        platform_details: PLATFORM_DETAILS_WINDOWS.to_string(),
        ..ImageSpec::default()
    }
}

/// The two images the rows below launch from: one Windows AMI and one Linux AMI of the
/// calling account.
struct Images {
    windows: String,
    linux: String,
}

/// Write the two images into the engine's store, the way a register operation would.
fn seed(engine: &engine::InMemoryEngine) -> Images {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let windows = guard.insert_image(windows_spec("password-data-windows"));
    let linux = guard.insert_image(ImageSpec {
        name: "password-data-linux".to_string(),
        ..ImageSpec::default()
    });
    Images { windows, linux }
}

/// One emulator over its own engine, with the two images seeded.
fn emulator() -> (MultiServiceRouter, Images) {
    let (router, engine) = super::ec2_query::emulator();
    let images = seed(&engine);
    (router, images)
}

/// The text of the response's immediate `name` child: `None` when the element is absent,
/// `Some("")` when it is present and empty — the two cases the `timestamp` and
/// `passwordData` assertions below have to tell apart.
fn member(body: &str, name: &str) -> Option<String> {
    let document = roxmltree::Document::parse(body)
        .unwrap_or_else(|error| panic!("the response body must be XML ({error}): {body}"));
    document
        .root_element()
        .children()
        .find(|node| node.is_element() && node.tag_name().name() == name)
        .map(|node| node.text().unwrap_or_default().to_string())
}

/// Launch one instance from `image_id`, optionally naming a key pair, and answer its id.
async fn launch(router: &MultiServiceRouter, image_id: &str, key_name: Option<&str>) -> String {
    let mut params = vec![
        p("ImageId", image_id),
        p("MinCount", "1"),
        p("MaxCount", "1"),
    ];
    if let Some(key_name) = key_name {
        params.push(p("KeyName", key_name));
    }
    let (status, body) = api(router, "RunInstances", &params).await;
    assert_eq!(status, 200, "RunInstances must succeed, got {status}: {body}");
    super::ec2_query::element(&body, "instanceId")
        .unwrap_or_else(|| panic!("RunInstances must report the instance it minted: {body}"))
}

/// The `instanceId`/`instanceState` pair `DescribeInstances` reports for one instance,
/// as a string the "nothing moved" assertions compare.
async fn described_state(router: &MultiServiceRouter, instance_id: &str) -> String {
    let (status, body) = api(
        router,
        "DescribeInstances",
        &[p("InstanceId.1", instance_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeInstances for {instance_id} must succeed, got {status}: {body}"
    );
    let document = roxmltree::Document::parse(&body)
        .unwrap_or_else(|error| panic!("the response body must be XML ({error}): {body}"));
    document
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "instanceState")
        .and_then(|state| {
            state
                .children()
                .find(|node| node.is_element() && node.tag_name().name() == "name")
        })
        .and_then(|name| name.text())
        .unwrap_or_default()
        .to_string()
}

/// Park one instance in `state`, reached only through the public API, and answer its id.
async fn instance_in_state(
    router: &MultiServiceRouter,
    image_id: &str,
    key_name: Option<&str>,
    state: &str,
) -> String {
    let instance_id = launch(router, image_id, key_name).await;
    match state {
        // A launch answers `pending`, and the transition span is never zero, so the
        // caller that calls straight after the launch still sees it.
        "pending" => {}
        "running" => past_the_transition().await,
        "stopped" => {
            past_the_transition().await;
            let (status, body) = api(
                router,
                "StopInstances",
                &[p("InstanceId.1", instance_id.as_str())],
            )
            .await;
            assert_eq!(status, 200, "StopInstances must succeed, got {status}: {body}");
            past_the_transition().await;
        }
        other => panic!("no way to park an instance in state {other:?}"),
    }
    instance_id
}

/// One launch form to read the password data of.
#[derive(Debug, Clone, Copy)]
struct Case {
    label: &'static str,
    /// Whether the launch uses the Windows AMI — the only platform EC2 generates an
    /// administrator password for at all.
    windows: bool,
    /// The key pair the launch names, if any: "the password is encrypted using the key
    /// pair that you specified when you launched the instance".
    key_name: Option<&'static str>,
    /// The state the instance is parked in before the password data is read.
    state: &'static str,
}

/// Rule 1: whatever the launch form, the service reports the instance's own id and an
/// empty `passwordData`, because it holds no key pair material to have encrypted a
/// password under — and reports no `timestamp`, because no password data was ever
/// written for that instant to belong to.
#[tokio::test]
async fn password_data_is_reported_for_every_launch_form() {
    let cases = [
        Case {
            label: "a Windows instance launched with a key pair, running",
            windows: true,
            key_name: Some("password-data-key"),
            state: "running",
        },
        Case {
            label: "a Windows instance launched with no key pair, running",
            windows: true,
            key_name: None,
            state: "running",
        },
        Case {
            label: "a Windows instance still pending",
            windows: true,
            key_name: Some("password-data-key"),
            state: "pending",
        },
        Case {
            label: "a Windows instance that has been stopped",
            windows: true,
            key_name: Some("password-data-key"),
            state: "stopped",
        },
        Case {
            label: "a Linux instance launched with a key pair, running",
            windows: false,
            key_name: Some("password-data-key"),
            state: "running",
        },
        Case {
            label: "a Linux instance launched with no key pair, running",
            windows: false,
            key_name: None,
            state: "running",
        },
    ];

    for case in cases {
        let (router, images) = emulator();
        let image_id = if case.windows {
            images.windows.clone()
        } else {
            images.linux.clone()
        };
        let instance_id = instance_in_state(&router, &image_id, case.key_name, case.state).await;

        let (status, body) = api(
            &router,
            "GetPasswordData",
            &[p("InstanceId", instance_id.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: GetPasswordData must succeed, got {status}: {body}",
            case.label
        );
        assert_eq!(
            member(&body, "instanceId"),
            Some(instance_id.clone()),
            "case {:?}: the response must name the instance the service resolved, got {body}",
            case.label
        );
        assert_eq!(
            member(&body, "passwordData"),
            Some(String::new()),
            "case {:?}: a password that is not available is reported as an empty \
             passwordData, got {body}",
            case.label
        );
        assert_eq!(
            member(&body, "timestamp"),
            None,
            "case {:?}: with no password data there is no instant it was last updated, \
             got {body}",
            case.label
        );

        // Rule 4: the read answers the same thing twice and moves nothing. A settled
        // state is compared directly; `pending` is one the clock may already have moved
        // on from, so only the repeat answer is compared for that row.
        let before = described_state(&router, &instance_id).await;
        let (repeat_status, repeat_body) = api(
            &router,
            "GetPasswordData",
            &[p("InstanceId", instance_id.as_str())],
        )
        .await;
        assert_eq!(
            (repeat_status, member(&repeat_body, "passwordData")),
            (status, Some(String::new())),
            "case {:?}: two reads of one instance must answer the same thing, got \
             {repeat_status}: {repeat_body}",
            case.label
        );
        if case.state != "pending" {
            let after = described_state(&router, &instance_id).await;
            assert_eq!(
                after, before,
                "case {:?}: reading the password data must not move the instance",
                case.label
            );
            assert_eq!(
                after, case.state,
                "case {:?}: the instance was meant to stay {}, got {after:?}",
                case.label, case.state
            );
        }
    }
}

/// Rule 2: an id outside the form EC2 issues is malformed, before any lookup; an id
/// inside it that names nothing is not found.
#[tokio::test]
async fn an_unresolvable_instance_id_is_refused_before_the_password_data_is_read() {
    let (router, images) = emulator();
    // One live instance, so the store is not empty when the unknown ids are looked up.
    let known = launch(&router, &images.linux, None).await;

    // (what is wrong with the id, the id, the code EC2 answers)
    let cases: [(&str, &str, &str); 12] = [
        (
            "the resource prefix of no kind at all",
            "instance-00000000000000001",
            "InvalidInstanceID.Malformed",
        ),
        (
            "no resource prefix",
            "00000000000000001",
            "InvalidInstanceID.Malformed",
        ),
        (
            "one digit short of the long form",
            "i-0123456789abcde",
            "InvalidInstanceID.Malformed",
        ),
        (
            "one digit past the long form",
            "i-0123456789abcdef01",
            "InvalidInstanceID.Malformed",
        ),
        (
            "the retired 8-digit short form",
            "i-0abc1234",
            "InvalidInstanceID.Malformed",
        ),
        (
            "a digit that is not hexadecimal",
            "i-0z23456789abcdef0",
            "InvalidInstanceID.Malformed",
        ),
        (
            "upper-case hexadecimal digits",
            "i-0ABCDEF0123456789",
            "InvalidInstanceID.Malformed",
        ),
        (
            "a leading digit above zero",
            "i-1234567890abcdef0",
            "InvalidInstanceID.Malformed",
        ),
        (
            "every digit above zero",
            "i-fffffffffffffffff",
            "InvalidInstanceID.Malformed",
        ),
        (
            "the null id",
            "i-00000000000000000",
            "InvalidInstanceID.Malformed",
        ),
        (
            "a well-formed id nothing was launched for",
            "i-00000000000000999",
            "InvalidInstanceID.NotFound",
        ),
        (
            "another well-formed id nothing was launched for",
            "i-0f0f0f0f0f0f0f0f0",
            "InvalidInstanceID.NotFound",
        ),
    ];

    for (label, instance_id, code) in cases {
        let (status, body) = api(&router, "GetPasswordData", &[p("InstanceId", instance_id)]).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, code),
            "case {label:?} ({instance_id}): expected 400 {code}, got {status}: {body}"
        );
    }

    // The id that does resolve still resolves: the refusals above are about the ids, not
    // about the operation.
    let (status, body) = api(&router, "GetPasswordData", &[p("InstanceId", known.as_str())]).await;
    assert_eq!(
        status, 200,
        "the launched instance must still be readable, got {status}: {body}"
    );
}

/// Rule 3: `DryRun` says whether the call would have succeeded — and a request that
/// would have been refused is refused with that reason instead.
#[tokio::test]
async fn dry_run_reports_whether_the_call_would_have_succeeded() {
    let (router, images) = emulator();
    let instance_id = launch(&router, &images.windows, Some("password-data-key")).await;
    past_the_transition().await;

    // (what the dry run names, the id, the status and code EC2 answers)
    let cases: [(&str, String, u16, &str); 3] = [
        (
            "an instance the account holds",
            instance_id.clone(),
            412,
            "DryRunOperation",
        ),
        (
            "an id inside the form that names nothing",
            "i-00000000000000999".to_string(),
            400,
            "InvalidInstanceID.NotFound",
        ),
        (
            "an id outside the form EC2 issues",
            "i-1234567890abcdef0".to_string(),
            400,
            "InvalidInstanceID.Malformed",
        ),
    ];

    let before = described_state(&router, &instance_id).await;
    for (label, id, expected_status, code) in cases {
        let (status, body) = api(
            &router,
            "GetPasswordData",
            &[p("InstanceId", id.as_str()), p("DryRun", "true")],
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (expected_status, code),
            "case {label:?} ({id}): expected {expected_status} {code}, got {status}: {body}"
        );
    }
    let after = described_state(&router, &instance_id).await;
    assert_eq!(
        after, before,
        "a dry run must leave the instance exactly as it was"
    );

    // And the same call without the flag is served.
    let (status, body) = api(
        &router,
        "GetPasswordData",
        &[p("InstanceId", instance_id.as_str())],
    )
    .await;
    assert_eq!(
        (status, member(&body, "passwordData")),
        (200, Some(String::new())),
        "the request the dry run stood for must be served, got {status}: {body}"
    );
}
