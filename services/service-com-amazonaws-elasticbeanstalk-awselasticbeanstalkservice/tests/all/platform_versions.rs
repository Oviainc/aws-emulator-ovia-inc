// SPDX-License-Identifier: CC0-1.0

//! `ListPlatformVersions`, driven end to end over the service's awsQuery HTTP
//! surface.
//!
//! The suite asserts the RULES the operation follows rather than the catalogue's
//! contents: every summary it reports is a platform version addressed in the
//! region the request was signed for; a `PlatformFilter` term selects
//! `Type Operator Values[1]` over the attribute `Type` names, several terms
//! being a conjunction; `MaxRecords`/`NextToken` walk the same total order in
//! pages without losing or repeating a version; and a term the service cannot
//! evaluate is refused with the service's own error code rather than ignored.
//!
//! Every expectation is computed from the UNFILTERED response, so the suite
//! holds as the published catalogue grows: a row states the rule, not the rows
//! it happens to select today.
//!
//! The HTTP harness ([`crate::lifecycle::Emulator`]) is the one the lifecycle
//! suite drives the service with.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use crate::lifecycle::{elements, Answer, Emulator, TEST_REGION};

/// The value set of `PlatformBranchLifecycleState` as a PLATFORM VERSION reports
/// it, from `PlatformDescription$PlatformBranchLifecycleState`: "Possible
/// values: `Beta` | `Supported` | `Deprecated` | `Retired`" — the capitalized
/// spelling the recorded platform-version responses carry. (`PlatformSummary`'s
/// own doc lists the same four in lower case; the two cannot both describe one
/// attribute, and the recording settles it. `ListPlatformBranches` reports its
/// own `LifecycleState` member, which is that suite's subject, not this one's.)
const BRANCH_LIFECYCLE_STATES: &[&str] = &["Beta", "Supported", "Deprecated", "Retired"];

/// The value set of `PlatformLifecycleState`, from
/// `PlatformDescription$PlatformLifecycleState`: "Possible values:
/// `Recommended` | `null`" — again the spelling the recorded responses carry,
/// where `PlatformSummary`'s doc spells it `recommended`.
const PLATFORM_LIFECYCLE_STATES: &[&str] = &["Recommended", ""];

/// The documented `PlatformStatus` values (the model's `PlatformStatus` enum).
const PLATFORM_STATUS_VALUES: &[&str] = &["Creating", "Failed", "Ready", "Deleting", "Deleted"];

/// The documented `SupportedTier` values, from
/// `PlatformBranchSummary$SupportedTierList`: "Possible values:
/// `WebServer/Standard` | `Worker/SQS/HTTP`".
const SUPPORTED_TIERS: &[&str] = &["WebServer/Standard", "Worker/SQS/HTTP"];

/// The `SupportedAddon` values a published version can carry: the three
/// `PlatformFilter$Values` documents ("`Log/S3` | `Monitoring/Healthd` |
/// `WorkerDaemon/SQSD`" — a list the model introduces as covering only SOME
/// attributes) plus `Log/CloudWatch`, which the recorded responses carry beside
/// them.
const SUPPORTED_ADDONS: &[&str] = &[
    "Log/CloudWatch",
    "Log/S3",
    "Monitoring/Healthd",
    "WorkerDaemon/SQSD",
];

/// One `PlatformSummary` as the response carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Summary {
    arn: String,
    owner: String,
    status: String,
    category: String,
    operating_system_name: String,
    operating_system_version: String,
    tiers: Vec<String>,
    addons: Vec<String>,
    lifecycle_state: String,
    version: String,
    branch_name: String,
    branch_lifecycle_state: String,
}

impl Summary {
    /// Read one `<member>` of the response's `PlatformSummaryList`.
    fn parse(member: &str) -> Self {
        Self {
            arn: field(member, "PlatformArn"),
            owner: field(member, "PlatformOwner"),
            status: field(member, "PlatformStatus"),
            category: field(member, "PlatformCategory"),
            operating_system_name: field(member, "OperatingSystemName"),
            operating_system_version: field(member, "OperatingSystemVersion"),
            tiers: string_list(member, "SupportedTierList"),
            addons: string_list(member, "SupportedAddonList"),
            lifecycle_state: field(member, "PlatformLifecycleState"),
            version: field(member, "PlatformVersion"),
            branch_name: field(member, "PlatformBranchName"),
            branch_lifecycle_state: field(member, "PlatformBranchLifecycleState"),
        }
    }

