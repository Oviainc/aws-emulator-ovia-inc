// SPDX-License-Identifier: CC0-1.0

//! `ListCompatibleImages`: the Amazon EC2-compatible images this emulator holds for the
//! account, driven end to end over HTTP.
//!
//! The rule under test: a listing of an account-owned collection reports the records this
//! emulator's state holds for the account — none at all when it holds none — each record
//! once, in the order the store recorded it, and pages the answer with the operation's
//! modelled `MaxResults`/`NextToken` members, refusing a token it did not mint for that
//! listing. For `ListCompatibleImages` those records are the AMIs the account's stored Snow
//! orders name as `Ec2AmiResource`s: an order may put one of the account's EC2 AMIs on a
//! device, so the images the emulator holds for the account are exactly the ones its
//! orders named.
//!
//! Every suite is a TABLE over an axis the rule generalises along — how many orders name an
//! AMI and how (a plain order, an export order and its job parts, a cluster's node jobs),
//! how many AMIs each names, the form of the identifier, the page size, the token's form,
//! the number of reads. No suite reaches into the state struct: an identifier can only be
//! reported because an order stored it, and the `ListCompatibleImages` request carries no
//! identifier of its own, so an answer that is not computed from stored state cannot pass.
//!
//! The harness (`router`, `create_address`, `deliverable_address`, `complete_order`, `ok`,
//! `refused`, `string`) and the cluster create (`base_cluster`) are the ones this crate's
//! other suites already drive the service with, so every call here goes through the same
//! front door a client uses.

use serde_json::{json, Value};
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::state::encode_cursor;
use smithy_server_runtime::MultiServiceRouter;

use super::cluster_updates::base_cluster;
use super::job_lifecycle::{
    complete_order, create_address, deliverable_address, ok, refused, router, string,
};

/// `MaxResults` on `ListCompatibleImages` is modelled `range 0..100`
/// (`smithy_query(query="operation", operation="ListCompatibleImages")`).
const MAX_RESULTS_MIN: i64 = 0;
const MAX_RESULTS_MAX: i64 = 100;

/// `NextToken`, `CompatibleImage$AmiId` and `CompatibleImage$Name` are all modelled
/// `len 1..1024`, `pattern .*` (`smithy_query(query="operation",
/// operation="ListCompatibleImages")`, `smithy_query(query="shape",
/// shape="CompatibleImage")`).
const STRING_LEN_MIN: usize = 1;
const STRING_LEN_MAX: usize = 1024;

/// The Amazon S3 bucket an order that transfers data names, for the rows whose `JobType`
/// is one that does.
const TRANSFER_BUCKET: &str = "arn:aws:s3:::compatible-images-bucket";

/// The identifiers the rows below put on orders: Amazon EC2 has minted both the legacy
/// eight-digit image id and the long seventeen-digit one, and `AmiId` is modelled
/// `pattern .*`, so a listing must report whichever form the order stored rather than one
/// form of its own.
const LONG_FORM_IDS: &[&str] = &[
    "ami-0123456789abcdef0",
    "ami-00f1e2d3c4b5a6978",
    "ami-0fedcba9876543210",
    "ami-0a1b2c3d4e5f60718",
];
const LEGACY_FORM_IDS: &[&str] = &["ami-1a2b3c4d", "ami-9f8e7d6c"];

