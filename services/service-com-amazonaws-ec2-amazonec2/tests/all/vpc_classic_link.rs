// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DisableVpcClassicLink` and the reader it is proved through,
//! `DescribeVpcClassicLink`, driven over the wire the service actually speaks: `ec2Query` —
//! an `application/x-www-form-urlencoded` POST whose body carries `Action=…`, answered with
//! XML (README.md / OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2`
//! signing name in the request's SigV4 credential scope, so every call here carries one.
//!
//! The rule under test is that whether a VPC is enabled for ClassicLink is STATE THE SERVICE
//! KEEPS PER VPC: `DisableVpcClassicLink` takes that enablement off the one VPC its `VpcId`
//! names — leaving every other VPC's as it was — and answers `true`, while
//! `DescribeVpcClassicLink` reports, for each VPC the account holds, the status the store
//! holds for it. A VPC the account does not hold is a refusal rather than a no-op, a VPC with
//! an EC2-Classic instance linked to it cannot be disabled, and a request that would have
//! succeeded reports that outcome and writes nothing when `DryRun` is set.
//!
//! So every table row below is a distinct input along what that generalises over: whether the
//! VPC was enabled to begin with, how many tags it carries, which VPC of several the request
//! names, how many times it is named, and which of the documented filters and ids the
//! describe narrows by. The one recorded call — a disable of a VPC that was never enabled,
//! answered `true` — is ONE row among them rather than the specification.

use std::collections::BTreeMap;

use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, envelope_children, error_code, error_message, indexed};
use super::wire::{child_text, document, param, set_items, Param};

/// The Region every call here is signed for. A VPC's ClassicLink status is a fact of the VPC
/// rather than of a Region, so the Region only selects the ec2Query envelope and the zones a
/// launch lands in.
const REGION: &str = "us-west-2";

/// The image the one launch here names. Which AMI it is does not matter to a ClassicLink
/// link.
const AMI: &str = "ami-0123456789abcdef0";

/// A VPC id of the right shape that the account holds no VPC for.
const UNHELD_VPC: &str = "vpc-0000000000000beef";

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// One VPC this suite seeds: the CIDR it is created with, whether something enabled it for
/// ClassicLink before the disable, and the tags it carries.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    cidr: &'static str,
    enabled_first: bool,
    tags: &'static [(&'static str, &'static str)],
}

/// The VPCs the suite works over: both starting statuses, and tag counts from none to two, so
/// the rule is asserted over a set of inputs rather than over one VPC.
const ROWS: [Row; 5] = [
    Row {
        case: "enabled, no tag",
        cidr: "10.0.0.0/16",
        enabled_first: true,
        tags: &[],
    },
    Row {
        case: "enabled, one tag",
        cidr: "10.1.0.0/16",
        enabled_first: true,
        tags: &[("Owner", "TeamA")],
    },
    Row {
        case: "enabled, two tags",
        cidr: "10.2.0.0/16",
        enabled_first: true,
        tags: &[("Owner", "TeamB"), ("Phase", "test")],
    },
    Row {
        case: "never enabled, no tag",
        cidr: "10.3.0.0/16",
        enabled_first: false,
        tags: &[],
    },
    Row {
        case: "never enabled, one tag",
        cidr: "10.4.0.0/16",
        enabled_first: false,
        tags: &[("Phase", "test")],
    },
];

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// One `DisableVpcClassicLink` call, as its HTTP status and XML body.
async fn disable(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DisableVpcClassicLink", params).await
}

/// One `DisableVpcClassicLink` that must succeed, having first pinned that it reported the
/// `<return>true</return>` its result member documents ("Returns true if the request
/// succeeds").
async fn disabled(router: &MultiServiceRouter, case: &str, vpc_id: &str) {
    let (status, body) = disable(router, &[param("VpcId", vpc_id)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DisableVpcClassicLink, got {status} with body \
         {body}"
    );
    let returned = envelope_children(case, &body, "DisableVpcClassicLink", "return");
    assert_eq!(
        returned,
        vec!["true".to_string()],
        "case {case:?}: expected one <return>true</return>, got {returned:?} in {body}"
    );
}

/// One `DescribeVpcClassicLink` call, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeVpcClassicLink", params).await
}

/// What one `DescribeVpcClassicLink` reports for one VPC: its ClassicLink status as the wire
/// spells it, and the tags it carries.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    classic_link_enabled: String,
    tags: Vec<(String, String)>,
}

