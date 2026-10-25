// SPDX-License-Identifier: CC0-1.0

//! The managed platform catalog as a client that follows a platform UPDATE reads
//! it, driven end to end over the service's awsQuery HTTP surface.
//!
//! Two rules, each asserted over a set of inputs rather than one recorded call:
//!
//! * A `SearchFilter` on an attribute whose value set the service PUBLISHES is
//!   matched without regard to case, so a value in the spelling a response
//!   carries selects the same branches as the spelling the filter grammar
//!   documents — a branch life-cycle state is published in both (`supported` in
//!   `ListPlatformBranchesRequest$Filters`, `Supported` in
//!   `PlatformBranchSummary$LifecycleState`), and a client filtering on the state
//!   it was just told must not be refused its own value. A value in NEITHER
//!   spelling is still refused.
//! * A platform branch publishes every version of it the service has released:
//!   the branch's current version, `Ready` and `Recommended`, and the release it
//!   superseded, still `Ready` and launchable — which is what makes a minor
//!   platform update available to an environment running the older one.
//!
//! The rows are the service's own catalog (`ListPlatformBranches`,
//! `ListPlatformVersions`), read over the wire rather than written out here, so
//! the rules are asserted over every branch and every state the catalog holds
//! and not only over the ones the issue cited.

use crate::lifecycle::{Emulator, TEST_REGION};
use crate::platform_branches::{branches, filter_params, full_catalog, list, Branch};

/// `PlatformStatus` of a platform version an environment can be launched on,
/// from `PlatformDescription$PlatformStatus` ("Possible values: `Creating` |
/// `Failed` | `Ready` | `Deleting` | `Deleted`").
const READY: &str = "Ready";

/// `PlatformLifecycleState` of a version that is its branch's current one, from
/// `PlatformDescription$PlatformLifecycleState` ("Possible values:
/// `Recommended` | `null`").
const RECOMMENDED: &str = "Recommended";

/// One spelling a `LifecycleState` filter value can be written in, and how this
/// suite renders a state into it. The rule is case-insensitivity, so the rows
/// span the documented lower-case spelling, the capitalized one every response
/// carries, and one no document uses at all.
struct Spelling {
    case: &'static str,
    render: fn(&str) -> String,
}

const SPELLINGS: &[Spelling] = &[
    Spelling {
        case: "the lower-case spelling the filter grammar documents",
        render: |state| state.to_lowercase(),
    },
    Spelling {
        case: "the capitalized spelling every response carries",
        render: |state| {
            let mut characters = state.chars();
            match characters.next() {
                Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
                None => String::new(),
            }
        },
    },
    Spelling {
        case: "upper case, which neither document uses",
        render: |state| state.to_uppercase(),
    },
];

/// One multi-term-capable operator and what it selects, restated here rather
/// than read from the service: `=` selects the branches in the state, `!=` the
/// ones that are not, `in` the union over the values and `not_in` its
/// complement.
struct OperatorCase {
    operator: &'static str,
    /// Whether a branch whose state IS the term's value satisfies the term.
    selects_the_state: bool,
}

const OPERATORS: &[OperatorCase] = &[
    OperatorCase {
        operator: "=",
        selects_the_state: true,
    },
    OperatorCase {
        operator: "!=",
        selects_the_state: false,
    },
    OperatorCase {
        operator: "in",
        selects_the_state: true,
    },
    OperatorCase {
        operator: "not_in",
        selects_the_state: false,
    },
];

/// The keys of `selected`, sorted, so two selections are compared as sets.
fn keys(selected: &[Branch]) -> Vec<String> {
    let mut keys: Vec<String> = selected.iter().map(Branch::key).collect();
    keys.sort();
    keys
}

/// One published platform version as `ListPlatformVersions` reported it.
struct PublishedVersion {
    arn: String,
    version: String,
    status: String,
    /// `PlatformLifecycleState`, or the empty string when the version carries none.
    lifecycle_state: String,
}

