// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ConfirmProductInstance`, driven over the wire the service
//! actually speaks — `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. for a product code the calling account OWNS, the operation reports whether that
//!    code is ATTACHED to the instance the request names — "returns true if the specified
//!    product code is owned by the requester and associated with the specified instance"
//!    (`ConfirmProductInstanceResult$Return`) — with `OwnerId`, "the Amazon Web Services
//!    account ID of the instance owner", present "only … if the product code is attached
//!    to the instance". A code reaches an instance through the AMI it was launched from
//!    ("the product code associated with the AMI used to launch the instance",
//!    `DescribeInstancesRequest$Filters`), so the answer is the image's own
//!    `Image$ProductCodes` — which is what `DescribeInstances` reports back as
//!    `Instance$ProductCodes`;
//! 2. a product code the account owns on NO stored resource is refused `AuthFailure`
//!    ("this action can only be used by the owner of the product code"), and that refusal
//!    comes BEFORE the instance id is resolved, so it is the answer whether the id names
//!    an instance, names none, or is not an instance id at all — and whether or not the
//!    code is attached to that instance through ANOTHER account's image;
//! 3. with an owned code, an instance id outside the form EC2 issues is
//!    `InvalidInstanceID.Malformed` and an id inside that form naming no instance of the
//!    account is `InvalidInstanceID.NotFound`;
//! 4. `DryRun` answers whether the call WOULD have succeeded — `DryRunOperation` for a
//!    request that would have been served, and the refusal itself for one that would have
//!    been refused;
//! 5. the operation is a READ: two calls about one instance answer the same thing and
//!    neither moves the instance.
//!
//! What EC2's envelope puts AROUND those answers — the `<RequestID>` a refusal closes it
//! with, the `<return>` flag and the media type — is one rule for the whole service and is
//! asserted once, over every operation, in [`super::ec2_query_envelope`]; it is not
//! restated here.
//!
//! Each table row is a distinct input along what the rule generalises over — which image
//! the instance was launched from (one code, two codes, none, another account's), which
//! of the account's codes is asked about, the `ProductCodeValues` type of that code
//! (`devpay` and `marketplace`, the two the model declares), the length of the code, and
//! for the id rule every way an id can fall outside the form — so no row is the
//! specification.

use std::time::Duration;

use ec2::ops::types::{PRODUCT_CODE_VALUES_VALUES, ProductCode};
use ec2::state::{ComAmazonawsEc2Amazonec2State as Ec2State, ImageSpec, TRANSITION_DELAY};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, error_code, member};

/// The Region every call here is signed for; it only selects the ec2Query envelope, since
/// a product code carries no Region member.
const REGION: &str = "us-west-2";

/// `ProductCodeValues`' `marketplace` member — the first entry of the model's own catalog
/// (`PRODUCT_CODE_VALUES_VALUES`), asserted against it below so a model change cannot
/// leave this suite testing a word the service no longer accepts.
const MARKETPLACE: &str = "marketplace";

/// `ProductCodeValues`' `devpay` member.
const DEVPAY: &str = "devpay";

/// A Marketplace code of the calling account's own paid AMI: 25 alphanumeric characters,
/// the shape AWS Marketplace issues.
const SOLD_CODE: &str = "a1b2c3d4e5f6g7h8i9j0k1l2m";

/// A DevPay code of a second paid AMI of the account, so the suite spans both
/// `ProductCodeValues` types.
const DEVPAY_CODE: &str = "zz9988776655443322110aabb";

/// A second code on that same AMI, shorter than the others: the operation's answer turns
/// on the code's VALUE, never on its length, and an image may carry more than one.
const EXTRA_CODE: &str = "9xk2code01";

/// A code carried only by ANOTHER account's image, so the calling account owns it for no
/// resource of its own.
const FOREIGN_CODE: &str = "f0reign0code0000000000zde";

/// A code no stored resource carries at all.
const UNSOLD_CODE: &str = "n0b0dysellsthiscode00001a";

/// The account the foreign image belongs to: any 12-digit account id that is not the one
/// the emulator serves (asserted in [`emulator`]).
const OTHER_ACCOUNT: &str = "210987654321";

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

/// One `Image$ProductCodes` entry.
fn code(product_code: &str, product_code_type: &str) -> ProductCode {
    ProductCode {
        product_code_id: Some(product_code.to_string()),
        product_code_type: Some(product_code_type.to_string()),
    }
}

