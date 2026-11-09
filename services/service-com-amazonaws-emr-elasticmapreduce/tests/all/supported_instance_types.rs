// SPDX-License-Identifier: CC0-1.0

//! `ListSupportedInstanceTypes`, end to end over the wire.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.ListSupportedInstanceTypes` and an `awsJson1_1`
//! body — through the router's `dispatch`, so these are the same bytes an SDK client
//! sends.
//!
//! The rule under test: the response is the service's instance-type catalog filtered
//! to the families the REQUESTED release supports, every row's facts derived from its
//! `<family>.<size>` name, paginated on `Marker`; a release label the service does not
//! offer is refused. The recorded case is one row of each table below.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_emr_elasticmapreduce::state::{
    catalog_release_labels, LIST_SUPPORTED_INSTANCE_TYPES_PAGE_SIZE as PAGE_SIZE,
};
use service_com_amazonaws_emr_elasticmapreduce::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A sanity bound on the marker walk, so a broken marker cannot loop forever: the
/// catalog is nowhere near `MAX_PAGES * PAGE_SIZE` rows.
const MAX_PAGES: usize = 20;

static NULL: Value = Value::Null;

/// One emulator: a router carrying only this service, over one shared engine.
///
/// `pub(crate)` so a sibling suite walks the catalog through the SAME marker-following
/// harness rather than growing its own copy.
pub(crate) struct Emr {
    router: MultiServiceRouter,
}

impl Emr {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    pub(crate) async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "elasticmapreduce.us-east-1.amazonaws.com")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("ElasticMapReduce.{operation}"))
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, payload)
    }

    pub(crate) async fn ok(&self, operation: &str, body: Value) -> Value {
        let (status, payload) = self.call(operation, body.clone()).await;
        assert_eq!(
            status, 200,
            "{operation} {body} must answer 200, got {status} with {payload}"
        );
        payload
    }

    /// One page of `ListSupportedInstanceTypes`.
    pub(crate) async fn page(&self, release: &str, marker: Option<&str>) -> (u16, Value) {
        let mut body = json!({ "ReleaseLabel": release });
        if let Some(marker) = marker {
            body["Marker"] = json!(marker);
        }
        self.call("ListSupportedInstanceTypes", body).await
    }

    /// Every instance type `release` supports, by following `Marker` to the last page.
    ///
    /// Asserts the pagination contract on the way: no page is longer than the page
    /// size, the marker of a page is the `Type` of the first row of the NEXT page, and
    /// the last page carries no marker.
    pub(crate) async fn all_supported(&self, release: &str) -> Vec<Value> {
        let mut rows: Vec<Value> = Vec::new();
        let mut marker: Option<String> = None;
        for page_number in 0..MAX_PAGES {
            let (status, payload) = self.page(release, marker.as_deref()).await;
            assert_eq!(
                status, 200,
                "case {release:?} page {page_number}: expected 200, got {status} with {payload}"
            );
            let page: Vec<Value> = at(&payload, &["SupportedInstanceTypes"])
                .as_array()
                .cloned()
                .unwrap_or_else(|| {
                    panic!("case {release:?}: SupportedInstanceTypes must be a list, got {payload}")
                });
            assert!(
                page.len() <= PAGE_SIZE,
                "case {release:?} page {page_number}: expected at most {PAGE_SIZE} rows, got {}",
                page.len()
            );
            if let Some(expected_first) = marker.as_deref() {
                assert_eq!(
                    types(&page).first().map(String::as_str),
                    Some(expected_first),
                    "case {release:?} page {page_number}: the marker must name the first row of \
                     the next page, got {:?}",
                    types(&page).first()
                );
            }
            rows.extend(page);
            match at(&payload, &["Marker"]).as_str() {
                Some(next) => {
                    assert!(
                        rows.len().is_multiple_of(PAGE_SIZE),
                        "case {release:?} page {page_number}: a marker means a further page, so \
                         the pages before it must be full, got {} rows so far",
                        rows.len()
                    );
                    marker = Some(next.to_string());
                }
                None => return rows,
            }
        }
        panic!("case {release:?}: the marker walk did not end within {MAX_PAGES} pages");
    }
}

