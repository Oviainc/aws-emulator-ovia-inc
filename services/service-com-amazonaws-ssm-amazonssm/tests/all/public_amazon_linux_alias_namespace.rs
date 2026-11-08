// SPDX-License-Identifier: CC0-1.0

//! The POPULATION of AWS's published Amazon Linux AMI alias hierarchy, end to end over the
//! front door.
//!
//! RULE under test: a path inside the hierarchy AWS publishes its own parameters in
//! enumerates ONE parameter per variant AWS publishes an image of under it — every
//! combination of distribution family, `minimal` variant, pinned kernel version (and the
//! family's `kernel-default` alias), architecture and root-device form that family is
//! published in — so a client that cannot hardcode a per-Region AMI id discovers every
//! candidate by reading the hierarchy, rather than the few of them some other read happens
//! to need. Each entry is reported as a `String`/`text` parameter with a server-assigned
//! version, a last-modified instant and the account-less ARN AWS publishes a parameter
//! under, and its value is the id of an image Amazon EC2 itself describes, with the
//! virtualization and root device the alias name carries.
//!
//! Every case goes through the wire a client uses: `POST /` with
//! `X-Amz-Target: AmazonSSM.<Operation>` and an `awsJson1_1` body for Parameter Store, and an
//! `ec2Query` form POST for the Amazon EC2 read. Nothing reaches into a state struct.
//!
//! The table below is the published alias namespace itself, written out from the name forms
//! Amazon documents for it (Amazon EC2 User Guide, "Use a Systems Manager parameter instead
//! of an AMI ID", and the Amazon Linux 2023 User Guide's "Find an AMI": an alias is the
//! published image name with its release version dropped out,
//! `amzn-ami[-minimal]-{hvm,pv}-x86_64-{ebs,gp2,s3}`,
//! `amzn2-ami[-minimal][-kernel-<kernel>]-hvm-<architecture>-{ebs,gp2}` and
//! `al2023-ami[-minimal]-kernel-{<kernel>,default}-<architecture>`). WHICH releases and
//! kernels a family publishes is service-published data no model describes, so the rows are
//! the catalog this emulator publishes.

use serde_json::{json, Value};

use super::parameter_store::REGION;
use super::public_parameter_catalog::{
    describe_image, element, emulator, is_ami_id, listing, names, read_published,
    recursive_listing, AMAZON_LINUX_NAMESPACE, PUBLISHED_DATA_TYPE, PUBLISHED_TYPE,
};

/// One published family of the namespace: the alias leaves it publishes, and the form of the
/// images behind them.
///
/// The leaves are spelled in full rather than generated, so this table is an independent
/// statement of the population the hierarchy must enumerate.
struct Family {
    /// What the family is called, for a case label.
    label: &'static str,
    /// The kernel version its `kernel-default` alias names — the newest kernel it publishes —
    /// or `None` for a family whose names pin no kernel.
    default_kernel: Option<&'static str>,
    /// Every alias leaf the family publishes, in no particular order.
    leaves: &'static [&'static str],
}

/// The `amzn-ami-*` leaves: the family's names carry the virtualization (`hvm` or `pv`) and
/// the root-device form (`ebs`, `gp2` or `s3`), and only 64-bit x86.
const AMAZON_LINUX: Family = Family {
    label: "Amazon Linux",
    default_kernel: None,
    leaves: &[
        "amzn-ami-hvm-x86_64-ebs",
        "amzn-ami-hvm-x86_64-gp2",
        "amzn-ami-hvm-x86_64-s3",
        "amzn-ami-minimal-hvm-x86_64-ebs",
        "amzn-ami-minimal-hvm-x86_64-s3",
        "amzn-ami-pv-x86_64-ebs",
        "amzn-ami-pv-x86_64-s3",
        "amzn-ami-minimal-pv-x86_64-ebs",
        "amzn-ami-minimal-pv-x86_64-s3",
    ],
};

