// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeFpgaImages`, driven over the wire the service actually
//! speaks — `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. EVERY AFI "AVAILABLE TO YOU" IS REPORTED AS THE STORE HOLDS IT, and no other: "public
//!    AFIs, private AFIs that you own, and AFIs owned by other Amazon Web Services accounts
//!    for which you have load permissions" (`DescribeFpgaImages`) — each member read off the
//!    stored AFI, with the state the clock has reached and the `Public` the AFI's load
//!    permissions make it; an account whose store holds none is reported as holding none
//!    rather than refused;
//! 2. EACH NARROWING SELECTS EXACTLY THE AFIs WHOSE ATTRIBUTE IT NAMES: `FpgaImageIds`,
//!    `Owners` and every documented filter, the values of one filter joined with an OR and
//!    the filters with an AND, an attribute no stored AFI carries selecting nothing;
//! 3. THE REPORT IS PAGED IN `FpgaImageId` ORDER: `MaxResults` bounds the page, the
//!    `NextToken` resumes after it, the last page carries none, and the pages of a walk are
//!    the whole selected set once each;
//! 4. A REQUEST THAT CANNOT BE SERVED IS REFUSED WITH EC2'S OWN CODE: an `FpgaImageIds` entry
//!    this report cannot resolve to an AFI the caller can see is one answer —
//!    `InvalidFpgaImageID.Malformed`, naming the id — whether it is no AFI id at all, a
//!    well-formed one naming nothing stored, or another account's private AFI; and an
//!    undocumented filter, a token this service did not mint, an owner term that is neither
//!    an account nor a documented alias, a page size outside the modelled range, and a
//!    `DryRun` that reports whether the call would have worked are each refused with EC2's own
//!    code for their own fault;
//! 5. A DESCRIBE IS A READ: two calls answer the same report and the store is as it was.
//!
//! Each table row is a distinct input along what the rule generalises over — which member
//! narrows, which attribute, which value form, how many values, which page size, which
//! refusal — so no row is the specification.
//!
//! The AFIs the rows read are SEEDED rather than created over the wire, for the reason
//! [`super::fpga_image_tasks_and_volume_io::seed_fpga_image`] records: `CreateFpgaImage`
//! reads the design checkpoint out of Amazon S3 and this emulator holds no Amazon S3 object
//! for any location to name, so it mints no AFI.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use ec2::ops::types::{LoadPermission, ProductCode, Tag};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, EXECUTABLE_BY_ALL, FPGA_IMAGE_STATE_FAILED,
    FPGA_IMAGE_STATE_PENDING,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, error_message, filters, indexed};
use super::fpga_image_tasks_and_volume_io::seed_fpga_image;
use super::volume_lifecycle::{REGION, p};

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// One row of a narrowing table: `(case, the parameters, the AFI ids it must select, in the
/// order they must be reported)`.
type NarrowingRow = (String, Params, Vec<String>);

/// One row of a refusal table: `(case, the parameters, the HTTP status, the error code)`.
type RefusalRow = (&'static str, Params, u16, &'static str);

/// The HTTP status EC2 answers a `DryRun` request with: 412, carrying `DryRunOperation`.
const DRY_RUN_STATUS: u16 = 412;

/// A well-formed AFI id, in the long (17 hex digit) form EC2 mints today, that the account
/// can see no AFI for.
const UNKNOWN_FPGA_IMAGE_ID: &str = "afi-0123456789abcdef0";

/// Another account, for the AFIs this suite's caller does not own.
const OTHER_ACCOUNT: &str = "210987654321";

/// The smallest and the largest page the modelled `DescribeFpgaImagesMaxResults` range
/// admits — "range 5..1000"
/// (`smithy_query(query="operation",operation="DescribeFpgaImages")`).
const MIN_MAX_RESULTS: i64 = 5;
const MAX_MAX_RESULTS: i64 = 1000;

/// One `action` call signed for the Region the suite works in.
async fn op(router: &MultiServiceRouter, action: &str, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    op(router, "DescribeFpgaImages", params).await
}

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// One AFI a `DescribeFpgaImages` answer reported: its direct members as the text the
/// response carried for each, and its tags and product codes as the entries they were
/// reported in.
#[derive(Debug, Clone, Default)]
struct ReportedFpgaImage {
    members: BTreeMap<String, String>,
    state_code: Option<String>,
    tags: Vec<(String, String)>,
    product_codes: Vec<String>,
}

impl ReportedFpgaImage {
    /// The text of the member `name`, or the empty string when the response reported none.
    fn member(&self, name: &str) -> &str {
        self.members.get(name).map_or("", String::as_str)
    }

    /// This AFI's tags as `(key, value)` pairs in key order, so a row compares against a set
    /// rather than against one reporting order.
    fn sorted_tags(&self) -> Vec<(String, String)> {
        let mut tags = self.tags.clone();
        tags.sort();
        tags
    }
}

/// Every AFI a `DescribeFpgaImages` answer reported, in the order it reported them.
///
/// Read through the XML tree rather than by substring, so a member of the AFI is never
/// confused with one of the same name nested inside a tag or a product code.
fn described_fpga_images(case: &str, body: &str) -> Vec<ReportedFpgaImage> {
    let document = super::wire::document(case, body);
    let mut reported_images = Vec::new();
    for item in super::wire::set_items(document.root_element(), "fpgaImageSet") {
        let mut reported = ReportedFpgaImage::default();
        for member in item.children().filter(roxmltree::Node::is_element) {
            match member.tag_name().name() {
                // The AFI's tag list reaches the wire as `<tags>` (`FpgaImage$Tags`'s
                // `xmlName`), not as the `<tagSet>` other EC2 resources carry.
                "tags" => {
                    for tag in member.children().filter(roxmltree::Node::is_element) {
                        reported.tags.push((
                            super::wire::child_text(tag, "key").unwrap_or_default(),
                            super::wire::child_text(tag, "value").unwrap_or_default(),
                        ));
                    }
                }
                "productCodes" => {
                    for code in member.children().filter(roxmltree::Node::is_element) {
                        reported
                            .product_codes
                            .push(super::wire::child_text(code, "productCode").unwrap_or_default());
                    }
                }
                "state" => {
                    reported.state_code = super::wire::child_text(member, "code");
                }
                name => {
                    reported.members.insert(
                        name.to_string(),
                        member.text().unwrap_or_default().to_string(),
                    );
                }
            }
        }
        reported_images.push(reported);
    }
    reported_images
}