/// Walk a JSON path, answering `null` for anything absent.
pub(crate) fn at<'a>(value: &'a Value, path: &[&str]) -> &'a Value {
    let mut cursor = value;
    for key in path {
        cursor = cursor.get(*key).unwrap_or(&NULL);
    }
    cursor
}

/// The `Type` of every row, in the order the service reported them.
pub(crate) fn types(rows: &[Value]) -> Vec<String> {
    rows.iter()
        .map(|row| {
            at(row, &["Type"])
                .as_str()
                .unwrap_or_else(|| panic!("every row carries a Type, got {row}"))
                .to_string()
        })
        .collect()
}

/// The row for `instance_type`, if the release reported it.
pub(crate) fn row_for<'a>(rows: &'a [Value], instance_type: &str) -> Option<&'a Value> {
    rows.iter()
        .find(|row| at(row, &["Type"]).as_str() == Some(instance_type))
}

fn assert_refused(case: &str, status: u16, payload: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {payload}"
    );
    let code = at(payload, &["__type"]).as_str().unwrap_or_default();
    assert!(
        code.contains("InvalidRequestException"),
        "case {case:?}: expected error code InvalidRequestException, got {code:?} in {payload}"
    );
}

/// Which release each instance family became supported in, as the catalog records it,
/// probed one release BELOW and one AT the introduction point. Rows differ along the
/// release (5.x, 6.x, 7.x), the family (general purpose, compute, memory and storage
/// optimised, accelerated), the generation (current and previous)
/// and the architecture (x86-64 and Graviton), which is what the filter generalises
/// over. SOURCE: the Amazon EMR Management Guide's "Supported instance types" table,
/// mirrored by `crate::state::INSTANCE_FAMILIES`.
const RELEASE_FILTER: &[(&str, &str, bool)] = &[
    // The fifth generation is supported by every release the catalog offers (the
    // oldest is emr-5.30.0, later than the emr-5.13.0/emr-5.20.0 releases that
    // introduced those families).
    ("emr-5.30.0", "m5.xlarge", true),
    ("emr-5.30.0", "c5.9xlarge", true),
    ("emr-5.30.0", "r5.xlarge", true),
    // Graviton reaches emr-5.31.0 in the 5.x series and emr-6.1.0 in the 6.x one, so
    // emr-6.0.0 — a LATER release than emr-5.31.0 — does not support it.
    ("emr-5.30.2", "m6g.xlarge", false),
    ("emr-5.31.0", "m6g.xlarge", true),
    ("emr-6.0.0", "m6g.xlarge", false),
    ("emr-6.1.0", "m6g.xlarge", true),
    ("emr-6.0.1", "c6g.2xlarge", false),
    ("emr-6.1.0", "c6g.2xlarge", true),
    ("emr-6.1.1", "r6g.16xlarge", true),
    // The sixth generation Intel/AMD families.
    ("emr-6.4.0", "r6i.32xlarge", false),
    ("emr-6.5.0", "r6i.32xlarge", true),
    ("emr-6.5.0", "m6a.48xlarge", false),
    ("emr-6.6.0", "m6a.48xlarge", true),
    // The seventh generation, including Graviton3.
    ("emr-6.14.0", "m7g.xlarge", false),
    ("emr-6.15.0", "m7g.xlarge", true),
    ("emr-6.14.0", "c7i.48xlarge", false),
    ("emr-6.15.0", "c7i.48xlarge", true),
    ("emr-7.5.0", "c7g.16xlarge", true),
    ("emr-7.0.0", "m7i.24xlarge", true),
    // A family this catalog does not publish is reported by no release: the
    // instance-store variants of the EBS-only families and the burstable ones are
    // outside it.
    ("emr-7.5.0", "m5d.xlarge", false),
    ("emr-7.5.0", "t3.xlarge", false),
    // The storage-optimised and accelerated families are published, from the release
    // each became supported in.
    ("emr-6.15.0", "i3.4xlarge", true),
    ("emr-6.15.0", "i3en.3xlarge", true),
    ("emr-6.4.0", "d3.2xlarge", false),
    ("emr-6.5.0", "d3.2xlarge", true),
    ("emr-6.15.0", "g4dn.8xlarge", true),
    ("emr-6.8.0", "g5.12xlarge", false),
    ("emr-6.9.0", "g5.12xlarge", true),
    ("emr-6.15.0", "p3.8xlarge", true),
    // The previous-generation families EMR still lists.
    ("emr-6.15.0", "m4.10xlarge", true),
    ("emr-6.15.0", "r4.16xlarge", true),
    // The families the 7.x series added, which the last 6.x release predates.
    ("emr-6.15.0", "m7a.xlarge", false),
    ("emr-7.0.0", "m7a.xlarge", true),
    ("emr-6.15.0", "c7a.2xlarge", false),
    ("emr-7.5.0", "c7a.2xlarge", true),
    ("emr-7.5.0", "r7a.48xlarge", true),
];