/// Every version `ListPlatformVersions` publishes for `branch_name`.
async fn versions_of(emulator: &Emulator, branch_name: &str) -> Vec<PublishedVersion> {
    let answer = emulator
        .call(
            "ListPlatformVersions",
            &[
                ("Filters.member.1.Type", "PlatformBranchName"),
                ("Filters.member.1.Operator", "="),
                ("Filters.member.1.Values.member.1", branch_name),
            ],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {branch_name:?}: expected 200 from ListPlatformVersions, got {} and body {}",
        answer.status, answer.body
    );
    answer
        .elements_at("PlatformSummaryList/member")
        .into_iter()
        .map(|member| PublishedVersion {
            arn: member.value("PlatformArn").unwrap_or_default().to_string(),
            version: member
                .value("PlatformVersion")
                .unwrap_or_default()
                .to_string(),
            status: member
                .value("PlatformStatus")
                .unwrap_or_default()
                .to_string(),
            lifecycle_state: member
                .value("PlatformLifecycleState")
                .unwrap_or_default()
                .to_string(),
        })
        .collect()
}

/// A `LifecycleState` term selects the same branches however the value is cased,
/// for every state the catalog holds and every operator the attribute accepts.
#[tokio::test]
async fn a_lifecycle_state_filter_accepts_the_spelling_a_response_carries() {
    let emulator = Emulator::new();
    let catalog = full_catalog(&emulator).await;

    // The states the catalog actually reports, read off the catalog rather than
    // written out, so every state a branch can be in is a row.
    let mut states: Vec<String> = catalog
        .iter()
        .map(|branch| branch.lifecycle_state.clone())
        .collect();
    states.sort();
    states.dedup();
    assert!(
        states.len() > 1,
        "case \"catalog states\": expected the catalog to span several life-cycle states, got \
         {states:?}"
    );

    for state in &states {
        for OperatorCase {
            operator,
            selects_the_state,
        } in OPERATORS
        {
            // What the rule says the answer is, computed from the unfiltered
            // catalog: the state is compared without regard to case either way.
            let expected = keys(
                &catalog
                    .iter()
                    .filter(|branch| {
                        branch.lifecycle_state.eq_ignore_ascii_case(state) == *selects_the_state
                    })
                    .cloned()
                    .collect::<Vec<Branch>>(),
            );
            for Spelling { case, render } in SPELLINGS {
                let value = render(state);
                let case = &format!("LifecycleState {operator} {value:?} ({case})");
                let answer = list(
                    &emulator,
                    &filter_params(0, "LifecycleState", operator, std::slice::from_ref(&value)),
                )
                .await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: the service reports this state, so a filter on it must be \
                     accepted; got {} and body {}",
                    answer.status, answer.body
                );
                assert_eq!(
                    keys(&branches(&answer, case)),
                    expected,
                    "case {case:?}: the term must select exactly the branches in that state, \
                     got body {}",
                    answer.body
                );
            }
        }
    }
}

/// A value in no spelling of the published set is still refused, and so is one
/// for the other closed-value attribute: case-insensitivity widens how a value
/// is written, not which values exist.
#[tokio::test]
async fn a_value_outside_the_published_set_is_still_refused() {
    let emulator = Emulator::new();

    let rows: &[(&str, &str, &str)] = &[
        ("a state no branch is in", "LifecycleState", "unsupported"),
        ("a state spelled as a word", "LifecycleState", "Sunset"),
        // "Possible values: WebServer/Standard | Worker/SQS/HTTP": the tier's
        // NAME alone is not one of them, in any case.
        ("a tier without its type", "TierType", "Worker"),
        ("a tier spelled in lower case", "TierType", "webserver"),
    ];

    for (case, attribute, value) in rows {
        let answer = list(
            &emulator,
            &filter_params(0, attribute, "=", &[(*value).to_string()]),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400 for {attribute} = {value:?}, got {} and body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got body {}",
            answer.body
        );
    }
}