/// The AFI ids a successful `DescribeFpgaImages` call reported, in report order.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeFpgaImages, got {status} with body {body}"
    );
    described_fpga_images(case, &body)
        .iter()
        .map(|fpga_image| fpga_image.member("fpgaImageId").to_string())
        .collect()
}

/// One AFI this suite seeds, and what a report of it must say.
#[derive(Debug, Clone, Copy)]
struct Seed {
    case: &'static str,
    name: &'static str,
    /// The owning account; `None` for the calling account.
    owner: Option<&'static str>,
    /// "The alias of the AFI owner" (`FpgaImage$OwnerAlias`), for an AFI a publisher owns.
    alias: Option<&'static str>,
    tags: &'static [(&'static str, &'static str)],
    product_codes: &'static [&'static str],
    shell_version: Option<&'static str>,
    /// A load permission naming the CALLING account, which is what gives another account's
    /// private AFI to this caller.
    shared_with_caller: bool,
    /// The `all` group in the load permissions, which is what makes an AFI public.
    public: bool,
    /// Whether the minting outcome the clock owes this AFI has already fallen due.
    resolved: bool,
}

/// The AFIs every table reads, differing in owner, alias, tags, product codes, shell
/// version, load permissions and how far their minting has got — the attributes the
/// narrowings select on. The last row is the one NO narrowing may reach: another account's
/// private AFI, which this caller may not see at all.
const SEEDS: [Seed; 5] = [
    Seed {
        case: "the caller's own AFI, still minting, with two tags",
        name: "edge-detect-v3",
        owner: None,
        alias: None,
        tags: &[("Owner", "TeamA"), ("Stage", "prod")],
        product_codes: &[],
        shell_version: None,
        shared_with_caller: false,
        public: false,
        resolved: false,
    },
    Seed {
        case: "the caller's own AFI whose minting outcome fell due",
        name: "video-encode-v1",
        owner: None,
        alias: None,
        tags: &[("Owner", "TeamB")],
        product_codes: &["abcdef0123456789abcdef012"],
        shell_version: Some("0x071417d3"),
        shared_with_caller: false,
        public: false,
        resolved: true,
    },
    Seed {
        case: "a Marketplace AFI every account may load",
        name: "market-cipher",
        owner: Some(OTHER_ACCOUNT),
        alias: Some("aws-marketplace"),
        tags: &[],
        product_codes: &["0123456789abcdef0123456789"],
        shell_version: Some("0x04261818"),
        shared_with_caller: false,
        public: true,
        resolved: true,
    },
    Seed {
        case: "another account's AFI shared with the caller",
        name: "shared-fir",
        owner: Some(OTHER_ACCOUNT),
        alias: None,
        tags: &[("Stage", "test")],
        product_codes: &[],
        shell_version: None,
        shared_with_caller: true,
        public: false,
        resolved: false,
    },
    Seed {
        case: "another account's private AFI, which this caller may not see",
        name: "not-yours",
        owner: Some(OTHER_ACCOUNT),
        alias: None,
        tags: &[("Owner", "TeamA")],
        product_codes: &[],
        shell_version: None,
        shared_with_caller: false,
        public: false,
        resolved: false,
    },
];

/// Store the AFI `seed` describes and answer its id and its global id.
///
/// The record is laid down by the one helper this crate's FPGA-image suites share and then
/// given the attributes the seed names, the way the operations that write them leave it: the
/// owner a mint records, the tags a create asked for, the load permissions
/// `ModifyFpgaImageAttribute` grants, and the product codes a Marketplace purchase carries.
fn seed(engine: &engine::InMemoryEngine, spec: &Seed) -> (String, String) {
    let account = engine::emulator_account_id().to_string();
    let (fpga_image_id, fpga_image_global_id) = seed_fpga_image(engine, spec.name);
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let record = guard
        .fpga_images
        .get_mut(&fpga_image_id)
        .expect("the seeded AFI is stored");
    record.view.owner_id = Some(spec.owner.unwrap_or(&account).to_string());
    record.view.owner_alias = spec.alias.map(str::to_string);
    record.view.shell_version = spec.shell_version.map(str::to_string);
    record.view.tags = Some(
        spec.tags
            .iter()
            .map(|(key, value)| Tag {
                key: Some((*key).to_string()),
                value: Some((*value).to_string()),
            })
            .collect(),
    );
    record.view.product_codes = Some(
        spec.product_codes
            .iter()
            .map(|code| ProductCode {
                product_code_id: Some((*code).to_string()),
                product_code_type: Some("marketplace".to_string()),
            })
            .collect(),
    );
    let mut permissions: Vec<LoadPermission> = Vec::new();
    if spec.shared_with_caller {
        permissions.push(LoadPermission {
            user_id: Some(account.clone()),
            group: None,
        });
    }
    if spec.public {
        permissions.push(LoadPermission {
            user_id: None,
            // The group that makes a resource public, read by the name the handler reads it
            // by rather than written out a second time.
            group: Some(EXECUTABLE_BY_ALL.to_string()),
        });
    }
    record.load_permissions = permissions;
    if spec.resolved {
        // The record as the clock left it once the minting outcome fell due, without waiting
        // out the span: the due instant is what every report derives the state from.
        record.resolved_at = Instant::now();
    }
    (fpga_image_id, fpga_image_global_id)
}