/// One order a row places, and the AMI identifiers it names.
enum Order {
    /// A local-compute order: one stored job, naming no Amazon S3 bucket.
    Job(Vec<&'static str>),
    /// An export order: a main job plus its job parts, each part carrying the order's
    /// resources, so the same identifiers are stored on several jobs.
    Export(Vec<&'static str>),
    /// A cluster order of `nodes` node jobs, each inheriting the cluster's resources.
    Cluster {
        nodes: i64,
        ami_ids: Vec<&'static str>,
    },
}

impl Order {
    /// The AMI identifiers this order names, in the order it names them.
    fn ami_ids(&self) -> &[&'static str] {
        match self {
            Order::Job(ami_ids) | Order::Export(ami_ids) => ami_ids,
            Order::Cluster { ami_ids, .. } => ami_ids,
        }
    }
}

/// The `Ec2AmiResources` list of an order naming `ami_ids`.
fn ec2_ami_resources(ami_ids: &[&str]) -> Value {
    Value::Array(
        ami_ids
            .iter()
            .map(|ami_id| json!({ "AmiId": ami_id }))
            .collect(),
    )
}

/// Place `order` against `address_id`, through the create operation its kind belongs to.
async fn place(router: &MultiServiceRouter, address_id: &str, order: &Order, case: &str) {
    match order {
        // A local-compute order is what an EC2 AMI is put on a device for, and it moves
        // no data to or from Amazon S3, so it names no bucket.
        Order::Job(ami_ids) => {
            let created = ok(
                router,
                "CreateJob",
                complete_order(&json!({
                    "AddressId": address_id,
                    "JobType": "LOCAL_USE",
                    "Resources": { "Ec2AmiResources": ec2_ami_resources(ami_ids) },
                })),
                case,
            )
            .await;
            string(&created, "JobId", case);
        }
        // An export order transfers named buckets, so it carries both kinds of resource.
        Order::Export(ami_ids) => {
            let created = ok(
                router,
                "CreateJob",
                complete_order(&json!({
                    "AddressId": address_id,
                    "JobType": "EXPORT",
                    "Resources": {
                        "S3Resources": [{ "BucketArn": TRANSFER_BUCKET }],
                        "Ec2AmiResources": ec2_ami_resources(ami_ids),
                    },
                })),
                case,
            )
            .await;
            string(&created, "JobId", case);
        }
        Order::Cluster { nodes, ami_ids } => {
            let mut body = base_cluster(address_id);
            body["InitialClusterSize"] = json!(nodes);
            body["Resources"] = json!({
                "S3Resources": [{ "BucketArn": TRANSFER_BUCKET }],
                "Ec2AmiResources": ec2_ami_resources(ami_ids),
            });
            let created = ok(router, "CreateCluster", body, case).await;
            string(&created, "ClusterId", case);
        }
    }
}

/// A fresh emulator with `orders` placed against one address, and the identifiers the
/// listing must report: every identifier the orders named, each once, in the order the
/// orders named them.
async fn emulator_with(orders: &[Order], case: &str) -> (MultiServiceRouter, Vec<String>) {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Compatible Images Receiver", "7 Image Way", json!({})),
        case,
    )
    .await;
    for (index, order) in orders.iter().enumerate() {
        place(&router, &address_id, order, &format!("{case}, order {index}")).await;
    }

    let mut expected: Vec<String> = Vec::new();
    for ami_id in orders.iter().flat_map(Order::ami_ids) {
        if !expected.iter().any(|held| held == ami_id) {
            expected.push((*ami_id).to_string());
        }
    }
    (router, expected)
}

/// One `ListCompatibleImages` that must succeed.
async fn list(router: &MultiServiceRouter, body: Value, case: &str) -> Value {
    ok(router, "ListCompatibleImages", body, case).await
}

/// The `AmiId` of every reported `CompatibleImage`, in wire order, holding each entry to
/// the modelled bounds on its members: an image is identified, and carries a `Name` only
/// where the store holds one — an order names an AMI by its id alone, so none does.
fn reported_images(body: &Value, case: &str) -> Vec<String> {
    body["CompatibleImages"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a CompatibleImages list, got {body}"))
        .iter()
        .map(|entry| {
            let ami_id = entry["AmiId"].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: expected an AmiId in each CompatibleImage, got {body}")
            });
            let length = ami_id.chars().count();
            assert!(
                (STRING_LEN_MIN..=STRING_LEN_MAX).contains(&length),
                "case {case:?}: expected an AmiId of 1..1024 characters, got {ami_id:?} \
                 ({length} characters)"
            );
            assert!(
                entry.get("Name").is_none(),
                "case {case:?}: the store holds no name for an AMI an order named by id \
                 alone, so no Name is reported, got {entry}"
            );
            ami_id.to_string()
        })
        .collect()
}