/// Every `vpcSet/item` of a response, by the `VpcId` it reports.
fn reported(case: &str, body: &str) -> BTreeMap<String, Reported> {
    let parsed = document(case, body);
    let root = parsed.root_element();
    let mut rows = BTreeMap::new();
    for item in set_items(root, "vpcSet") {
        let vpc_id = child_text(item, "vpcId")
            .unwrap_or_else(|| panic!("case {case:?}: an item with no <vpcId> in {body}"));
        let classic_link_enabled = child_text(item, "classicLinkEnabled").unwrap_or_else(|| {
            panic!("case {case:?}: {vpc_id} reported with no <classicLinkEnabled> in {body}")
        });
        let mut tags: Vec<(String, String)> = set_items(item, "tagSet")
            .into_iter()
            .map(|tag| {
                (
                    child_text(tag, "key").unwrap_or_default(),
                    child_text(tag, "value").unwrap_or_default(),
                )
            })
            .collect();
        tags.sort();
        rows.insert(
            vpc_id,
            Reported {
                classic_link_enabled,
                tags,
            },
        );
    }
    rows
}

/// One `DescribeVpcClassicLink` that must succeed, as the rows it reported.
async fn described(
    router: &MultiServiceRouter,
    case: &str,
    params: &[Param],
) -> BTreeMap<String, Reported> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeVpcClassicLink, got {status} with body \
         {body}"
    );
    reported(case, &body)
}

/// The `VpcId`s one successful describe reported, in the order it reported them.
async fn described_ids(router: &MultiServiceRouter, case: &str, params: &[Param]) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeVpcClassicLink, got {status} with body \
         {body}"
    );
    elements(&body, "vpcId")
}

/// A VPC `CreateVpc` makes with `tags` on it, as its `VpcId`.
async fn create_vpc(
    router: &MultiServiceRouter,
    case: &str,
    cidr: &str,
    tags: &[(&str, &str)],
) -> String {
    let mut params = vec![param("CidrBlock", cidr)];
    if !tags.is_empty() {
        params.push(param("TagSpecification.1.ResourceType", "vpc"));
        for (position, (key, value)) in tags.iter().enumerate() {
            params.push(param(
                &format!("TagSpecification.1.Tag.{}.Key", position + 1),
                key,
            ));
            params.push(param(
                &format!("TagSpecification.1.Tag.{}.Value", position + 1),
                value,
            ));
        }
    }
    let (status, body) = super::ec2_query::call(router, REGION, "CreateVpc", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateVpc must succeed, got {status} with body {body}"
    );
    element(&body, "vpcId").unwrap_or_else(|| panic!("case {case:?}: no <vpcId> in {body}"))
}

/// The emulator with [`ROWS`] seeded into it: one VPC per row, each carrying its tags, and
/// the rows that start enabled enabled for ClassicLink through the store's one enablement
/// seam — which is what `EnableVpcClassicLink` (still a scaffold) must write through.
async fn emulator() -> (
    MultiServiceRouter,
    std::sync::Arc<engine::InMemoryEngine>,
    Vec<String>,
) {
    let (router, engine) = super::ec2_query::emulator();
    let mut ids = Vec::new();
    for row in ROWS {
        let vpc_id = create_vpc(&router, row.case, row.cidr, row.tags).await;
        if row.enabled_first {
            let took = engine
                .state::<Ec2State>()
                .lock()
                .set_vpc_classic_link_enabled(&vpc_id, true);
            assert!(
                took,
                "case {case:?}: the store must take the enablement of {vpc_id}",
                case = row.case
            );
        }
        ids.push(vpc_id);
    }
    (router, engine, ids)
}

/// `"true"` / `"false"` as the wire spells a reported boolean.
fn wire_bool(value: bool) -> String {
    if value { "true" } else { "false" }.to_string()
}

/// Assert that `expected` — one entry per seeded VPC, in [`ROWS`] order — is exactly what the
/// describe reports for those VPCs, tags included.
async fn assert_statuses(
    router: &MultiServiceRouter,
    case: &str,
    ids: &[String],
    expected: &[bool],
) {
    let rows = described(router, case, &[]).await;
    for (index, vpc_id) in ids.iter().enumerate() {
        let want_enabled = wire_bool(expected[index]);
        let mut want_tags: Vec<(String, String)> = ROWS[index]
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        want_tags.sort();
        let got = rows.get(vpc_id).unwrap_or_else(|| {
            panic!("case {case:?}: {vpc_id} must be reported, got {rows:?}")
        });
        assert_eq!(
            got.classic_link_enabled, want_enabled,
            "case {case:?}: expected {vpc_id} reported as classicLinkEnabled {want_enabled}, \
             got {actual:?}",
            actual = got.classic_link_enabled
        );
        assert_eq!(
            got.tags, want_tags,
            "case {case:?}: expected {vpc_id} reported with tags {want_tags:?}, got {actual:?}",
            actual = got.tags
        );
    }
}