#[tokio::test]
async fn a_release_reports_the_families_it_supports() {
    let emr = Emr::new();
    for (release, instance_type, expected) in RELEASE_FILTER {
        let rows = emr.all_supported(release).await;
        let present = row_for(&rows, instance_type).is_some();
        assert_eq!(
            present, *expected,
            "case {release:?}/{instance_type:?}: expected present={expected}, got \
             present={present} among {} reported types",
            rows.len()
        );
    }
}

/// The facts a row carries, derived from its `<family>.<size>` name: EC2 sizes a family
/// in units of two vCPUs (`large` 2, `xlarge` 4, `<n>xlarge` 4n) and memory is a
/// per-family constant number of GiB per vCPU (4 general purpose, 2 compute optimised,
/// 8 memory optimised). `InstanceFamilyId` is "the Amazon EC2 family and generation" as
/// EMR reports it: the family's workload class and its generation, never the family part
/// of the type name. Rows span both architectures, all three memory ratios of the
/// EBS-only families, the three classes those families belong to and the bottom, middle
/// and top of several size ladders; the storage-optimised and accelerated families are
/// pinned by `instance_store_catalog.rs`.
const INSTANCE_FACTS: &[(&str, &str, i64, f64, &str, &str)] = &[
    // (release, type, vCPU, MemoryGB, InstanceFamilyId, Architecture)
    ("emr-7.5.0", "m5.large", 2, 8.0, "GENERAL_CURRENT_GEN", "X86_64"),
    ("emr-7.5.0", "m5.xlarge", 4, 16.0, "GENERAL_CURRENT_GEN", "X86_64"),
    (
        "emr-7.5.0",
        "m5.24xlarge",
        96,
        384.0,
        "GENERAL_CURRENT_GEN",
        "X86_64",
    ),
    (
        "emr-7.5.0",
        "m5a.12xlarge",
        48,
        192.0,
        "GENERAL_CURRENT_GEN",
        "X86_64",
    ),
    ("emr-7.5.0", "c5.large", 2, 4.0, "COMPUTE_CURRENT_GEN", "X86_64"),
    (
        "emr-7.5.0",
        "c5.9xlarge",
        36,
        72.0,
        "COMPUTE_CURRENT_GEN",
        "X86_64",
    ),
    (
        "emr-7.5.0",
        "c5.18xlarge",
        72,
        144.0,
        "COMPUTE_CURRENT_GEN",
        "X86_64",
    ),
    ("emr-7.5.0", "r5.large", 2, 16.0, "HI_MEM_CURRENT_GEN", "X86_64"),
    (
        "emr-7.5.0",
        "r5.8xlarge",
        32,
        256.0,
        "HI_MEM_CURRENT_GEN",
        "X86_64",
    ),
    (
        "emr-7.5.0",
        "m6i.32xlarge",
        128,
        512.0,
        "GENERAL_CURRENT_GEN",
        "X86_64",
    ),
    (
        "emr-7.5.0",
        "m6a.48xlarge",
        192,
        768.0,
        "GENERAL_CURRENT_GEN",
        "X86_64",
    ),
    (
        "emr-7.5.0",
        "c7i.48xlarge",
        192,
        384.0,
        "COMPUTE_CURRENT_GEN",
        "X86_64",
    ),
    (
        "emr-7.5.0",
        "m6g.large",
        2,
        8.0,
        "GENERAL_CURRENT_GEN",
        "AARCH64",
    ),
    (
        "emr-7.5.0",
        "c7g.large",
        2,
        4.0,
        "COMPUTE_CURRENT_GEN",
        "AARCH64",
    ),
    (
        "emr-7.5.0",
        "r7g.16xlarge",
        64,
        512.0,
        "HI_MEM_CURRENT_GEN",
        "AARCH64",
    ),
    (
        "emr-6.1.0",
        "r6g.12xlarge",
        48,
        384.0,
        "HI_MEM_CURRENT_GEN",
        "AARCH64",
    ),
];

