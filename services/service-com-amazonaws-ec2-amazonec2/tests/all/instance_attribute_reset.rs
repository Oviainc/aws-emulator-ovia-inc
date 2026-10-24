// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ResetInstanceAttribute`: which attributes it resets, what it
//! resets each one TO, which instance states admit the reset, and what it refuses.
//!
//! The transport is the crate's shared [`super::ec2_query`] harness and the XML reading
//! goes through [`super::wire`]'s tree helpers, so this suite adds only the readers the
//! instance-attribute assertions need. Every assertion is driven from a TABLE: the rule
//! under test holds over a CLASS of inputs (every `InstanceAttributeName`, every instance
//! state, every id this service cannot resolve), and one recorded request is a single row
//! of it.

use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2::ops::types::{Image, INSTANCE_ATTRIBUTE_NAME_VALUES};
use service_com_amazonaws_ec2_amazonec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, DEFAULT_SOURCE_DEST_CHECK,
    INSTANCE_ATTRIBUTE_KERNEL, INSTANCE_ATTRIBUTE_RAMDISK, INSTANCE_ATTRIBUTE_SOURCE_DEST_CHECK,
    MAX_KERNEL_OR_RAMDISK_ID_CHARS, RESETTABLE_INSTANCE_ATTRIBUTES, TRANSITION_DELAY,
};

use super::ec2_query;
use super::wire::{child, child_text, document};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The image every launch here names. No operation of this emulator registers an image
/// carrying a kernel or a RAM disk, so the default this AMI contributes to a reset of
/// `kernel` or `ramdisk` is "none at all".
const AMI: &str = "ami-0123456789abcdef0";

/// A kernel id a launch may name (`RunInstancesRequest$KernelId`); AWS kernel images are
/// `aki-` ids.
const KERNEL: &str = "aki-0123456789abcdef0";

/// A second kernel id, so a row that varies the value does not reuse the first.
const OTHER_KERNEL: &str = "aki-00abcdef123456789";

/// A RAM disk id a launch may name (`RunInstancesRequest$RamdiskId`); AWS RAM disk images
/// are `ari-` ids.
const RAMDISK: &str = "ari-0123456789abcdef0";

/// A second RAM disk id, for the same reason as [`OTHER_KERNEL`].
const OTHER_RAMDISK: &str = "ari-00abcdef123456789";

/// An image that DOES carry a kernel and a RAM disk of its own (`Image$KernelId` /
/// `Image$RamdiskId`), seeded into the store: `RegisterImage` is still a stub, so no call
/// can register one, and without it "reset restores the image's kernel" could not be told
/// apart from "reset clears the kernel".
const AMI_WITH_KERNEL: &str = "ami-00fedcba987654321";

/// The kernel [`AMI_WITH_KERNEL`] carries.
const IMAGE_KERNEL: &str = "aki-0abcdef1234567890";

/// The RAM disk [`AMI_WITH_KERNEL`] carries.
const IMAGE_RAMDISK: &str = "ari-0abcdef1234567890";

/// Wait until every clock-owed transition started before now has certainly fallen due.
/// Derived from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

/// One EC2 emulator over its own engine, as [`super::ec2_query::emulator`] builds it. The
/// engine is kept so the two branches no implemented operation can reach — an instance
/// whose source/destination checking is OFF, and an image that carries a kernel — can be
/// seeded straight into the store, the way `byoip_cidr_lifecycle` seeds its ranges.
struct Ec2 {
    router: smithy_server_runtime::MultiServiceRouter,
    engine: ::std::sync::Arc<engine::InMemoryEngine>,
}

impl Ec2 {
    fn new() -> Self {
        let (router, engine) = ec2_query::emulator();
        Self { router, engine }
    }

    async fn call(&self, action: &str, params: Vec<(String, String)>) -> Answer {
        let (status, body) = ec2_query::call(&self.router, REGION, action, &params).await;
        Answer { status, body }
    }

    /// Set the stored `sourceDestCheck` of one instance. Nothing in a launch disables
    /// source/destination checking and `ModifyInstanceAttribute` is still a 501 stub, so
    /// this is the only way to reach the non-default value a reset has to restore from.
    fn set_source_dest_check(&self, instance_id: &str, value: bool) {
        let store = self.engine.state::<Ec2State>();
        let mut guard = store.lock();
        let instance = guard
            .instances
            .get_mut(instance_id)
            .unwrap_or_else(|| panic!("the store must hold the launched instance {instance_id}"));
        instance.source_dest_check = value;
    }