    /// The platform name the ARN carries — the attribute the `PlatformName`
    /// filter type is applied to, which `PlatformSummary` reports only inside
    /// `PlatformArn` (`…:platform/<name>/<version>`).
    fn platform_name(&self) -> &str {
        let resource = self
            .arn
            .split(':')
            .next_back()
            .expect("an ARN has a resource part");
        let after_kind = resource.strip_prefix("platform/").unwrap_or(resource);
        match after_kind.rfind('/') {
            Some(at) => &after_kind[..at],
            None => after_kind,
        }
    }
}

/// The first `<tag>` directly in `member`, or `""` when the member carries it
/// empty (the `PlatformLifecycleState` of a version that is not its branch's
/// recommended one).
fn field(member: &str, tag: &str) -> String {
    elements(member, tag)
        .first()
        .map(|value| (*value).to_string())
        .unwrap_or_default()
}

/// The `<member>` values of the `wrapper` list inside `member`.
fn string_list(member: &str, wrapper: &str) -> Vec<String> {
    elements(member, wrapper)
        .first()
        .map(|list| {
            elements(list, "member")
                .into_iter()
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Every `PlatformSummary` in `answer`, in response order.
fn summaries(answer: &Answer) -> Vec<Summary> {
    answer
        .values_at("PlatformSummaryList/member")
        .into_iter()
        .map(Summary::parse)
        .collect()
}

/// `answer`'s `NextToken`, absent on a final page.
fn next_token(answer: &Answer) -> Option<String> {
    answer.value("NextToken").map(str::to_string)
}

/// The ARNs of `summaries`, in order — what a page is identified by.
fn arns(summaries: &[Summary]) -> Vec<String> {
    summaries
        .iter()
        .map(|summary| summary.arn.clone())
        .collect()
}

/// Order two version strings the way a version ordering must: component by
/// component numerically, so `4.0.11` is ABOVE `4.0.9`. This is the test's own
/// statement of the rule the `<`/`<=`/`>`/`>=` filter operators follow.
fn version_cmp(left: &str, right: &str) -> Ordering {
    let components = |value: &str| -> Option<Vec<u64>> {
        value
            .split('.')
            .map(|component| component.parse::<u64>().ok())
            .collect()
    };
    match (components(left), components(right)) {
        (Some(left), Some(right)) => left.cmp(&right),
        _ => left.cmp(right),
    }
}

/// Every platform version the emulator publishes in [`TEST_REGION`], asserted
/// to be a non-empty 200.
async fn all_versions(emulator: &Emulator, case: &str) -> Vec<Summary> {
    let answer = emulator.call("ListPlatformVersions", &[]).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from ListPlatformVersions, got {} and body {}",
        answer.status, answer.body
    );
    let all = summaries(&answer);
    assert!(
        !all.is_empty(),
        "case {case:?}: ListPlatformVersions must report the platform versions available to the \
         account, got {}",
        answer.body
    );
    all
}

// ---------------------------------------------------------------------------
// What a reported platform version is
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_reported_platform_version_is_a_well_formed_summary() {
    let emulator = Emulator::new();
    for summary in all_versions(&emulator, "summary shape").await {
        let case = summary.arn.clone();
        // The ARN names this platform version, in the region the request was
        // signed for and under this emulator's account.
        assert_eq!(
            summary.arn,
            format!(
                "arn:aws:elasticbeanstalk:{TEST_REGION}::platform/{}/{}",
                summary.platform_name(),
                summary.version
            ),
            "case {case:?}: a managed PlatformArn names the platform and the version in the \
             signed region, with no account id, got {:?}",
            summary.arn
        );
        assert!(
            !summary.version.is_empty(),
            "case {case:?}: expected a PlatformVersion, got {:?}",
            summary.version
        );
        assert!(
            PLATFORM_STATUS_VALUES.contains(&summary.status.as_str()),
            "case {case:?}: PlatformStatus must be a modelled value, got {:?}",
            summary.status
        );
        assert!(
            !summary.owner.is_empty(),
            "case {case:?}: expected a PlatformOwner, got {:?}",
            summary.owner
        );
        assert!(
            PLATFORM_LIFECYCLE_STATES.contains(&summary.lifecycle_state.as_str()),
            "case {case:?}: PlatformLifecycleState must be \"Recommended\" or empty, got {:?}",
            summary.lifecycle_state
        );
        assert!(
            BRANCH_LIFECYCLE_STATES.contains(&summary.branch_lifecycle_state.as_str()),
            "case {case:?}: PlatformBranchLifecycleState must be a documented value, got {:?}",
            summary.branch_lifecycle_state
        );
        assert!(
            !summary.branch_name.is_empty(),
            "case {case:?}: expected a PlatformBranchName, got {:?}",
            summary.branch_name
        );
        assert!(
            !summary.tiers.is_empty()
                && summary
                    .tiers
                    .iter()
                    .all(|tier| SUPPORTED_TIERS.contains(&tier.as_str())),
            "case {case:?}: SupportedTierList must be documented tiers, got {:?}",
            summary.tiers
        );
        assert!(
            summary
                .addons
                .iter()
                .all(|addon| SUPPORTED_ADDONS.contains(&addon.as_str())),
            "case {case:?}: SupportedAddonList must be documented addons, got {:?}",
            summary.addons
        );
        // A published version states its category and the operating system it
        // runs on: both are catalog data, not something a platform definition
        // archive has to be read for.
        assert!(
            !summary.category.is_empty(),
            "case {case:?}: expected a PlatformCategory, got {:?}",
            summary.category
        );
        assert!(
            !summary.operating_system_name.is_empty()
                && !summary.operating_system_version.is_empty(),
            "case {case:?}: expected an operating system name and version, got {:?} and {:?}",
            summary.operating_system_name,
            summary.operating_system_version
        );
        assert!(
            !summary.addons.is_empty(),
            "case {case:?}: expected the add-ons the version supports, got {:?}",
            summary.addons
        );
    }
}

#[tokio::test]
async fn the_catalogue_spans_branches_versions_and_lifecycle_states() {
    let emulator = Emulator::new();
    let all = all_versions(&emulator, "catalogue spread").await;

    // A platform version is identified by its ARN, so no two may share one.
    let mut unique = arns(&all);
    unique.sort();
    let before = unique.len();
    unique.dedup();
    assert_eq!(
        unique.len(),
        before,
        "case \"catalogue spread\": every PlatformArn must be unique, got {:?}",
        arns(&all)
    );

    // Only the newest version of a branch is its recommended one, so a branch
    // holding more than one version must report a non-recommended member: the
    // empty `PlatformLifecycleState` has to be reachable.
    let mut branches: Vec<&str> = all
        .iter()
        .map(|summary| summary.branch_name.as_str())
        .collect();
    branches.sort_unstable();
    branches.dedup();
    assert!(
        branches.len() > 1,
        "case \"catalogue spread\": expected several platform branches, got {branches:?}"
    );
    for branch in branches {
        let mut in_branch: Vec<&Summary> = all
            .iter()
            .filter(|summary| summary.branch_name == branch)
            .collect();
        in_branch.sort_by(|left, right| version_cmp(&right.version, &left.version));
        let recommended: Vec<&str> = in_branch
            .iter()
            .filter(|summary| summary.lifecycle_state == "Recommended")
            .map(|summary| summary.version.as_str())
            .collect();
        assert_eq!(
            recommended,
            vec![in_branch[0].version.as_str()],
            "case {branch:?}: exactly the newest version of a branch is its recommended one, got \
             {recommended:?} recommended out of {:?}",
            in_branch
                .iter()
                .map(|summary| summary.version.as_str())
                .collect::<Vec<&str>>()
        );
    }
}

#[tokio::test]
async fn every_version_agrees_with_the_branch_list_platform_branches_reports() {
    let emulator = Emulator::new();
    let all = all_versions(&emulator, "branch agreement").await;

    // `ListPlatformBranches` reports the branches of the same published
    // catalogue, so a version's branch attributes must BE that branch's rather
    // than a second copy of them: one life-cycle state and one tier list per
    // branch, whichever operation is asked.
    let branches = emulator.call("ListPlatformBranches", &[]).await;
    assert_eq!(
        branches.status, 200,
        "case \"branch agreement\": expected 200 from ListPlatformBranches, got {} and body {}",
        branches.status, branches.body
    );
    let reported: BTreeMap<String, (String, Vec<String>)> = branches
        .values_at("PlatformBranchSummaryList/member")
        .into_iter()
        .map(|member| {
            (
                field(member, "BranchName"),
                (
                    field(member, "LifecycleState"),
                    string_list(member, "SupportedTierList"),
                ),
            )
        })
        .collect();
    assert!(
        !reported.is_empty(),
        "case \"branch agreement\": expected ListPlatformBranches to report branches, got {}",
        branches.body
    );

    for summary in &all {
        let case = summary.arn.clone();
        let Some((lifecycle_state, tiers)) = reported.get(&summary.branch_name) else {
            panic!(
                "case {case:?}: the branch {:?} a version belongs to must be one \
                 ListPlatformBranches reports, which reports {:?}",
                summary.branch_name,
                reported.keys().collect::<Vec<&String>>()
            );
        };
        // One life-cycle state per branch, whichever operation is asked: the
        // platform-version surface reports it capitalized
        // (`PlatformDescription$PlatformBranchLifecycleState`) and
        // `ListPlatformBranches` reports its own `LifecycleState` member, so the
        // two must name the same state rather than carry the same text.
        assert!(
            summary
                .branch_lifecycle_state
                .eq_ignore_ascii_case(lifecycle_state),
            "case {case:?}: PlatformBranchLifecycleState must be the branch's own state, got \
             {:?} against {lifecycle_state:?}",
            summary.branch_lifecycle_state
        );
        assert_eq!(
            &summary.tiers, tiers,
            "case {case:?}: a version runs exactly the tiers its branch supports"
        );
        // The worker-daemon addon follows the worker tier: it is the add-on that
        // runs a WORKER environment, so a version whose branch supports no
        // worker tier does not carry it and every other version does.
        assert_eq!(
            summary
                .addons
                .iter()
                .any(|addon| addon == "WorkerDaemon/SQSD"),
            tiers.iter().any(|tier| tier == "Worker/SQS/HTTP"),
            "case {case:?}: the worker daemon addon must follow the worker tier, got {:?} for \
             tiers {tiers:?}",
            summary.addons
        );
    }
}

#[tokio::test]
async fn platform_arns_are_addressed_in_the_region_the_request_was_signed_for() {
    let emulator = Emulator::new();
    // Two regions that are not the emulator's fallback, so neither answer can
    // pass by defaulting.
    for region in ["eu-west-1", "ap-southeast-2"] {
        let answer = emulator
            .call_in_region(region, "ListPlatformVersions", &[])
            .await;
        assert_eq!(
            answer.status, 200,
            "case {region:?}: expected 200, got {} and body {}",
            answer.status, answer.body
        );
        let in_region = summaries(&answer);
        assert!(
            !in_region.is_empty(),
            "case {region:?}: expected platform versions, got {}",
            answer.body
        );
        for summary in &in_region {
            assert!(
                summary
                    .arn
                    .starts_with(&format!("arn:aws:elasticbeanstalk:{region}:")),
                "case {region:?}: a PlatformArn must carry the signed region, got {:?}",
                summary.arn
            );
        }
        // The catalogue itself is the same in every region: only the ARNs move.
        let names: Vec<&str> = in_region
            .iter()
            .map(|summary| summary.platform_name())
            .collect();
        let here = all_versions(&emulator, region).await;
        let home_names: Vec<&str> = here.iter().map(|summary| summary.platform_name()).collect();
        assert_eq!(
            names, home_names,
            "case {region:?}: the same platform versions must be published in every region"
        );
    }
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// One filter term and the rule it selects by, stated over a summary.
struct FilterCase {
    case: &'static str,
    params: &'static [(&'static str, &'static str)],
    /// Whether the term holds for this summary — the expectation, computed from
    /// the unfiltered response rather than from the catalogue's contents.
    holds: fn(&Summary) -> bool,
    /// Whether the term is expected to select FEWER than everything, so a term
    /// that silently matched everything cannot pass as working.
    narrows: bool,
}

#[tokio::test]
async fn a_filter_term_selects_by_the_attribute_its_type_names() {
    let emulator = Emulator::new();
    let all = all_versions(&emulator, "filters").await;

    let cases: &[FilterCase] = &[
        FilterCase {
            case: "PlatformStatus = Ready",
            params: &[
                ("Filters.member.1.Type", "PlatformStatus"),
                ("Filters.member.1.Operator", "="),
                ("Filters.member.1.Values.member.1", "Ready"),
            ],
            holds: |summary| summary.status == "Ready",
            narrows: false,
        },
        FilterCase {
            case: "PlatformStatus != Ready",
            params: &[
                ("Filters.member.1.Type", "PlatformStatus"),
                ("Filters.member.1.Operator", "!="),
                ("Filters.member.1.Values.member.1", "Ready"),
            ],
            holds: |summary| summary.status != "Ready",
            narrows: true,
        },
        // No operator: "Type Operator Values[1]" with the equality test. The
        // model documents the filter VALUE for this attribute in lower case
        // (`PlatformFilter$Values`: "PlatformLifecycleState: recommended") while
        // the state is REPORTED capitalized, so both spellings name the one
        // state.
        FilterCase {
            case: "PlatformLifecycleState Recommended, operator defaulted",
            params: &[
                ("Filters.member.1.Type", "PlatformLifecycleState"),
                ("Filters.member.1.Values.member.1", "Recommended"),
            ],
            holds: |summary| summary.lifecycle_state == "Recommended",
            narrows: false,
        },
        FilterCase {
            case: "PlatformLifecycleState recommended, the documented filter spelling",
            params: &[
                ("Filters.member.1.Type", "PlatformLifecycleState"),
                ("Filters.member.1.Operator", "="),
                ("Filters.member.1.Values.member.1", "recommended"),
            ],
            holds: |summary| summary.lifecycle_state == "Recommended",
            narrows: false,
        },
        FilterCase {
            case: "PlatformLifecycleState != recommended",
            params: &[
                ("Filters.member.1.Type", "PlatformLifecycleState"),
                ("Filters.member.1.Operator", "!="),
                ("Filters.member.1.Values.member.1", "recommended"),
            ],
            holds: |summary| summary.lifecycle_state != "Recommended",
            narrows: true,
        },
        FilterCase {
            case: "PlatformName contains Python",
            params: &[
                ("Filters.member.1.Type", "PlatformName"),
                ("Filters.member.1.Operator", "contains"),
                ("Filters.member.1.Values.member.1", "Python"),
            ],
            holds: |summary| summary.platform_name().contains("Python"),
            narrows: true,
        },
        FilterCase {
            case: "PlatformName begins_with Node.js",
            params: &[
                ("Filters.member.1.Type", "PlatformName"),
                ("Filters.member.1.Operator", "begins_with"),
                ("Filters.member.1.Values.member.1", "Node.js"),
            ],
            holds: |summary| summary.platform_name().starts_with("Node.js"),
            narrows: true,
        },
        FilterCase {
            case: "PlatformName ends_with 64bit Amazon Linux 2023",
            params: &[
                ("Filters.member.1.Type", "PlatformName"),
                ("Filters.member.1.Operator", "ends_with"),
                (
                    "Filters.member.1.Values.member.1",
                    "64bit Amazon Linux 2023",
                ),
            ],
            holds: |summary| summary.platform_name().ends_with("64bit Amazon Linux 2023"),
            narrows: true,
        },
        FilterCase {
            case: "PlatformBranchName = the Python 3.11 AL2023 branch",
            params: &[
                ("Filters.member.1.Type", "PlatformBranchName"),
                ("Filters.member.1.Operator", "="),
                (
                    "Filters.member.1.Values.member.1",
                    "Python 3.11 running on 64bit Amazon Linux 2023",
                ),
            ],
            holds: |summary| summary.branch_name == "Python 3.11 running on 64bit Amazon Linux 2023",
            narrows: true,
        },
        FilterCase {
            case: "PlatformOwner = AWSElasticBeanstalk",
            params: &[
                ("Filters.member.1.Type", "PlatformOwner"),
                ("Filters.member.1.Operator", "="),
                ("Filters.member.1.Values.member.1", "AWSElasticBeanstalk"),
            ],
            holds: |summary| summary.owner == "AWSElasticBeanstalk",
            narrows: false,
        },
        FilterCase {
            case: "PlatformOwner = a foreign account",
            params: &[
                ("Filters.member.1.Type", "PlatformOwner"),
                ("Filters.member.1.Operator", "="),
                ("Filters.member.1.Values.member.1", "999999999999"),
            ],
            holds: |summary| summary.owner == "999999999999",
            narrows: true,
        },
        // A multi-valued attribute: the term holds when ONE of its values does,
        // and the catalogue has a branch that runs only the web-server tier, so
        // a term on the worker tier narrows.
        FilterCase {
            case: "SupportedTier = Worker/SQS/HTTP",
            params: &[
                ("Filters.member.1.Type", "SupportedTier"),
                ("Filters.member.1.Operator", "="),
                ("Filters.member.1.Values.member.1", "Worker/SQS/HTTP"),
            ],
            holds: |summary| summary.tiers.iter().any(|tier| tier == "Worker/SQS/HTTP"),
            narrows: true,
        },
        FilterCase {
            case: "SupportedTier = WebServer/Standard",
            params: &[
                ("Filters.member.1.Type", "SupportedTier"),
                ("Filters.member.1.Operator", "="),
                ("Filters.member.1.Values.member.1", "WebServer/Standard"),
            ],
            holds: |summary| summary.tiers.iter().any(|tier| tier == "WebServer/Standard"),
            narrows: false,
        },
        // Every published version reports enhanced health, and the worker daemon
        // is an add-on only of a version whose branch runs the worker tier — and
        // the catalogue holds a branch that runs only the web-server tier — so
        // the first term holds everywhere and the second narrows.
        FilterCase {
            case: "SupportedAddon = Monitoring/Healthd",
            params: &[
                ("Filters.member.1.Type", "SupportedAddon"),
                ("Filters.member.1.Operator", "="),
                ("Filters.member.1.Values.member.1", "Monitoring/Healthd"),
            ],
            holds: |summary| summary.addons.iter().any(|addon| addon == "Monitoring/Healthd"),
            narrows: false,
        },
        FilterCase {
            case: "SupportedAddon = WorkerDaemon/SQSD",
            params: &[
                ("Filters.member.1.Type", "SupportedAddon"),
                ("Filters.member.1.Operator", "="),
                ("Filters.member.1.Values.member.1", "WorkerDaemon/SQSD"),
            ],
            holds: |summary| summary.addons.iter().any(|addon| addon == "WorkerDaemon/SQSD"),
            narrows: true,
        },
        FilterCase {
            case: "SupportedAddon = an addon no platform carries",
            params: &[
                ("Filters.member.1.Type", "SupportedAddon"),
                ("Filters.member.1.Operator", "="),
                ("Filters.member.1.Values.member.1", "Log/Nowhere"),
            ],
            holds: |summary| summary.addons.iter().any(|addon| addon == "Log/Nowhere"),
            narrows: true,
        },
        FilterCase {
            case: "OperatingSystemName = Amazon Linux",
            params: &[
                ("Filters.member.1.Type", "OperatingSystemName"),
                ("Filters.member.1.Operator", "="),
                ("Filters.member.1.Values.member.1", "Amazon Linux"),
            ],
            holds: |summary| summary.operating_system_name == "Amazon Linux",
            narrows: true,
        },
        // The ordering operators compare versions component by component, so
        // 4.0.11 is above 4.0.9 — a byte comparison would put it below.
        FilterCase {
            case: "PlatformVersion > 4.0.9",
            params: &[
                ("Filters.member.1.Type", "PlatformVersion"),
                ("Filters.member.1.Operator", ">"),
                ("Filters.member.1.Values.member.1", "4.0.9"),
            ],
            holds: |summary| version_cmp(&summary.version, "4.0.9") == Ordering::Greater,
            narrows: true,
        },
        FilterCase {
            case: "PlatformVersion <= 4.0.11",
            params: &[
                ("Filters.member.1.Type", "PlatformVersion"),
                ("Filters.member.1.Operator", "<="),
                ("Filters.member.1.Values.member.1", "4.0.11"),
            ],
            holds: |summary| version_cmp(&summary.version, "4.0.11") != Ordering::Greater,
            narrows: true,
        },
        FilterCase {
            case: "PlatformVersion >= 0.0.0",
            params: &[
                ("Filters.member.1.Type", "PlatformVersion"),
                ("Filters.member.1.Operator", ">="),
                ("Filters.member.1.Values.member.1", "0.0.0"),
            ],
            holds: |summary| version_cmp(&summary.version, "0.0.0") != Ordering::Less,
            narrows: false,
        },
        FilterCase {
            case: "PlatformVersion < 0.0.1",
            params: &[
                ("Filters.member.1.Type", "PlatformVersion"),
                ("Filters.member.1.Operator", "<"),
                ("Filters.member.1.Values.member.1", "0.0.1"),
            ],
            holds: |summary| version_cmp(&summary.version, "0.0.1") == Ordering::Less,
            narrows: true,
        },
        // `ProgrammingLanguageName` is the one filter attribute `PlatformSummary`
        // models no member for: the languages a published version runs are the
        // catalogue's, and a term on them is answered from it — so an `=` term on
        // a language the catalogue publishes selects exactly the versions running
        // it, and one on a language it does not publish selects nothing.
        FilterCase {
            case: "ProgrammingLanguageName = Python",
            params: &[
                ("Filters.member.1.Type", "ProgrammingLanguageName"),
                ("Filters.member.1.Operator", "="),
                ("Filters.member.1.Values.member.1", "Python"),
            ],
            holds: |summary| summary.category == "Python",
            narrows: true,
        },
        FilterCase {
            case: "ProgrammingLanguageName = a language no platform runs",
            params: &[
                ("Filters.member.1.Type", "ProgrammingLanguageName"),
                ("Filters.member.1.Operator", "="),
                ("Filters.member.1.Values.member.1", "COBOL"),
            ],
            holds: |_summary| false,
            narrows: true,
        },
    ];

    for FilterCase {
        case,
        params,
        holds,
        narrows,
    } in cases
    {
        let answer = emulator.call("ListPlatformVersions", params).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} and body {}",
            answer.status, answer.body
        );
        let expected: Vec<String> = arns(
            &all.iter()
                .filter(|summary| holds(summary))
                .cloned()
                .collect::<Vec<Summary>>(),
        );
        let selected = arns(&summaries(&answer));
        assert_eq!(
            selected, expected,
            "case {case:?}: the term must select exactly the platform versions it holds for, got \
             {selected:?} and body {}",
            answer.body
        );
        if *narrows {
            assert!(
                selected.len() < all.len(),
                "case {case:?}: the term must narrow the list, got all {} versions back",
                all.len()
            );
        } else {
            assert_eq!(
                selected.len(),
                all.len(),
                "case {case:?}: the term holds for every published version, got {}",
                selected.len()
            );
        }
    }
}

#[tokio::test]
async fn several_filter_terms_are_a_conjunction() {
    let emulator = Emulator::new();
    let all = all_versions(&emulator, "conjunction").await;
    let case = "PlatformName contains 'running on' AND SupportedTier = Worker/SQS/HTTP";

    let answer = emulator
        .call(
            "ListPlatformVersions",
            &[
                ("Filters.member.1.Type", "PlatformName"),
                ("Filters.member.1.Operator", "contains"),
                ("Filters.member.1.Values.member.1", "running on"),
                ("Filters.member.2.Type", "SupportedTier"),
                ("Filters.member.2.Operator", "="),
                ("Filters.member.2.Values.member.1", "Worker/SQS/HTTP"),
            ],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200, got {} and body {}",
        answer.status, answer.body
    );

    let both: Vec<Summary> = all
        .iter()
        .filter(|summary| {
            summary.platform_name().contains("running on")
                && summary.tiers.iter().any(|tier| tier == "Worker/SQS/HTTP")
        })
        .cloned()
        .collect();
    let first_only = all
        .iter()
        .filter(|summary| summary.platform_name().contains("running on"))
        .count();
    assert_eq!(
        arns(&summaries(&answer)),
        arns(&both),
        "case {case:?}: both terms must hold for every selected version, got body {}",
        answer.body
    );
    assert!(
        both.len() < first_only,
        "case {case:?}: the second term must narrow the first, got {} of {first_only}",
        both.len()
    );
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

#[tokio::test]
async fn max_records_pages_the_same_total_order_without_loss_or_repetition() {
    let emulator = Emulator::new();
    let all = all_versions(&emulator, "pagination").await;
    let expected = arns(&all);

    // Page sizes below, at and above the number of published versions, plus one
    // that does not divide it.
    let sizes = [1usize, 2, 3, expected.len(), expected.len() + 1, 1000];
    for size in sizes {
        let case = format!("MaxRecords={size}");
        let max_records = size.to_string();
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // One iteration per page, bounded: a token that did not advance would
        // otherwise spin here.
        for page_number in 0..=expected.len() {
            let mut params: Vec<(&str, &str)> = vec![("MaxRecords", max_records.as_str())];
            if let Some(token) = token.as_deref() {
                params.push(("NextToken", token));
            }
            let answer = emulator.call("ListPlatformVersions", &params).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: page {page_number} expected 200, got {} and body {}",
                answer.status, answer.body
            );
            let page = arns(&summaries(&answer));
            assert!(
                !page.is_empty() && page.len() <= size,
                "case {case:?}: page {page_number} must hold between one and {size} versions, got \
                 {}",
                page.len()
            );
            walked.extend(page);
            token = next_token(&answer);
            let Some(next) = token.as_deref() else {
                break;
            };
            assert!(
                expected.contains(&next.to_string()),
                "case {case:?}: a NextToken must name the first version of the next page, got \
                 {next:?}"
            );
        }
        assert!(
            token.is_none(),
            "case {case:?}: the last page must carry no NextToken, got {token:?}"
        );
        assert_eq!(
            walked, expected,
            "case {case:?}: paging must walk the unpaginated order exactly once"
        );
    }
}

#[tokio::test]
async fn a_page_token_is_honoured_together_with_the_filters_that_produced_it() {
    let emulator = Emulator::new();
    let case = "filtered pagination";
    let filter: &[(&str, &str)] = &[
        ("Filters.member.1.Type", "PlatformName"),
        ("Filters.member.1.Operator", "ends_with"),
        (
            "Filters.member.1.Values.member.1",
            "64bit Amazon Linux 2023",
        ),
    ];

    let unpaged = emulator.call("ListPlatformVersions", filter).await;
    let expected = arns(&summaries(&unpaged));
    assert!(
        expected.len() > 2,
        "case {case:?}: expected several Amazon Linux 2023 platform versions, got {expected:?}"
    );

    let mut params = filter.to_vec();
    params.push(("MaxRecords", "2"));
    let first = emulator.call("ListPlatformVersions", &params).await;
    let first_page = arns(&summaries(&first));
    assert_eq!(
        first_page,
        expected[..2].to_vec(),
        "case {case:?}: the first page must be the head of the filtered order, got body {}",
        first.body
    );
    let token = next_token(&first).unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected a NextToken on a short page, got {}",
            first.body
        )
    });

    params.push(("NextToken", token.as_str()));
    let second = emulator.call("ListPlatformVersions", &params).await;
    let second_page = arns(&summaries(&second));
    assert_eq!(
        second_page,
        expected[2..].iter().take(2).cloned().collect::<Vec<String>>(),
        "case {case:?}: the second page must resume where the first ended, got body {}",
        second.body
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_term_the_service_cannot_evaluate_is_refused() {
    let emulator = Emulator::new();

    struct RefusalCase {
        case: &'static str,
        params: &'static [(&'static str, &'static str)],
        code: &'static str,
        /// The text the message must name, so the caller can fix the request
        /// from it alone.
        names: &'static str,
    }

    let cases: &[RefusalCase] = &[
        RefusalCase {
            case: "a filter term with no Type",
            params: &[("Filters.member.1.Values.member.1", "Ready")],
            code: "MissingRequiredParameter",
            names: "Filters[0].Type",
        },
        RefusalCase {
            case: "a Type that is not a platform version attribute",
            params: &[
                ("Filters.member.1.Type", "PlatformBranchLifecycleState"),
                ("Filters.member.1.Values.member.1", "supported"),
            ],
            code: "InvalidParameterValue",
            names: "Filters[0].Type",
        },
        RefusalCase {
            case: "an operator the filter grammar does not define",
            params: &[
                ("Filters.member.1.Type", "PlatformName"),
                ("Filters.member.1.Operator", "like"),
                ("Filters.member.1.Values.member.1", "Python"),
            ],
            code: "InvalidParameterValue",
            names: "Filters[0].Operator",
        },
        RefusalCase {
            case: "a filter term with no value",
            params: &[
                ("Filters.member.1.Type", "PlatformStatus"),
                ("Filters.member.1.Operator", "="),
            ],
            code: "MissingRequiredParameter",
            names: "Filters[0].Values",
        },
        RefusalCase {
            case: "the second term of a list, so the position is reported",
            params: &[
                ("Filters.member.1.Type", "PlatformStatus"),
                ("Filters.member.1.Values.member.1", "Ready"),
                ("Filters.member.2.Type", "NotAnAttribute"),
                ("Filters.member.2.Values.member.1", "whatever"),
            ],
            code: "InvalidParameterValue",
            names: "Filters[1].Type",
        },
        RefusalCase {
            case: "a NextToken no page boundary matches",
            params: &[("NextToken", "not-a-platform-arn")],
            code: "InvalidParameterValue",
            names: "not-a-platform-arn",
        },
    ];

    for RefusalCase {
        case,
        params,
        code,
        names,
    } in cases
    {
        let answer = emulator.call("ListPlatformVersions", params).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} and body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            *code,
            "case {case:?}: unexpected error code in {}",
            answer.body
        );
        assert!(
            answer.body.contains(names),
            "case {case:?}: the message must name {names:?}, got {}",
            answer.body
        );
    }
}

#[tokio::test]
async fn a_max_records_below_the_modelled_range_is_refused_at_the_boundary() {
    let emulator = Emulator::new();
    // `PlatformMaxRecords` is modelled `range min 1`, so the boundary validator
    // refuses zero before the handler runs — and the handler must not answer a
    // page for it.
    let answer = emulator
        .call("ListPlatformVersions", &[("MaxRecords", "0")])
        .await;
    assert!(
        (400..500).contains(&answer.status),
        "case \"MaxRecords=0\": expected a client refusal, got {} and body {}",
        answer.status, answer.body
    );
    assert_ne!(
        answer.error_code(),
        "NotImplemented",
        "case \"MaxRecords=0\": the operation is implemented, got {}",
        answer.body
    );
}