// ---------------------------------------------------------------------------
// The status the store holds is what a disable changes, and what a describe reports
// ---------------------------------------------------------------------------

/// Disabling takes the ClassicLink enablement off the ONE VPC the request names: the VPCs it
/// did not name keep the status they had, the disabled one reads back as not enabled through
/// `DescribeVpcClassicLink`, and disabling it again succeeds the same way and still leaves it
/// not enabled. A VPC that was never enabled is one row of the table, not a case of its own.
#[tokio::test]
async fn disabling_takes_the_enablement_off_the_named_vpc_and_leaves_every_other_alone() {
    let (router, _engine, ids) = emulator().await;

    // What the seeded world reports before anything is disabled.
    let mut expected: Vec<bool> = ROWS.iter().map(|row| row.enabled_first).collect();
    assert_statuses(&router, "as seeded", &ids, &expected).await;

    for (index, row) in ROWS.iter().enumerate() {
        disabled(&router, row.case, &ids[index]).await;
        expected[index] = false;
        assert_statuses(&router, row.case, &ids, &expected).await;

        // Naming the same VPC a second time is the same request: it succeeds, reports
        // `true`, and the status stays off.
        let repeated = format!("{case}, disabled again", case = row.case);
        disabled(&router, &repeated, &ids[index]).await;
        assert_statuses(&router, &repeated, &ids, &expected).await;
    }
}

/// The set a describe reports is the account's VPCs: every VPC `DescribeVpcs` reports, each
/// with a ClassicLink status, and nothing else. Describing is a READ, so two calls report the
/// same thing.
#[tokio::test]
async fn the_reported_set_is_the_accounts_vpcs_and_describing_changes_nothing() {
    let (router, _engine, ids) = emulator().await;

    // `DescribeVpcs` first, so the account's ambient default VPC — which that describe is
    // what materialises — is already there when the ClassicLink statuses are read.
    let (status, body) = super::ec2_query::call(&router, REGION, "DescribeVpcs", &[]).await;
    assert_eq!(
        status, 200,
        "DescribeVpcs must succeed, got {status} with body {body}"
    );
    let mut held = elements(&body, "vpcId");
    held.sort();
    assert!(
        ids.iter().all(|vpc_id| held.contains(vpc_id)),
        "DescribeVpcs must report every seeded VPC: expected {ids:?} within {held:?}"
    );

    let first = described(&router, "every VPC", &[]).await;
    let mut reported_ids: Vec<String> = first.keys().cloned().collect();
    reported_ids.sort();
    assert_eq!(
        reported_ids, held,
        "every VPC: expected the ClassicLink statuses of {held:?}, got {reported_ids:?}"
    );
    // A VPC nobody enabled — the ambient default one among them — is reported as not
    // enabled rather than left without a status.
    for (vpc_id, row) in &first {
        if !ids.contains(vpc_id) {
            assert_eq!(
                row.classic_link_enabled, "false",
                "every VPC: expected the unenabled {vpc_id} reported as false, got {actual:?}",
                actual = row.classic_link_enabled
            );
        }
    }

    let second = described(&router, "described twice", &[]).await;
    assert_eq!(
        second, first,
        "described twice: expected the same report, got {second:?} after {first:?}"
    );
}

// ---------------------------------------------------------------------------
// Narrowing a describe
// ---------------------------------------------------------------------------

/// `VpcIds` narrows the reported set to the ids named, in the store's `VpcId` order, and a
/// repeated id reports that VPC once.
#[tokio::test]
async fn describing_narrows_by_vpc_ids() {
    let (router, _engine, ids) = emulator().await;

    let cases: [(&str, Vec<&str>); 4] = [
        ("one id", vec![ids[0].as_str()]),
        ("two ids", vec![ids[3].as_str(), ids[1].as_str()]),
        ("one id twice", vec![ids[2].as_str(), ids[2].as_str()]),
        (
            "every id",
            ids.iter().map(String::as_str).collect::<Vec<&str>>(),
        ),
    ];
    for (case, wanted) in cases {
        let params = indexed("VpcId", &wanted);
        let got = described_ids(&router, case, &params).await;
        let mut expected: Vec<String> = wanted.iter().map(|id| (*id).to_string()).collect();
        expected.sort();
        expected.dedup();
        assert_eq!(
            got, expected,
            "case {case:?}: expected the statuses of {expected:?}, got {got:?}"
        );
    }

    // "The VPCs for which you want to describe the ClassicLink status": an id the account
    // holds no VPC for is a refusal, not an absent row.
    let (status, body) = describe(&router, &indexed("VpcId", &[ids[0].as_str(), UNHELD_VPC])).await;
    assert_eq!(
        status, 400,
        "an unheld id: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidVpcID.NotFound",
        "an unheld id: unexpected error code in {body}"
    );
    assert_eq!(
        error_message(&body),
        format!("The vpc ID '{UNHELD_VPC}' does not exist"),
        "an unheld id: unexpected message in {body}"
    );
}