    /// Give the store an image carrying `kernel` and `ramdisk` of its own.
    fn store_image_with_kernel(&self, image_id: &str, kernel: &str, ramdisk: &str) {
        let store = self.engine.state::<Ec2State>();
        let mut guard = store.lock();
        guard.images.insert(
            image_id.to_string(),
            Image {
                image_id: Some(image_id.to_string()),
                kernel_id: Some(kernel.to_string()),
                ramdisk_id: Some(ramdisk.to_string()),
                ..Default::default()
            },
        );
    }
}

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One response, kept as text so each assertion parses only what it needs.
struct Answer {
    status: u16,
    body: String,
}

impl Answer {
    /// The `<Code>` of the ec2Query error envelope.
    fn error_code(&self) -> String {
        ec2_query::error_code(&self.body)
    }

    /// The envelope's success flag.
    fn success_flag(&self) -> Option<String> {
        ec2_query::element(&self.body, "return")
    }
}

/// The three instance attributes this suite reads back off `DescribeInstances`, as the
/// response reported them — `None` for a member the response left out.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Attributes {
    kernel_id: Option<String>,
    ramdisk_id: Option<String>,
    source_dest_check: Option<String>,
    state: Option<String>,
}

/// Describe `instance_id` and read its attributes off the one instance reported for it.
async fn describe(case: &str, ec2: &Ec2, instance_id: &str) -> Attributes {
    let answer = ec2
        .call("DescribeInstances", vec![p("InstanceId.1", instance_id)])
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: DescribeInstances for {instance_id} must succeed, got {} with body {}",
        answer.status, answer.body
    );
    let parsed = document(case, &answer.body);
    let instance = parsed
        .descendants()
        .find(|node| {
            node.is_element()
                && child_text(*node, "instanceId").as_deref() == Some(instance_id)
                && child(*node, "instanceState").is_some()
        })
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: no instance {instance_id} in {body}",
                body = answer.body
            )
        });
    Attributes {
        kernel_id: child_text(instance, "kernelId"),
        ramdisk_id: child_text(instance, "ramdiskId"),
        source_dest_check: child_text(instance, "sourceDestCheck"),
        state: child(instance, "instanceState").and_then(|state| child_text(state, "name")),
    }
}

/// Launch one instance from `image`, naming `kernel` and `ramdisk` when the row carries
/// them, and answer its id.
async fn launch_from(
    case: &str,
    ec2: &Ec2,
    image: &str,
    kernel: Option<&str>,
    ramdisk: Option<&str>,
) -> String {
    let mut params = vec![
        p("ImageId", image),
        p("MinCount", "1"),
        p("MaxCount", "1"),
    ];
    if let Some(kernel) = kernel {
        params.push(p("KernelId", kernel));
    }
    if let Some(ramdisk) = ramdisk {
        params.push(p("RamdiskId", ramdisk));
    }
    let answer = ec2.call("RunInstances", params).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: RunInstances must succeed, got {} with body {}",
        answer.status, answer.body
    );
    ec2_query::element(&answer.body, "instanceId")
        .unwrap_or_else(|| panic!("case {case:?}: RunInstances reported no instance: {}", answer.body))
}

/// The state the clock owes an instance currently in `state`, or `state` itself when it
/// owes none. A transient state is only transient, so an assertion about a parked state
/// must admit the successor the clock may already have applied.
fn owed_next(state: &str) -> &str {
    match state {
        "pending" => "running",
        "stopping" => "stopped",
        "shutting-down" => "terminated",
        settled => settled,
    }
}