#[tokio::test]
async fn a_row_reports_the_facts_of_its_instance_type() {
    let emr = Emr::new();
    for (release, instance_type, vcpu, memory_gb, family, architecture) in INSTANCE_FACTS {
        let case = format!("{release}/{instance_type}");
        let rows = emr.all_supported(release).await;
        let row = row_for(&rows, instance_type)
            .unwrap_or_else(|| panic!("case {case:?}: {instance_type} must be reported"));

        assert_eq!(
            at(row, &["VCPU"]).as_i64(),
            Some(*vcpu),
            "case {case:?}: expected VCPU {vcpu}, got {row}"
        );
        let reported_memory = at(row, &["MemoryGB"]).as_f64();
        assert!(
            reported_memory.is_some_and(|got| (got - *memory_gb).abs() < 0.001),
            "case {case:?}: expected MemoryGB {memory_gb}, got {reported_memory:?}"
        );
        assert_eq!(
            at(row, &["InstanceFamilyId"]).as_str(),
            Some(*family),
            "case {case:?}: expected InstanceFamilyId {family}, got {row}"
        );
        assert_eq!(
            at(row, &["Architecture"]).as_str(),
            Some(*architecture),
            "case {case:?}: expected Architecture {architecture}, got {row}"
        );
        // Every family in the table above is EBS-only: "StorageGB … is 0 for Amazon
        // EBS-only instance types", and so is NumberOfDisks. The instance-store
        // families carry their disks instead, which `instance_store_catalog.rs` pins.
        assert_eq!(
            at(row, &["StorageGB"]).as_i64(),
            Some(0),
            "case {case:?}: expected StorageGB 0 on an EBS-only type, got {row}"
        );
        assert_eq!(
            at(row, &["NumberOfDisks"]).as_i64(),
            Some(0),
            "case {case:?}: expected NumberOfDisks 0 on an EBS-only type, got {row}"
        );
        assert_eq!(
            at(row, &["EbsStorageOnly"]).as_bool(),
            Some(true),
            "case {case:?}: expected EbsStorageOnly true, got {row}"
        );
        assert_eq!(
            at(row, &["Is64BitsOnly"]).as_bool(),
            Some(true),
            "case {case:?}: expected Is64BitsOnly true, got {row}"
        );
        assert_eq!(
            at(row, &["EbsOptimizedAvailable"]).as_bool(),
            Some(true),
            "case {case:?}: expected EbsOptimizedAvailable true, got {row}"
        );
    }
}