/// Walk every page of the listing at page size `max_results`, returning the identifiers the
/// pages reported in order.
async fn page_through(
    router: &MultiServiceRouter,
    max_results: Option<i64>,
    expected_total: usize,
    case: &str,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut body = json!({});
        if let Some(max) = max_results {
            body["MaxResults"] = json!(max);
        }
        if let Some(token) = &next_token {
            body["NextToken"] = json!(token);
        }
        let listed = list(router, body, case).await;
        let page = reported_images(&listed, case);
        pages += 1;
        assert!(
            pages <= expected_total + 1,
            "case {case:?}: the walk produced {pages} pages for {expected_total} images"
        );
        if let Some(max) = max_results.filter(|max| *max > 0) {
            assert!(
                page.len() <= usize::try_from(max).expect("a positive page size"),
                "case {case:?}: expected at most {max} images in a page, got {}",
                page.len()
            );
        }
        seen.extend(page);
        next_token = listed
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(token) = &next_token {
            let length = token.chars().count();
            assert!(
                (STRING_LEN_MIN..=STRING_LEN_MAX).contains(&length),
                "case {case:?}: expected a NextToken of 1..1024 characters, got {token:?}"
            );
        } else {
            break;
        }
    }
    seen
}

#[tokio::test]
async fn an_account_whose_orders_name_no_ami_is_reported_as_holding_none() {
    // Each row is a store that holds no EC2 image for the account: no order at all, an
    // order with no resources, an order whose resources name only buckets, and an order
    // whose EC2 resource list is empty. None of them may be answered with an image.
    let rows: Vec<(&str, Value)> = vec![
        ("an order with no Resources", json!({})),
        (
            "an order whose Resources name only buckets",
            json!({ "Resources": { "S3Resources": [{ "BucketArn": TRANSFER_BUCKET }] } }),
        ),
        (
            "an order whose Ec2AmiResources is empty",
            json!({ "Resources": { "Ec2AmiResources": [] } }),
        ),
    ];

    let case = "a store with no order at all";
    let empty_store = router();
    let listed = list(&empty_store, json!({}), case).await;
    assert!(
        reported_images(&listed, case).is_empty(),
        "case {case:?}: an account whose orders name no AMI holds none, got {listed}"
    );
    assert!(
        listed.get("NextToken").is_none(),
        "case {case:?}: a listing with nothing after it carries no NextToken, got {listed}"
    );

    for (case, resources) in rows {
        let router = router();
        let address_id = create_address(
            &router,
            deliverable_address("No Image Receiver", "8 Image Way", json!({})),
            case,
        )
        .await;
        let mut body = complete_order(&json!({
            "AddressId": address_id,
            "JobType": "LOCAL_USE",
        }));
        body.as_object_mut()
            .expect("the order is a JSON object")
            .remove("Resources");
        for (member, value) in resources.as_object().expect("the row is a JSON object") {
            body[member] = value.clone();
        }
        let created = ok(&router, "CreateJob", body, case).await;
        string(&created, "JobId", case);

        let listed = list(&router, json!({}), case).await;
        assert!(
            reported_images(&listed, case).is_empty(),
            "case {case:?}: an account whose orders name no AMI holds none, got {listed}"
        );
        assert!(
            listed.get("NextToken").is_none(),
            "case {case:?}: a listing with nothing after it carries no NextToken, got {listed}"
        );
    }
}