/// One instance parked in `state`, reached only through the public API, launched from
/// `image` with `kernel` and `ramdisk`.
async fn instance_from_image_in_state(
    case: &str,
    ec2: &Ec2,
    image: &str,
    state: &str,
    kernel: Option<&str>,
    ramdisk: Option<&str>,
) -> String {
    let instance_id = launch_from(case, ec2, image, kernel, ramdisk).await;
    // `pending` is the state a launch leaves the instance in, so that row is the only one
    // that does not first wait for the clock-owed move to `running`.
    if state != "pending" {
        past_the_transition().await;
    }
    match state {
        "pending" | "running" => {}
        "stopping" | "stopped" => {
            let stop = ec2
                .call("StopInstances", vec![p("InstanceId.1", &instance_id)])
                .await;
            assert_eq!(
                stop.status, 200,
                "case {case:?}: StopInstances must succeed, got {} with body {}",
                stop.status, stop.body
            );
            if state == "stopped" {
                past_the_transition().await;
            }
        }
        "terminated" => {
            let terminate = ec2
                .call("TerminateInstances", vec![p("InstanceId.1", &instance_id)])
                .await;
            assert_eq!(
                terminate.status, 200,
                "case {case:?}: TerminateInstances must succeed, got {} with body {}",
                terminate.status, terminate.body
            );
            past_the_transition().await;
        }
        other => panic!("case {case:?}: no way to park an instance in state {other:?}"),
    }
    let observed = describe(case, ec2, &instance_id).await;
    let seen = observed.state.clone().unwrap_or_default();
    assert!(
        seen == state || seen == owed_next(state),
        "case {case:?}: the instance was meant to be parked in {state} (or the {owed} the \
         clock owes it), got {observed:?}",
        owed = owed_next(state)
    );
    instance_id
}

/// One instance parked in `state`, launched from [`AMI`] with `kernel` and `ramdisk`.
async fn instance_in_state(
    case: &str,
    ec2: &Ec2,
    state: &str,
    kernel: Option<&str>,
    ramdisk: Option<&str>,
) -> String {
    instance_from_image_in_state(case, ec2, AMI, state, kernel, ramdisk).await
}

/// Reset `attribute` of `instance_id`, with the extra form fields `extra`.
async fn reset(
    ec2: &Ec2,
    instance_id: &str,
    attribute: &str,
    extra: Vec<(String, String)>,
) -> Answer {
    let mut params = vec![p("InstanceId", instance_id), p("Attribute", attribute)];
    params.extend(extra);
    ec2.call("ResetInstanceAttribute", params).await
}

/// The value the named attribute of an instance launched from [`AMI`] holds once it has
/// been reset: the documented default for `sourceDestCheck`, and the image's own kernel or
/// RAM disk — which this AMI does not carry — for the other two.
fn default_of(attribute: &str) -> Option<String> {
    match attribute {
        INSTANCE_ATTRIBUTE_SOURCE_DEST_CHECK => Some(DEFAULT_SOURCE_DEST_CHECK.to_string()),
        _ => None,
    }
}

/// The value of one named attribute, as `DescribeInstances` reported it.
fn reported(attributes: &Attributes, attribute: &str) -> Option<String> {
    match attribute {
        INSTANCE_ATTRIBUTE_KERNEL => attributes.kernel_id.clone(),
        INSTANCE_ATTRIBUTE_RAMDISK => attributes.ramdisk_id.clone(),
        INSTANCE_ATTRIBUTE_SOURCE_DEST_CHECK => attributes.source_dest_check.clone(),
        other => panic!("{other} is not an attribute this suite reads"),
    }
}

// --- tests ------------------------------------------------------------------

