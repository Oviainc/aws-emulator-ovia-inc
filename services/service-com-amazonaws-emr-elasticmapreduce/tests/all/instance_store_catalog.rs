// SPDX-License-Identifier: CC0-1.0

//! The instance-store and accelerated half of the `ListSupportedInstanceTypes`
//! catalogue, end to end over the wire.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.ListSupportedInstanceTypes` and an `awsJson1_1`
//! body — through the marker-following harness `tests/all/supported_instance_types.rs`
//! owns, so these are the same bytes an SDK client sends.
//!
//! THE RULE: `SupportedInstanceType$InstanceFamilyId` is "the Amazon EC2 family and
//! generation", so a release's catalogue spans the workload classes of the Amazon EC2
//! naming convention crossed with the generations EMR still lists — the
//! storage-optimised and accelerated families beside the general-purpose, compute- and
//! memory-optimised ones — and each row reports the hardware of its own instance type:
//! `StorageGB` and `NumberOfDisks` are "0 for Amazon EBS-only instance types" and the
//! instance-store capacity the type carries otherwise.

use std::collections::BTreeSet;

use serde_json::json;

use crate::supported_instance_types::{at, row_for, types, Emr};

/// The five workload classes `InstanceFamilyId` is built from, and the two generations.
///
/// SOURCE: the values Amazon EMR reports for this member — the workload classes of the
/// Amazon EC2 instance-type naming convention (general purpose, compute, memory and
/// storage optimised, accelerated) under the generation the family belongs to.
const WORKLOAD_CLASSES: &[&str] = &["GENERAL", "COMPUTE", "HI_MEM", "STORAGE", "GPU"];
const GENERATIONS: &[&str] = &["CURRENT_GEN", "PREVIOUS_GEN"];

/// The hardware facts of one catalogued instance type.
///
/// SOURCE: the Amazon EMR Management Guide's "Supported instance types" table (which
/// release supports which family) crossed with the Amazon EC2 instance-types page (the
/// vCPU, memory and instance-store configuration of each size: `i3.4xlarge`'s
/// "2 × 1,900 NVMe SSD", say). `StorageGB` is the capacity the type carries, which is
/// `NumberOfDisks` of those disks.
///
/// Rows differ along what the rule generalises over: the workload class (storage
/// optimised and accelerated, plus the previous-generation general-purpose and
/// memory-optimised ones), the generation, the release, the storage kind (NVMe SSD, HDD
/// and EBS-only inside one class), the number of disks (one, several and none) and the
/// bottom, middle and top of each family's size ladder.
struct Facts {
    release: &'static str,
    instance_type: &'static str,
    vcpu: i64,
    memory_gb: f64,
    instance_family_id: &'static str,
    storage_gb: i64,
    number_of_disks: i64,
}

