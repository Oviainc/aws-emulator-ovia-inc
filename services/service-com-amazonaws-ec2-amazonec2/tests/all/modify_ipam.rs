// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ModifyIpam` and the `DescribeIpams` that reads it back, driven over
//! the wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=ModifyIpam`, answered
//! with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that `ModifyIpam` APPLIES EXACTLY THE MEMBERS THE REQUEST CARRIES
//! to the stored IPAM it names and leaves every other member as it was: the description, the
//! tier, the metered account and the private-GUA setting it names replace the stored ones,
//! the operating Regions are the SET the IPAM holds less the ones removed plus the ones added,
//! and everything the IPAM's own creation derived — its id, its ARN, its owner, its home
//! Region, its default scopes and resource discovery, its counts and its tags — is untouched.
//! The modified IPAM is answered in the non-terminal `modify-in-progress` state the clock then
//! owes `modify-complete`, and every member of it reads back through `DescribeIpams`.
//!
//! Each table row is a distinct input along what that generalises over: which member the
//! request carries, how many operating Regions it adds and removes, whether a Region named is
//! one the IPAM already holds, and how often a Region is named — so the one case a recording
//! happens to contain is one row among many rather than the specification.

use std::time::Duration;

use ec2::state::{
    INVALID_IPAM_ID_MALFORMED, INVALID_IPAM_ID_NOT_FOUND, IPAM_STATE_CREATE_COMPLETE,
    IPAM_STATE_CREATE_IN_PROGRESS, IPAM_STATE_MODIFY_COMPLETE, IPAM_STATE_MODIFY_IN_PROGRESS,
    TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, error_code, error_message};
use super::wire::{child, child_text, document, param, Param};

/// The Region every call here is signed for, which is therefore each IPAM's home Region.
const REGION: &str = "us-east-1";

/// The description the IPAM every case starts from carries, so a row that changes it can tell
/// the new value from the old one.
const BASELINE_DESCRIPTION: &str = "the IPAM before the modification";

/// The operating Regions, beside its home Region, the IPAM every case starts from holds.
const BASELINE_REGIONS: [&str; 2] = ["eu-west-1", "ap-south-1"];

/// Longer than the span the clock owes an IPAM a `ModifyIpam` left `modify-in-progress`, so a
/// read after this much has certainly passed the due instant.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it: the router and
/// the engine its store lives in.
fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// One call of `action` signed for the Region above.
async fn invoke(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// One call of `action` requiring success, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = invoke(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// The form fields for an operating-Region list: `OperatingRegion.N.RegionName` for a create,
/// `AddOperatingRegion.N.RegionName` / `RemoveOperatingRegion.N.RegionName` for a modify.
fn region_params(prefix: &str, regions: &[&str]) -> Vec<Param> {
    regions
        .iter()
        .enumerate()
        .map(|(index, region)| param(&format!("{prefix}.{}.RegionName", index + 1), region))
        .collect()
}
/// One IPAM as a response reports it: every member read off the wire, with the operating
/// Regions and the tags flattened into comparable values.
///
/// Comparing WHOLE records is what makes "and nothing else changed" an assertion rather than a
/// list of members somebody remembered to check.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedIpam {
    ipam_id: Option<String>,
    ipam_arn: Option<String>,
    ipam_region: Option<String>,
    owner_id: Option<String>,
    public_default_scope_id: Option<String>,
    private_default_scope_id: Option<String>,
    scope_count: Option<String>,
    default_resource_discovery_id: Option<String>,
    default_resource_discovery_association_id: Option<String>,
    resource_discovery_association_count: Option<String>,
    description: Option<String>,
    tier: Option<String>,
    metered_account: Option<String>,
    enable_private_gua: Option<String>,
    state: Option<String>,
    state_message: Option<String>,
    /// The `regionName`s of the IPAM, sorted: operating Regions are a SET
    /// (`Ipam$OperatingRegions`), so the order they are reported in is not part of the rule,
    /// but how MANY times each appears is — sorting rather than collecting into a set is what
    /// lets a duplicate fail the comparison.
    operating_regions: Vec<String>,
    tags: Vec<(String, String)>,
}