/// Every value Amazon EMR reports for `SupportedInstanceType$InstanceFamilyId` ("The
/// Amazon EC2 family and generation"): a workload class crossed with a generation.
///
/// SOURCE: the values EMR reports for this member — the workload classes of the Amazon
/// EC2 instance-type naming convention (general purpose, compute, memory, storage and
/// accelerated) under the generation the family belongs to — mirrored by
/// `crate::state::family_class`.
const FAMILY_CLASSES: &[&str] = &["GENERAL", "COMPUTE", "HI_MEM", "STORAGE", "GPU"];
const FAMILY_GENERATIONS: &[&str] = &["CURRENT", "PREVIOUS"];

/// Every GiB-per-vCPU ratio the catalogue publishes, paired with the workload class it
/// implies where it implies one: an Amazon EBS-only family of 4, 2 or 8 GiB per vCPU is
/// general purpose, compute optimised or memory optimised respectively, while the
/// Xen-generation 7.625 is shared by more than one class and implies nothing. A ratio
/// outside this closed set is a typo in the catalogue, so it fails the case rather than
/// skipping the assertion.
const MEMORY_RATIOS: &[(f64, Option<&str>)] = &[
    (4.0, Some("GENERAL")),
    (2.0, Some("COMPUTE")),
    (8.0, Some("HI_MEM")),
    (7.625, None),
];

/// The workload class a row's memory ratio implies, for the rows whose class it DOES
/// imply. `None` where the ratio implies no single class; a storage-optimised or
/// accelerated family is classified by the hardware it carries rather than by its ratio,
/// so the caller applies this to the Amazon EBS-only rows.
fn class_for_memory_ratio(case: &str, memory_gb: f64, vcpu: i64) -> Option<&'static str> {
    let ratio = memory_gb / vcpu as f64;
    MEMORY_RATIOS
        .iter()
        .find(|(catalogued, _)| (ratio - catalogued).abs() < 0.001)
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a catalogued memory ratio, got {ratio} GiB per vCPU")
        })
        .1
}

/// THE RULE: `InstanceFamilyId` is the family's workload class and generation, drawn from
/// the closed set above — never the family part of the type name — and it is the same for
/// every size of one family and consistent with that family's own reported facts.
#[tokio::test]
async fn every_row_reports_its_family_and_generation_class() {
    let emr = Emr::new();
    let permitted: BTreeSet<String> = FAMILY_CLASSES
        .iter()
        .flat_map(|class| {
            FAMILY_GENERATIONS
                .iter()
                .map(move |generation| format!("{class}_{generation}_GEN"))
        })
        .collect();

    // Rows differ along the release, so a family reached through different releases is
    // checked each time it is reported.
    for release in ["emr-5.30.0", "emr-6.1.0", "emr-6.15.0", "emr-7.5.0"] {
        let rows = emr.all_supported(release).await;
        // family part of the name -> the class every size of it reported
        let mut by_family: std::collections::BTreeMap<String, String> =
            std::collections::BTreeMap::new();
        for row in &rows {
            let instance_type = at(row, &["Type"]).as_str().unwrap_or_default().to_string();
            let case = format!("{release}/{instance_type}");
            let family_id = at(row, &["InstanceFamilyId"])
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: every row carries an InstanceFamilyId"))
                .to_string();
            assert!(
                permitted.contains(&family_id),
                "case {case:?}: expected one of {permitted:?}, got {family_id:?}"
            );
            let family = instance_type
                .split_once('.')
                .map(|(family, _)| family.to_string())
                .unwrap_or_else(|| panic!("case {case:?}: a type is <family>.<size>"));
            assert_ne!(
                family_id, family,
                "case {case:?}: the family and generation class is not the type prefix, got \
                 {family_id:?}"
            );

            let vcpu = at(row, &["VCPU"]).as_i64().unwrap_or_default();
            let memory_gb = at(row, &["MemoryGB"]).as_f64().unwrap_or_default();
            let ebs_only = at(row, &["EbsStorageOnly"]).as_bool().unwrap_or_default();
            // Called for EVERY row, so an uncatalogued ratio fails the case; the class
            // it implies is asserted on the Amazon EBS-only rows, the ones whose class
            // the ratio decides.
            if let Some(expected) =
                class_for_memory_ratio(&case, memory_gb, vcpu).filter(|_| ebs_only)
            {
                assert_eq!(
                    family_id.strip_suffix("_CURRENT_GEN").or_else(|| family_id
                        .strip_suffix("_PREVIOUS_GEN")),
                    Some(expected),
                    "case {case:?}: an EBS-only family of {memory_gb} GiB over {vcpu} vCPUs is \
                     {expected}, got {family_id:?}"
                );
            }

            match by_family.get(&family) {
                Some(already) => assert_eq!(
                    already, &family_id,
                    "case {case:?}: every size of {family:?} reports one class, got \
                     {family_id:?} after {already:?}"
                ),
                None => {
                    by_family.insert(family, family_id);
                }
            }
        }
        assert!(
            by_family.len() > 3,
            "case {release:?}: the catalogue must publish several families, got {by_family:?}"
        );
    }
}