const INSTANCE_FACTS: &[Facts] = &[
    // Storage optimised, NVMe SSD: i3 carries one disk up to 2xlarge and several above.
    Facts { release: "emr-6.15.0", instance_type: "i3.large", vcpu: 2, memory_gb: 15.25,
            instance_family_id: "STORAGE_CURRENT_GEN", storage_gb: 475, number_of_disks: 1 },
    Facts { release: "emr-6.15.0", instance_type: "i3.2xlarge", vcpu: 8, memory_gb: 61.0,
            instance_family_id: "STORAGE_CURRENT_GEN", storage_gb: 1900, number_of_disks: 1 },
    Facts { release: "emr-6.15.0", instance_type: "i3.4xlarge", vcpu: 16, memory_gb: 122.0,
            instance_family_id: "STORAGE_CURRENT_GEN", storage_gb: 3800, number_of_disks: 2 },
    Facts { release: "emr-5.30.0", instance_type: "i3.16xlarge", vcpu: 64, memory_gb: 488.0,
            instance_family_id: "STORAGE_CURRENT_GEN", storage_gb: 15200, number_of_disks: 8 },
    Facts { release: "emr-7.5.0", instance_type: "i3en.large", vcpu: 2, memory_gb: 16.0,
            instance_family_id: "STORAGE_CURRENT_GEN", storage_gb: 1250, number_of_disks: 1 },
    Facts { release: "emr-6.15.0", instance_type: "i3en.3xlarge", vcpu: 12, memory_gb: 96.0,
            instance_family_id: "STORAGE_CURRENT_GEN", storage_gb: 7500, number_of_disks: 1 },
    Facts { release: "emr-6.15.0", instance_type: "i3en.24xlarge", vcpu: 96, memory_gb: 768.0,
            instance_family_id: "STORAGE_CURRENT_GEN", storage_gb: 60000, number_of_disks: 8 },
    // Storage optimised, HDD spindles: d3 carries three per xlarge.
    Facts { release: "emr-6.5.0", instance_type: "d3.xlarge", vcpu: 4, memory_gb: 32.0,
            instance_family_id: "STORAGE_CURRENT_GEN", storage_gb: 5940, number_of_disks: 3 },
    Facts { release: "emr-6.15.0", instance_type: "d3.8xlarge", vcpu: 32, memory_gb: 256.0,
            instance_family_id: "STORAGE_CURRENT_GEN", storage_gb: 47520, number_of_disks: 24 },
    // Accelerated computing: g4dn and g5 carry instance-store NVMe, p3 is EBS-only.
    Facts { release: "emr-6.15.0", instance_type: "g4dn.xlarge", vcpu: 4, memory_gb: 16.0,
            instance_family_id: "GPU_CURRENT_GEN", storage_gb: 125, number_of_disks: 1 },
    Facts { release: "emr-7.5.0", instance_type: "g4dn.12xlarge", vcpu: 48, memory_gb: 192.0,
            instance_family_id: "GPU_CURRENT_GEN", storage_gb: 900, number_of_disks: 1 },
    Facts { release: "emr-6.9.0", instance_type: "g5.xlarge", vcpu: 4, memory_gb: 16.0,
            instance_family_id: "GPU_CURRENT_GEN", storage_gb: 250, number_of_disks: 1 },
    Facts { release: "emr-6.15.0", instance_type: "g5.48xlarge", vcpu: 192, memory_gb: 768.0,
            instance_family_id: "GPU_CURRENT_GEN", storage_gb: 7600, number_of_disks: 2 },
    Facts { release: "emr-6.15.0", instance_type: "p3.2xlarge", vcpu: 8, memory_gb: 61.0,
            instance_family_id: "GPU_CURRENT_GEN", storage_gb: 0, number_of_disks: 0 },
    Facts { release: "emr-6.15.0", instance_type: "p3.16xlarge", vcpu: 64, memory_gb: 488.0,
            instance_family_id: "GPU_CURRENT_GEN", storage_gb: 0, number_of_disks: 0 },
    // The previous generation, which is the other half of every InstanceFamilyId.
    Facts { release: "emr-6.15.0", instance_type: "m4.large", vcpu: 2, memory_gb: 8.0,
            instance_family_id: "GENERAL_PREVIOUS_GEN", storage_gb: 0, number_of_disks: 0 },
    Facts { release: "emr-6.15.0", instance_type: "m4.10xlarge", vcpu: 40, memory_gb: 160.0,
            instance_family_id: "GENERAL_PREVIOUS_GEN", storage_gb: 0, number_of_disks: 0 },
    Facts { release: "emr-5.30.0", instance_type: "r4.large", vcpu: 2, memory_gb: 15.25,
            instance_family_id: "HI_MEM_PREVIOUS_GEN", storage_gb: 0, number_of_disks: 0 },
    Facts { release: "emr-7.5.0", instance_type: "r4.16xlarge", vcpu: 64, memory_gb: 488.0,
            instance_family_id: "HI_MEM_PREVIOUS_GEN", storage_gb: 0, number_of_disks: 0 },
];

/// The workload-class half of an `InstanceFamilyId`: the value with its generation
/// suffix taken off, so `HI_MEM_CURRENT_GEN` is `HI_MEM`.
fn workload_class(case: &str, family_id: &str) -> String {
    for generation in GENERATIONS {
        if let Some(class) = family_id.strip_suffix(&format!("_{generation}")) {
            return class.to_string();
        }
    }
    panic!("case {case:?}: expected a generation suffix on {family_id:?}")
}

