// SPDX-License-Identifier: CC0-1.0

//! Three capability facts `DescribeInstanceTypes` reports about an instance type, driven
//! over the wire the service speaks (`ec2Query`, an `application/x-www-form-urlencoded`
//! POST answered with XML; README.md / OPERATIONS.md "Wire protocol").
//!
//! The rules under test:
//!
//! 1. A TYPE THAT HAS AN EBS-OPTIMIZED MODE REPORTS ITS EBS-OPTIMIZED PERFORMANCE —
//!    "describes the optimized EBS performance for the instance type"
//!    (`EbsInfo$EbsOptimizedInfo`) — and a type whose `EbsOptimizedSupport` is
//!    `unsupported` reports none. The six figures are one quantity in three pairs: a
//!    baseline that never exceeds the maximum, and a throughput in MB/s that is the same
//!    bandwidth as the Mbps figure beside it. The figures grow with the SIZE of the
//!    instance, which is its share of its host.
//! 2. NVME FOR EBS FOLLOWS THE TYPE'S DEVICE MODEL (`EbsInfo$NvmeSupport`, one of
//!    `unsupported | supported | required` — `smithy_query(query="shape",
//!    shape="EbsNvmeSupport")`): `required` where every volume attaches as an NVMe device,
//!    `supported` where the host has the NVMe model but does not require it — a type whose
//!    local instance store is already NVMe — and `unsupported` where there is no NVMe
//!    device at all. It is what a client picks `nvme*` over `xvd*` guest device names by.
//! 3. HIBERNATION IS A FACT OF THE FAMILY, THE GENERATION AND THE RAM
//!    (`InstanceTypeInfo$HibernationSupported`, "indicates whether On-Demand hibernation
//!    is supported"): the families the hibernation prerequisites name, from the generation
//!    they name, below the published RAM bound, and not bare metal — local instance storage
//!    does not disqualify a type, since the prerequisites list the `d` variants of those
//!    same families.
//!
//! Every row is a different instance type along what those rules generalise over — the
//! family, the generation, the capability letters and the size — so the types the issue
//! behind this suite happened to name are rows among many rather than the specification.

use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, indexed};

/// The Region every call here is signed for; the catalog is the same in each.
const REGION: &str = "us-east-1";

/// The `EbsOptimizedSupport` of a type with no EBS-optimized mode at all.
const EBS_OPTIMIZED_UNSUPPORTED: &str = "unsupported";

/// The megabits in a megabyte, which is all that separates the `Mbps` figures of
/// `EbsOptimizedInfo` from its `MB/s` ones.
const MEGABITS_PER_MEGABYTE: f64 = 8.0;