/// Release labels the service's release catalog offers no release for. Rows differ
/// along the way the label is wrong: a plausible but unoffered minor, a release older
/// than the oldest the catalog offers, an unoffered patch level, a release newer than
/// the newest it offers, a version far outside every line, a missing or extra
/// component, a missing prefix, a non-numeric component, a padded component, trailing
/// whitespace and an empty label.
///
/// `pub(crate)` so a sibling suite refuses the same labels through one list rather than
/// its own copy.
pub(crate) const BAD_RELEASE_LABELS: &[&str] = &[
    "emr-6.99.0",
    "emr-5.13.0",
    "emr-6.15.1",
    "emr-7.90.0",
    "emr-999.0.0",
    "emr-4.9.5",
    "emr-6.15",
    "emr-6.15.0.1",
    "6.15.0",
    "emr-",
    "emr-6.x.0",
    "emr-06.15.0",
    "emr-6.15.0 ",
    "",
];

#[tokio::test]
async fn a_release_the_catalog_does_not_offer_is_refused() {
    let emr = Emr::new();
    for label in BAD_RELEASE_LABELS {
        let (status, payload) = emr.page(label, None).await;
        assert_refused(label, status, &payload);
    }
}

#[tokio::test]
async fn a_marker_naming_no_catalogued_type_is_refused() {
    let emr = Emr::new();
    for marker in ["m5.xlarge-but-not", "", "zz9.xlarge", "m5d.xlarge"] {
        let (status, payload) = emr.page("emr-7.5.0", Some(marker)).await;
        assert_refused(marker, status, &payload);
    }
}

#[tokio::test]
async fn the_marker_walk_reports_every_type_once() {
    let emr = Emr::new();
    for release in ["emr-5.30.0", "emr-6.1.0", "emr-6.15.0", "emr-7.5.0"] {
        // `all_supported` asserts the page-size and marker contract as it walks.
        let rows = emr.all_supported(release).await;
        let reported = types(&rows);
        assert!(
            !reported.is_empty(),
            "case {release:?}: an offered release supports at least one instance type"
        );
        let distinct: BTreeSet<&String> = reported.iter().collect();
        assert_eq!(
            distinct.len(),
            reported.len(),
            "case {release:?}: expected every type once, got {} rows and {} distinct",
            reported.len(),
            distinct.len()
        );

        // The first page is the prefix of the walk, so the paged read and a single
        // page agree on where the catalog starts.
        let (status, first) = emr.page(release, None).await;
        assert_eq!(status, 200, "case {release:?}: expected 200, got {status}");
        let first_page = types(
            at(&first, &["SupportedInstanceTypes"])
                .as_array()
                .unwrap_or_else(|| panic!("case {release:?}: expected a list, got {first}")),
        );
        assert_eq!(
            first_page.as_slice(),
            &reported[..first_page.len()],
            "case {release:?}: the first page must be the prefix of the walk"
        );
    }
}

