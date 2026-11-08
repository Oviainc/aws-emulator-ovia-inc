// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeAccountAttributes` — driven over the wire the service
//! actually speaks: `ec2Query`, an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=…`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that the operation REPORTS the attributes of the account: every
//! attribute the operation documents carries the value the account currently holds for it,
//! `AttributeNames` narrows the report to the attributes it names, and a name that is no
//! account attribute is refused rather than answered with an empty report. The one value
//! that comes from stored state is `default-vpc` — "the ID of the default VPC for your
//! account, or none" — so it is asserted AGAINST the VPC the account's own `DescribeVpcs`
//! reports, in Regions that have one and in a Region that has none.
//!
//! So the tables below span that class rather than one recorded request: every documented
//! attribute asked for alone, in pairs, all together, twice over and in the reverse order;
//! several Regions and a Region of no partition; repeated reads; and the refusals.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{error_code, error_message, filters, indexed};
use super::wire::{child_text, document, param, set_items, Param};

/// The Region most calls here are signed for.
const REGION: &str = "us-east-1";

/// The account attributes the operation documents as "the supported account attributes",
/// written out here rather than read off the handler's own catalog so the expectation is the
/// DOCUMENTED set and not whatever the implementation happens to carry
/// (`smithy_query(query="doc",shape="DescribeAccountAttributes")`: default-vpc,
/// max-instances, max-elastic-ips, supported-platforms, vpc-max-elastic-ips,
/// vpc-max-security-groups-per-interface).
const DOCUMENTED_ATTRIBUTES: [&str; 6] = [
    "default-vpc",
    "max-instances",
    "max-elastic-ips",
    "supported-platforms",
    "vpc-max-elastic-ips",
    "vpc-max-security-groups-per-interface",
];

/// The attribute names the MODEL's own `AccountAttributeName` enum declares
/// (`smithy_query(query="shape",shape="AccountAttributeName")`: supported-platforms,
/// default-vpc) — the shorter list, the operation's doc documenting four attributes the enum
/// never caught up with. [`every_modelled_attribute_name_is_reported`] pins this list
/// against the generated catalog, so a member the model adds cannot stay untested here.
const MODELLED_ATTRIBUTES: [&str; 2] = ["default-vpc", "supported-platforms"];

/// The word `default-vpc` carries for an account with no default VPC in the Region asked in:
/// "The ID of the default VPC for your account, or none" (the operation's own doc).
const NO_DEFAULT_VPC: &str = "none";

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One call signed for `region`.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    params: &[Param],
) -> (u16, String) {
    super::ec2_query::call(router, region, action, params).await
}

/// One call signed for [`REGION`].
async fn call(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    call_in(router, REGION, action, params).await
}

/// The attributes a `DescribeAccountAttributes` response reports, in the order it reported
/// them: each attribute's name with its values.
fn reported(case: &str, body: &str) -> Vec<(String, Vec<String>)> {
    let parsed = document(case, body);
    set_items(parsed.root_element(), "accountAttributeSet")
        .into_iter()
        .map(|item| {
            let name = child_text(item, "attributeName").unwrap_or_else(|| {
                panic!("case {case:?}: an account attribute carries no <attributeName>: {body}")
            });
            let values = set_items(item, "attributeValueSet")
                .into_iter()
                .map(|value| {
                    child_text(value, "attributeValue").unwrap_or_else(|| {
                        panic!("case {case:?}: an attribute value carries no \
                                <attributeValue>: {body}")
                    })
                })
                .collect();
            (name, values)
        })
        .collect()
}

/// The attributes `DescribeAccountAttributes` reports in `region` for `params`, asserted to
/// be a success.
async fn describe(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    params: &[Param],
) -> Vec<(String, Vec<String>)> {
    let (status, body) = call_in(router, region, "DescribeAccountAttributes", params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeAccountAttributes, got {status} with \
         body {body}"
    );
    reported(case, &body)
}