/// The `amzn2-ami-*` leaves: an unqualified name, a name per pinned kernel and the
/// `kernel-default` alias, each in the architectures and root-device forms the family is
/// published in.
const AMAZON_LINUX_2: Family = Family {
    label: "Amazon Linux 2",
    default_kernel: Some("5.10"),
    leaves: &[
        "amzn2-ami-hvm-x86_64-gp2",
        "amzn2-ami-hvm-x86_64-ebs",
        "amzn2-ami-hvm-arm64-gp2",
        "amzn2-ami-minimal-hvm-x86_64-ebs",
        "amzn2-ami-minimal-hvm-arm64-ebs",
        "amzn2-ami-kernel-4.14-hvm-x86_64-gp2",
        "amzn2-ami-kernel-4.14-hvm-x86_64-ebs",
        "amzn2-ami-kernel-4.14-hvm-arm64-gp2",
        "amzn2-ami-minimal-kernel-4.14-hvm-x86_64-ebs",
        "amzn2-ami-minimal-kernel-4.14-hvm-arm64-ebs",
        "amzn2-ami-kernel-5.10-hvm-x86_64-gp2",
        "amzn2-ami-kernel-5.10-hvm-x86_64-ebs",
        "amzn2-ami-kernel-5.10-hvm-arm64-gp2",
        "amzn2-ami-minimal-kernel-5.10-hvm-x86_64-gp2",
        "amzn2-ami-minimal-kernel-5.10-hvm-arm64-gp2",
        "amzn2-ami-kernel-default-hvm-x86_64-gp2",
        "amzn2-ami-kernel-default-hvm-x86_64-ebs",
        "amzn2-ami-kernel-default-hvm-arm64-gp2",
        "amzn2-ami-minimal-kernel-default-hvm-x86_64-gp2",
        "amzn2-ami-minimal-kernel-default-hvm-arm64-gp2",
    ],
};

/// The `al2023-ami-*` leaves: a name per pinned kernel plus the `kernel-default` alias, in
/// both variants and both architectures. The family's names carry no root-device form.
const AMAZON_LINUX_2023: Family = Family {
    label: "Amazon Linux 2023",
    default_kernel: Some("6.18"),
    leaves: &[
        "al2023-ami-kernel-6.1-x86_64",
        "al2023-ami-kernel-6.1-arm64",
        "al2023-ami-kernel-6.12-x86_64",
        "al2023-ami-kernel-6.12-arm64",
        "al2023-ami-kernel-6.18-x86_64",
        "al2023-ami-kernel-6.18-arm64",
        "al2023-ami-kernel-default-x86_64",
        "al2023-ami-kernel-default-arm64",
        "al2023-ami-minimal-kernel-6.1-x86_64",
        "al2023-ami-minimal-kernel-6.1-arm64",
        "al2023-ami-minimal-kernel-6.12-x86_64",
        "al2023-ami-minimal-kernel-6.12-arm64",
        "al2023-ami-minimal-kernel-6.18-x86_64",
        "al2023-ami-minimal-kernel-6.18-arm64",
        "al2023-ami-minimal-kernel-default-x86_64",
        "al2023-ami-minimal-kernel-default-arm64",
    ],
};

/// The `al2027-preview-ami-*` leaves: the newest family the catalog publishes, named the same
/// way Amazon Linux 2023 is.
const AMAZON_LINUX_2027_PREVIEW: Family = Family {
    label: "Amazon Linux 2027 Preview",
    default_kernel: Some("7.2"),
    leaves: &[
        "al2027-preview-ami-kernel-7.1-x86_64",
        "al2027-preview-ami-kernel-7.1-arm64",
        "al2027-preview-ami-kernel-7.2-x86_64",
        "al2027-preview-ami-kernel-7.2-arm64",
        "al2027-preview-ami-kernel-default-x86_64",
        "al2027-preview-ami-kernel-default-arm64",
        "al2027-preview-ami-minimal-kernel-7.1-x86_64",
        "al2027-preview-ami-minimal-kernel-7.1-arm64",
        "al2027-preview-ami-minimal-kernel-7.2-x86_64",
        "al2027-preview-ami-minimal-kernel-7.2-arm64",
        "al2027-preview-ami-minimal-kernel-default-x86_64",
        "al2027-preview-ami-minimal-kernel-default-arm64",
    ],
};

/// Every family the namespace publishes.
const FAMILIES: &[Family] = &[
    AMAZON_LINUX,
    AMAZON_LINUX_2,
    AMAZON_LINUX_2023,
    AMAZON_LINUX_2027_PREVIEW,
];