/// An AMI as a register operation would store one: `name` for the image, `product_codes`
/// for "any product codes associated with the AMI" (`Image$ProductCodes`), and `owner_id`
/// for the account that published it.
fn image_spec(name: &str, owner_id: &str, product_codes: Vec<ProductCode>) -> ImageSpec {
    ImageSpec {
        name: name.to_string(),
        owner_id: owner_id.to_string(),
        product_codes,
        ..ImageSpec::default()
    }
}

/// The four images the rows below launch from.
struct Images {
    /// The account's own paid AMI, carrying one Marketplace code.
    sold: String,
    /// A second paid AMI of the account, carrying a DevPay code and one more code.
    devpay: String,
    /// An AMI of the account that nobody sells, so it carries no code.
    plain: String,
    /// Another account's paid AMI, carrying a code this account does not own.
    foreign: String,
}

/// One instance launched from each of those images.
struct Instances {
    sold: String,
    devpay: String,
    plain: String,
    foreign: String,
}

/// Write the four images into the engine's store, the way a register operation would.
fn seed(engine: &engine::InMemoryEngine) -> Images {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    Images {
        sold: guard.insert_image(image_spec(
            "confirm-product-sold",
            engine::emulator_account_id(),
            vec![code(SOLD_CODE, MARKETPLACE)],
        )),
        devpay: guard.insert_image(image_spec(
            "confirm-product-devpay",
            engine::emulator_account_id(),
            vec![code(DEVPAY_CODE, DEVPAY), code(EXTRA_CODE, MARKETPLACE)],
        )),
        plain: guard.insert_image(image_spec(
            "confirm-product-plain",
            engine::emulator_account_id(),
            Vec::new(),
        )),
        foreign: guard.insert_image(image_spec(
            "confirm-product-foreign",
            OTHER_ACCOUNT,
            vec![code(FOREIGN_CODE, MARKETPLACE)],
        )),
    }
}

/// One emulator over its own engine, with the four images seeded.
fn emulator() -> (MultiServiceRouter, Images) {
    // The two words this suite names are the two the model declares.
    assert_eq!(
        PRODUCT_CODE_VALUES_VALUES,
        [DEVPAY, MARKETPLACE],
        "the ProductCodeValues catalog this suite tests against must be the model's"
    );
    assert_ne!(
        OTHER_ACCOUNT,
        engine::emulator_account_id(),
        "the foreign image's owner must not be the account the emulator serves"
    );
    let (router, engine) = super::ec2_query::emulator();
    let images = seed(&engine);
    (router, images)
}

/// Launch one instance from `image_id` and answer its id.
async fn launch(router: &MultiServiceRouter, image_id: &str) -> String {
    let (status, body) = api(
        router,
        "RunInstances",
        &[
            p("ImageId", image_id),
            p("MinCount", "1"),
            p("MaxCount", "1"),
        ],
    )
    .await;
    assert_eq!(status, 200, "RunInstances must succeed, got {status}: {body}");
    super::ec2_query::element(&body, "instanceId")
        .unwrap_or_else(|| panic!("RunInstances must report the instance it minted: {body}"))
}

/// One emulator with one instance launched from each seeded image.
async fn emulator_with_instances() -> (MultiServiceRouter, Instances) {
    let (router, images) = emulator();
    let instances = Instances {
        sold: launch(&router, &images.sold).await,
        devpay: launch(&router, &images.devpay).await,
        plain: launch(&router, &images.plain).await,
        foreign: launch(&router, &images.foreign).await,
    };
    (router, instances)
}

/// One `ConfirmProductInstance` call, with `DryRun` set only when asked for.
async fn confirm(
    router: &MultiServiceRouter,
    instance_id: &str,
    product_code: &str,
    dry_run: bool,
) -> (u16, String) {
    let mut params = vec![p("InstanceId", instance_id), p("ProductCode", product_code)];
    if dry_run {
        params.push(p("DryRun", "true"));
    }
    api(router, "ConfirmProductInstance", &params).await
}

/// The instance ids `DescribeInstances` reports for one filter, sorted so a comparison is
/// about WHICH instances matched, not about the order the store walks them in.
async fn instances_matching(
    router: &MultiServiceRouter,
    name: &str,
    value: &str,
) -> Vec<String> {
    let (status, body) = api(
        router,
        "DescribeInstances",
        &super::ec2_query::filters(&[(name, &[value])]),
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeInstances filtered on {name}={value} must succeed, got {status}: {body}"
    );
    let mut ids = super::ec2_query::elements(&body, "instanceId");
    ids.sort();
    ids
}