/// A reset restores exactly the attribute the request named to its default value, and
/// leaves every other attribute of the instance as it stands.
///
/// The rows differ along the attribute reset and along the values the launch gave the
/// instance, including a launch that named neither a kernel nor a RAM disk.
#[tokio::test]
async fn a_reset_restores_the_named_attribute_to_its_default() {
    // (case, attribute, launched kernel, launched RAM disk)
    let rows: [(&str, &str, Option<&str>, Option<&str>); 5] = [
        (
            "kernel, on an instance launched with a kernel and a RAM disk",
            INSTANCE_ATTRIBUTE_KERNEL,
            Some(KERNEL),
            Some(RAMDISK),
        ),
        (
            "ramdisk, on an instance launched with a kernel and a RAM disk",
            INSTANCE_ATTRIBUTE_RAMDISK,
            Some(KERNEL),
            Some(RAMDISK),
        ),
        (
            "kernel, on an instance launched with other ids",
            INSTANCE_ATTRIBUTE_KERNEL,
            Some(OTHER_KERNEL),
            Some(OTHER_RAMDISK),
        ),
        (
            "kernel, on an instance whose launch named neither",
            INSTANCE_ATTRIBUTE_KERNEL,
            None,
            None,
        ),
        (
            "sourceDestCheck, on an instance launched with a kernel",
            INSTANCE_ATTRIBUTE_SOURCE_DEST_CHECK,
            Some(KERNEL),
            None,
        ),
    ];

    for (case, attribute, kernel, ramdisk) in rows {
        let ec2 = Ec2::new();
        // `kernel` and `ramdisk` may be reset only from `stopped`, so every row is parked
        // there and the state table below is what varies the state.
        let instance_id = instance_in_state(case, &ec2, "stopped", kernel, ramdisk).await;

        let before = describe(case, &ec2, &instance_id).await;
        assert_eq!(
            before.kernel_id.as_deref(),
            kernel,
            "case {case:?}: the launch's kernel must be readable back, got {before:?}"
        );
        assert_eq!(
            before.ramdisk_id.as_deref(),
            ramdisk,
            "case {case:?}: the launch's RAM disk must be readable back, got {before:?}"
        );
        assert_eq!(
            before.source_dest_check.as_deref(),
            Some(DEFAULT_SOURCE_DEST_CHECK.to_string().as_str()),
            "case {case:?}: a launch leaves source/destination checking at its default, \
             got {before:?}"
        );

        let answer = reset(&ec2, &instance_id, attribute, Vec::new()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.success_flag().as_deref(),
            Some("true"),
            "case {case:?}: expected the envelope's success flag, got {}",
            answer.body
        );

        let after = describe(case, &ec2, &instance_id).await;
        assert_eq!(
            reported(&after, attribute),
            default_of(attribute),
            "case {case:?}: expected {attribute} back at its default {:?}, got {after:?}",
            default_of(attribute)
        );
        for untouched in RESETTABLE_INSTANCE_ATTRIBUTES
            .iter()
            .filter(|name| **name != attribute)
        {
            assert_eq!(
                reported(&after, untouched),
                reported(&before, untouched),
                "case {case:?}: resetting {attribute} must leave {untouched} alone, \
                 got {after:?} after {before:?}"
            );
        }
    }
}

/// Resetting an attribute twice answers the same thing twice and leaves the attribute at
/// its default: the operation is idempotent.
#[tokio::test]
async fn resetting_an_attribute_twice_is_idempotent() {
    let ec2 = Ec2::new();
    for attribute in RESETTABLE_INSTANCE_ATTRIBUTES {
        let case = format!("{attribute}, reset twice");
        let instance_id =
            instance_in_state(&case, &ec2, "stopped", Some(KERNEL), Some(RAMDISK)).await;
        for round in 1..=2 {
            let answer = reset(&ec2, &instance_id, attribute, Vec::new()).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: round {round} expected HTTP 200, got {} with body {}",
                answer.status, answer.body
            );
            let after = describe(&case, &ec2, &instance_id).await;
            assert_eq!(
                reported(&after, attribute),
                default_of(attribute),
                "case {case:?}: round {round} expected {attribute} at its default {:?}, \
                 got {after:?}",
                default_of(attribute)
            );
        }
    }
}

/// Only the three attributes the operation documents may be reset: "You can only reset
/// the following attributes: kernel | ramdisk | sourceDestCheck". The table is the WHOLE
/// `InstanceAttributeName` catalog the model declares
/// (`crate::ops::types::INSTANCE_ATTRIBUTE_NAME_VALUES`), so every member the model adds
/// is covered, and a refused request leaves the instance untouched.
#[tokio::test]
async fn only_the_documented_attributes_may_be_reset() {
    let ec2 = Ec2::new();
    for attribute in INSTANCE_ATTRIBUTE_NAME_VALUES {
        let case = format!("Attribute={attribute}");
        let instance_id =
            instance_in_state(&case, &ec2, "stopped", Some(KERNEL), Some(RAMDISK)).await;
        let before = describe(&case, &ec2, &instance_id).await;
        let answer = reset(&ec2, &instance_id, attribute, Vec::new()).await;
        if RESETTABLE_INSTANCE_ATTRIBUTES.contains(attribute) {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: a resettable attribute must be accepted, got {} with body {}",
                answer.status, answer.body
            );
            continue;
        }
        assert_eq!(
            answer.status, 400,
            "case {case:?}: an attribute outside the resettable set must be refused, \
             got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {}",
            answer.body
        );
        let after = describe(&case, &ec2, &instance_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused reset must change nothing, got {after:?} after {before:?}"
        );
    }
}