/// The token a family's `kernel-default` alias carries where a pinned name carries its kernel
/// version.
const DEFAULT_KERNEL_TOKEN: &str = "kernel-default";

/// Every alias name the namespace must enumerate, as a client writes it.
fn published_alias_names() -> Vec<String> {
    FAMILIES
        .iter()
        .flat_map(|family| family.leaves.iter())
        .map(|leaf| format!("{AMAZON_LINUX_NAMESPACE}/{leaf}"))
        .collect()
}

/// RULE: a recursive read of the published Amazon Linux namespace enumerates exactly the
/// aliases AWS publishes in it — every family, variant, kernel, architecture and root-device
/// form, each exactly once.
#[tokio::test]
async fn the_namespace_enumerates_every_published_alias() {
    let router = emulator();
    let case = "the alias namespace";
    let reported = names(&recursive_listing(&router, case, AMAZON_LINUX_NAMESPACE).await);

    for family in FAMILIES {
        for leaf in family.leaves {
            let expected = format!("{AMAZON_LINUX_NAMESPACE}/{leaf}");
            let case = format!("{} alias {leaf}", family.label);
            assert!(
                reported.contains(&expected),
                "case {case:?}: expected the hierarchy read to report {expected:?}, got \
                 {reported:?}"
            );
        }
    }

    let mut expected = published_alias_names();
    expected.sort();
    let mut once = reported.clone();
    once.dedup();
    assert_eq!(
        once, reported,
        "expected each alias exactly once, got {reported:?}"
    );
    assert_eq!(
        reported, expected,
        "expected the namespace to enumerate exactly the published aliases, got {reported:?}"
    );
}

