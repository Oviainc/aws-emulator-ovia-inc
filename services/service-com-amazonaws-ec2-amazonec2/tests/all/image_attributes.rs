// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeImageAttribute`, driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=DescribeImageAttribute`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol").
//!
//! The rule under test is that the operation REPORTS ONE ATTRIBUTE OF THE STORED IMAGE: the
//! response carries the single member `Attribute` names, read off the stored `Image` member
//! AWS documents that attribute as, beside the image's own id — and nothing else. So the
//! tables below span the `ImageAttributeName` catalog rather than one attribute, and each
//! attribute is asked of images that differ along what it reads: an image that carries a
//! value for it and one that carries none, a private image and a public one, the account's
//! own image and another account's.
//!
//! Three suites:
//!
//! 1. the catalog table — every attribute the model publishes, on three images, with the
//!    value the store holds for it;
//! 2. the agreement table — the value `DescribeImageAttribute` reports for an attribute is
//!    the value `DescribeImages` reports for the member of the same image, so an attribute
//!    read cannot drift from an image read;
//! 3. the refusal table — the ids and the attributes it refuses, and the `DryRun` probe.
//!
//! The images are seeded through the store's own writer
//! (`ComAmazonawsEc2Amazonec2State::insert_image`, what a register operation calls), because
//! `RegisterImage` and `CreateImage` are still scaffolds with their own stub issues; the few
//! members no `ImageSpec` carries (the kernel, the RAM disk, the NitroTPM support and the
//! last launched time) are written onto the stored image afterwards, which is what a
//! register of a paravirtual AMI would have stored.

use ec2::ops::types::ProductCode;
use ec2::state::{
    BOOT_MODE_UEFI, ComAmazonawsEc2Amazonec2State as Ec2State, EXECUTABLE_BY_ALL,
    IMAGE_DEREGISTRATION_PROTECTION_DISABLED, IMAGE_SRIOV_NET_SUPPORT_SIMPLE, IMDS_SUPPORT_V2,
    ImageSpec,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, error_code, error_message, unescaped};

/// The Region every call here is signed for.
const REGION: &str = "eu-west-1";

/// An account that is not the one this emulator serves, so an image of its own is another
/// account's.
const PARTNER_ACCOUNT: &str = "210987654321";

/// "The name of the group" a public image's launch permission names
/// (`LaunchPermission$Group`): the `PermissionGroup` enum's only member (the generated
/// catalog `ec2::ops::types::PERMISSION_GROUP_VALUES`), which is the same term
/// `DescribeImages` selects public AMIs with (`ec2::state::EXECUTABLE_BY_ALL`).
const LAUNCH_PERMISSION_GROUP_ALL: &str = EXECUTABLE_BY_ALL;

/// The kernel the paravirtual image carries (`Image$KernelId`): an `aki-` id in the long
/// form every EC2 id of this store is minted in.
const KERNEL_ID: &str = "aki-0a1b2c3d4e5f60001";

/// The RAM disk that image carries (`Image$RamdiskId`).
const RAMDISK_ID: &str = "ari-0a1b2c3d4e5f60002";

/// "If the image is configured for NitroTPM support, the value is `v2.0`"
/// (`Image$TpmSupport`).
const TPM_SUPPORT_V2: &str = "v2.0";

/// "The date and time, in ISO 8601 date-time format, when the AMI was last used to launch an
/// EC2 instance" (`Image$LastLaunchedTime`), for the one seeded image whose usage has been
/// reported.
const LAST_LAUNCHED_TIME: &str = "2026-01-01T00:00:00.000Z";

/// The Marketplace code the paid image carries, with its type — one of
/// `PRODUCT_CODE_VALUES_VALUES` (`crate::ops::types`), the `ProductCodeValues` catalog.
const PRODUCT_CODE: &str = "aw0evgkw8e5c1q413zjy5pjce";

/// That code's type.
const PRODUCT_CODE_TYPE: &str = "marketplace";

/// The description of the account's own private image.
const PRIVATE_DESCRIPTION: &str = "The account's own paravirtual golden image";

/// The description of the account's own public image.
const PUBLIC_DESCRIPTION: &str = "The account's own image, shared with everyone";