/// What seeding [`SEEDS`] produced: the id and the global id of each row, in seed order.
#[derive(Debug, Clone)]
struct Seeded {
    ids: Vec<String>,
    global_ids: Vec<String>,
}

impl Seeded {
    /// The ids of the rows `rows` names, in that order.
    fn ids_at(&self, rows: &[usize]) -> Vec<String> {
        rows.iter().map(|row| self.ids[*row].clone()).collect()
    }
}

/// Store every row of [`SEEDS`]. The ids are minted from the store's own monotonic counter,
/// so seed order IS `FpgaImageId` order — which is the order the report pages in.
fn seed_all(engine: &engine::InMemoryEngine) -> Seeded {
    let mut seeded = Seeded {
        ids: Vec::new(),
        global_ids: Vec::new(),
    };
    for spec in &SEEDS {
        let (fpga_image_id, global_id) = seed(engine, spec);
        seeded.ids.push(fpga_image_id);
        seeded.global_ids.push(global_id);
    }
    seeded
}

/// The rows of [`SEEDS`] this caller may see, in `FpgaImageId` order: its own two AFIs, the
/// public one and the one shared with it — every row but the last.
const VISIBLE_ROWS: [usize; 4] = [0, 1, 2, 3];

/// The row of [`SEEDS`] no report may carry.
const INVISIBLE_ROW: usize = 4;

/// The `FpgaImage$OwnerAlias` the published AFI catalog carries: an AFI Amazon itself
/// published, which every account can already see.
const PUBLISHED_OWNER_ALIAS: &str = "amazon";

/// The `FpgaImageStateCode` a published AFI is in: ready for use, with nothing owed on it.
const FPGA_IMAGE_STATE_AVAILABLE: &str = "available";

/// Read the published AFI catalog out of a report of it, and answer its ids in the order the
/// report carried them.
///
/// The catalog is what AWS publishes rather than what this account created: it is reported
/// beside the account's own AFIs and is NOT in the account's own AFI store, which this also
/// pins — a report writes nothing.
async fn published_catalog_ids(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
) -> Vec<String> {
    let (status, body) = describe(router, &[]).await;
    assert_eq!(
        status, 200,
        "reading the published catalog: expected HTTP 200, got {status} with body {body}"
    );
    let catalog: Vec<String> = described_fpga_images("the published catalog", &body)
        .iter()
        .filter(|fpga_image| fpga_image.member("ownerAlias") == PUBLISHED_OWNER_ALIAS)
        .map(|fpga_image| fpga_image.member("fpgaImageId").to_string())
        .collect();
    assert!(
        !catalog.is_empty(),
        "the account can already see the public AFIs AWS publishes, got {body}"
    );
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    for fpga_image_id in &catalog {
        assert!(
            !guard.fpga_images.contains_key(fpga_image_id),
            "a published AFI is not an AFI of the account: {fpga_image_id} must not be in the \
             account's own store, which holds {:?}",
            guard.fpga_images.keys().collect::<Vec<_>>()
        );
    }
    catalog
}

/// Every AFI a report must carry for a store holding [`SEEDS`]: the rows this caller may see,
/// then the published catalog, which is the `FpgaImageId` order they are reported in (a
/// published AFI's id is a fixed published identifier, and sorts after the ids this store
/// mints from its own counter).
fn visible_then_catalog(seeded: &Seeded, catalog: &[String]) -> Vec<String> {
    let mut expected = seeded.ids_at(&VISIBLE_ROWS);
    expected.extend(catalog.iter().cloned());
    expected
}

/// The timestamp the store holds for the AFI `fpga_image_id`, rendered as the wire carries
/// it — so a `create-time` or `update-time` filter row is written against what the service
/// itself reported rather than against a clock read twice.
fn stored_timestamp(engine: &engine::InMemoryEngine, fpga_image_id: &str, member: &str) -> String {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    let record = guard
        .fpga_images
        .get(fpga_image_id)
        .expect("the seeded AFI is stored");
    let at = match member {
        "create-time" => record.view.create_time,
        "update-time" => record.view.update_time,
        other => panic!("{other} is not a timestamp member of an AFI"),
    }
    .expect("a seeded AFI carries both of its timestamps");
    at.fmt(::aws_smithy_types::date_time::Format::DateTime)
        .expect("a stored timestamp renders")
}