#[tokio::test]
async fn every_ami_the_stored_orders_name_is_reported_once_in_order() {
    // Each row is a way the account's orders can name AMIs: one order naming one, one
    // order naming several, several orders naming one each, two orders naming the SAME
    // one, an export order whose job parts each carry the order's resources, a cluster
    // whose node jobs inherit them, and identifiers in both of Amazon EC2's id forms.
    let rows: Vec<(&str, Vec<Order>)> = vec![
        (
            "one order naming one AMI",
            vec![Order::Job(vec![LONG_FORM_IDS[0]])],
        ),
        (
            "one order naming three AMIs",
            vec![Order::Job(LONG_FORM_IDS[..3].to_vec())],
        ),
        (
            "three orders naming one AMI each",
            vec![
                Order::Job(vec![LONG_FORM_IDS[0]]),
                Order::Job(vec![LONG_FORM_IDS[1]]),
                Order::Job(vec![LONG_FORM_IDS[2]]),
            ],
        ),
        (
            "two orders naming the same AMI",
            vec![
                Order::Job(vec![LONG_FORM_IDS[0]]),
                Order::Job(vec![LONG_FORM_IDS[0]]),
            ],
        ),
        (
            "an order naming the same AMI twice",
            vec![Order::Job(vec![LONG_FORM_IDS[1], LONG_FORM_IDS[1]])],
        ),
        (
            "an export order, whose job parts carry its resources",
            vec![Order::Export(LONG_FORM_IDS[..2].to_vec())],
        ),
        (
            "a cluster, whose node jobs inherit its resources",
            vec![Order::Cluster {
                nodes: 3,
                ami_ids: LONG_FORM_IDS[..2].to_vec(),
            }],
        ),
        (
            "identifiers in Amazon EC2's legacy id form",
            vec![Order::Job(LEGACY_FORM_IDS.to_vec())],
        ),
        (
            "both id forms, across an order and a cluster",
            vec![
                Order::Job(vec![LEGACY_FORM_IDS[0], LONG_FORM_IDS[3]]),
                Order::Cluster {
                    nodes: 2,
                    ami_ids: vec![LONG_FORM_IDS[3], LEGACY_FORM_IDS[1]],
                },
            ],
        ),
    ];

    for (case, orders) in rows {
        let (router, expected) = emulator_with(&orders, case).await;
        let listed = list(&router, json!({}), case).await;
        assert_eq!(
            reported_images(&listed, case),
            expected,
            "case {case:?}: expected every AMI the stored orders name, once each, in the \
             order they were named, got {listed}"
        );
        assert!(
            listed.get("NextToken").is_none(),
            "case {case:?}: a page holding the whole listing carries no NextToken, got {listed}"
        );
    }
}

#[tokio::test]
async fn the_listing_pages_at_every_page_size() {
    // A store holding four images across two orders, so a small page leaves images after
    // the first page and the published maximum does not.
    let case = "the paged listing";
    let (router, expected) = emulator_with(
        &[
            Order::Job(LONG_FORM_IDS[..2].to_vec()),
            Order::Job(LONG_FORM_IDS[2..4].to_vec()),
        ],
        case,
    )
    .await;
    let total = expected.len();
    assert!(
        total > 1,
        "case {case:?}: this suite needs more than one image to tell page sizes apart, got \
         {total}"
    );

    // Each row is a page size: the published bounds, sizes that divide the listing and
    // sizes that do not, the listing's own length, one more than it, and a request that
    // declares none.
    let mut rows: Vec<(String, Option<i64>)> = vec![
        ("no MaxResults".to_string(), None),
        (
            "MaxResults at the published minimum".to_string(),
            Some(MAX_RESULTS_MIN),
        ),
    ];
    for size in 1..=total + 1 {
        rows.push((
            format!("MaxResults {size}"),
            Some(i64::try_from(size).expect("a small listing")),
        ));
    }
    rows.push((
        "MaxResults at the published maximum".to_string(),
        Some(MAX_RESULTS_MAX),
    ));

    for (row, max_results) in rows {
        let case = format!("paging, {row}");
        let seen = page_through(&router, max_results, total, &case).await;
        assert_eq!(
            seen, expected,
            "case {case:?}: expected the pages to report every stored image once, in the \
             order the orders named them"
        );
    }

    // One past the published maximum, and one below the published minimum, are outside the
    // modelled range and are refused before the handler runs.
    for (row, max_results) in [
        ("one past the published maximum", MAX_RESULTS_MAX + 1),
        ("one below the published minimum", MAX_RESULTS_MIN - 1),
    ] {
        refused(
            &router,
            "ListCompatibleImages",
            json!({ "MaxResults": max_results }),
            "ValidationException",
            row,
        )
        .await;
    }
}