/// RULE: a family's `kernel-default` alias is a SECOND published name for the build of the
/// newest kernel it publishes, so the two resolve to one AMI rather than to two — and a family
/// that gains a newer kernel moves its default alias with it.
#[tokio::test]
async fn the_kernel_default_alias_names_the_newest_published_kernel() {
    let router = emulator();

    for family in FAMILIES {
        let Some(default_kernel) = family.default_kernel else {
            continue;
        };
        let defaults = family
            .leaves
            .iter()
            .filter(|leaf| leaf.contains(DEFAULT_KERNEL_TOKEN));
        let mut asserted = 0usize;
        for leaf in defaults {
            let pinned = leaf.replace(DEFAULT_KERNEL_TOKEN, &format!("kernel-{default_kernel}"));
            let case = format!("{} alias {leaf} beside {pinned}", family.label);
            assert!(
                family.leaves.contains(&pinned.as_str()),
                "case {case:?}: the newest kernel's own alias must be published too, got {:?}",
                family.leaves
            );
            let through_default =
                read_published(&router, &format!("{AMAZON_LINUX_NAMESPACE}/{leaf}")).await["Value"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
            let through_pinned =
                read_published(&router, &format!("{AMAZON_LINUX_NAMESPACE}/{pinned}")).await
                    ["Value"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
            assert_eq!(
                through_default, through_pinned,
                "case {case:?}: both names must resolve to one AMI, got {through_default:?} and \
                 {through_pinned:?}"
            );
            asserted += 1;
        }
        assert!(
            asserted > 1,
            "{}: a family that pins a kernel publishes a default alias per variant, got \
             {asserted}",
            family.label
        );
    }
}

/// RULE: every alias of the namespace lies one level below it, so the ONE-LEVEL read reports
/// the same population as the recursive one.
#[tokio::test]
async fn the_one_level_read_of_the_namespace_reports_the_same_population() {
    let router = emulator();
    let case = "the alias namespace, one level";
    let recursive = names(&recursive_listing(&router, case, AMAZON_LINUX_NAMESPACE).await);
    let one_level = names(&listing(&router, case, AMAZON_LINUX_NAMESPACE, false).await);
    assert_eq!(
        one_level, recursive,
        "case {case:?}: expected the one-level read to report the recursive population, got \
         {one_level:?}"
    );
}

/// RULE: each enumerated alias is shaped the way AWS publishes a parameter of its own — a
/// `String` carrying `text`, a server-assigned version, a last-modified instant, and the
/// ARN of a parameter that belongs to no account.
#[tokio::test]
async fn every_enumerated_alias_is_shaped_as_a_published_parameter() {
    let router = emulator();
    let reported = recursive_listing(&router, "the alias shapes", AMAZON_LINUX_NAMESPACE).await;
    assert_eq!(
        reported.len(),
        published_alias_names().len(),
        "expected one entry per published alias, got {reported:?}"
    );

    for parameter in &reported {
        let name = parameter["Name"].as_str().unwrap_or_default().to_string();
        let case = format!("alias {name}");
        assert_eq!(
            parameter["Type"].as_str(),
            Some(PUBLISHED_TYPE),
            "case {case:?}: expected Type {PUBLISHED_TYPE}, got {parameter}"
        );
        assert_eq!(
            parameter["DataType"].as_str(),
            Some(PUBLISHED_DATA_TYPE),
            "case {case:?}: expected DataType {PUBLISHED_DATA_TYPE}, got {parameter}"
        );
        assert!(
            parameter["Version"].as_i64().unwrap_or_default() >= 1,
            "case {case:?}: expected a server-assigned version of at least 1, got {parameter}"
        );
        assert!(
            !parameter["LastModifiedDate"].is_null(),
            "case {case:?}: expected a last-modified instant, got {parameter}"
        );
        assert_eq!(
            parameter["ARN"].as_str(),
            Some(format!("arn:aws:ssm:{REGION}::parameter{name}").as_str()),
            "case {case:?}: expected the account-less published-parameter ARN, got {parameter}"
        );
        assert!(
            is_ami_id(parameter["Value"].as_str().unwrap_or_default()),
            "case {case:?}: expected the alias to resolve to an AMI id, got {parameter}"
        );
    }
}

/// RULE (the backed outcome): the id each alias resolves to names an image Amazon EC2 itself
/// describes, and the image's facts are the ones the alias name carries — its architecture,
/// its virtualization and where its root file system lives.
#[tokio::test]
async fn every_alias_resolves_to_an_image_with_the_facts_its_name_carries() {
    let router = emulator();

    for name in published_alias_names() {
        let case = format!("alias {name}");
        let image_id = read_published(&router, &name).await["Value"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let (status, body) = describe_image(&router, &image_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeImages for {image_id}, got {status} \
             with {body}"
        );
        assert_eq!(
            element(&body, "imageId"),
            Some(image_id.clone()),
            "case {case:?}: expected DescribeImages to report {image_id}, got {body}"
        );

        // The architecture level of an alias names the architecture of the image it
        // resolves to.
        let architecture = if name.contains("arm64") {
            "arm64"
        } else {
            "x86_64"
        };
        assert_eq!(
            element(&body, "architecture").as_deref(),
            Some(architecture),
            "case {case:?}: expected an {architecture} image, got {body}"
        );

        // So do the virtualization and the root-device form, for the families whose names
        // carry them: `-pv-` is paravirtual where `-hvm-` is HVM, and a name ending `-s3`
        // is an instance-store-backed image where the others are EBS-backed.
        let virtualization = if name.contains("-pv-") {
            "paravirtual"
        } else {
            "hvm"
        };
        if name.contains("-pv-") || name.contains("-hvm-") {
            assert_eq!(
                element(&body, "virtualizationType").as_deref(),
                Some(virtualization),
                "case {case:?}: expected a {virtualization} image, got {body}"
            );
        }
        let root_device = if name.ends_with("-s3") {
            "instance-store"
        } else {
            "ebs"
        };
        assert_eq!(
            element(&body, "rootDeviceType").as_deref(),
            Some(root_device),
            "case {case:?}: expected a {root_device} root device, got {body}"
        );
    }
}

/// RULE: reading the namespace changes nothing a later caller sees — two identical reads
/// report the same page, and no write of this account can appear in it.
#[tokio::test]
async fn reading_the_namespace_changes_nothing() {
    let router = emulator();
    let body = json!({ "Path": AMAZON_LINUX_NAMESPACE, "Recursive": true, "MaxResults": 10 });
    let first = super::parameter_store::call(&router, "GetParametersByPath", &body).await;
    let second = super::parameter_store::call(&router, "GetParametersByPath", &body).await;
    assert_eq!(
        first, second,
        "expected two identical reads of {AMAZON_LINUX_NAMESPACE} to report the same page"
    );
    let (status, page): (u16, Value) = first;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from the first read, got {status} with {page}"
    );
}