/// RULE 1: an unfiltered describe reports every AFI the caller may see, in `FpgaImageId`
/// order, with each member read off the stored AFI — and never the AFI the caller may not
/// see.
#[tokio::test]
async fn every_available_afi_is_reported_as_the_store_holds_it() {
    let (router, engine) = emulator();
    let seeded = seed_all(&engine);
    let account = engine::emulator_account_id();
    let catalog = published_catalog_ids(&router, &engine).await;

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "an unfiltered describe: expected HTTP 200, got {status} with body {body}"
    );
    let reported = described_fpga_images("every available AFI", &body);
    let reported_ids: Vec<String> = reported
        .iter()
        .map(|fpga_image| fpga_image.member("fpgaImageId").to_string())
        .collect();
    assert_eq!(
        reported_ids,
        visible_then_catalog(&seeded, &catalog),
        "an unfiltered describe reports every AFI available to the caller in id order, got \
         {reported_ids:?}"
    );
    assert!(
        !reported_ids.contains(&seeded.ids[INVISIBLE_ROW]),
        "case {:?}: expected it NOT to be reported, got {reported_ids:?}",
        SEEDS[INVISIBLE_ROW].case
    );

    // Every entry of the published catalog is public, ready for use and aliased to its
    // publisher — the three facts that make it one of the "public AFIs" this report carries.
    for fpga_image in reported
        .iter()
        .filter(|fpga_image| catalog.contains(&fpga_image.member("fpgaImageId").to_string()))
    {
        let case = format!("the published AFI {}", fpga_image.member("name"));
        assert_eq!(
            fpga_image.member("public"),
            "true",
            "case {case:?}: expected a published AFI to be public, got {:?}",
            fpga_image.members
        );
        assert_eq!(
            fpga_image.state_code.as_deref(),
            Some(FPGA_IMAGE_STATE_AVAILABLE),
            "case {case:?}: expected a published AFI to be available, got {:?}",
            fpga_image.state_code
        );
        assert_eq!(
            fpga_image.member("ownerAlias"),
            PUBLISHED_OWNER_ALIAS,
            "case {case:?}: expected the publisher's alias, got {:?}",
            fpga_image.members
        );
        assert!(
            !fpga_image.member("shellVersion").is_empty()
                && !fpga_image.member("name").is_empty()
                && !fpga_image.member("description").is_empty(),
            "case {case:?}: expected a published AFI to name its shell version, its name and \
             its description, got {:?}",
            fpga_image.members
        );
    }

    for (row, fpga_image) in VISIBLE_ROWS.iter().zip(reported.iter()) {
        let spec = &SEEDS[*row];
        let case = spec.case;
        assert_eq!(
            fpga_image.member("fpgaImageId"),
            seeded.ids[*row],
            "case {case:?}: expected the stored AFI id, got {:?}",
            fpga_image.members
        );
        assert_eq!(
            fpga_image.member("fpgaImageGlobalId"),
            seeded.global_ids[*row],
            "case {case:?}: expected the stored global id, got {:?}",
            fpga_image.members
        );
        assert_eq!(
            fpga_image.member("name"),
            spec.name,
            "case {case:?}: expected the stored name, got {:?}",
            fpga_image.members
        );
        assert_eq!(
            fpga_image.member("description"),
            format!("the AFI {}", spec.name),
            "case {case:?}: expected the stored description, got {:?}",
            fpga_image.members
        );
        assert_eq!(
            fpga_image.member("ownerId"),
            spec.owner.unwrap_or(account),
            "case {case:?}: expected the stored owner, got {:?}",
            fpga_image.members
        );
        assert_eq!(
            fpga_image.members.get("ownerAlias").map(String::as_str),
            spec.alias,
            "case {case:?}: expected the stored owner alias, got {:?}",
            fpga_image.members
        );
        assert_eq!(
            fpga_image.members.get("shellVersion").map(String::as_str),
            spec.shell_version,
            "case {case:?}: expected the stored shell version, got {:?}",
            fpga_image.members
        );
        // The state is the one the CLOCK has reached, derived from the stored due instant.
        let expected_state = if spec.resolved {
            FPGA_IMAGE_STATE_FAILED
        } else {
            FPGA_IMAGE_STATE_PENDING
        };
        assert_eq!(
            fpga_image.state_code.as_deref(),
            Some(expected_state),
            "case {case:?}: expected the {expected_state} state, got {:?}",
            fpga_image.state_code
        );
        // "Indicates whether the AFI is public" (`FpgaImage$Public`) is what the AFI's load
        // permissions make it: the `all` group, and nothing else, is public.
        assert_eq!(
            fpga_image.member("public"),
            if spec.public { "true" } else { "false" },
            "case {case:?}: expected public={}, got {:?}",
            spec.public,
            fpga_image.members
        );
        let expected_tags: Vec<(String, String)> = {
            let mut tags: Vec<(String, String)> = spec
                .tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect();
            tags.sort();
            tags
        };
        assert_eq!(
            fpga_image.sorted_tags(),
            expected_tags,
            "case {case:?}: expected the stored tags, got {:?}",
            fpga_image.tags
        );
        let expected_codes: Vec<String> = spec
            .product_codes
            .iter()
            .map(|code| (*code).to_string())
            .collect();
        assert_eq!(
            fpga_image.product_codes, expected_codes,
            "case {case:?}: expected the stored product codes, got {:?}",
            fpga_image.product_codes
        );
        assert!(
            super::ec2_query::as_timestamp(fpga_image.member("createTime")).is_some()
                && super::ec2_query::as_timestamp(fpga_image.member("updateTime")).is_some(),
            "case {case:?}: expected both timestamps as ISO 8601 instants, got {:?}",
            fpga_image.members
        );
    }
}

/// RULE 1: an account that has created no AFI of its own still sees the public AFIs AWS
/// publishes, and nothing else — reported as a result, never as a refusal, with no
/// `NextToken` while nothing follows the page.
#[tokio::test]
async fn an_account_that_created_no_afi_sees_the_published_catalog() {
    let (router, engine) = emulator();

    let catalog = published_catalog_ids(&router, &engine).await;
    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "an account with no AFI of its own: expected HTTP 200, got {status} with body {body}"
    );
    let reported = described_fpga_images("an account with no AFI of its own", &body);
    let reported_ids: Vec<String> = reported
        .iter()
        .map(|fpga_image| fpga_image.member("fpgaImageId").to_string())
        .collect();
    assert_eq!(
        reported_ids, catalog,
        "an account with no AFI of its own sees exactly the published catalog, got \
         {reported_ids:?}"
    );
    for fpga_image in &reported {
        let case = format!("the published AFI {}", fpga_image.member("name"));
        assert_eq!(
            fpga_image.member("public"),
            "true",
            "case {case:?}: expected it to be public, got {:?}",
            fpga_image.members
        );
        assert_eq!(
            fpga_image.state_code.as_deref(),
            Some(FPGA_IMAGE_STATE_AVAILABLE),
            "case {case:?}: expected it to be available, got {:?}",
            fpga_image.state_code
        );
    }
    assert_eq!(
        element(&body, "nextToken"),
        None,
        "a page nothing follows: expected no NextToken, got {body}"
    );
}