/// The same term in both spellings, on the OTHER closed-value attribute: a tier
/// is published in exactly one spelling, so the documented value is accepted and
/// selects the branches that support that tier.
#[tokio::test]
async fn a_tier_type_filter_selects_the_branches_supporting_the_tier() {
    let emulator = Emulator::new();
    let catalog = full_catalog(&emulator).await;

    for tier in crate::platform_branches::TIERS {
        for (case, value) in [
            ("as published", (*tier).to_string()),
            ("lower-cased", tier.to_lowercase()),
        ] {
            let case = &format!("TierType = {value:?} ({case})");
            let answer = list(
                &emulator,
                &filter_params(0, "TierType", "=", std::slice::from_ref(&value)),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} and body {}",
                answer.status, answer.body
            );
            let expected = keys(
                &catalog
                    .iter()
                    .filter(|branch| {
                        branch
                            .supported_tiers
                            .iter()
                            .any(|supported| supported.eq_ignore_ascii_case(&value))
                    })
                    .cloned()
                    .collect::<Vec<Branch>>(),
            );
            assert_eq!(
                keys(&branches(&answer, case)),
                expected,
                "case {case:?}: the term must select exactly the branches supporting the tier, \
                 got body {}",
                answer.body
            );
        }
    }
}

/// Every branch that publishes a platform version publishes MORE THAN ONE, all
/// `Ready`, with exactly one of them — the newest — its `Recommended` version.
/// That is what makes a platform update available to an environment running any
/// of the others.
#[tokio::test]
async fn every_published_branch_offers_a_superseded_ready_version() {
    let emulator = Emulator::new();
    let catalog = full_catalog(&emulator).await;

    let mut branches_with_versions = 0usize;
    for branch in &catalog {
        let case = branch.branch_name.as_str();
        let versions = versions_of(&emulator, case).await;
        // A retired branch publishes no version an account can deploy, so it
        // publishes none at all; every other branch is a row of this rule.
        if versions.is_empty() {
            continue;
        }
        branches_with_versions += 1;

        assert!(
            versions.len() >= 2,
            "case {case:?}: a branch publishes the release its current version superseded \
             beside it, got {:?}",
            published(&versions)
        );
        for version in &versions {
            assert_eq!(
                version.status, READY,
                "case {case:?}: every published version is launchable, so {} must be \
                 {READY}, got {:?} for {}",
                version.version, version.status, version.arn
            );
            assert!(
                version
                    .arn
                    .ends_with(&format!("/{case}/{}", version.version)),
                "case {case:?}: a version's ARN names its branch and itself, got {:?}",
                version.arn
            );
        }
        let recommended: Vec<&str> = versions
            .iter()
            .filter(|version| version.lifecycle_state == RECOMMENDED)
            .map(|version| version.version.as_str())
            .collect();
        assert_eq!(
            recommended.len(),
            1,
            "case {case:?}: exactly one published version of a branch is its recommended one, \
             got {:?}",
            published(&versions)
        );
        // And it is the NEWEST: every other published version orders below it.
        for version in &versions {
            if version.lifecycle_state == RECOMMENDED {
                continue;
            }
            assert!(
                eb_version_below(&version.version, recommended[0]),
                "case {case:?}: the superseded version {:?} must order below the \
                 recommended {:?}",
                version.version,
                recommended[0]
            );
        }
    }
    assert!(
        branches_with_versions > 1,
        "case \"catalog spread\": expected several branches to publish versions, got \
         {branches_with_versions}"
    );
}

/// The versions `published` carries, as the `(version, status, life-cycle state)` triples
/// an assertion quotes them by.
fn published(versions: &[PublishedVersion]) -> Vec<(&str, &str, &str)> {
    versions
        .iter()
        .map(|version| {
            (
                version.version.as_str(),
                version.status.as_str(),
                version.lifecycle_state.as_str(),
            )
        })
        .collect()
}

/// Whether the dotted-numeric `left` orders below `right`, compared component by
/// component numerically — this suite's own statement of a version ordering, so
/// it does not read the service's.
fn eb_version_below(left: &str, right: &str) -> bool {
    let components = |value: &str| -> Vec<u64> {
        value
            .split('.')
            .map(|component| component.parse::<u64>().unwrap_or_default())
            .collect()
    };
    components(left) < components(right)
}