/// The state the instance must be in for the reset to be admitted: "To reset the kernel or
/// ramdisk, the instance must be in a stopped state. To reset the sourceDestCheck, the
/// instance can be either running or stopped." A state that admits neither is refused
/// with `IncorrectInstanceState`, and the attribute keeps the value it had.
///
/// `pending` and `stopping` are on their way to exactly `running` and `stopped`, so the
/// handler admits them for `sourceDestCheck`; every row's expected status is the same for
/// the transient state and for the successor the clock owes it, so no row depends on
/// catching an instance mid-transition.
#[tokio::test]
async fn the_instance_state_decides_whether_a_reset_is_admitted() {
    // (case, attribute, instance state, expected HTTP status)
    let rows: [(&str, &str, &str, u16); 11] = [
        ("kernel of a stopped instance", INSTANCE_ATTRIBUTE_KERNEL, "stopped", 200),
        ("kernel of a running instance", INSTANCE_ATTRIBUTE_KERNEL, "running", 400),
        ("kernel of a pending instance", INSTANCE_ATTRIBUTE_KERNEL, "pending", 400),
        ("kernel of a terminated instance", INSTANCE_ATTRIBUTE_KERNEL, "terminated", 400),
        ("ramdisk of a stopped instance", INSTANCE_ATTRIBUTE_RAMDISK, "stopped", 200),
        ("ramdisk of a running instance", INSTANCE_ATTRIBUTE_RAMDISK, "running", 400),
        (
            "sourceDestCheck of a running instance",
            INSTANCE_ATTRIBUTE_SOURCE_DEST_CHECK,
            "running",
            200,
        ),
        (
            "sourceDestCheck of a pending instance",
            INSTANCE_ATTRIBUTE_SOURCE_DEST_CHECK,
            "pending",
            200,
        ),
        (
            "sourceDestCheck of a stopping instance",
            INSTANCE_ATTRIBUTE_SOURCE_DEST_CHECK,
            "stopping",
            200,
        ),
        (
            "sourceDestCheck of a stopped instance",
            INSTANCE_ATTRIBUTE_SOURCE_DEST_CHECK,
            "stopped",
            200,
        ),
        (
            "sourceDestCheck of a terminated instance",
            INSTANCE_ATTRIBUTE_SOURCE_DEST_CHECK,
            "terminated",
            400,
        ),
    ];

    for (case, attribute, state, expected) in rows {
        let ec2 = Ec2::new();
        let instance_id =
            instance_in_state(case, &ec2, state, Some(KERNEL), Some(RAMDISK)).await;
        let before = describe(case, &ec2, &instance_id).await;
        let answer = reset(&ec2, &instance_id, attribute, Vec::new()).await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} with body {}",
            answer.status, answer.body
        );
        if expected == 200 {
            continue;
        }
        assert_eq!(
            answer.error_code(),
            "IncorrectInstanceState",
            "case {case:?}: expected IncorrectInstanceState, got {}",
            answer.body
        );
        let after = describe(case, &ec2, &instance_id).await;
        assert_eq!(
            reported(&after, attribute),
            reported(&before, attribute),
            "case {case:?}: a refused reset must leave {attribute} alone, got {after:?} \
             after {before:?}"
        );
    }
}

/// An instance id this service cannot resolve is refused: malformed before unknown, each
/// with the code EC2 answers it with. The rows span the forms a bad id takes — not an id
/// at all, a short legacy id, the null long id, and a well-formed id the account holds no
/// instance for.
#[tokio::test]
async fn an_instance_id_this_account_cannot_resolve_is_refused() {
    let ec2 = Ec2::new();
    // Launched so the store is not empty: the refusal must come from the id in the
    // request, not from an account with nothing in it.
    let _launched = instance_in_state("seed", &ec2, "stopped", Some(KERNEL), None).await;

    // (case, instance id, expected <Code>)
    let rows: [(&str, &str, &str); 5] = [
        ("not an id at all", "not-an-instance", "InvalidInstanceID.Malformed"),
        ("a short legacy id", "i-12345678", "InvalidInstanceID.Malformed"),
        (
            "the null long id",
            "i-00000000000000000",
            "InvalidInstanceID.Malformed",
        ),
        (
            "a well-formed id nothing was launched under",
            "i-0fedcba9876543210",
            "InvalidInstanceID.NotFound",
        ),
        (
            "a well-formed id of another resource kind",
            "vol-0123456789abcdef0",
            "InvalidInstanceID.Malformed",
        ),
    ];

    for (case, instance_id, expected) in rows {
        for attribute in RESETTABLE_INSTANCE_ATTRIBUTES {
            let case = format!("{case}, Attribute={attribute}");
            let answer = reset(&ec2, instance_id, attribute, Vec::new()).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                expected,
                "case {case:?}: expected {expected}, got {}",
                answer.body
            );
        }
    }
}