/// RULE 2: `FpgaImageIds` and `Owners` select exactly the AFIs they name, each list joined
/// with an OR inside itself and with an AND against the rest of the request.
#[tokio::test]
async fn the_id_list_and_the_owners_narrow_the_report() {
    let (router, engine) = emulator();
    let seeded = seed_all(&engine);
    let account = engine::emulator_account_id();
    let catalog = published_catalog_ids(&router, &engine).await;

    let mut rows: Vec<NarrowingRow> = vec![
        (
            "one id".to_string(),
            indexed("FpgaImageId", &[seeded.ids[1].as_str()]),
            seeded.ids_at(&[1]),
        ),
        (
            "three ids, named out of order".to_string(),
            indexed(
                "FpgaImageId",
                &[
                    seeded.ids[3].as_str(),
                    seeded.ids[0].as_str(),
                    seeded.ids[2].as_str(),
                ],
            ),
            seeded.ids_at(&[0, 2, 3]),
        ),
        (
            "self, which is the calling account".to_string(),
            indexed("Owner", &["self"]),
            seeded.ids_at(&[0, 1]),
        ),
        (
            "the calling account by id".to_string(),
            indexed("Owner", &[account]),
            seeded.ids_at(&[0, 1]),
        ),
        (
            "another account by id, which selects only what the caller may see".to_string(),
            indexed("Owner", &[OTHER_ACCOUNT]),
            seeded.ids_at(&[2, 3]),
        ),
        (
            "the aws-marketplace alias".to_string(),
            indexed("Owner", &["aws-marketplace"]),
            seeded.ids_at(&[2]),
        ),
        (
            "the publisher's amazon alias, which the published catalog carries".to_string(),
            indexed("Owner", &[PUBLISHED_OWNER_ALIAS]),
            catalog.clone(),
        ),
        (
            "two owner terms, joined with an OR".to_string(),
            indexed("Owner", &["self", "aws-marketplace"]),
            seeded.ids_at(&[0, 1, 2]),
        ),
    ];
    // An id list and an owner list in one request are joined with an AND.
    let mut both = indexed("FpgaImageId", &[seeded.ids[1].as_str(), seeded.ids[2].as_str()]);
    both.extend(indexed("Owner", &["self"]));
    rows.push((
        "an id list and an owner list together".to_string(),
        both,
        seeded.ids_at(&[1]),
    ));

    for (case, params, expected) in rows {
        let reported = described_ids(&router, &case, &params).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// The filter names `DescribeFpgaImagesRequest$Filters` documents:
///
/// ```text
/// smithy_query(query="doc",shape="DescribeFpgaImagesRequest",member="Filters",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
/// ```
///
/// `tag:<key>` is the one name with a variable part, so it stands for the whole family.
/// [`every_documented_filter_narrows_the_report`] pins that each of these is exercised, so a
/// filter this suite stops covering fails it rather than going untested.
const DOCUMENTED_FILTERS: [&str; 11] = [
    "create-time",
    "fpga-image-global-id",
    "fpga-image-id",
    "name",
    "owner-id",
    "product-code",
    "shell-version",
    "state",
    "tag:",
    "tag-key",
    "update-time",
];

/// RULE 2: every documented filter selects exactly the AFIs whose attribute it names, with
/// the values of one filter joined with an OR, EC2's wildcards honoured, and two filters
/// joined with an AND.
#[tokio::test]
async fn every_documented_filter_narrows_the_report() {
    let (router, engine) = emulator();
    let seeded = seed_all(&engine);
    let account = engine::emulator_account_id();
    let catalog = published_catalog_ids(&router, &engine).await;
    let created_at = stored_timestamp(&engine, &seeded.ids[0], "create-time");
    let updated_at = stored_timestamp(&engine, &seeded.ids[1], "update-time");

    // `(case, the filter name, its values, the ids it must select)`. Every row names a
    // filter of [`DOCUMENTED_FILTERS`]; the cases differ along value form (exact, wildcard),
    // value count (one, two) and what the attribute is (a timestamp, an id, a word, a tag).
    let filter_rows: Vec<(&str, &str, Vec<String>, Vec<String>)> = vec![
        (
            "the creation time of the first AFI",
            "create-time",
            vec![created_at.clone()],
            seeded.ids_at(&[0]),
        ),
        (
            "a creation time no AFI carries",
            "create-time",
            vec!["2001-02-03T04:05:06Z".to_string()],
            Vec::new(),
        ),
        (
            "the most recent update of the second AFI",
            "update-time",
            vec![updated_at.clone()],
            seeded.ids_at(&[1]),
        ),
        (
            "every creation time of this account's own AFIs, by wildcard",
            "create-time",
            vec![format!("{}*", &created_at[..10])],
            seeded.ids_at(&VISIBLE_ROWS),
        ),
        (
            "one global id",
            "fpga-image-global-id",
            vec![seeded.global_ids[2].clone()],
            seeded.ids_at(&[2]),
        ),
        (
            "every global id this service mints, by wildcard",
            "fpga-image-global-id",
            vec!["agfi-*".to_string()],
            visible_then_catalog(&seeded, &catalog),
        ),
        (
            "one AFI id",
            "fpga-image-id",
            vec![seeded.ids[3].clone()],
            seeded.ids_at(&[3]),
        ),
        (
            "an AFI id the caller may not see",
            "fpga-image-id",
            vec![seeded.ids[INVISIBLE_ROW].clone()],
            Vec::new(),
        ),
        (
            "one name, exactly",
            "name",
            vec!["video-encode-v1".to_string()],
            seeded.ids_at(&[1]),
        ),
        (
            "two names, joined with an OR",
            "name",
            vec!["video-encode-v1".to_string(), "market-cipher".to_string()],
            seeded.ids_at(&[1, 2]),
        ),
        (
            "two names matched by one wildcard",
            "name",
            vec!["*-v?".to_string()],
            seeded.ids_at(&[0, 1]),
        ),
        (
            "a name matched by a prefix wildcard",
            "name",
            vec!["video-*".to_string()],
            seeded.ids_at(&[1]),
        ),
        (
            "the calling account as owner",
            "owner-id",
            vec![account.to_string()],
            seeded.ids_at(&[0, 1]),
        ),
        (
            "another account as owner",
            "owner-id",
            vec![OTHER_ACCOUNT.to_string()],
            seeded.ids_at(&[2, 3]),
        ),
        (
            "one product code",
            "product-code",
            vec!["abcdef0123456789abcdef012".to_string()],
            seeded.ids_at(&[1]),
        ),
        (
            "two product codes, joined with an OR",
            "product-code",
            vec![
                "abcdef0123456789abcdef012".to_string(),
                "0123456789abcdef0123456789".to_string(),
            ],
            seeded.ids_at(&[1, 2]),
        ),
        (
            "one shell version",
            "shell-version",
            vec!["0x071417d3".to_string()],
            seeded.ids_at(&[1]),
        ),
        (
            "a shell version no AFI carries",
            "shell-version",
            vec!["0x00000000".to_string()],
            Vec::new(),
        ),
        (
            "the pending state, which the clock has not moved past",
            "state",
            vec![FPGA_IMAGE_STATE_PENDING.to_string()],
            seeded.ids_at(&[0, 3]),
        ),
        (
            "the failed state the clock has reached",
            "state",
            vec![FPGA_IMAGE_STATE_FAILED.to_string()],
            seeded.ids_at(&[1, 2]),
        ),
        (
            "the available state the published catalog is in",
            "state",
            vec![FPGA_IMAGE_STATE_AVAILABLE.to_string()],
            catalog.clone(),
        ),
        (
            "two states, joined with an OR",
            "state",
            vec![
                FPGA_IMAGE_STATE_PENDING.to_string(),
                FPGA_IMAGE_STATE_FAILED.to_string(),
            ],
            seeded.ids_at(&VISIBLE_ROWS),
        ),
        (
            "one tag's value",
            "tag:Owner",
            vec!["TeamA".to_string()],
            seeded.ids_at(&[0]),
        ),
        (
            "two values of one tag, joined with an OR",
            "tag:Owner",
            vec!["TeamA".to_string(), "TeamB".to_string()],
            seeded.ids_at(&[0, 1]),
        ),
        (
            "a tag key, whatever its value",
            "tag-key",
            vec!["Stage".to_string()],
            seeded.ids_at(&[0, 3]),
        ),
        (
            "a tag key no AFI carries",
            "tag-key",
            vec!["Absent".to_string()],
            Vec::new(),
        ),
    ];

    let mut covered: BTreeSet<&str> = BTreeSet::new();
    for (case, name, values, expected) in &filter_rows {
        covered.insert(if name.starts_with("tag:") { "tag:" } else { name });
        let value_refs: Vec<&str> = values.iter().map(String::as_str).collect();
        let params = filters(&[(name, value_refs.as_slice())]);
        let reported = described_ids(&router, case, &params).await;
        assert_eq!(
            &reported, expected,
            "case {case:?} on the {name} filter: expected {expected:?}, got {reported:?}"
        );
    }
    let documented: BTreeSet<&str> = DOCUMENTED_FILTERS.iter().copied().collect();
    assert_eq!(
        covered, documented,
        "every documented filter must be exercised, got {covered:?} against {documented:?}"
    );

    // Two filters in one request are joined with an AND, so one that agrees narrows and one
    // that disagrees selects nothing.
    let and_rows: Vec<NarrowingRow> = vec![
        (
            "two filters that agree".to_string(),
            filters(&[("tag:Owner", &["TeamA"]), ("name", &["edge-detect-v3"])]),
            seeded.ids_at(&[0]),
        ),
        (
            "two filters that disagree".to_string(),
            filters(&[("tag:Owner", &["TeamA"]), ("name", &["market-cipher"])]),
            Vec::new(),
        ),
        (
            "a filter and an id list that disagree".to_string(),
            {
                let mut params = filters(&[("name", &["market-cipher"])]);
                params.extend(indexed("FpgaImageId", &[seeded.ids[0].as_str()]));
                params
            },
            Vec::new(),
        ),
    ];
    for (case, params, expected) in and_rows {
        let reported = described_ids(&router, &case, &params).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// RULE 3: the walk covers the selected set EXACTLY ONCE in `FpgaImageId` order for every
/// page size, the last page carries no token, and a page is cut from the FILTERED set — so
/// `MaxResults`, a filter and the token compose.
#[tokio::test]
async fn paging_walks_the_selected_set_exactly_once() {
    let (router, engine) = emulator();

    // More AFIs than the smallest page the model admits, so a walk takes several pages and
    // the last one is a remainder. Every other AFI is tagged, which is the filterable split.
    let mut all_ids: Vec<String> = Vec::new();
    let mut tagged: Vec<String> = Vec::new();
    for nth in 0..13usize {
        let (fpga_image_id, _) = seed_fpga_image(&engine, &format!("paged-afi-{nth:02}"));
        if nth % 2 == 0 {
            let store = engine.state::<Ec2State>();
            let mut guard = store.lock();
            let record = guard
                .fpga_images
                .get_mut(&fpga_image_id)
                .expect("the seeded AFI is stored");
            record.view.tags = Some(vec![Tag {
                key: Some("Paged".to_string()),
                value: Some("yes".to_string()),
            }]);
            drop(guard);
            tagged.push(fpga_image_id.clone());
        }
        all_ids.push(fpga_image_id);
    }
    // The published catalog is part of every report, so it is part of the set a walk must
    // cover; its ids are minted after the seeded ones, so it sorts after them.
    all_ids.extend(published_catalog_ids(&router, &engine).await);

    // `(case, the page size, the extra narrowing, the set the walk must cover)`.
    let walks: Vec<(&str, i64, Params, Vec<String>)> = vec![
        (
            "the smallest page the model admits",
            MIN_MAX_RESULTS,
            Params::new(),
            all_ids.clone(),
        ),
        ("a page of seven", 7, Params::new(), all_ids.clone()),
        (
            "a page larger than the whole set",
            MAX_MAX_RESULTS,
            Params::new(),
            all_ids.clone(),
        ),
        (
            "a filtered walk",
            MIN_MAX_RESULTS,
            filters(&[("tag-key", &["Paged"])]),
            tagged.clone(),
        ),
    ];

    for (case, page_size, narrowing, expected) in walks {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut params = narrowing.clone();
            params.push(p("MaxResults", page_size.to_string()));
            if let Some(next) = token.as_deref() {
                params.push(p("NextToken", next));
            }
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: page {pages} expected HTTP 200, got {status} with body {body}"
            );
            let page: Vec<String> = described_fpga_images(case, &body)
                .iter()
                .map(|fpga_image| fpga_image.member("fpgaImageId").to_string())
                .collect();
            assert!(
                page.len() <= usize::try_from(page_size).expect("a page size fits a usize"),
                "case {case:?}: page {pages} holds {} AFIs, above the page size {page_size}",
                page.len()
            );
            walked.extend(page);
            token = element(&body, "nextToken");
            pages += 1;
            if token.is_none() {
                break;
            }
            assert!(
                pages < 20,
                "case {case:?}: the walk must terminate, got {pages} pages"
            );
        }
        assert_eq!(
            walked, expected,
            "case {case:?}: the walk must cover the selected set once each in id order, got \
             {walked:?}"
        );
    }
}

/// RULE 3 and RULE 4: each published bound of the modelled `MaxResults` range is served, and
/// one step outside either bound is refused before the handler runs.
#[tokio::test]
async fn max_results_is_served_at_its_bounds_and_refused_outside_them() {
    let (router, engine) = emulator();
    let seeded = seed_all(&engine);
    let catalog = published_catalog_ids(&router, &engine).await;
    let whole_set = visible_then_catalog(&seeded, &catalog);
    let smallest_page = usize::try_from(MIN_MAX_RESULTS).expect("a page size fits a usize");

    // `(case, the page size, the rows it must report, whether a token is due)`. The whole set
    // is larger than the smallest page the model admits, so that bound reports a page and a
    // token while the largest reports the set and none.
    let served: Vec<(&str, i64, Vec<String>, bool)> = vec![
        (
            "the published minimum, one page of the set",
            MIN_MAX_RESULTS,
            whole_set
                .iter()
                .take(smallest_page)
                .cloned()
                .collect::<Vec<String>>(),
            whole_set.len() > smallest_page,
        ),
        (
            "the published maximum, which holds the whole set",
            MAX_MAX_RESULTS,
            whole_set.clone(),
            false,
        ),
    ];
    for (case, page_size, expected, token_due) in served {
        let (status, body) = describe(&router, &[p("MaxResults", page_size.to_string())]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let reported: Vec<String> = described_fpga_images(case, &body)
            .iter()
            .map(|fpga_image| fpga_image.member("fpgaImageId").to_string())
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
        assert_eq!(
            element(&body, "nextToken").is_some(),
            token_due,
            "case {case:?}: expected a token to be due: {token_due}, got {body}"
        );
    }

    // The modelled `@range` is enforced by the boundary validator in front of the handler: a
    // page size BELOW the smallest the operation serves carries EC2's own `InvalidMaxResults`,
    // while one above the largest keeps the validator's generic code.
    let outside: Vec<(&str, i64, &str)> = vec![
        ("one below the published minimum", MIN_MAX_RESULTS - 1, "InvalidMaxResults"),
        // A value that is no page size at all — zero or negative — is not a page size the
        // operation declines to serve but a parameter it cannot use, which carries EC2's own
        // `InvalidParameter` instead.
        ("a page of none", 0, "InvalidParameter"),
        ("one above the published maximum", MAX_MAX_RESULTS + 1, "ValidationError"),
    ];
    for (case, page_size, expected_code) in outside {
        let (status, body) = describe(&router, &[p("MaxResults", page_size.to_string())]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for MaxResults={page_size}, got {status} with \
             body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }
}

/// RULE 4: every request this operation cannot serve is refused with EC2's own code for the
/// cause, and an AFI id is resolved MALFORMED BEFORE NOT FOUND with `DryRun` answered
/// between them.
#[tokio::test]
async fn a_request_that_cannot_be_served_is_refused_with_ec2s_own_code() {
    let (router, engine) = emulator();
    let seeded = seed_all(&engine);

    let rows: Vec<RefusalRow> = vec![
        (
            "an id that is not an AFI id at all",
            indexed("FpgaImageId", &["not-an-afi"]),
            400,
            "InvalidFpgaImageID.Malformed",
        ),
        (
            "another resource's id",
            indexed("FpgaImageId", &["ami-0123456789abcdef0"]),
            400,
            "InvalidFpgaImageID.Malformed",
        ),
        (
            "the null AFI id",
            indexed("FpgaImageId", &["afi-00000000000000000"]),
            400,
            "InvalidFpgaImageID.Malformed",
        ),
        (
            "a well-formed id the account can see no AFI for",
            indexed("FpgaImageId", &[UNKNOWN_FPGA_IMAGE_ID]),
            400,
            "InvalidFpgaImageID.Malformed",
        ),
        (
            "the short id form, which nothing is stored under",
            indexed("FpgaImageId", &["afi-01234567"]),
            400,
            "InvalidFpgaImageID.Malformed",
        ),
        (
            "an undocumented filter",
            filters(&[("fpga-image-name", &["edge-detect-v3"])]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a tag filter with no key",
            filters(&[("tag:", &["TeamA"])]),
            400,
            "InvalidParameterValue",
        ),
        (
            "an owner term that is neither an account nor an alias",
            indexed("Owner", &["somebody"]),
            400,
            "InvalidUserID.Malformed",
        ),
        (
            "an owner term one digit short of an account id",
            indexed("Owner", &["21098765432"]),
            400,
            "InvalidUserID.Malformed",
        ),
        (
            "a token this service did not mint",
            vec![p("NextToken", "not-a-token")],
            400,
            "InvalidPaginationToken",
        ),
        (
            "a DryRun, which reports whether the call would have worked",
            vec![p("DryRun", "true")],
            DRY_RUN_STATUS,
            "DryRunOperation",
        ),
    ];

    for (case, params, expected_status, expected_code) in rows {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }

    // Every id this report cannot resolve is refused with ONE answer, in ONE wording, naming
    // the id the REQUEST carried: a value that is no AFI id, a well-formed one naming nothing
    // stored, and another account's private AFI the caller may not see.
    for (case, fpga_image_id) in [
        ("a value that is no AFI id", "not-an-afi".to_string()),
        ("an id nothing is stored under", UNKNOWN_FPGA_IMAGE_ID.to_string()),
        (
            "another account's private AFI",
            seeded.ids[INVISIBLE_ROW].clone(),
        ),
    ] {
        let (status, body) = describe(&router, &indexed("FpgaImageId", &[&fpga_image_id])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidFpgaImageID.Malformed",
            "case {case:?}: expected InvalidFpgaImageID.Malformed, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("Image ID '{fpga_image_id}' is invalid."),
            "case {case:?}: expected the refusal to name the id the request carried, got {body}"
        );
    }

    // The wording of the NOT-FOUND refusal belongs to the operations that act on ONE named
    // AFI: a well-formed id of this account that names nothing is what they answer it for, so
    // the two readings stay distinguishable.
    let (status, body) = op(
        &router,
        "DescribeFpgaImageAttribute",
        &[
            p("FpgaImageId", UNKNOWN_FPGA_IMAGE_ID),
            p("Attribute", "loadPermission"),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "describing one unknown AFI's attribute: expected HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidFpgaImageID.NotFound",
        "describing one unknown AFI's attribute: expected the not-found refusal, got {body}"
    );

    // `DryRun` asks whether the CALLER MAY make the request, so it is answered once the
    // request's own members have been read and BEFORE any AFI is resolved — but a request
    // whose own members are unreadable is refused for THAT reason instead.
    let mut dry_run_unknown = indexed("FpgaImageId", &[UNKNOWN_FPGA_IMAGE_ID]);
    dry_run_unknown.push(p("DryRun", "true"));
    let (status, body) = describe(&router, &dry_run_unknown).await;
    assert_eq!(
        status, DRY_RUN_STATUS,
        "a DryRun naming an unknown AFI: expected HTTP {DRY_RUN_STATUS}, got {status} with \
         body {body}"
    );
    let mut dry_run_malformed = indexed("FpgaImageId", &["not-an-afi"]);
    dry_run_malformed.push(p("DryRun", "true"));
    let (status, body) = describe(&router, &dry_run_malformed).await;
    assert_eq!(
        status, 400,
        "a DryRun naming a malformed AFI id: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidFpgaImageID.Malformed",
        "a DryRun naming a malformed AFI id: expected the malformed refusal, got {body}"
    );
}

/// RULE 5: describing twice answers the same report, and the store is as it was — the AFIs
/// it held, the states they were in and the load permissions they carried. The published
/// catalog is reported without being stored, so repeated reads report one catalog rather than
/// a growing one.
#[tokio::test]
async fn a_describe_changes_nothing_a_later_caller_sees() {
    let (router, engine) = emulator();
    let seeded = seed_all(&engine);
    let catalog = published_catalog_ids(&router, &engine).await;

    let before = {
        let store = engine.state::<Ec2State>();
        let guard = store.lock();
        guard
            .fpga_images
            .iter()
            .map(|(id, record)| {
                (
                    id.clone(),
                    record.load_permissions.len(),
                    record.view.tags.clone().unwrap_or_default().len(),
                )
            })
            .collect::<Vec<_>>()
    };

    let (status, first) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "the first describe: expected HTTP 200, got {status} with body {first}"
    );
    let (status, second) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "the second describe: expected HTTP 200, got {status} with body {second}"
    );
    let first_report = described_fpga_images("the first describe", &first);
    let second_report = described_fpga_images("the second describe", &second);
    let reported_ids: Vec<String> = second_report
        .iter()
        .map(|image| image.member("fpgaImageId").to_string())
        .collect();
    assert_eq!(
        first_report
            .iter()
            .map(|image| (image.member("fpgaImageId").to_string(), image.state_code.clone()))
            .collect::<Vec<_>>(),
        second_report
            .iter()
            .map(|image| (image.member("fpgaImageId").to_string(), image.state_code.clone()))
            .collect::<Vec<_>>(),
        "two describes answer the same report, got {first} then {second}"
    );
    // The catalog is reported ONCE however many times it is read: a report that computed it
    // twice, or stored it, would carry it twice here.
    for fpga_image_id in &catalog {
        assert_eq!(
            reported_ids
                .iter()
                .filter(|reported| *reported == fpga_image_id)
                .count(),
            1,
            "the published AFI {fpga_image_id} is reported exactly once, got {reported_ids:?}"
        );
    }

    let after = {
        let store = engine.state::<Ec2State>();
        let guard = store.lock();
        guard
            .fpga_images
            .iter()
            .map(|(id, record)| {
                (
                    id.clone(),
                    record.load_permissions.len(),
                    record.view.tags.clone().unwrap_or_default().len(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        after, before,
        "a describe writes nothing: expected the store as it was, got {after:?}"
    );
    assert_eq!(
        after.len(),
        SEEDS.len(),
        "a describe removes no AFI and stores none: expected {} stored, got {after:?}",
        SEEDS.len()
    );
    // The AFI the caller may not see is still stored — it is withheld from the report, not
    // taken out of the store.
    assert!(
        after
            .iter()
            .any(|(id, _, _)| *id == seeded.ids[INVISIBLE_ROW]),
        "the AFI the caller may not see stays in the store, got {after:?}"
    );
}