#[tokio::test]
async fn reading_the_catalog_changes_nothing() {
    let emr = Emr::new();
    let release = "emr-6.15.0";
    let first = emr.all_supported(release).await;
    // A cluster created in between must not touch the catalog either: the catalog is
    // service data, not per-account state.
    emr.ok(
        "RunJobFlow",
        json!({
            "Name": "catalog-is-not-account-state",
            "ReleaseLabel": release,
            "JobFlowRole": crate::job_flow_lifecycle::INSTANCE_PROFILE, "ServiceRole": crate::job_flow_lifecycle::SERVICE_ROLE,
            "Instances": { "MasterInstanceType": "m5.xlarge", "InstanceCount": 1 },
        }),
    )
    .await;
    let second = emr.all_supported(release).await;
    assert_eq!(
        types(&first),
        types(&second),
        "case {release:?}: two reads of the catalog must report the same types"
    );
    assert_eq!(
        first, second,
        "case {release:?}: two reads of the catalog must report the same rows"
    );
}

/// Every release the catalog offers answers a non-empty list, and within one major
/// series the supported set only GROWS with the release: a family a release supports is
/// supported by every later release of that series.
///
/// The cases are the WHOLE space of labels in a bounded box — every
/// `emr-<major>.<minor>.<patch>` for `major 5..=7`, `minor 0..=40`, `patch 0..=3` — with
/// the expectation taken from the service's own release catalog
/// (`state::catalog_release_labels`, the one `DescribeReleaseLabel` reads) rather than
/// from a second hand-written list: a label the catalog offers must answer 200, every
/// other label in the box must be refused 400 `InvalidRequestException`.
#[tokio::test]
async fn a_later_release_of_a_series_supports_at_least_as_much() {
    let emr = Emr::new();
    let offered: BTreeSet<String> = catalog_release_labels().into_iter().collect();
    assert!(
        offered.len() > 10,
        "the service's release catalog must offer the Amazon EMR releases, got {offered:?}"
    );
    let mut checked = 0usize;

    for major in 5..=7u32 {
        let mut previous: Option<(String, BTreeSet<String>)> = None;
        let mut offered_in_series = 0usize;
        for minor in 0..=40u32 {
            for patch in 0..=3u32 {
                let label = format!("emr-{major}.{minor}.{patch}");
                let (status, payload) = emr.page(&label, None).await;
                if !offered.contains(&label) {
                    assert_refused(&label, status, &payload);
                    continue;
                }
                assert_eq!(
                    status, 200,
                    "case {label:?}: the release catalog offers it, so it must answer 200, got \
                     {status} with {payload}"
                );
                offered_in_series += 1;
                checked += 1;
                let supported: BTreeSet<String> =
                    types(&emr.all_supported(&label).await).into_iter().collect();
                assert!(
                    !supported.is_empty(),
                    "case {label:?}: an offered release supports at least one instance type"
                );
                if let Some((earlier_label, earlier)) = &previous {
                    assert!(
                        earlier.is_subset(&supported),
                        "case {label:?}: must support everything {earlier_label:?} does, missing \
                         {:?}",
                        earlier.difference(&supported).collect::<Vec<_>>()
                    );
                }
                previous = Some((label, supported));
            }
        }
        assert!(
            offered_in_series > 0,
            "case major {major}: the emr-{major}.x series must offer at least one release"
        );
    }

    assert_eq!(
        checked,
        offered.len(),
        "every offered release must fall inside the walked box, got {checked} of {}",
        offered.len()
    );
}