/// The kernel and RAM disk ids a launch retains on every instance it mints are bounded by
/// the length of an EC2 image id: a value at that bound is accepted and reported back, one
/// character past it is refused with `InvalidParameterValue` and nothing is launched.
#[tokio::test]
async fn a_kernel_or_ramdisk_id_longer_than_an_ec2_image_id_is_refused() {
    let ec2 = Ec2::new();
    for (member, prefix) in [("KernelId", "aki-"), ("RamdiskId", "ari-")] {
        for length in [
            MAX_KERNEL_OR_RAMDISK_ID_CHARS,
            MAX_KERNEL_OR_RAMDISK_ID_CHARS + 1,
        ] {
            let case = format!("{member} of {length} characters");
            let id = format!("{prefix}{}", "a".repeat(length - prefix.len()));
            let answer = ec2
                .call(
                    "RunInstances",
                    vec![
                        p("ImageId", AMI),
                        p("MinCount", "1"),
                        p("MaxCount", "1"),
                        p(member, id.clone()),
                    ],
                )
                .await;
            if length > MAX_KERNEL_OR_RAMDISK_ID_CHARS {
                assert_eq!(
                    answer.status, 400,
                    "case {case:?}: expected HTTP 400, got {} with body {}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    "InvalidParameterValue",
                    "case {case:?}: expected InvalidParameterValue, got {}",
                    answer.body
                );
                continue;
            }
            assert_eq!(
                answer.status, 200,
                "case {case:?}: an id at the bound must be accepted, got {} with body {}",
                answer.status, answer.body
            );
            let instance_id = ec2_query::element(&answer.body, "instanceId").unwrap_or_else(|| {
                panic!("case {case:?}: RunInstances reported no instance: {}", answer.body)
            });
            let reported_back = describe(&case, &ec2, &instance_id).await;
            let stored = if member == "KernelId" {
                reported_back.kernel_id.clone()
            } else {
                reported_back.ramdisk_id.clone()
            };
            assert_eq!(
                stored.as_deref(),
                Some(id.as_str()),
                "case {case:?}: expected the launch's {member} back, got {reported_back:?}"
            );
        }
    }
}

/// Resetting `sourceDestCheck` turns source/destination checking back ON for an instance
/// whose stored attribute was OFF — the branch a launch cannot reach, since nothing in a
/// launch disables checking and `ModifyInstanceAttribute` is still a stub, so the store is
/// seeded. Rows: both states the operation documents for this attribute.
#[tokio::test]
async fn a_reset_turns_source_dest_checking_back_on() {
    for state in ["running", "stopped"] {
        let case = format!("a {state} instance whose source/destination check was off");
        let ec2 = Ec2::new();
        let instance_id = instance_in_state(&case, &ec2, state, Some(KERNEL), Some(RAMDISK)).await;
        ec2.set_source_dest_check(&instance_id, !DEFAULT_SOURCE_DEST_CHECK);

        let before = describe(&case, &ec2, &instance_id).await;
        assert_eq!(
            before.source_dest_check.as_deref(),
            Some((!DEFAULT_SOURCE_DEST_CHECK).to_string().as_str()),
            "case {case:?}: the seeded value must be what a reader reports, got {before:?}"
        );

        let answer = reset(
            &ec2,
            &instance_id,
            INSTANCE_ATTRIBUTE_SOURCE_DEST_CHECK,
            Vec::new(),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
            answer.status, answer.body
        );

        let after = describe(&case, &ec2, &instance_id).await;
        assert_eq!(
            after.source_dest_check.as_deref(),
            Some(DEFAULT_SOURCE_DEST_CHECK.to_string().as_str()),
            "case {case:?}: expected checking back on, got {after:?}"
        );
        assert_eq!(
            (after.kernel_id, after.ramdisk_id),
            (before.kernel_id.clone(), before.ramdisk_id.clone()),
            "case {case:?}: the kernel and RAM disk must be left alone, after {before:?}"
        );
    }
}