/// One filter case: the `Filters` entries it sends and the [`ROWS`] the describe must then
/// report, by their index.
struct FilterCase {
    case: &'static str,
    entries: &'static [(&'static str, &'static [&'static str])],
    expected: &'static [usize],
}

/// Each documented filter selects on its own attribute, values within one filter are OR'd and
/// several filters are AND'd, and a filter name this operation does not document is refused.
#[tokio::test]
async fn describing_narrows_by_every_documented_filter() {
    let (router, _engine, ids) = emulator().await;

    let cases: [FilterCase; 7] = [
        // "Whether the VPC is enabled for ClassicLink (true | false)".
        FilterCase {
            case: "is-classic-link-enabled true",
            entries: &[("is-classic-link-enabled", &["true"])],
            expected: &[0, 1, 2],
        },
        FilterCase {
            case: "is-classic-link-enabled false",
            entries: &[("is-classic-link-enabled", &["false"])],
            expected: &[3, 4],
        },
        FilterCase {
            case: "is-classic-link-enabled either value",
            entries: &[("is-classic-link-enabled", &["true", "false"])],
            expected: &[0, 1, 2, 3, 4],
        },
        // "The key/value combination of a tag assigned to the resource."
        FilterCase {
            case: "tag:Owner TeamA",
            entries: &[("tag:Owner", &["TeamA"])],
            expected: &[1],
        },
        FilterCase {
            case: "tag:Owner either team",
            entries: &[("tag:Owner", &["TeamA", "TeamB"])],
            expected: &[1, 2],
        },
        // "The key of a tag assigned to the resource … regardless of the tag value."
        FilterCase {
            case: "tag-key Phase",
            entries: &[("tag-key", &["Phase"])],
            expected: &[2, 4],
        },
        // Two filters are joined with an AND.
        FilterCase {
            case: "tag-key Phase and enabled",
            entries: &[
                ("tag-key", &["Phase"]),
                ("is-classic-link-enabled", &["true"]),
            ],
            expected: &[2],
        },
    ];
    for FilterCase {
        case,
        entries,
        expected: wanted,
    } in cases
    {
        // The ids of the table's own VPCs are named beside the filters — the account also
        // holds the ambient default VPC, which is no row of this table — so what each case
        // asserts is which of ITS VPCs the filter selects.
        let mut params = indexed(
            "VpcId",
            &ids.iter().map(String::as_str).collect::<Vec<&str>>(),
        );
        params.extend(super::ec2_query::filters(entries));
        let got = described_ids(&router, case, &params).await;
        let mut expected: Vec<String> = wanted.iter().map(|index| ids[*index].clone()).collect();
        expected.sort();
        assert_eq!(
            got, expected,
            "case {case:?}: expected {expected:?}, got {got:?}"
        );
    }

    // A name the operation does not document is refused whether or not any VPC would have
    // matched it — including the near-misses of the documented ones.
    for undocumented in ["classic-link-enabled", "vpc-id", "tag:", "TAG-KEY"] {
        let (status, body) = describe(
            &router,
            &super::ec2_query::filters(&[(undocumented, &["true"])]),
        )
        .await;
        assert_eq!(
            status, 400,
            "filter {undocumented:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "filter {undocumented:?}: unexpected error code in {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("The filter '{undocumented}' is invalid"),
            "filter {undocumented:?}: unexpected message in {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// What a disable refuses
// ---------------------------------------------------------------------------

/// A `VpcId` the account holds no VPC for is refused — whatever the value's shape, and
/// including a VPC that has since been deleted — and nothing else is changed by the attempt.
#[tokio::test]
async fn disabling_a_vpc_the_account_does_not_hold_is_refused() {
    let (router, _engine, ids) = emulator().await;

    // A VPC the account held until it deleted it is as unheld as an id it never issued.
    let deleted = create_vpc(&router, "deleted", "10.9.0.0/16", &[]).await;
    let (status, body) =
        super::ec2_query::call(&router, REGION, "DeleteVpc", &[param("VpcId", &deleted)]).await;
    assert_eq!(
        status, 200,
        "deleted: DeleteVpc must succeed, got {status} with body {body}"
    );

    for (case, vpc_id) in [
        ("an id the account never held", UNHELD_VPC),
        ("a value that is no VPC id at all", "not-a-vpc"),
        ("the empty value", ""),
        ("a deleted VPC", deleted.as_str()),
    ] {
        let (status, body) = disable(&router, &[param("VpcId", vpc_id)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidVpcID.NotFound",
            "case {case:?}: unexpected error code in {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("The vpc ID '{vpc_id}' does not exist"),
            "case {case:?}: unexpected message in {body}"
        );
    }

    // Every seeded VPC still reports the status it was seeded with: a refused request writes
    // nothing.
    let expected: Vec<bool> = ROWS.iter().map(|row| row.enabled_first).collect();
    assert_statuses(&router, "after the refusals", &ids, &expected).await;
}

/// "You cannot disable ClassicLink for a VPC that has EC2-Classic instances linked to it": the
/// VPC with a link is refused and keeps its status, while another VPC of the same account is
/// disabled as usual.
#[tokio::test]
async fn a_vpc_with_a_linked_ec2_classic_instance_cannot_be_disabled() {
    let (router, engine, ids) = emulator().await;

    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        "RunInstances",
        &[
            param("ImageId", AMI),
            param("MinCount", "1"),
            param("MaxCount", "1"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "RunInstances must succeed, got {status} with body {body}"
    );
    let instance_id = element(&body, "instanceId")
        .unwrap_or_else(|| panic!("RunInstances must report an <instanceId>: {body}"));

    // Written the way `AttachClassicLinkVpc` must write it: through the store's one link
    // seam, which is what that operation (still a scaffold) has to call.
    let linked_vpc = ids[0].clone();
    let took = engine
        .state::<Ec2State>()
        .lock()
        .link_classic_link_vpc(&instance_id, &linked_vpc, Vec::new());
    assert!(
        took,
        "the store must take the link of {instance_id} to {linked_vpc}"
    );

    for case in ["with a link", "with a link, dry run"] {
        let mut params = vec![param("VpcId", &linked_vpc)];
        if case.ends_with("dry run") {
            params.push(param("DryRun", "true"));
        }
        let (status, body) = disable(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DependencyViolation",
            "case {case:?}: unexpected error code in {body}"
        );
        assert_eq!(
            error_message(&body),
            format!(
                "The vpc '{linked_vpc}' has dependencies and cannot be disabled for ClassicLink."
            ),
            "case {case:?}: unexpected message in {body}"
        );
    }

    // The refused VPC keeps its enablement; a VPC with no link of its own still disables.
    let mut expected: Vec<bool> = ROWS.iter().map(|row| row.enabled_first).collect();
    assert_statuses(&router, "the linked VPC keeps its status", &ids, &expected).await;
    disabled(&router, "a VPC with no link", &ids[1]).await;
    expected[1] = false;
    assert_statuses(&router, "a VPC with no link", &ids, &expected).await;
}

/// `DryRun` reports the outcome of a request that would have succeeded and performs none of
/// it: the status is untouched, and the refusals above still take precedence over it.
#[tokio::test]
async fn a_dry_run_reports_the_outcome_and_writes_nothing() {
    let (router, _engine, ids) = emulator().await;

    for (case, vpc_id) in [
        ("a dry run on an enabled VPC", ids[0].as_str()),
        ("a dry run on a VPC that was never enabled", ids[3].as_str()),
    ] {
        let (status, body) =
            disable(&router, &[param("VpcId", vpc_id), param("DryRun", "true")]).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: unexpected error code in {body}"
        );
        assert_eq!(
            error_message(&body),
            "Request would have succeeded, but DryRun flag is set.",
            "case {case:?}: unexpected message in {body}"
        );
    }

    // A dry run of a request that would NOT have succeeded is refused with that reason
    // instead.
    let (status, body) = disable(
        &router,
        &[param("VpcId", UNHELD_VPC), param("DryRun", "true")],
    )
    .await;
    assert_eq!(
        status, 400,
        "a dry run on an unheld VPC: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidVpcID.NotFound",
        "a dry run on an unheld VPC: unexpected error code in {body}"
    );

    // A describe is a read, and its own dry run reports nothing at all.
    let (status, body) = describe(&router, &[param("DryRun", "true")]).await;
    assert_eq!(
        status, 412,
        "a describe dry run: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "a describe dry run: unexpected error code in {body}"
    );

    // Nothing above wrote anything: every VPC still reports the status it was seeded with.
    let expected: Vec<bool> = ROWS.iter().map(|row| row.enabled_first).collect();
    assert_statuses(&router, "after the dry runs", &ids, &expected).await;
}