/// An environment really launches on the release a branch's current version
/// superseded: the create is accepted, the environment reports THAT version's
/// ARN and solution stack name, and `DescribePlatformVersion` answers for it.
///
/// This is the state a managed platform update starts from, so the older
/// release has to be a platform version this service serves in full, not just a
/// row in a list.
#[tokio::test]
async fn an_environment_launches_on_the_superseded_release() {
    let emulator = Emulator::new();
    let catalog = full_catalog(&emulator).await;

    // Three branches of the catalog, so the rule is not asserted on one row:
    // the first branch that publishes versions, and the next two after it.
    let mut rows: Vec<(String, Vec<PublishedVersion>)> = Vec::new();
    for branch in &catalog {
        let versions = versions_of(&emulator, &branch.branch_name).await;
        if versions.len() >= 2 {
            rows.push((branch.branch_name.clone(), versions));
        }
        if rows.len() == 3 {
            break;
        }
    }
    assert_eq!(
        rows.len(),
        3,
        "case \"catalog spread\": expected at least three branches publishing several versions"
    );

    let created = emulator
        .call(
            "CreateApplication",
            &[("ApplicationName", "platform-update-app")],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "expected 200 from CreateApplication, got {} and body {}",
        created.status, created.body
    );

    for (index, (branch_name, versions)) in rows.iter().enumerate() {
        let case = branch_name.as_str();
        // The superseded release: the one published version that is not the
        // branch's recommended one.
        let superseded = versions
            .iter()
            .find(|version| version.lifecycle_state != RECOMMENDED)
            .unwrap_or_else(|| panic!("case {case:?}: the branch publishes a superseded release"));
        let arn = &superseded.arn;
        let version = &superseded.version;

        let described = emulator
            .call("DescribePlatformVersion", &[("PlatformArn", arn)])
            .await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: the superseded release must be describable, got {} and body {}",
            described.status, described.body
        );
        assert_eq!(
            described.value("PlatformDescription/PlatformStatus"),
            Some(READY),
            "case {case:?}: the superseded release must be {READY}, got {}",
            described.body
        );
        assert_eq!(
            described.value("PlatformDescription/PlatformLifecycleState"),
            None,
            "case {case:?}: only the newest release of a branch is its recommended one, got {}",
            described.body
        );
        let stack_name = described
            .expect("PlatformDescription/SolutionStackName", case)
            .to_string();

        let environment_name = format!("superseded-env-{index}");
        let launched = emulator
            .call(
                "CreateEnvironment",
                &[
                    ("ApplicationName", "platform-update-app"),
                    ("EnvironmentName", &environment_name),
                    ("PlatformArn", arn),
                ],
            )
            .await;
        assert_eq!(
            launched.status, 200,
            "case {case:?}: an environment must launch on the superseded release {version}, got \
             {} and body {}",
            launched.status, launched.body
        );
        assert_eq!(
            launched.value("PlatformArn"),
            Some(arn.as_str()),
            "case {case:?}: the environment must report the platform version it was launched on, \
             got {}",
            launched.body
        );
        // The two names of ONE platform version: the environment reports the
        // solution stack name of the release it runs, not of the newer one.
        assert_eq!(
            launched.value("SolutionStackName"),
            Some(stack_name.as_str()),
            "case {case:?}: expected the solution stack name of {version}, got {}",
            launched.body
        );
        assert!(
            arn.contains(TEST_REGION),
            "case {case:?}: the platform version is regional, got {arn:?}"
        );

        let environments = emulator
            .call(
                "DescribeEnvironments",
                &[("EnvironmentNames.member.1", &environment_name)],
            )
            .await;
        assert_eq!(
            environments.value("Environments/member/PlatformArn"),
            Some(arn.as_str()),
            "case {case:?}: the stored environment must report the superseded release, got {}",
            environments.body
        );
    }
}