/// One `(key, value)` request parameter.
fn p(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// One `DescribeImageAttribute` call for `image_id` and `attribute`.
async fn describe(
    router: &MultiServiceRouter,
    image_id: &str,
    attribute: &str,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut params = vec![p("ImageId", image_id), p("Attribute", attribute)];
    params.extend_from_slice(extra);
    call(router, REGION, "DescribeImageAttribute", &params).await
}

/// The images the tables below ask about.
struct Seeded {
    /// The account's own PRIVATE image: a paravirtual AMI with a kernel, a RAM disk,
    /// NitroTPM support, a boot mode, IMDSv2 support, a Marketplace product code, a
    /// description and a reported last launched time.
    private: String,
    /// The account's own PUBLIC image: no kernel, no RAM disk, no NitroTPM support, no boot
    /// mode, no IMDS support, no product code and no reported last launched time.
    public: String,
    /// Another account's PUBLIC image — one this account can see without owning it.
    partner_public: String,
    /// Another account's PRIVATE image — one this account cannot see at all.
    partner_private: String,
}

/// Write the four images into the engine's store, the way a register operation would.
fn seed(engine: &engine::InMemoryEngine) -> Seeded {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();

    let private = guard.insert_image(ImageSpec {
        name: "image-attribute-private".to_string(),
        description: PRIVATE_DESCRIPTION.to_string(),
        boot_mode: Some(BOOT_MODE_UEFI.to_string()),
        imds_support: Some(IMDS_SUPPORT_V2.to_string()),
        product_codes: vec![ProductCode {
            product_code_id: Some(PRODUCT_CODE.to_string()),
            product_code_type: Some(PRODUCT_CODE_TYPE.to_string()),
        }],
        ..ImageSpec::default()
    });
    // The members `ImageSpec` carries no field for, written the way a register of a
    // paravirtual AMI that has already been launched from would have stored them.
    if let Some(image) = guard.images.get_mut(&private) {
        image.kernel_id = Some(KERNEL_ID.to_string());
        image.ramdisk_id = Some(RAMDISK_ID.to_string());
        image.tpm_support = Some(TPM_SUPPORT_V2.to_string());
        image.last_launched_time = Some(LAST_LAUNCHED_TIME.to_string());
    }

    let public = guard.insert_image(ImageSpec {
        name: "image-attribute-public".to_string(),
        description: PUBLIC_DESCRIPTION.to_string(),
        public: true,
        ..ImageSpec::default()
    });
    let partner_public = guard.insert_image(ImageSpec {
        name: "image-attribute-partner-public".to_string(),
        description: "Another account's public image".to_string(),
        public: true,
        owner_id: PARTNER_ACCOUNT.to_string(),
        ..ImageSpec::default()
    });
    let partner_private = guard.insert_image(ImageSpec {
        name: "image-attribute-partner-private".to_string(),
        description: "Another account's private image".to_string(),
        owner_id: PARTNER_ACCOUNT.to_string(),
        ..ImageSpec::default()
    });

    Seeded {
        private,
        public,
        partner_public,
        partner_private,
    }
}

/// Every element of the response envelope other than the request id, as (element name, the
/// texts its leaf elements carry in document order).
///
/// Read with the XML parser rather than by substring, because the rule under test is about
/// WHICH members the envelope carries: a leaf element with no text reports an attribute the
/// image holds no value for, and that is a different answer from the element being absent.
fn reported(case: &str, body: &str) -> Vec<(String, Vec<String>)> {
    let document = roxmltree::Document::parse(body).unwrap_or_else(|error| {
        panic!("case {case:?}: the response body must be XML ({error}): {body}")
    });
    document
        .root_element()
        .children()
        .filter(|node| node.is_element() && node.tag_name().name() != "requestId")
        .map(|node| {
            let texts = node
                .descendants()
                .filter(|inner| {
                    inner.is_element() && !inner.children().any(|child| child.is_element())
                })
                .map(|leaf| leaf.text().unwrap_or_default().to_string())
                .collect();
            (node.tag_name().name().to_string(), texts)
        })
        .collect()
}

/// The texts the envelope's `name` element carries, or `None` when it carries no such
/// element at all.
fn attribute_texts(case: &str, body: &str, name: &str) -> Option<Vec<String>> {
    reported(case, body)
        .into_iter()
        .find(|(element, _)| element == name)
        .map(|(_, texts)| texts)
}

/// The element names the envelope carries, the image id apart — which must be exactly the
/// one attribute the request asked about ("You can specify only one attribute at a time").
fn attribute_elements(case: &str, body: &str) -> Vec<String> {
    reported(case, body)
        .into_iter()
        .map(|(element, _)| element)
        .filter(|element| element != "imageId")
        .collect()
}

/// Rule 1: for every attribute of the `ImageAttributeName` catalog, the response carries
/// THAT member and no other, the image's own id, and the value the store holds for it.
#[tokio::test]
async fn reports_the_one_named_attribute_out_of_the_stored_image() {
    let (router, engine) = super::ec2_query::emulator();
    let images = seed(&engine);

    // Rows: (case, image, attribute, the element it is reported in, the texts that element
    // must carry). Every value the model publishes for `ImageAttributeName` appears, each on
    // an image that carries a value for it AND on one that carries none, so no row is the
    // specification. The element names are the model's own `xmlName`s
    // (`smithy_query(query="operation",operation="DescribeImageAttribute")`).
    let cases: Vec<(&str, &String, &str, &str, Vec<&str>)> = vec![
        (
            "the description of the account's own private image",
            &images.private,
            "description",
            "description",
            vec![PRIVATE_DESCRIPTION],
        ),
        (
            "the description of another account's public image",
            &images.partner_public,
            "description",
            "description",
            vec!["Another account's public image"],
        ),
        (
            "the kernel of a paravirtual image",
            &images.private,
            "kernel",
            "kernel",
            vec![KERNEL_ID],
        ),
        (
            "the kernel of an image that carries none",
            &images.public,
            "kernel",
            "kernel",
            vec![""],
        ),
        (
            "the RAM disk of a paravirtual image",
            &images.private,
            "ramdisk",
            "ramdisk",
            vec![RAMDISK_ID],
        ),
        (
            "the RAM disk of an image that carries none",
            &images.public,
            "ramdisk",
            "ramdisk",
            vec![""],
        ),
        (
            "the enhanced networking support every stored image carries",
            &images.private,
            "sriovNetSupport",
            "sriovNetSupport",
            vec![IMAGE_SRIOV_NET_SUPPORT_SIMPLE],
        ),
        (
            "the boot mode of an image that declares one",
            &images.private,
            "bootMode",
            "bootMode",
            vec![BOOT_MODE_UEFI],
        ),
        (
            "the boot mode of an image that declares none",
            &images.public,
            "bootMode",
            "bootMode",
            vec![""],
        ),
        (
            "the NitroTPM support of an image configured for it",
            &images.private,
            "tpmSupport",
            "tpmSupport",
            vec![TPM_SUPPORT_V2],
        ),
        (
            "the NitroTPM support of an image not configured for it",
            &images.public,
            "tpmSupport",
            "tpmSupport",
            vec![""],
        ),
        (
            "the IMDS support of an image that requires IMDSv2",
            &images.private,
            "imdsSupport",
            "imdsSupport",
            vec![IMDS_SUPPORT_V2],
        ),
        (
            "the IMDS support of an image that requires nothing",
            &images.public,
            "imdsSupport",
            "imdsSupport",
            vec![""],
        ),
        (
            "the last launched time of an image whose usage has been reported",
            &images.private,
            "lastLaunchedTime",
            "lastLaunchedTime",
            vec![LAST_LAUNCHED_TIME],
        ),
        (
            "the last launched time of an image nothing has launched",
            &images.public,
            "lastLaunchedTime",
            "lastLaunchedTime",
            vec![""],
        ),
        (
            "the deregistration protection every stored image carries",
            &images.public,
            "deregistrationProtection",
            "deregistrationProtection",
            vec![IMAGE_DEREGISTRATION_PROTECTION_DISABLED],
        ),
        (
            "the UEFI variable store no image of this emulator holds",
            &images.private,
            "uefiData",
            "uefiData",
            vec![""],
        ),
        (
            "the launch permissions of a public image",
            &images.public,
            "launchPermission",
            "launchPermission",
            vec![LAUNCH_PERMISSION_GROUP_ALL],
        ),
        (
            "the launch permissions of another account's public image",
            &images.partner_public,
            "launchPermission",
            "launchPermission",
            vec![LAUNCH_PERMISSION_GROUP_ALL],
        ),
        (
            "the product codes of a paid image",
            &images.private,
            "productCodes",
            "productCodes",
            vec![PRODUCT_CODE, PRODUCT_CODE_TYPE],
        ),
    ];

    for (case, image_id, attribute, element, expected) in cases {
        let (status, body) = describe(&router, image_id, attribute, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );

        let expected_texts: Vec<String> = expected.iter().map(|text| (*text).to_string()).collect();
        assert_eq!(
            attribute_texts(case, &body, element),
            Some(expected_texts.clone()),
            "case {case:?}: expected the <{element}> element to carry {expected_texts:?}, \
             got body {body}"
        );
        assert_eq!(
            attribute_elements(case, &body),
            vec![element.to_string()],
            "case {case:?}: expected <{element}> as the only attribute of the envelope, \
             got body {body}"
        );
        assert_eq!(
            attribute_texts(case, &body, "imageId"),
            Some(vec![image_id.to_string()]),
            "case {case:?}: expected the envelope to name the image {image_id}, got body {body}"
        );

        // A read: asking twice answers the same, so reporting an attribute changed nothing
        // a later caller sees.
        let (again_status, again_body) = describe(&router, image_id, attribute, &[]).await;
        assert_eq!(
            (again_status, reported(case, &again_body)),
            (status, reported(case, &body)),
            "case {case:?}: a second read must answer exactly the same, got {again_body}"
        );
    }

    // The two LIST attributes on an image that carries no entry for them: the report names
    // the image and carries no entry at all — the empty list, however the ec2Query envelope
    // renders one. A private image has no public launch permission, and an image nobody
    // sells carries no product code.
    for (case, image_id, attribute) in [
        (
            "the launch permissions of a private image",
            &images.private,
            "launchPermission",
        ),
        (
            "the product codes of an image nobody sells",
            &images.public,
            "productCodes",
        ),
    ] {
        let (status, body) = describe(&router, image_id, attribute, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let texts = attribute_texts(case, &body, attribute).unwrap_or_default();
        assert!(
            texts.iter().all(|text| text.is_empty()),
            "case {case:?}: expected no entry for <{attribute}>, got {texts:?} in {body}"
        );
        assert_eq!(
            attribute_texts(case, &body, "imageId"),
            Some(vec![image_id.to_string()]),
            "case {case:?}: expected the envelope to name the image {image_id}, got body {body}"
        );
    }
}

/// Rule 2: the value reported for an attribute is the value `DescribeImages` reports for the
/// member of the same image, so the attribute read and the image read cannot disagree.
#[tokio::test]
async fn agrees_with_the_image_describe_about_every_attribute() {
    let (router, engine) = super::ec2_query::emulator();
    let images = seed(&engine);

    // Rows: the `ImageAttributeName` value, and the `Image` member it reads — the two
    // spellings AWS gives the same fact (`smithy_query(query="shape",shape="Image")` for the
    // member's `xmlName`).
    let attributes: [(&str, &str); 9] = [
        ("description", "description"),
        ("kernel", "kernelId"),
        ("ramdisk", "ramdiskId"),
        ("sriovNetSupport", "sriovNetSupport"),
        ("bootMode", "bootMode"),
        ("tpmSupport", "tpmSupport"),
        ("imdsSupport", "imdsSupport"),
        ("lastLaunchedTime", "lastLaunchedTime"),
        ("deregistrationProtection", "deregistrationProtection"),
    ];

    for image_id in [&images.private, &images.public, &images.partner_public] {
        let (status, described) = call(
            &router,
            REGION,
            "DescribeImages",
            &[p("ImageId.1", image_id)],
        )
        .await;
        assert_eq!(
            status, 200,
            "DescribeImages for {image_id} must succeed, got {status} with body {described}"
        );

        for (attribute, member) in attributes {
            let case = format!("{attribute} of {image_id}");
            let (status, body) = describe(&router, image_id, attribute, &[]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );

            // The image describe omits a member the image holds no value for, where the
            // attribute report carries the element with no value — so an absent member and
            // an empty attribute are the same answer.
            let from_image =
                unescaped(&super::ec2_query::element(&described, member).unwrap_or_default());
            let from_attribute = attribute_texts(&case, &body, attribute)
                .unwrap_or_default()
                .join("");
            assert_eq!(
                from_attribute, from_image,
                "case {case:?}: the attribute reported {from_attribute:?} where the image \
                 reports {from_image:?} for <{member}>: {body}"
            );
        }
    }
}

/// Rule 3b: an `Attribute` outside the modelled `ImageAttributeName` catalog never reaches
/// the handler — the request walk refuses it first, with the validator's own code and a
/// message naming the member and the value the request sent.
#[tokio::test]
async fn an_attribute_outside_the_modelled_catalog_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let images = seed(&engine);

    // Rows: a misspelling of a catalog value (the enum is case-sensitive), a name the model
    // declares for no shape, and the empty value.
    for (case, attribute) in [
        ("a catalog value in the wrong case", "bootmode"),
        ("a name the model declares for no attribute", "sshKey"),
        ("no attribute at all", ""),
    ] {
        let (status, body) = describe(&router, &images.private, attribute, &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {case:?}: expected the request walk's own code, got body {body}"
        );
        let message = error_message(&body);
        assert!(
            message.contains(&format!("'{attribute}'")) && message.contains("'attribute'"),
            "case {case:?}: expected a message naming the member and the value \
             {attribute:?}, got {message:?}"
        );
    }
}

/// One refusal row: the request, and the status, code and message it must be refused with.
struct Refusal {
    /// What this row is a case of, named in every assertion of it.
    case: &'static str,
    /// The `ImageId` the request carries.
    image_id: String,
    /// The `Attribute` it asks for.
    attribute: &'static str,
    /// Any further request members — the `DryRun` flag, for the probe rows.
    extra: Vec<(String, String)>,
    /// The HTTP status of the refusal.
    status: u16,
    /// Its `<Code>`.
    code: &'static str,
    /// Its `<Message>`, built from the request the row sends.
    message: String,
}

/// Rule 3: what it refuses — an image id it cannot resolve, the deprecated attribute, and
/// the `DryRun` probe of a request that would have succeeded.
#[tokio::test]
async fn refuses_what_it_cannot_report() {
    let (router, engine) = super::ec2_query::emulator();
    let images = seed(&engine);

    // Rows: (case, image id, attribute, extra members, status, code, message). The image
    // ids span the forms a caller sends: a value that is not an AMI id at all, the long
    // 17-hexadecimal-digit form the service issues, the short 8-digit form, and a
    // well-formed id naming another account's private image — each of which this operation
    // answers with the one refusal it gives an id it cannot resolve.
    let partner_private = images.partner_private.clone();
    let unresolvable = |case: &'static str, image_id: &str, attribute: &'static str| Refusal {
        case,
        image_id: image_id.to_string(),
        attribute,
        extra: Vec::new(),
        status: 400,
        code: "InvalidAMIID.Malformed",
        message: format!("The image ID '{image_id}' is malformed"),
    };
    let cases: Vec<Refusal> = vec![
        unresolvable(
            "a value that is not an AMI id at all",
            "not-an-ami",
            "description",
        ),
        unresolvable(
            "a long-form AMI id that names no image",
            "ami-0123456789abcdef0",
            "launchPermission",
        ),
        unresolvable(
            "a short-form AMI id that names no image",
            "ami-0abc1234",
            "productCodes",
        ),
        unresolvable(
            "another account's private image",
            &partner_private,
            "description",
        ),
        Refusal {
            case: "the deprecated block device mapping attribute",
            image_id: images.private.clone(),
            attribute: "blockDeviceMapping",
            extra: Vec::new(),
            status: 400,
            code: "Client.AuthFailure",
            message: "The blockDeviceMapping attribute is deprecated. To get information \
                      about the block device mappings for an AMI, describe the image instead"
                .to_string(),
        },
        Refusal {
            case: "a dry run of a request that would have succeeded",
            image_id: images.public.clone(),
            attribute: "launchPermission",
            extra: vec![p("DryRun", "true")],
            status: 412,
            code: "DryRunOperation",
            message: "Request would have succeeded, but DryRun flag is set.".to_string(),
        },
        Refusal {
            extra: vec![p("DryRun", "true")],
            ..unresolvable(
                "a dry run of a request that would have been refused",
                "ami-0123456789abcdef0",
                "description",
            )
        },
    ];

    for row in cases {
        let case = row.case;
        let (actual_status, body) = describe(&router, &row.image_id, row.attribute, &row.extra).await;
        assert_eq!(
            actual_status, row.status,
            "case {case:?}: expected HTTP {}, got {actual_status} with body {body}",
            row.status
        );
        assert_eq!(
            error_code(&body),
            row.code,
            "case {case:?}: expected the code {:?}, got body {body}",
            row.code
        );
        assert_eq!(
            error_message(&body),
            row.message,
            "case {case:?}: expected the message {:?}, got body {body}",
            row.message
        );
    }

    // Nothing above wrote: every image is still there, and the one attribute asked about
    // reads the same as it did before the refusals.
    let (status, body) = describe(&router, &images.private, "description", &[]).await;
    assert_eq!(
        status, 200,
        "the stored image must still report its description, got {status} with body {body}"
    );
    assert_eq!(
        attribute_texts("after the refusals", &body, "description"),
        Some(vec![PRIVATE_DESCRIPTION.to_string()]),
        "the refusals must leave the stored image alone, got body {body}"
    );
}