/// The `(product code, type)` pairs `DescribeInstances` reports for one instance, in
/// document order — the read-back of `Instance$ProductCodes`.
async fn described_product_codes(
    router: &MultiServiceRouter,
    instance_id: &str,
) -> Vec<(String, String)> {
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
    let child = |node: roxmltree::Node<'_, '_>, name: &str| -> String {
        node.children()
            .find(|candidate| candidate.is_element() && candidate.tag_name().name() == name)
            .and_then(|candidate| candidate.text())
            .unwrap_or_default()
            .to_string()
    };
    document
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "productCodes")
        .flat_map(|codes| codes.children().filter(|item| item.is_element()))
        .map(|item| (child(item, "productCode"), child(item, "type")))
        .collect()
}

/// The `instanceState` `DescribeInstances` reports for one instance, as the "nothing
/// moved" assertions compare it — the shared harness's reader, for this suite's Region.
async fn described_state(router: &MultiServiceRouter, instance_id: &str) -> String {
    super::ec2_query::described_instance_state(router, REGION, instance_id).await
}

/// One confirmation of an owned code against one instance.
#[derive(Debug, Clone, Copy)]
struct Case {
    label: &'static str,
    /// Which seeded image's instance the request names.
    instance: &'static str,
    /// The product code asked about — one the calling account owns in every row here.
    code: &'static str,
    /// Whether that code is attached to that instance, through the AMI it was launched
    /// from.
    attached: bool,
}

/// The instance id for a case's `instance` word.
fn instance_of(instances: &Instances, which: &str) -> String {
    match which {
        "sold" => instances.sold.clone(),
        "devpay" => instances.devpay.clone(),
        "plain" => instances.plain.clone(),
        "foreign" => instances.foreign.clone(),
        other => panic!("no instance was launched for {other:?}"),
    }
}

/// The text of a failure's `<Message>`, with the XML entities the service escaped it with
/// resolved.
fn error_message(body: &str) -> String {
    super::ec2_query::unescaped(
        &super::ec2_query::element(body, "Message")
            .unwrap_or_else(|| panic!("a failure body must carry a <Message>: {body}")),
    )
}

/// Rule 1: for a code the account owns, the answer is whether that code is attached to
/// the instance, and the instance owner is reported only when it is.
#[tokio::test]
async fn an_owned_product_code_is_confirmed_against_the_instances_image() {
    let cases = [
        Case {
            label: "the Marketplace code of the AMI the instance was launched from",
            instance: "sold",
            code: SOLD_CODE,
            attached: true,
        },
        Case {
            label: "the DevPay code of the AMI the instance was launched from",
            instance: "devpay",
            code: DEVPAY_CODE,
            attached: true,
        },
        Case {
            label: "the second, shorter code of that same AMI",
            instance: "devpay",
            code: EXTRA_CODE,
            attached: true,
        },
        Case {
            label: "an owned code the instance's AMI does not carry",
            instance: "sold",
            code: DEVPAY_CODE,
            attached: false,
        },
        Case {
            label: "an owned code against an instance of an AMI nobody sells",
            instance: "plain",
            code: SOLD_CODE,
            attached: false,
        },
        Case {
            label: "an owned code against an instance of another account's AMI",
            instance: "foreign",
            code: SOLD_CODE,
            attached: false,
        },
        Case {
            label: "the other owned code against that same instance",
            instance: "devpay",
            code: SOLD_CODE,
            attached: false,
        },
    ];

    let (router, instances) = emulator_with_instances().await;
    for case in cases {
        let instance_id = instance_of(&instances, case.instance);
        let (status, body) = confirm(&router, &instance_id, case.code, false).await;
        assert_eq!(
            status, 200,
            "case {:?}: ConfirmProductInstance must succeed, got {status}: {body}",
            case.label
        );
        assert_eq!(
            member(&body, "return").as_deref(),
            Some(if case.attached { "true" } else { "false" }),
            "case {:?}: expected return {}, got {body}",
            case.label,
            case.attached
        );
        assert_eq!(
            member(&body, "ownerId"),
            case.attached.then(|| engine::emulator_account_id().to_string()),
            "case {:?}: the instance owner is reported only for an attached code, got \
             {body}",
            case.label
        );
    }

    // Rule 1, read back the other way: the codes the operation confirms are exactly the
    // ones the instance reports as `Instance$ProductCodes`.
    assert_eq!(
        described_product_codes(&router, &instances.sold).await,
        vec![(SOLD_CODE.to_string(), MARKETPLACE.to_string())],
        "the instance must report the product codes of the AMI it was launched from"
    );
    assert_eq!(
        described_product_codes(&router, &instances.devpay).await,
        vec![
            (DEVPAY_CODE.to_string(), DEVPAY.to_string()),
            (EXTRA_CODE.to_string(), MARKETPLACE.to_string()),
        ],
        "an instance of an AMI carrying two codes must report both, with their types"
    );
    assert_eq!(
        described_product_codes(&router, &instances.plain).await,
        Vec::new(),
        "an instance of an AMI nobody sells must report no product code"
    );
}