/// The one IPAM a response carries, or a panic naming the case.
///
/// Only DIRECT children of the IPAM element are read, so a `regionName` of a nested
/// `operatingRegionSet` item or a `key` of a `tagSet` item is never mistaken for a member of
/// the IPAM itself.
fn reported(case: &str, body: &str) -> ReportedIpam {
    let parsed = document(case, body);
    let ipam = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "ipamId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no IPAM (no <ipamId>) in {body}"));
    let read = |name: &str| child_text(ipam, name);
    let items = |set: &str| {
        child(ipam, set)
            .map(|node| {
                node.children()
                    .filter(|item| item.is_element() && item.tag_name().name() == "item")
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    let mut operating_regions: Vec<String> = items("operatingRegionSet")
        .into_iter()
        .filter_map(|item| child_text(item, "regionName"))
        .collect();
    operating_regions.sort();
    ReportedIpam {
        ipam_id: read("ipamId"),
        ipam_arn: read("ipamArn"),
        ipam_region: read("ipamRegion"),
        owner_id: read("ownerId"),
        public_default_scope_id: read("publicDefaultScopeId"),
        private_default_scope_id: read("privateDefaultScopeId"),
        scope_count: read("scopeCount"),
        default_resource_discovery_id: read("defaultResourceDiscoveryId"),
        default_resource_discovery_association_id: read("defaultResourceDiscoveryAssociationId"),
        resource_discovery_association_count: read("resourceDiscoveryAssociationCount"),
        description: read("description"),
        tier: read("tier"),
        metered_account: read("meteredAccount"),
        enable_private_gua: read("enablePrivateGua"),
        state: read("state"),
        state_message: read("stateMessage"),
        operating_regions,
        tags: items("tagSet")
            .into_iter()
            .map(|item| {
                (
                    child_text(item, "key").unwrap_or_default(),
                    child_text(item, "value").unwrap_or_default(),
                )
            })
            .collect(),
    }
}

/// Create the IPAM every case starts from: every member a modification can change set to a
/// value no row's expectation shares with the default, so a member left alone can be told from
/// one that was reset.
async fn create_baseline_ipam(case: &str, router: &MultiServiceRouter) -> ReportedIpam {
    let mut params = vec![
        param("Description", BASELINE_DESCRIPTION),
        param("Tier", "advanced"),
        param("MeteredAccount", "resource-owner"),
        param("EnablePrivateGua", "true"),
        param("TagSpecification.1.ResourceType", "ipam"),
        param("TagSpecification.1.Tag.1.Key", "Name"),
        param("TagSpecification.1.Tag.1.Value", "baseline"),
    ];
    params.extend(region_params("OperatingRegion", &BASELINE_REGIONS));
    let body = ok(case, router, "CreateIpam", &params).await;
    reported(case, &body)
}

/// The `IpamId` of a reported IPAM, or a panic naming the case.
fn id_of(case: &str, ipam: &ReportedIpam) -> String {
    ipam.ipam_id
        .clone()
        .unwrap_or_else(|| panic!("case {case:?}: the created IPAM reports no id: {ipam:?}"))
}

/// The operating Regions of the baseline IPAM — its home Region and [`BASELINE_REGIONS`] —
/// with `removed` taken out and `added` put in, sorted the way a report is read.
fn expected_regions(added: &[&str], removed: &[&str]) -> Vec<String> {
    let mut regions: Vec<String> = Vec::new();
    for region in BASELINE_REGIONS
        .iter()
        .copied()
        .chain(std::iter::once(REGION))
        .chain(added.iter().copied())
    {
        if !removed.contains(&region) && !regions.iter().any(|held| held.as_str() == region) {
            regions.push(region.to_string());
        }
    }
    regions.sort();
    regions
}

// ---------------------------------------------------------------------------
// What the modification applies, and what it leaves alone
// ---------------------------------------------------------------------------

/// The rule: `ModifyIpam` applies the members the request carries to the IPAM it names, leaves
/// every member the request does not carry exactly as it was, and resolves the operating
/// Regions as a SET — the ones held, less the ones removed, plus the ones added — then answers
/// the IPAM in the non-terminal `modify-in-progress` state, with every member of it reading
/// back through `DescribeIpams`.
///
/// The rows differ along what that generalises over: each modifiable member on its own, each
/// member of each two-member enum, both booleans, one Region added, one removed, several at
/// once, a Region already held added, a Region not held removed, a Region named twice, and
/// every member in one request.
#[tokio::test]
async fn modify_ipam_applies_the_members_the_request_carries_and_leaves_the_rest_as_they_were() {
    // (case, the request's form fields beside the id, expected description, tier, metered
    //  account, private GUA, Regions added, Regions removed)
    #[allow(clippy::type_complexity)]
    let cases: [(
        &str,
        Vec<Param>,
        &str,
        &str,
        &str,
        &str,
        Vec<&str>,
        Vec<&str>,
    ); 13] = [
        (
            "nothing but the id: every member stays as it was",
            Vec::new(),
            BASELINE_DESCRIPTION,
            "advanced",
            "resource-owner",
            "true",
            Vec::new(),
            Vec::new(),
        ),
        (
            "a new description",
            vec![param("Description", "the IPAM after the modification")],
            "the IPAM after the modification",
            "advanced",
            "resource-owner",
            "true",
            Vec::new(),
            Vec::new(),
        ),
        (
            "the other member of the tier enum",
            vec![param("Tier", "free")],
            BASELINE_DESCRIPTION,
            "free",
            "resource-owner",
            "true",
            Vec::new(),
            Vec::new(),
        ),
        (
            "the tier it is already in",
            vec![param("Tier", "advanced")],
            BASELINE_DESCRIPTION,
            "advanced",
            "resource-owner",
            "true",
            Vec::new(),
            Vec::new(),
        ),
        (
            "the other member of the metered-account enum",
            vec![param("MeteredAccount", "ipam-owner")],
            BASELINE_DESCRIPTION,
            "advanced",
            "ipam-owner",
            "true",
            Vec::new(),
            Vec::new(),
        ),
        (
            "private GUA turned off",
            vec![param("EnablePrivateGua", "false")],
            BASELINE_DESCRIPTION,
            "advanced",
            "resource-owner",
            "false",
            Vec::new(),
            Vec::new(),
        ),
        (
            "one operating Region added",
            region_params("AddOperatingRegion", &["ap-northeast-1"]),
            BASELINE_DESCRIPTION,
            "advanced",
            "resource-owner",
            "true",
            vec!["ap-northeast-1"],
            Vec::new(),
        ),
        (
            "one operating Region removed",
            region_params("RemoveOperatingRegion", &["eu-west-1"]),
            BASELINE_DESCRIPTION,
            "advanced",
            "resource-owner",
            "true",
            Vec::new(),
            vec!["eu-west-1"],
        ),
        (
            "two added and one removed in one request",
            {
                let mut params = region_params("AddOperatingRegion", &["us-west-2", "ca-central-1"]);
                params.extend(region_params("RemoveOperatingRegion", &["ap-south-1"]));
                params
            },
            BASELINE_DESCRIPTION,
            "advanced",
            "resource-owner",
            "true",
            vec!["us-west-2", "ca-central-1"],
            vec!["ap-south-1"],
        ),
        (
            // Operating Regions are a set, so adding one already in it adds nothing.
            "an operating Region the IPAM already holds added",
            region_params("AddOperatingRegion", &["eu-west-1"]),
            BASELINE_DESCRIPTION,
            "advanced",
            "resource-owner",
            "true",
            Vec::new(),
            Vec::new(),
        ),
        (
            // …and removing one that is not in it removes nothing.
            "an operating Region the IPAM does not hold removed",
            region_params("RemoveOperatingRegion", &["sa-east-1"]),
            BASELINE_DESCRIPTION,
            "advanced",
            "resource-owner",
            "true",
            Vec::new(),
            Vec::new(),
        ),
        (
            "one Region named twice in the same list",
            region_params("AddOperatingRegion", &["eu-central-1", "eu-central-1"]),
            BASELINE_DESCRIPTION,
            "advanced",
            "resource-owner",
            "true",
            vec!["eu-central-1"],
            Vec::new(),
        ),
        (
            "every modifiable member in one request",
            {
                let mut params = vec![
                    param("Description", "every member at once"),
                    param("Tier", "free"),
                    param("MeteredAccount", "ipam-owner"),
                    param("EnablePrivateGua", "false"),
                ];
                params.extend(region_params("AddOperatingRegion", &["eu-north-1"]));
                params.extend(region_params("RemoveOperatingRegion", &["ap-south-1"]));
                params
            },
            "every member at once",
            "free",
            "ipam-owner",
            "false",
            vec!["eu-north-1"],
            vec!["ap-south-1"],
        ),
    ];

    for (case, changes, description, tier, metered_account, private_gua, added, removed) in cases {
        // A fresh IPAM per row, so no row sees what another one changed.
        let (router, _engine) = emulator();
        let created = create_baseline_ipam(case, &router).await;
        let ipam_id = id_of(case, &created);

        let mut params = vec![param("IpamId", &ipam_id)];
        params.extend(changes);
        let modified = reported(case, &ok(case, &router, "ModifyIpam", &params).await);

        // What the row expects: the created IPAM with exactly the members this request
        // carried replaced, and the transient state the modification leaves it in.
        let expected = ReportedIpam {
            description: Some(description.to_string()),
            tier: Some(tier.to_string()),
            metered_account: Some(metered_account.to_string()),
            enable_private_gua: Some(private_gua.to_string()),
            operating_regions: expected_regions(&added, &removed),
            state: Some(IPAM_STATE_MODIFY_IN_PROGRESS.to_string()),
            ..created.clone()
        };
        assert_eq!(
            modified, expected,
            "case {case:?}: expected ModifyIpam to answer {expected:?}, got {modified:?}"
        );

        // The stored record is the one the answer reported: a modify and the describe that
        // reads it back cannot drift apart.
        let read = reported(
            case,
            &ok(
                case,
                &router,
                "DescribeIpams",
                &[param("IpamId.1", &ipam_id)],
            )
            .await,
        );
        assert_eq!(
            read, expected,
            "case {case:?}: expected DescribeIpams to report {expected:?}, got {read:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The state the modification leaves behind
// ---------------------------------------------------------------------------

/// The rule: a modified IPAM is answered in the non-terminal `modify-in-progress` state and
/// reaches `modify-complete` once the clock has made that move due — a caller acting straight
/// after the call still sees `modify-in-progress`, and a read never brings the move forward.
/// The IPAM's default resource discovery follows the IPAM it was created for, since that is
/// the discovery that monitors the Regions the IPAM operates in.
#[tokio::test]
async fn a_modified_ipam_completes_only_once_the_clock_says_so() {
    let (router, engine) = emulator();
    let case = "one operating Region added and the description changed";
    let created = create_baseline_ipam(case, &router).await;
    let ipam_id = id_of(case, &created);

    let mut params = vec![
        param("IpamId", &ipam_id),
        param("Description", "monitored in one more Region"),
    ];
    params.extend(region_params("AddOperatingRegion", &["ap-northeast-1"]));
    let modified = reported(case, &ok(case, &router, "ModifyIpam", &params).await);
    assert_eq!(
        modified.state.as_deref(),
        Some(IPAM_STATE_MODIFY_IN_PROGRESS),
        "case {case:?}: a caller acting straight after ModifyIpam still sees \
         modify-in-progress, got {modified:?}"
    );

    // Read again straight away: the state is a property of the clock, not of being read.
    let read = reported(
        case,
        &ok(
            case,
            &router,
            "DescribeIpams",
            &[param("IpamId.1", &ipam_id)],
        )
        .await,
    );
    assert_eq!(
        read.state.as_deref(),
        Some(IPAM_STATE_MODIFY_IN_PROGRESS),
        "case {case:?}: a read before the transition is due must not bring it forward, \
         got {read:?}"
    );

    // The default resource discovery of this IPAM, in the store the handlers share: it
    // discovers in the Regions the IPAM operates in, so the modification moved it too.
    let discovery_id = created
        .default_resource_discovery_id
        .clone()
        .unwrap_or_else(|| panic!("case {case:?}: the IPAM reports no default discovery"));
    {
        let state = ec2::state::service_state(&*engine).expect("the EC2 store");
        let store = state.lock();
        let discovery = store
            .ipam_resource_discoveries
            .get(&discovery_id)
            .unwrap_or_else(|| panic!("case {case:?}: discovery {discovery_id} is not stored"));
        let mut regions: Vec<String> = discovery
            .operating_regions
            .iter()
            .flatten()
            .filter_map(|operating| operating.region_name.clone())
            .collect();
        regions.sort();
        assert_eq!(
            regions,
            expected_regions(&["ap-northeast-1"], &[]),
            "case {case:?}: the default resource discovery operates where the IPAM does, \
             got {discovery:?}"
        );
        assert_eq!(
            discovery.description.as_deref(),
            Some("monitored in one more Region"),
            "case {case:?}: the default resource discovery carries the IPAM's description, \
             got {discovery:?}"
        );
    }

    tokio::time::sleep(past_the_transition()).await;

    let read = reported(
        case,
        &ok(
            case,
            &router,
            "DescribeIpams",
            &[param("IpamId.1", &ipam_id)],
        )
        .await,
    );
    assert_eq!(
        read.state.as_deref(),
        Some(IPAM_STATE_MODIFY_COMPLETE),
        "case {case:?}: a modified IPAM completes on its own once the move is due, got {read:?}"
    );
}

// ---------------------------------------------------------------------------
// What the modification refuses
// ---------------------------------------------------------------------------

/// The rule: a request the service cannot satisfy is refused with the EC2 error code for what
/// is wrong with it and nothing is changed — something that is not an IPAM id at all is the
/// IPAM's `Malformed` code, an id this account holds no IPAM for its `NotFound` code, a Region
/// name that is not one `InvalidParameterValue`, an operating Region with no name
/// `MissingParameter`, one Region both added and removed `InvalidParameterCombination`, a value
/// outside a modelled enum the boundary validator's own `ValidationError`, and a `DryRun` that
/// would have succeeded `DryRunOperation`.
#[tokio::test]
async fn modify_ipam_refuses_a_request_it_cannot_satisfy_and_changes_nothing() {
    let (router, _engine) = emulator();
    let created = create_baseline_ipam("the IPAM the refusals name", &router).await;
    let ipam_id = id_of("the IPAM the refusals name", &created);

    // (case, the request's form fields, the expected EC2 error code)
    let cases: Vec<(&str, Vec<Param>, &str)> = vec![
        (
            "an id that is not an IPAM id at all",
            vec![param("IpamId", "not-an-ipam")],
            INVALID_IPAM_ID_MALFORMED,
        ),
        (
            "an IPAM id of another resource's kind",
            vec![param("IpamId", "vpc-0123456789abcdef0")],
            INVALID_IPAM_ID_MALFORMED,
        ),
        (
            "a well-formed id this account holds no IPAM for",
            vec![param("IpamId", "ipam-0123456789abcdef0")],
            INVALID_IPAM_ID_NOT_FOUND,
        ),
        (
            "an added Region that is not a Region name",
            {
                let mut params = vec![param("IpamId", &ipam_id)];
                params.extend(region_params(
                    "AddOperatingRegion",
                    &["US East (N. Virginia)"],
                ));
                params
            },
            "InvalidParameterValue",
        ),
        (
            // Well formed as a Region code, but no such Region: an operating Region a REQUEST
            // names is held to the catalog of Regions AWS publishes, not to the grammar.
            "an added Region AWS does not publish",
            {
                let mut params = vec![param("IpamId", &ipam_id)];
                params.extend(region_params("AddOperatingRegion", &["eu-west-9"]));
                params
            },
            "InvalidParameterValue",
        ),
        (
            // A Region of another partition than the one this emulator's ARNs name.
            "an added Region of another partition",
            {
                let mut params = vec![param("IpamId", &ipam_id)];
                params.extend(region_params("AddOperatingRegion", &["us-gov-west-1"]));
                params
            },
            "InvalidParameterValue",
        ),
        (
            "a removed Region AWS does not publish",
            {
                let mut params = vec![param("IpamId", &ipam_id)];
                params.extend(region_params("RemoveOperatingRegion", &["eu-west-9"]));
                params
            },
            "InvalidParameterValue",
        ),
        (
            "an added operating Region with no name",
            vec![
                param("IpamId", &ipam_id),
                param("AddOperatingRegion.1.RegionName", ""),
            ],
            "MissingParameter",
        ),
        (
            "a removed operating Region with no name",
            vec![
                param("IpamId", &ipam_id),
                param("RemoveOperatingRegion.1.RegionName", ""),
            ],
            "MissingParameter",
        ),
        (
            "the IPAM's own home Region removed",
            {
                let mut params = vec![param("IpamId", &ipam_id)];
                params.extend(region_params("RemoveOperatingRegion", &[REGION]));
                params
            },
            "InvalidParameterValue",
        ),
        (
            "one Region both added and removed",
            {
                let mut params = vec![param("IpamId", &ipam_id)];
                params.extend(region_params("AddOperatingRegion", &["us-west-2"]));
                params.extend(region_params("RemoveOperatingRegion", &["us-west-2"]));
                params
            },
            "InvalidParameterCombination",
        ),
        (
            "a tier the IpamTier enum does not define",
            vec![param("IpamId", &ipam_id), param("Tier", "premium")],
            "ValidationError",
        ),
        (
            "a tier in the wrong case",
            vec![param("IpamId", &ipam_id), param("Tier", "Free")],
            "ValidationError",
        ),
        (
            "a metered account the enum does not define",
            vec![param("IpamId", &ipam_id), param("MeteredAccount", "payer")],
            "ValidationError",
        ),
        (
            "a dry run that would have succeeded",
            vec![
                param("IpamId", &ipam_id),
                param("Description", "never stored"),
                param("DryRun", "true"),
            ],
            "DryRunOperation",
        ),
    ];

    for (case, params, expected) in cases {
        let (status, body) = invoke(&router, "ModifyIpam", &params).await;
        assert_ne!(
            status, 200,
            "case {case:?}: the request must be refused, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected the error code {expected:?}, got {body}"
        );
    }

    // The refusal of the id nothing resolves names the id the REQUEST carried, so a caller can
    // tell which of its parameters was at fault.
    let (_status, body) = invoke(
        &router,
        "ModifyIpam",
        &[param("IpamId", "ipam-0123456789abcdef0")],
    )
    .await;
    assert_eq!(
        error_message(&body),
        "The IPAM (ipam-0123456789abcdef0) does not exist.",
        "the not-found refusal names the id the request carried, got {body}"
    );

    // Not one of those refusals changed the IPAM: it still reads back exactly as it was
    // created, and its state is still one of the CREATE states — nothing moved it into a
    // modify state, whatever the clock has made due of its creation by now.
    let read = reported(
        "after the refusals",
        &ok(
            "after the refusals",
            &router,
            "DescribeIpams",
            &[param("IpamId.1", &ipam_id)],
        )
        .await,
    );
    assert!(
        read.state.as_deref() == Some(IPAM_STATE_CREATE_IN_PROGRESS)
            || read.state.as_deref() == Some(IPAM_STATE_CREATE_COMPLETE),
        "a refused modification leaves the IPAM in the state its creation gave it, \
         got {read:?}"
    );
    let unchanged = ReportedIpam {
        state: created.state.clone(),
        ..read.clone()
    };
    assert_eq!(
        unchanged, created,
        "a refused modification changes nothing: expected {created:?}, got {read:?}"
    );
}