/// `kernel` and `ramdisk` go back to the ids the instance's own AMI carries, not to
/// nothing: an instance launched from an image that carries both, whose launch named
/// DIFFERENT ids, reports the IMAGE's id for the attribute that was reset — which also
/// tells "restores the image's value" apart from "clears the value".
#[tokio::test]
async fn a_reset_restores_the_kernel_and_ramdisk_the_image_carries() {
    // (case, attribute, the image id expected back, the id expected to stay)
    let rows: [(&str, &str); 2] = [
        ("kernel", INSTANCE_ATTRIBUTE_KERNEL),
        ("ramdisk", INSTANCE_ATTRIBUTE_RAMDISK),
    ];

    for (case, attribute) in rows {
        let case = format!("{case} of an instance whose image carries one");
        let ec2 = Ec2::new();
        ec2.store_image_with_kernel(AMI_WITH_KERNEL, IMAGE_KERNEL, IMAGE_RAMDISK);
        let instance_id = instance_from_image_in_state(
            &case,
            &ec2,
            AMI_WITH_KERNEL,
            "stopped",
            Some(KERNEL),
            Some(RAMDISK),
        )
        .await;

        let before = describe(&case, &ec2, &instance_id).await;
        assert_eq!(
            (before.kernel_id.as_deref(), before.ramdisk_id.as_deref()),
            (Some(KERNEL), Some(RAMDISK)),
            "case {case:?}: the launch's own ids must win over the image's, got {before:?}"
        );

        let answer = reset(&ec2, &instance_id, attribute, Vec::new()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
            answer.status, answer.body
        );

        let after = describe(&case, &ec2, &instance_id).await;
        let expected = if attribute == INSTANCE_ATTRIBUTE_KERNEL {
            (Some(IMAGE_KERNEL), Some(RAMDISK))
        } else {
            (Some(KERNEL), Some(IMAGE_RAMDISK))
        };
        assert_eq!(
            (after.kernel_id.as_deref(), after.ramdisk_id.as_deref()),
            expected,
            "case {case:?}: expected {expected:?} after resetting {attribute}, got {after:?}"
        );
    }
}

/// `DryRun` answers the permission question and writes nothing: a request that would have
/// succeeded is refused with `DryRunOperation` and leaves the attribute as it was, while
/// one that would have been refused is refused with ITS reason instead.
#[tokio::test]
async fn a_dry_run_is_refused_and_changes_nothing() {
    let ec2 = Ec2::new();
    let instance_id =
        instance_in_state("dry run", &ec2, "stopped", Some(KERNEL), Some(RAMDISK)).await;

    for attribute in RESETTABLE_INSTANCE_ATTRIBUTES {
        let case = format!("DryRun=true for {attribute}");
        let before = describe(&case, &ec2, &instance_id).await;
        let answer = reset(&ec2, &instance_id, attribute, vec![p("DryRun", "true")]).await;
        assert_eq!(
            answer.status, 412,
            "case {case:?}: expected the dry-run status, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {}",
            answer.body
        );
        let after = describe(&case, &ec2, &instance_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a dry run must change nothing, got {after:?} after {before:?}"
        );
    }

    // A dry run that would have been refused reports the refusal, not DryRunOperation.
    let unknown = reset(
        &ec2,
        "i-0fedcba9876543210",
        INSTANCE_ATTRIBUTE_KERNEL,
        vec![p("DryRun", "true")],
    )
    .await;
    assert_eq!(
        unknown.error_code(),
        "InvalidInstanceID.NotFound",
        "case \"DryRun on an unknown instance\": expected the id refusal, got {}",
        unknown.body
    );
    let unsupported = reset(
        &ec2,
        &instance_id,
        "instanceType",
        vec![p("DryRun", "true")],
    )
    .await;
    assert_eq!(
        unsupported.error_code(),
        "InvalidParameterValue",
        "case \"DryRun for an attribute that cannot be reset\": expected the attribute \
         refusal, got {}",
        unsupported.body
    );
}