#[tokio::test]
async fn a_next_token_this_listing_did_not_mint_is_refused() {
    let case = "the listing's own cursor";
    let (router, expected) = emulator_with(&[Order::Job(LONG_FORM_IDS[..3].to_vec())], case).await;

    // The token the service itself minted, from a walk that has more pages to go.
    let first_page = list(&router, json!({ "MaxResults": 1 }), case).await;
    let token = string(&first_page, "NextToken", case);
    let mut altered = token.clone();
    altered.pop();
    altered.push(if token.ends_with('0') { '1' } else { '0' });

    // Each row is a token this listing did not issue: not a cursor at all, a cursor for an
    // identifier the account's orders never named, a cursor naming a key of another of
    // this service's listings, and a cursor altered in transit.
    let rows: Vec<(String, String)> = vec![
        ("not a cursor at all".to_string(), "zzzz".to_string()),
        (
            "a cursor for an identifier no order named".to_string(),
            encode_cursor("ami-00000000000000000"),
        ),
        (
            "a cursor minted for the job listing".to_string(),
            encode_cursor("JID00000000-0000-0000-0000-000000000000"),
        ),
        ("a cursor altered in transit".to_string(), altered),
    ];

    for (row, bad_token) in rows {
        refused(
            &router,
            "ListCompatibleImages",
            json!({ "NextToken": bad_token }),
            "InvalidNextTokenException",
            &row,
        )
        .await;
    }

    // The token the service did mint still resumes the walk after every refusal, so a
    // refused page leaves the listing exactly where it was.
    let resumed = list(&router, json!({ "NextToken": token }), case).await;
    assert_eq!(
        reported_images(&resumed, case),
        expected[1..].to_vec(),
        "case {case:?}: expected the minted cursor to resume at the second image, got {resumed}"
    );
}

#[tokio::test]
async fn reading_the_listing_leaves_it_unchanged_for_the_next_caller() {
    // Each row is a number of reads: reporting the account's images neither consumes nor
    // reorders them, so every read of the same store answers the same listing, and the
    // orders behind it are still there afterwards.
    for reads in [1usize, 2, 5] {
        let case = format!("{reads} reads");
        let (router, expected) = emulator_with(
            &[
                Order::Job(LONG_FORM_IDS[..2].to_vec()),
                Order::Cluster {
                    nodes: 2,
                    ami_ids: vec![LONG_FORM_IDS[2]],
                },
            ],
            &case,
        )
        .await;
        for read in 0..reads {
            let case = format!("{case}, read {read}");
            let listed = list(&router, json!({}), &case).await;
            assert_eq!(
                reported_images(&listed, &case),
                expected,
                "case {case:?}: expected every read to report the same listing, got {listed}"
            );
        }

        // The jobs the listing is read off are untouched by having been read.
        let listed_jobs = ok(&router, "ListJobs", json!({}), &case).await;
        let job_count = listed_jobs["JobListEntries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a JobListEntries list"))
            .len();
        assert_eq!(
            job_count, 3,
            "case {case:?}: expected the one job and the cluster's two node jobs to still be \
             stored, got {listed_jobs}"
        );
    }
}