#[tokio::test]
async fn each_instance_store_and_accelerated_row_reports_its_own_hardware() {
    let emr = Emr::new();
    for facts in INSTANCE_FACTS {
        let case = format!("{}/{}", facts.release, facts.instance_type);
        let rows = emr.all_supported(facts.release).await;
        let row = row_for(&rows, facts.instance_type).unwrap_or_else(|| {
            panic!(
                "case {case:?}: {} must be reported, got {} types",
                facts.instance_type,
                rows.len()
            )
        });

        assert_eq!(
            at(row, &["VCPU"]).as_i64(),
            Some(facts.vcpu),
            "case {case:?}: expected VCPU {}, got {row}",
            facts.vcpu
        );
        let reported_memory = at(row, &["MemoryGB"]).as_f64();
        assert!(
            reported_memory.is_some_and(|got| (got - facts.memory_gb).abs() < 0.001),
            "case {case:?}: expected MemoryGB {}, got {reported_memory:?}",
            facts.memory_gb
        );
        assert_eq!(
            at(row, &["InstanceFamilyId"]).as_str(),
            Some(facts.instance_family_id),
            "case {case:?}: expected InstanceFamilyId {}, got {row}",
            facts.instance_family_id
        );
        assert_eq!(
            at(row, &["StorageGB"]).as_i64(),
            Some(facts.storage_gb),
            "case {case:?}: expected StorageGB {}, got {row}",
            facts.storage_gb
        );
        assert_eq!(
            at(row, &["NumberOfDisks"]).as_i64(),
            Some(facts.number_of_disks),
            "case {case:?}: expected NumberOfDisks {}, got {row}",
            facts.number_of_disks
        );
        // "Indicates whether the SupportedInstanceType only supports Amazon EBS": a
        // type carrying instance-store disks does not.
        assert_eq!(
            at(row, &["EbsStorageOnly"]).as_bool(),
            Some(facts.number_of_disks == 0),
            "case {case:?}: expected EbsStorageOnly {}, got {row}",
            facts.number_of_disks == 0
        );
    }
}

/// The newest `emr-6.x` release, as a caller finds it: `ListReleaseLabels` filtered on
/// the series prefix answers the catalogue newest first.
async fn newest_emr_6_release(emr: &Emr) -> String {
    let payload = emr
        .ok(
            "ListReleaseLabels",
            json!({ "Filters": { "Prefix": "emr-6." } }),
        )
        .await;
    let labels = at(&payload, &["ReleaseLabels"])
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("ListReleaseLabels must answer a list, got {payload}"));
    labels
        .first()
        .and_then(|label| label.as_str())
        .unwrap_or_else(|| panic!("the emr-6 series must offer a release, got {payload}"))
        .to_string()
}

/// THE RULE: a release's catalogue spans the five workload classes, under the
/// generations EMR lists — asserted on the newest release of the 6.x series as a caller
/// reaches it, and on releases of the 5.x and 7.x series too, so it is the catalogue's
/// shape rather than one label's.
#[tokio::test]
async fn a_release_catalogue_spans_every_workload_class() {
    let emr = Emr::new();
    let newest_6 = newest_emr_6_release(&emr).await;
    let permitted: BTreeSet<String> = WORKLOAD_CLASSES
        .iter()
        .flat_map(|class| {
            GENERATIONS
                .iter()
                .map(move |generation| format!("{class}_{generation}"))
        })
        .collect();

    for release in ["emr-5.30.0", "emr-6.15.0", newest_6.as_str(), "emr-7.5.0"] {
        let rows = emr.all_supported(release).await;
        let family_ids: BTreeSet<String> = rows
            .iter()
            .map(|row| {
                at(row, &["InstanceFamilyId"])
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {release:?}: every row carries an InstanceFamilyId, got {row}")
                    })
                    .to_string()
            })
            .collect();
        assert!(
            family_ids.is_subset(&permitted),
            "case {release:?}: expected family ids drawn from {permitted:?}, got {:?}",
            family_ids.difference(&permitted).collect::<Vec<_>>()
        );

        let classes: BTreeSet<String> = family_ids
            .iter()
            .map(|id| workload_class(release, id))
            .collect();
        let expected: BTreeSet<String> = WORKLOAD_CLASSES
            .iter()
            .map(|class| (*class).to_string())
            .collect();
        assert_eq!(
            classes, expected,
            "case {release:?}: the catalogue must span every workload class, got {classes:?}"
        );

        let generations: BTreeSet<&str> = family_ids
            .iter()
            .filter_map(|id| {
                GENERATIONS
                    .iter()
                    .copied()
                    .find(|generation| id.ends_with(generation))
            })
            .collect();
        assert_eq!(
            generations,
            GENERATIONS.iter().copied().collect::<BTreeSet<&str>>(),
            "case {release:?}: the catalogue must span both generations, got {generations:?}"
        );
    }
}