/// Rule 1, selecting instead of reporting: the `product-code` and `product-code.type`
/// `DescribeInstances` filters read the SAME projection, so a filter selects exactly the
/// instances whose AMI carries that code — or that type of code — and nothing matches a
/// code no stored image carries.
#[tokio::test]
async fn the_product_code_filters_select_on_the_same_projection() {
    let (router, instances) = emulator_with_instances().await;

    // (the filter, its value, the instances that must come back)
    let cases: [(&str, &str, Vec<String>); 8] = [
        (
            "product-code",
            SOLD_CODE,
            vec![instances.sold.clone()],
        ),
        (
            "product-code",
            DEVPAY_CODE,
            vec![instances.devpay.clone()],
        ),
        (
            "product-code",
            EXTRA_CODE,
            vec![instances.devpay.clone()],
        ),
        (
            "product-code",
            FOREIGN_CODE,
            vec![instances.foreign.clone()],
        ),
        ("product-code", UNSOLD_CODE, Vec::new()),
        (
            "product-code.type",
            MARKETPLACE,
            vec![
                instances.sold.clone(),
                instances.devpay.clone(),
                instances.foreign.clone(),
            ],
        ),
        (
            "product-code.type",
            DEVPAY,
            vec![instances.devpay.clone()],
        ),
        ("product-code.type", "no-such-type", Vec::new()),
    ];

    for (name, value, expected) in cases {
        let mut expected = expected;
        expected.sort();
        assert_eq!(
            instances_matching(&router, name, value).await,
            expected,
            "case {name}={value}: expected exactly {expected:?} to match"
        );
    }

    // The instance of the AMI nobody sells matches no product-code filter at all, and is
    // still reported when nothing filters on a product code.
    let (status, body) = api(&router, "DescribeInstances", &[]).await;
    assert_eq!(status, 200, "DescribeInstances must succeed, got {status}: {body}");
    assert!(
        super::ec2_query::elements(&body, "instanceId").contains(&instances.plain),
        "case unfiltered: the instance of an AMI with no product code must still be \
         reported, got {body}"
    );
}

/// Rule 2: a code the account owns on no resource of its own is `AuthFailure`, answered
/// before the instance id is looked at.
#[tokio::test]
async fn a_product_code_the_account_does_not_own_is_refused_before_the_instance() {
    let (router, instances) = emulator_with_instances().await;

    // (what the request names, the instance id, the code, whether DryRun is set)
    let cases: [(&str, String, &str, bool); 7] = [
        (
            "a code carried only by another account's AMI, against an instance of that \
             very AMI",
            instances.foreign.clone(),
            FOREIGN_CODE,
            false,
        ),
        (
            "that same foreign code against an instance of the account's own AMI",
            instances.sold.clone(),
            FOREIGN_CODE,
            false,
        ),
        (
            "a code no stored resource carries, against an instance the account holds",
            instances.sold.clone(),
            UNSOLD_CODE,
            false,
        ),
        (
            "an unowned code against an id inside the form that names nothing",
            "i-00000000000000999".to_string(),
            UNSOLD_CODE,
            false,
        ),
        (
            "an unowned code against an id outside the form EC2 issues",
            "i-1234567890abcdef0".to_string(),
            UNSOLD_CODE,
            false,
        ),
        (
            "an unowned code against something that is not an instance id at all",
            "instance-00000000000000001".to_string(),
            FOREIGN_CODE,
            false,
        ),
        (
            "an unowned code under DryRun",
            instances.plain.clone(),
            UNSOLD_CODE,
            true,
        ),
    ];

    for (label, instance_id, product_code, dry_run) in cases {
        let (status, body) = confirm(&router, &instance_id, product_code, dry_run).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "AuthFailure"),
            "case {label:?} ({instance_id}, {product_code}): expected 400 AuthFailure, \
             got {status}: {body}"
        );
        // The refusal names the code the REQUEST asked about, so a caller confirming
        // several codes learns which one of them it does not own.
        assert_eq!(
            error_message(&body),
            format!("Not authorised for product code {product_code}"),
            "case {label:?} ({instance_id}): expected the refusal to name the product \
             code the request carried, got {body}"
        );
    }

    // The code the account does own still resolves: the refusals above are about
    // ownership, not about the operation.
    let (status, body) = confirm(&router, &instances.sold, SOLD_CODE, false).await;
    assert_eq!(
        (status, member(&body, "return").as_deref()),
        (200, Some("true")),
        "an owned code must still be confirmed, got {status}: {body}"
    );
}