/// The values one attribute carries, or a panic naming the case when the report does not
/// carry that attribute at all.
fn values_of(case: &str, attributes: &[(String, Vec<String>)], name: &str) -> Vec<String> {
    attributes
        .iter()
        .find(|(reported_name, _)| reported_name == name)
        .map(|(_, values)| values.clone())
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the report to carry {name:?}, got {attributes:?}")
        })
}

/// The names a report carries, in the order it reported them.
fn names_of(attributes: &[(String, Vec<String>)]) -> Vec<String> {
    attributes
        .iter()
        .map(|(name, _)| name.clone())
        .collect()
}

/// The `default-vpc` the operation reports in `region`.
async fn reported_default_vpc(router: &MultiServiceRouter, region: &str, case: &str) -> String {
    let attributes = describe(router, region, case, &[]).await;
    let values = values_of(case, &attributes, "default-vpc");
    assert_eq!(
        values.len(),
        1,
        "case {case:?}: default-vpc carries exactly one value, got {values:?}"
    );
    values[0].clone()
}

/// The id of the default VPC the account's own `DescribeVpcs` reports in `region`, or
/// `None` when it reports none: the `isDefault` filter the operation documents.
async fn described_default_vpc(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
) -> Option<String> {
    let (status, body) = call_in(
        router,
        region,
        "DescribeVpcs",
        &filters(&[("isDefault", &["true"])]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeVpcs, got {status} with body {body}"
    );
    let parsed = document(case, &body);
    let ids: Vec<String> = set_items(parsed.root_element(), "vpcSet")
        .into_iter()
        .filter_map(|item| child_text(item, "vpcId"))
        .collect();
    assert!(
        ids.len() <= 1,
        "case {case:?}: an account holds at most one default VPC per Region, got {ids:?}"
    );
    ids.into_iter().next()
}

/// The ids of the subnets of `vpc_id`, as `DescribeSubnets` reports them under the `vpc-id`
/// filter that operation documents.
async fn subnets_of(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    vpc_id: &str,
) -> Vec<String> {
    let (status, body) = call_in(
        router,
        region,
        "DescribeSubnets",
        &filters(&[("vpc-id", &[vpc_id])]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeSubnets, got {status} with body {body}"
    );
    let parsed = document(case, &body);
    set_items(parsed.root_element(), "subnetSet")
        .into_iter()
        .filter_map(|item| child_text(item, "subnetId"))
        .collect()
}

// ---------------------------------------------------------------------------
// The catalog
// ---------------------------------------------------------------------------
/// Every attribute the operation documents is reported, exactly once, carrying at least one
/// non-empty value — a request that names none gets the whole account.
///
/// The rows are the documented attributes themselves, so an attribute the implementation
/// forgets or reports twice fails here whichever one it is.
#[tokio::test]
async fn a_request_naming_nothing_reports_every_documented_attribute() {
    let case = "the whole account";
    let router = emulator();
    let attributes = describe(&router, REGION, case, &[]).await;

    for name in DOCUMENTED_ATTRIBUTES {
        let carried = attributes
            .iter()
            .filter(|(reported_name, _)| reported_name == name)
            .count();
        assert_eq!(
            carried, 1,
            "case {case:?}: expected {name:?} to be reported exactly once, got {carried} in \
             {attributes:?}"
        );
        let values = values_of(case, &attributes, name);
        assert!(
            !values.is_empty(),
            "case {case:?}: expected {name:?} to carry at least one value, got {values:?}"
        );
        for value in &values {
            assert!(
                !value.is_empty(),
                "case {case:?}: expected every value of {name:?} to be non-empty, got \
                 {values:?}"
            );
        }
    }
    assert_eq!(
        attributes.len(),
        DOCUMENTED_ATTRIBUTES.len(),
        "case {case:?}: expected exactly the documented attributes {DOCUMENTED_ATTRIBUTES:?}, \
         got {attributes:?}"
    );
}

/// Every member of the model's own `AccountAttributeName` enum is an attribute this service
/// reports: the enum is the shorter list (the operation's doc names more), so a name the
/// model does declare must be answerable.
#[tokio::test]
async fn every_modelled_attribute_name_is_reported() {
    let case = "the modelled AccountAttributeName catalog";
    let modelled = ec2::ops::types::ACCOUNT_ATTRIBUTE_NAME_VALUES;
    for name in modelled {
        assert!(
            DOCUMENTED_ATTRIBUTES.contains(name),
            "case {case:?}: expected the modelled name {name:?} to be a documented account \
             attribute, got {DOCUMENTED_ATTRIBUTES:?}"
        );
    }

    let mut declared: Vec<&str> = modelled.to_vec();
    declared.sort_unstable();
    let mut under_test = MODELLED_ATTRIBUTES.to_vec();
    under_test.sort_unstable();
    assert_eq!(
        under_test, declared,
        "case {case:?}: expected the modelled names under test to be the model's own enum, \
         got {under_test:?} against {declared:?}"
    );

    let router = emulator();
    let attributes = describe(&router, REGION, case, &[]).await;
    for name in modelled {
        let values = values_of(case, &attributes, name);
        assert!(
            !values.is_empty(),
            "case {case:?}: expected {name:?} to carry a value, got {values:?}"
        );
    }
}

/// A numeric attribute reports a number, and the platform attribute reports a platform the
/// account can launch into — so a caller that parses the value gets one.
///
/// The rows are the attributes whose value is NOT an id: the three quotas, read as integers,
/// and `supported-platforms`, whose value is the platform word for a VPC-only account.
#[tokio::test]
async fn the_quota_attributes_report_numbers_and_the_platform_reports_a_platform() {
    let case = "the shape of each value";
    let router = emulator();
    let attributes = describe(&router, REGION, case, &[]).await;

    for name in [
        "max-instances",
        "max-elastic-ips",
        "vpc-max-elastic-ips",
        "vpc-max-security-groups-per-interface",
    ] {
        let values = values_of(case, &attributes, name);
        assert_eq!(
            values.len(),
            1,
            "case {case:?}: expected {name:?} to carry one value, got {values:?}"
        );
        let parsed = values[0].parse::<u32>();
        assert!(
            parsed.is_ok_and(|count| count > 0),
            "case {case:?}: expected {name:?} to report a positive whole number, got \
             {values:?}"
        );
    }

    // "VPC" is the platform of an account that launches only into VPCs, which is every
    // account this emulator serves: EC2-Classic is retired and every launch here resolves a
    // subnet of a VPC.
    let platforms = values_of(case, &attributes, "supported-platforms");
    assert_eq!(
        platforms,
        vec!["VPC".to_string()],
        "case {case:?}: expected the account's platforms to be [\"VPC\"], got {platforms:?}"
    );
}

// ---------------------------------------------------------------------------
// Narrowing the report
// ---------------------------------------------------------------------------

/// `AttributeNames` reports EXACTLY the attributes it names, whatever and however many it
/// names.
///
/// The rows differ along the set the rule generalises over: each documented attribute alone,
/// the two the model's enum declares, every attribute at once, the documented order
/// reversed, and one attribute named twice (one attribute, not two).
#[tokio::test]
async fn naming_attributes_reports_exactly_those() {
    let router = emulator();
    let mut cases: Vec<(String, Vec<&str>, Vec<&str>)> = Vec::new();
    for name in DOCUMENTED_ATTRIBUTES {
        cases.push((format!("{name} alone"), vec![name], vec![name]));
    }
    cases.push((
        "the two the model declares".to_string(),
        MODELLED_ATTRIBUTES.to_vec(),
        MODELLED_ATTRIBUTES.to_vec(),
    ));
    cases.push((
        "every attribute at once".to_string(),
        DOCUMENTED_ATTRIBUTES.to_vec(),
        DOCUMENTED_ATTRIBUTES.to_vec(),
    ));
    let mut reversed = DOCUMENTED_ATTRIBUTES.to_vec();
    reversed.reverse();
    cases.push((
        "the documented order reversed".to_string(),
        reversed,
        DOCUMENTED_ATTRIBUTES.to_vec(),
    ));
    cases.push((
        "one attribute named twice".to_string(),
        vec!["max-elastic-ips", "max-elastic-ips"],
        vec!["max-elastic-ips"],
    ));

    for (case, named, expected) in cases {
        let attributes = describe(
            &router,
            REGION,
            &case,
            &indexed("AttributeName", &named),
        )
        .await;
        let mut reported_names = names_of(&attributes);
        reported_names.sort();
        let mut wanted: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        wanted.sort();
        assert_eq!(
            reported_names, wanted,
            "case {case:?}: expected exactly {wanted:?}, got {reported_names:?}"
        );
        for name in &expected {
            let values = values_of(&case, &attributes, name);
            assert!(
                !values.is_empty(),
                "case {case:?}: expected {name:?} to carry a value, got {values:?}"
            );
        }
    }
}

/// A narrowed report carries the SAME value for an attribute as the whole report does:
/// naming an attribute changes which rows are reported, never what one row says.
#[tokio::test]
async fn narrowing_the_report_does_not_change_a_value() {
    let case = "one attribute at a time against the whole account";
    let router = emulator();
    let whole = describe(&router, REGION, case, &[]).await;

    for name in DOCUMENTED_ATTRIBUTES {
        let narrowed = describe(
            &router,
            REGION,
            case,
            &indexed("AttributeName", &[name]),
        )
        .await;
        let alone = values_of(case, &narrowed, name);
        let together = values_of(case, &whole, name);
        assert_eq!(
            alone, together,
            "case {case:?}: expected {name:?} to report {together:?} whether named or not, \
             got {alone:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// `default-vpc` is the account's own VPC
// ---------------------------------------------------------------------------

/// `default-vpc` is the id of the default VPC the account holds IN THE REGION asked in — the
/// very VPC that Region's `DescribeVpcs` reports under the `isDefault` filter — and the two
/// reports agree.
///
/// The rows are several Regions, so the attribute is read per Region rather than once: an
/// account has one default VPC in each Region, and no two Regions share it.
#[tokio::test]
async fn the_default_vpc_attribute_is_the_default_vpc_of_the_region() {
    // Regions of three different partitions' worth of geography, so the rule is checked on
    // more than the Region the store seeds first.
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2", "sa-east-1"];
    let router = emulator();
    let mut seen: Vec<String> = Vec::new();

    for region in regions {
        let case = format!("the default VPC of {region}");
        let attribute = reported_default_vpc(&router, region, &case).await;
        let described = described_default_vpc(&router, region, &case).await;
        assert_eq!(
            Some(attribute.clone()),
            described,
            "case {case:?}: expected the attribute to name the VPC DescribeVpcs reports, got \
             {attribute:?} against {described:?}"
        );
        assert!(
            attribute.starts_with("vpc-"),
            "case {case:?}: expected a VPC id, got {attribute:?}"
        );
        assert!(
            !seen.contains(&attribute),
            "case {case:?}: expected each Region's default VPC to be its own, got \
             {attribute:?} again after {seen:?}"
        );
        seen.push(attribute);
    }
}

/// An account with NO default VPC in the Region asked in reports the word `none`: "The ID of
/// the default VPC for your account, or none".
///
/// The row is a well-formed Region code that names no Region of the catalog — an account has
/// a default VPC in the Regions that exist, not in every string shaped like a Region code —
/// and `DescribeVpcs` reports no default VPC there either, so both reports agree.
#[tokio::test]
async fn a_region_with_no_default_vpc_reports_none() {
    let router = emulator();
    for region in ["us-east-9", "zz-central-3"] {
        let case = format!("a Region of no partition: {region}");
        let attribute = reported_default_vpc(&router, region, &case).await;
        assert_eq!(
            attribute, NO_DEFAULT_VPC,
            "case {case:?}: expected {NO_DEFAULT_VPC:?}, got {attribute:?}"
        );
        let described = described_default_vpc(&router, region, &case).await;
        assert_eq!(
            described, None,
            "case {case:?}: expected DescribeVpcs to report no default VPC there, got \
             {described:?}"
        );
    }
}

/// The attribute TRACKS the account's state: take the default VPC of a Region away and the
/// report says `none`; ask for a new default VPC and the report names the one just created.
///
/// The whole round trip runs through the API — the subnets of the default VPC are deleted,
/// then the VPC itself, then `CreateDefaultVpc` gives the Region a new one — so the value is
/// read back out of the same store every other operation writes.
#[tokio::test]
async fn the_attribute_follows_the_default_vpc_through_a_delete_and_a_create() {
    let case = "the default VPC deleted and created again";
    let router = emulator();
    let region = "eu-west-1";

    let first = reported_default_vpc(&router, region, case).await;
    assert!(
        first.starts_with("vpc-"),
        "case {case:?}: expected a VPC id to start from, got {first:?}"
    );

    // "You must terminate all running instances in the subnet before you can delete the
    // subnet" and a VPC's subnets are what hold it (`DeleteVpc`), so the default subnet of
    // every Availability Zone of the Region goes first.
    for subnet_id in subnets_of(&router, region, case, &first).await {
        let (status, body) = call_in(
            &router,
            region,
            "DeleteSubnet",
            &[param("SubnetId", &subnet_id)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 deleting {subnet_id}, got {status} with body \
             {body}"
        );
    }
    let (status, body) = call_in(&router, region, "DeleteVpc", &[param("VpcId", &first)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 deleting {first}, got {status} with body {body}"
    );

    let without = reported_default_vpc(&router, region, case).await;
    assert_eq!(
        without, NO_DEFAULT_VPC,
        "case {case:?}: expected {NO_DEFAULT_VPC:?} once the account has no default VPC \
         there, got {without:?}"
    );

    let (status, body) = call_in(&router, region, "CreateDefaultVpc", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateDefaultVpc, got {status} with body {body}"
    );
    let created = super::wire::member(case, &body, "vpcId");
    assert_ne!(
        created, first,
        "case {case:?}: expected a NEW default VPC, got the deleted one {first:?}"
    );

    let after = reported_default_vpc(&router, region, case).await;
    assert_eq!(
        after, created,
        "case {case:?}: expected the attribute to name the created VPC {created:?}, got \
         {after:?}"
    );
    let described = described_default_vpc(&router, region, case).await;
    assert_eq!(
        described,
        Some(created.clone()),
        "case {case:?}: expected DescribeVpcs to report {created:?} as the default VPC, got \
         {described:?}"
    );
}

/// `DescribeAccountAttributes` is a REPORT, so reading cannot change what a later caller
/// sees: a run of calls with nothing between them reports exactly the same attributes, and
/// the VPC the first call reported is still the account's afterwards.
#[tokio::test]
async fn reading_the_attributes_never_changes_them() {
    /// Reads per Region — enough to catch a read that mints or consumes something.
    const READS: usize = 4;

    let router = emulator();
    for region in [REGION, "us-east-9"] {
        let case = format!("repeated reads in {region}");
        let first = describe(&router, region, &case, &[]).await;
        for read in 2..=READS {
            let case = format!("{case}, read {read}");
            let again = describe(&router, region, &case, &[]).await;
            assert_eq!(
                again, first,
                "case {case:?}: expected the same report as the first read {first:?}, got \
                 {again:?}"
            );
        }
        let described = described_default_vpc(&router, region, &case).await;
        let attribute = values_of(&case, &first, "default-vpc");
        let expected = described.unwrap_or_else(|| NO_DEFAULT_VPC.to_string());
        assert_eq!(
            attribute,
            vec![expected.clone()],
            "case {case:?}: expected default-vpc to still be {expected:?} after the reads, \
             got {attribute:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The refusals
// ---------------------------------------------------------------------------

/// A name that is no account attribute is REFUSED, not answered with an empty report: a
/// caller that misspells one has to see it.
///
/// The rows differ along the way the name is wrong: the right attribute spelled with
/// underscores, a name of another API's attribute, the wrong casing, an empty name, and a
/// bad name beside a good one — none of which is an account attribute. The refusal is EC2's
/// own parameter error, naming the value the request sent and nothing of the catalog, never a
/// report and never a 501 saying the operation is unimplemented.
#[tokio::test]
async fn a_name_that_is_no_account_attribute_is_refused() {
    let router = emulator();
    for named in [
        vec!["default_vpc"],
        vec!["max_instances"],
        vec!["enableDnsSupport"],
        vec!["Default-VPC"],
        vec![""],
        vec!["default-vpc", "no-such-attribute"],
    ] {
        let case = format!("naming {named:?}");
        let (status, body) = call(
            &router,
            "DescribeAccountAttributes",
            &indexed("AttributeName", &named),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert!(
            !body.contains("accountAttributeSet"),
            "case {case:?}: a refusal reports no attribute at all, got body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code,
            ec2::state::INVALID_PARAMETER,
            "case {case:?}: expected error code {:?}, got {code:?} with body {body}",
            ec2::state::INVALID_PARAMETER
        );
        // The offending name is the FIRST one of the row that is no account attribute \u2014 the
        // order the request lists them in \u2014 and the sentence quotes it and says nothing
        // else about the catalog.
        let offender = named
            .iter()
            .find(|name| !DOCUMENTED_ATTRIBUTES.contains(name))
            .expect("every row names at least one value that is no account attribute");
        let message = error_message(&body);
        assert_eq!(
            message,
            format!("The property '{offender}' is not valid"),
            "case {case:?}: expected the refusal to name {offender:?}, got {message:?}"
        );
    }
}

/// A `DryRun` request "would have succeeded", so it is refused `DryRunOperation` at HTTP 412
/// and reports nothing: "If you have the required permissions, the error response is
/// DryRunOperation" (`DryRun`). Never a 501, which would say the operation is unimplemented.
///
/// The rows are a dry run with no names and one that narrows the report, so the flag is
/// honoured whatever else the request carries; and `DryRun=false` is not a dry run at all.
#[tokio::test]
async fn a_dry_run_is_refused_with_dry_run_operation() {
    let router = emulator();
    let cases: [(&str, Vec<Param>); 3] = [
        (
            "a dry run of the whole account",
            vec![param("DryRun", "true")],
        ),
        (
            "a dry run of one attribute",
            vec![
                param("DryRun", "true"),
                param("AttributeName.1", "default-vpc"),
            ],
        ),
        (
            "a dry run that also names every attribute",
            {
                let mut params = vec![param("DryRun", "true")];
                params.extend(indexed("AttributeName", &DOCUMENTED_ATTRIBUTES));
                params
            },
        ),
    ];

    for (case, params) in cases {
        let (status, body) = call(&router, "DescribeAccountAttributes", &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code,
            ec2::state::DRY_RUN_OPERATION,
            "case {case:?}: expected error code {:?}, got {code:?} with body {body}",
            ec2::state::DRY_RUN_OPERATION
        );
    }

    // `DryRun=false` is not a dry run: the request runs and reports the account.
    let case = "DryRun=false";
    let attributes = describe(&router, REGION, case, &[param("DryRun", "false")]).await;
    assert_eq!(
        attributes.len(),
        DOCUMENTED_ATTRIBUTES.len(),
        "case {case:?}: expected the whole account, got {attributes:?}"
    );
}