/// The `ebsInfo` of one instance type, as its XML fragment.
async fn ebs_info(case: &str, router: &MultiServiceRouter, instance_type: &str) -> String {
    let (status, body) = call(
        router,
        REGION,
        "DescribeInstanceTypes",
        &indexed("InstanceType", &[instance_type]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 for {instance_type}, got {status} with body {body}"
    );
    element(&body, "ebsInfo").unwrap_or_else(|| {
        panic!("case {case:?}: no <ebsInfo> reported for {instance_type}: {body}")
    })
}

/// One member of an instance type, as the whole type's body carries it.
async fn type_member(
    case: &str,
    router: &MultiServiceRouter,
    instance_type: &str,
    name: &str,
) -> Option<String> {
    let (status, body) = call(
        router,
        REGION,
        "DescribeInstanceTypes",
        &indexed("InstanceType", &[instance_type]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 for {instance_type}, got {status} with body {body}"
    );
    element(&body, name)
}

/// The six figures of an `ebsOptimizedInfo`, read off one type's `ebsInfo`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Performance {
    baseline_bandwidth: i64,
    baseline_throughput: f64,
    baseline_iops: i64,
    maximum_bandwidth: i64,
    maximum_throughput: f64,
    maximum_iops: i64,
}

/// The `ebsOptimizedInfo` of `ebs_info`, or `None` when the type reports none.
fn performance(case: &str, ebs_info: &str) -> Option<Performance> {
    let info = element(ebs_info, "ebsOptimizedInfo")?;
    let number = |name: &str| -> f64 {
        let text = element(&info, name).unwrap_or_else(|| {
            panic!("case {case:?}: no <{name}> in the reported ebsOptimizedInfo {info:?}")
        });
        text.parse().unwrap_or_else(|_| {
            panic!("case {case:?}: <{name}> must be a number, got {text:?}")
        })
    };
    Some(Performance {
        baseline_bandwidth: number("baselineBandwidthInMbps") as i64,
        baseline_throughput: number("baselineThroughputInMBps"),
        baseline_iops: number("baselineIops") as i64,
        maximum_bandwidth: number("maximumBandwidthInMbps") as i64,
        maximum_throughput: number("maximumThroughputInMBps"),
        maximum_iops: number("maximumIops") as i64,
    })
}

/// Rule 1: every type with an EBS-optimized mode reports its performance, consistently;
/// one without reports none.
#[tokio::test]
async fn an_ebs_optimized_type_reports_its_ebs_optimized_performance() {
    let (router, _engine) = super::ec2_query::emulator();

    // Rows: the size ladder of a Nitro family, a pre-Nitro family that offers EBS
    // optimization as an option, a bare metal size, a Graviton family, a storage optimized
    // pre-Nitro family, and two types with no EBS-optimized mode at all (the pre-Nitro
    // burstable family and the oldest families).
    let rows = [
        "t3.nano",
        "m5.large",
        "m5.xlarge",
        "m5.2xlarge",
        "m5.4xlarge",
        "m5.8xlarge",
        "m5.24xlarge",
        "m5.metal",
        "m7g.large",
        "c4.large",
        "i3.large",
        "t2.micro",
        "m1.small",
    ];

    for instance_type in rows {
        let case = instance_type;
        let info = ebs_info(case, &router, instance_type).await;
        let support = element(&info, "ebsOptimizedSupport").unwrap_or_default();
        let reported = performance(case, &info);
        assert_eq!(
            reported.is_some(),
            support != EBS_OPTIMIZED_UNSUPPORTED,
            "case {case:?}: a type reports its EBS-optimized performance exactly when it \
             has an EBS-optimized mode (support {support:?}), got {reported:?}"
        );
        let Some(figures) = reported else {
            continue;
        };
        assert!(
            figures.baseline_bandwidth > 0
                && figures.baseline_iops > 0
                && figures.baseline_throughput > 0.0,
            "case {case:?}: every baseline figure must be positive, got {figures:?}"
        );
        assert!(
            figures.maximum_bandwidth >= figures.baseline_bandwidth
                && figures.maximum_iops >= figures.baseline_iops
                && figures.maximum_throughput >= figures.baseline_throughput,
            "case {case:?}: a baseline figure cannot exceed its maximum, got {figures:?}"
        );
        for (bandwidth, throughput, which) in [
            (figures.baseline_bandwidth, figures.baseline_throughput, "baseline"),
            (figures.maximum_bandwidth, figures.maximum_throughput, "maximum"),
        ] {
            let expected = bandwidth as f64 / MEGABITS_PER_MEGABYTE;
            assert!(
                (throughput - expected).abs() < 0.01,
                "case {case:?}: the {which} throughput must be the {which} bandwidth in \
                 MB/s ({expected}), got {throughput}"
            );
        }
    }

    // The figures grow with the size of the instance: one family's ladder, read in order.
    let mut previous: Option<Performance> = None;
    for instance_type in ["m5.large", "m5.xlarge", "m5.2xlarge", "m5.4xlarge", "m5.8xlarge"] {
        let case = instance_type;
        let info = ebs_info(case, &router, instance_type).await;
        let figures = performance(case, &info)
            .unwrap_or_else(|| panic!("case {case:?}: {instance_type} must report its \
                 EBS-optimized performance"));
        if let Some(smaller) = previous {
            assert!(
                figures.baseline_bandwidth > smaller.baseline_bandwidth
                    && figures.baseline_iops > smaller.baseline_iops,
                "case {case:?}: a larger size must carry more sustained EBS performance \
                 than the size below it ({smaller:?}), got {figures:?}"
            );
            assert!(
                figures.maximum_bandwidth >= smaller.maximum_bandwidth,
                "case {case:?}: a larger size's ceiling cannot be lower than the size \
                 below it ({smaller:?}), got {figures:?}"
            );
        }
        previous = Some(figures);
    }

    // A Nitro type's sustained figure is a baseline it bursts above at the small sizes,
    // while a pre-Nitro EBS-optimized type has one dedicated figure and no burst.
    let nitro = ebs_info("m5.large bursts", &router, "m5.large").await;
    let nitro = performance("m5.large bursts", &nitro).expect("m5.large is EBS-optimized");
    assert!(
        nitro.maximum_bandwidth > nitro.baseline_bandwidth,
        "m5.large: a small Nitro size bursts above its baseline, got {nitro:?}"
    );
    let xen = ebs_info("c4.large does not burst", &router, "c4.large").await;
    let xen = performance("c4.large does not burst", &xen).expect("c4.large is EBS-optimized");
    assert_eq!(
        xen.maximum_bandwidth, xen.baseline_bandwidth,
        "c4.large: a pre-Nitro EBS-optimized type has one dedicated figure, got {xen:?}"
    );
}

/// Rule 2: NVMe for EBS follows the type's device model.
#[tokio::test]
async fn nvme_for_ebs_follows_the_types_device_model() {
    let (router, _engine) = super::ec2_query::emulator();

    // Rows: Nitro types of three families (every volume is NVMe), two sizes of the storage
    // optimized family whose instance store is NVMe while the instance itself is pre-Nitro
    // (NVMe is available, not required), and pre-Nitro types with no NVMe device — one with
    // no instance store at all, one with a SATA SSD store and one with magnetic disks.
    let rows = [
        ("m5.large", "required"),
        ("t3.micro", "required"),
        ("c6g.medium", "required"),
        ("i3.large", "supported"),
        ("i3.2xlarge", "supported"),
        ("t2.micro", "unsupported"),
        ("m4.large", "unsupported"),
        ("m3.medium", "unsupported"),
        ("m1.small", "unsupported"),
    ];

    for (instance_type, expected) in rows {
        let case = instance_type;
        let info = ebs_info(case, &router, instance_type).await;
        let reported = element(&info, "nvmeSupport").unwrap_or_default();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected nvmeSupport {expected:?}, got {reported:?} in {info}"
        );
    }

    // And the filter on the member selects on the same answer, so a client can ask the
    // catalog for the types it can name `nvme*` devices on.
    for value in ["required", "supported", "unsupported"] {
        let (status, body) = call(
            &router,
            REGION,
            "DescribeInstanceTypes",
            &super::ec2_query::filters(&[("ebs-info.nvme-support", &[value])]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {value:?}: expected HTTP 200 from the nvme-support filter, got {status}: \
             {body}"
        );
        assert!(
            body.contains(&format!("<nvmeSupport>{value}</nvmeSupport>")),
            "case {value:?}: the filter must select the types reporting it, got {body}"
        );
    }
}

/// Rule 3: hibernation support follows the family, the generation, the size and the RAM.
#[tokio::test]
async fn hibernation_support_follows_the_family_generation_and_ram() {
    let (router, _engine) = super::ec2_query::emulator();

    // Rows: the named families at and above the generation the prerequisites name, with and
    // without the `d` capability letter (whose local storage does not disqualify a type),
    // the published RAM bound either side of 150 GiB, a bare metal size, a family the
    // prerequisites do not name, and a generation below the one they name.
    let rows = [
        ("m5.large", true),
        ("m5d.large", true),
        ("c5d.xlarge", true),
        ("r5d.large", true),
        ("t3.micro", true),
        ("t2.small", true),
        ("m5.8xlarge", true),
        ("m5.12xlarge", false),
        ("m5.24xlarge", false),
        ("m5.metal", false),
        ("i3.large", false),
        ("x2gd.medium", false),
        ("m1.small", false),
        ("t1.micro", false),
    ];

    for (instance_type, expected) in rows {
        let case = instance_type;
        let reported = type_member(case, &router, instance_type, "hibernationSupported").await;
        assert_eq!(
            reported.as_deref(),
            Some(expected.to_string().as_str()),
            "case {case:?}: expected hibernationSupported {expected}, got {reported:?}"
        );
    }
}