/// Rule 3: with an owned code, an id outside the form EC2 issues is malformed, before any
/// lookup; an id inside it that names nothing is not found.
#[tokio::test]
async fn an_unresolvable_instance_id_is_refused_when_the_code_is_owned() {
    let (router, instances) = emulator_with_instances().await;

    // (what is wrong with the id, the id, the code EC2 answers)
    let cases: [(&str, &str, &str); 11] = [
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

    for (label, instance_id, expected_code) in cases {
        let (status, body) = confirm(&router, instance_id, SOLD_CODE, false).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, expected_code),
            "case {label:?} ({instance_id}): expected 400 {expected_code}, got {status}: \
             {body}"
        );
    }

    // The id that does resolve still resolves.
    let (status, body) = confirm(&router, &instances.devpay, DEVPAY_CODE, false).await;
    assert_eq!(
        (status, member(&body, "return").as_deref()),
        (200, Some("true")),
        "the launched instance must still be confirmable, got {status}: {body}"
    );
}

/// Rule 4: `DryRun` says whether the call would have succeeded — and a request that would
/// have been refused is refused with that reason instead.
#[tokio::test]
async fn dry_run_reports_whether_the_call_would_have_succeeded() {
    let (router, instances) = emulator_with_instances().await;
    past_the_transition().await;

    // (what the dry run names, the id, the code, the status and code EC2 answers)
    let cases: [(&str, String, &str, u16, &str); 4] = [
        (
            "an owned code attached to an instance the account holds",
            instances.sold.clone(),
            SOLD_CODE,
            412,
            "DryRunOperation",
        ),
        (
            "an owned code that is not attached to it",
            instances.plain.clone(),
            SOLD_CODE,
            412,
            "DryRunOperation",
        ),
        (
            "an owned code against an id inside the form that names nothing",
            "i-00000000000000999".to_string(),
            SOLD_CODE,
            400,
            "InvalidInstanceID.NotFound",
        ),
        (
            "an owned code against an id outside the form EC2 issues",
            "i-1234567890abcdef0".to_string(),
            SOLD_CODE,
            400,
            "InvalidInstanceID.Malformed",
        ),
    ];

    let before = described_state(&router, &instances.sold).await;
    for (label, instance_id, product_code, expected_status, expected_code) in cases {
        let (status, body) = confirm(&router, &instance_id, product_code, true).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (expected_status, expected_code),
            "case {label:?} ({instance_id}): expected {expected_status} \
             {expected_code}, got {status}: {body}"
        );
    }
    let after = described_state(&router, &instances.sold).await;
    assert_eq!(
        after, before,
        "a dry run must leave the instance exactly as it was"
    );

    // And the same call without the flag is served.
    let (status, body) = confirm(&router, &instances.sold, SOLD_CODE, false).await;
    assert_eq!(
        (status, member(&body, "return").as_deref()),
        (200, Some("true")),
        "the request the dry run stood for must be served, got {status}: {body}"
    );
}

/// Rule 5: the confirmation is a read — it answers the same thing twice, and moves
/// nothing.
#[tokio::test]
async fn confirming_a_product_code_changes_nothing() {
    let (router, instances) = emulator_with_instances().await;
    past_the_transition().await;

    // (which instance, which code)
    let cases: [(&str, &str); 3] = [
        ("sold", SOLD_CODE),
        ("devpay", EXTRA_CODE),
        ("plain", DEVPAY_CODE),
    ];

    for (which, product_code) in cases {
        let instance_id = instance_of(&instances, which);
        let state_before = described_state(&router, &instance_id).await;
        let codes_before = described_product_codes(&router, &instance_id).await;
        let (first_status, first_body) = confirm(&router, &instance_id, product_code, false).await;
        let (second_status, second_body) = confirm(&router, &instance_id, product_code, false).await;
        assert_eq!(
            (second_status, member(&second_body, "return")),
            (first_status, member(&first_body, "return")),
            "case {which:?} ({product_code}): two confirmations must answer the same \
             thing, got {second_status}: {second_body} after {first_status}: {first_body}"
        );
        assert_eq!(
            described_state(&router, &instance_id).await,
            state_before,
            "case {which:?} ({product_code}): confirming a product code must not move \
             the instance"
        );
        assert_eq!(
            described_product_codes(&router, &instance_id).await,
            codes_before,
            "case {which:?} ({product_code}): confirming a product code must not change \
             the codes the instance carries"
        );
    }
}