/// THE RULE for the `ReleaseLabel` argument: the catalogue is the families the
/// REQUESTED release supports, so a release of a later series reports more than the
/// newest release of an earlier one rather than the same list. Rows differ along the
/// pair of releases compared — across series and within one.
#[tokio::test]
async fn a_later_series_reports_a_catalogue_of_its_own() {
    let emr = Emr::new();
    for (earlier, later) in [
        ("emr-5.30.0", "emr-6.15.0"),
        ("emr-6.15.0", "emr-7.0.0"),
        ("emr-6.15.0", "emr-7.5.0"),
        ("emr-6.0.0", "emr-6.15.0"),
    ] {
        let earlier_types: BTreeSet<String> = catalogued_types(&emr, earlier).await;
        let later_types: BTreeSet<String> = catalogued_types(&emr, later).await;
        assert!(
            earlier_types.is_subset(&later_types),
            "case {earlier:?} -> {later:?}: a later release keeps what the earlier supports, \
             missing {:?}",
            earlier_types.difference(&later_types).collect::<Vec<_>>()
        );
        assert_ne!(
            earlier_types, later_types,
            "case {earlier:?} -> {later:?}: the catalogue follows the ReleaseLabel, got the \
             same {} types for both",
            later_types.len()
        );
    }
}

/// Every type a release reports, as a set — read through the `Type` extractor
/// `supported_instance_types.rs` owns.
async fn catalogued_types(emr: &Emr, release: &str) -> BTreeSet<String> {
    types(&emr.all_supported(release).await).into_iter().collect()
}

/// THE INVARIANT behind the two storage members, over the WHOLE catalogue of several
/// releases rather than the rows named above: `StorageGB` and `NumberOfDisks` are "0 for
/// Amazon EBS-only instance types" and non-zero together otherwise, `EbsStorageOnly`
/// says which of the two a type is, and an instance-store capacity divides evenly by the
/// number of disks it is spread over — a type reports whole disks, never a fraction of
/// one. Every size of one family agrees on being instance-store or EBS-only, and at
/// least one family of each kind is reported.
#[tokio::test]
async fn the_storage_members_agree_on_every_row() {
    let emr = Emr::new();
    for release in ["emr-5.30.0", "emr-6.5.0", "emr-6.15.0", "emr-7.5.0"] {
        let rows = emr.all_supported(release).await;
        let mut instance_store_families: BTreeSet<String> = BTreeSet::new();
        let mut ebs_only_families: BTreeSet<String> = BTreeSet::new();

        for row in &rows {
            let instance_type = at(row, &["Type"]).as_str().unwrap_or_default().to_string();
            let case = format!("{release}/{instance_type}");
            let storage_gb = at(row, &["StorageGB"])
                .as_i64()
                .unwrap_or_else(|| panic!("case {case:?}: every row carries StorageGB, got {row}"));
            let disks = at(row, &["NumberOfDisks"]).as_i64().unwrap_or_else(|| {
                panic!("case {case:?}: every row carries NumberOfDisks, got {row}")
            });
            let ebs_only = at(row, &["EbsStorageOnly"]).as_bool().unwrap_or_else(|| {
                panic!("case {case:?}: every row carries EbsStorageOnly, got {row}")
            });

            assert_eq!(
                storage_gb == 0,
                disks == 0,
                "case {case:?}: StorageGB and NumberOfDisks are 0 together, got {row}"
            );
            assert_eq!(
                ebs_only,
                disks == 0,
                "case {case:?}: EbsStorageOnly must be {}, got {row}",
                disks == 0
            );
            assert!(
                storage_gb >= 0 && disks >= 0,
                "case {case:?}: neither storage member is negative, got {row}"
            );
            if disks > 0 {
                assert_eq!(
                    storage_gb % disks,
                    0,
                    "case {case:?}: {storage_gb} GB over {disks} disks is not whole disks, \
                     got {row}"
                );
            }

            let family = instance_type
                .split_once('.')
                .map(|(family, _)| family.to_string())
                .unwrap_or_else(|| panic!("case {case:?}: a type is <family>.<size>"));
            if ebs_only {
                assert!(
                    !instance_store_families.contains(&family),
                    "case {case:?}: every size of {family:?} is instance-store or EBS-only, \
                     not both"
                );
                ebs_only_families.insert(family);
            } else {
                assert!(
                    !ebs_only_families.contains(&family),
                    "case {case:?}: every size of {family:?} is instance-store or EBS-only, \
                     not both"
                );
                instance_store_families.insert(family);
            }
        }

        assert!(
            !instance_store_families.is_empty(),
            "case {release:?}: the catalogue reports instance-store families, got none"
        );
        assert!(
            !ebs_only_families.is_empty(),
            "case {release:?}: the catalogue reports EBS-only families, got none"
        );
    }
}
