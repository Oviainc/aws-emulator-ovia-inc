// SPDX-License-Identifier: CC0-1.0

//! `ListPlatformBranches`, driven end to end over the service's awsQuery HTTP
//! surface.
//!
//! The rules under test are the ones the operation's own model documents, and
//! every one of them is asserted over a SET of inputs rather than a single
//! recorded call:
//!
//! * every reported branch carries the modelled members, with `LifecycleState`
//!   and `SupportedTierList` inside their documented value sets;
//! * `Filters` is applied by the SERVER, as "a logical conjunction (AND) of the
//!   separate `SearchFilter` terms", each term being `Attribute Operator
//!   Values[1]` (or a union over the values for `in` / `not_in`) — checked for
//!   every attribute and operator the documentation lists;
//! * a filter the grammar does not allow is REFUSED, never silently ignored;
//! * `MaxRecords`/`NextToken` page one total order, so paging the catalog at any
//!   page size yields the unpaged list exactly once.
//!
//! The emulator harness (the form encoder, the XML element reader) is the one
//! `lifecycle.rs` already owns; this suite reuses it rather than writing a
//! second copy.

use crate::lifecycle::{elements, Answer, Emulator};

/// The documented `PlatformBranchSummary$LifecycleState` value set as a FILTER
/// takes it ("Attribute = LifecycleState: Operator: = | != | in | not_in
/// Values: beta | supported | deprecated | retired"), restated here so the
/// test does not read it from the code under test.
pub(crate) const LIFECYCLE_STATES: &[&str] = &["beta", "deprecated", "retired", "supported"];

/// The same four states as a RESPONSE carries them: capitalized, the form the
/// real service answers `PlatformBranchSummary$LifecycleState` with and the one
/// `PlatformDescription$PlatformBranchLifecycleState` documents ("Beta |
/// Supported | Deprecated | Retired"). `reported(state)` below is the mapping
/// between the two spellings.
pub(crate) const REPORTED_LIFECYCLE_STATES: &[&str] = &["Beta", "Deprecated", "Retired", "Supported"];

/// The documented filter spelling `state` is reported under.
pub(crate) fn reported(state: &str) -> String {
    let mut characters = state.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
        None => String::new(),
    }
}

/// The documented `PlatformBranchSummary$SupportedTierList` value set
/// ("Possible values: WebServer/Standard | Worker/SQS/HTTP").
pub(crate) const TIERS: &[&str] = &["WebServer/Standard", "Worker/SQS/HTTP"];

/// "Within each `SearchFilter` item, the `Values` array is limited to 10
/// items", and "Array size: limited to 10 `SearchFilter` objects".
const MAX_FILTER_ITEMS: usize = 10;

/// One platform branch as the emulator reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Branch {
    pub(crate) platform_name: String,
    pub(crate) branch_name: String,
    pub(crate) lifecycle_state: String,
    pub(crate) branch_order: i32,
    pub(crate) supported_tiers: Vec<String>,
}

impl Branch {
    /// The identity of a branch: a branch name is unique within its platform.
    pub(crate) fn key(&self) -> String {
        format!("{}/{}", self.platform_name, self.branch_name)
    }
}

/// Call `ListPlatformBranches` with `params` already rendered as awsQuery form
/// values.
pub(crate) async fn list(emulator: &Emulator, params: &[(String, String)]) -> Answer {
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    emulator.call("ListPlatformBranches", &borrowed).await
}

/// The `PlatformBranchSummaryList` of a 200 answer, in the order it was sent.
pub(crate) fn branches(answer: &Answer, case: &str) -> Vec<Branch> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from ListPlatformBranches, got {} and body {}",
        answer.status, answer.body
    );
    elements(&answer.body, "PlatformBranchSummaryList")
        .iter()
        .flat_map(|list| elements(list, "member"))
        .map(|member| Branch {
            platform_name: text(member, "PlatformName"),
            branch_name: text(member, "BranchName"),
            lifecycle_state: text(member, "LifecycleState"),
            // `BranchOrder` carries `@default: 0`, so a serializer is free to
            // omit it when it holds that default: an absent element reads as 0.
            branch_order: text(member, "BranchOrder").parse().unwrap_or(0),
            supported_tiers: elements(member, "SupportedTierList")
                .iter()
                .flat_map(|list| elements(list, "member"))
                .map(|tier| tier.trim().to_string())
                .collect(),
        })
        .collect()
}

/// The text of the first `<tag>` in `xml`, or `""` when there is none.
fn text(xml: &str, tag: &str) -> String {
    elements(xml, tag)
        .first()
        .map(|value| value.trim().to_string())
        .unwrap_or_default()
}

/// The whole catalog: no filters, no page size.
pub(crate) async fn full_catalog(emulator: &Emulator) -> Vec<Branch> {
    let answer = list(emulator, &[]).await;
    assert_eq!(
        answer.value("NextToken"),
        None,
        "case \"unpaged\": a call with no MaxRecords must return every branch in one page, \
         got body {}",
        answer.body
    );
    branches(&answer, "unpaged")
}

/// One filter term as awsQuery form parameters. `index` is 0-based; the wire
/// form is 1-based (`Filters.member.1.…`).
pub(crate) fn filter_params(
    index: usize,
    attribute: &str,
    operator: &str,
    values: &[String],
) -> Vec<(String, String)> {
    let term = index + 1;
    let mut params = vec![
        (
            format!("Filters.member.{term}.Attribute"),
            attribute.to_string(),
        ),
        (
            format!("Filters.member.{term}.Operator"),
            operator.to_string(),
        ),
    ];
    for (position, value) in values.iter().enumerate() {
        params.push((
            format!("Filters.member.{term}.Values.member.{}", position + 1),
            value.clone(),
        ));
    }
    params
}

/// The documented semantics of ONE filter term, restated independently of the
/// implementation: `Attribute Operator Values[1]` for the single-value
/// operators, and a logical union over the values for `in` / `not_in`.
fn term_matches(branch: &Branch, attribute: &str, operator: &str, values: &[String]) -> bool {
    if attribute == "TierType" {
        // `SupportedTierList` is a list, so the comparison is membership.
        let supported = values
            .iter()
            .any(|value| branch.supported_tiers.contains(value));
        return match operator {
            "!=" => !supported,
            "=" => supported,
            other => panic!("TierType does not accept the operator {other:?}"),
        };
    }
    let subject = match attribute {
        "BranchName" => branch.branch_name.clone(),
        // A `LifecycleState` filter value is one of the documented lower-case
        // spellings, while the member is reported capitalized, so the term is
        // evaluated against the branch's state in the filter's own spelling.
        "LifecycleState" => branch.lifecycle_state.to_lowercase(),
        "PlatformName" => branch.platform_name.clone(),
        other => panic!("no such filter attribute {other:?}"),
    };
    let subject = &subject;
    let first = values.first().expect("a term carries at least one value");
    match operator {
        "=" => subject == first,
        "!=" => subject != first,
        "begins_with" => subject.starts_with(first.as_str()),
        "ends_with" => subject.ends_with(first.as_str()),
        "contains" => subject.contains(first.as_str()),
        "in" => values.contains(subject),
        "not_in" => !values.contains(subject),
        other => panic!("no such filter operator {other:?}"),
    }
}

/// The first `length` characters of `value`.
fn prefix(value: &str, length: usize) -> String {
    value.chars().take(length).collect()
}

/// The last `length` characters of `value`.
fn suffix(value: &str, length: usize) -> String {
    let characters: Vec<char> = value.chars().collect();
    characters[characters.len().saturating_sub(length)..]
        .iter()
        .collect()
}

/// `length` characters of `value` starting at `from` — a substring that is
/// neither a prefix nor a suffix, for the `contains` operator.
fn infix(value: &str, from: usize, length: usize) -> String {
    value.chars().skip(from).take(length).collect()
}

fn owned(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

// ---------------------------------------------------------------------------
// The catalog itself
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_reported_branch_satisfies_the_modelled_value_sets() {
    let emulator = Emulator::new();
    let catalog = full_catalog(&emulator).await;

    assert!(
        catalog.len() >= MAX_FILTER_ITEMS,
        "case \"catalog\": expected a platform catalog with at least {MAX_FILTER_ITEMS} branches \
         to page and filter over, got {}",
        catalog.len()
    );

    let mut keys: Vec<String> = Vec::new();
    for branch in &catalog {
        let case = branch.key();
        assert!(
            !branch.platform_name.is_empty(),
            "case {case:?}: expected a PlatformName, got {branch:?}"
        );
        assert!(
            !branch.branch_name.is_empty(),
            "case {case:?}: expected a BranchName, got {branch:?}"
        );
        assert!(
            REPORTED_LIFECYCLE_STATES.contains(&branch.lifecycle_state.as_str()),
            "case {case:?}: expected LifecycleState in {REPORTED_LIFECYCLE_STATES:?}, got {:?}",
            branch.lifecycle_state
        );
        // "The environment tiers that platform versions in this branch support":
        // a branch that names a runtime and an operating system runs
        // environments, so it names the tiers they run in. A branch that is not
        // a runtime at all (the custom-platform builder) names none, which is
        // the empty list the service reports for it.
        if branch.branch_name.contains(" running on ") {
            assert!(
                !branch.supported_tiers.is_empty(),
                "case {case:?}: expected a non-empty SupportedTierList, got {branch:?}"
            );
        }
        for tier in &branch.supported_tiers {
            assert!(
                TIERS.contains(&tier.as_str()),
                "case {case:?}: expected every SupportedTierList entry in {TIERS:?}, got {tier:?}"
            );
        }
        assert!(
            branch.branch_order >= 0,
            "case {case:?}: expected a non-negative BranchOrder, got {}",
            branch.branch_order
        );
        keys.push(branch.key());
    }

    let mut unique = keys.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        keys.len(),
        "case \"catalog\": expected every (PlatformName, BranchName) pair to appear once, got {keys:?}"
    );

    // The catalog publishes branches in more than one life-cycle state and in
    // both documented tiers: a filter on either has something to select AND
    // something to exclude, which is what makes the filter cases below
    // meaningful rather than vacuous. WHICH states are published is catalog
    // data (AWS moves a branch's state when it deprecates or retires it), so
    // what is pinned here is that the published set is a non-trivial subset of
    // the documented one.
    let mut published: Vec<String> = catalog
        .iter()
        .map(|branch| branch.lifecycle_state.clone())
        .collect();
    published.sort();
    published.dedup();
    assert!(
        published.len() >= 2,
        "case \"catalog\": expected branches in at least two life-cycle states, got {published:?}"
    );
    for state in LIFECYCLE_STATES {
        assert!(
            REPORTED_LIFECYCLE_STATES.contains(&reported(state).as_str()),
            "case {state:?}: expected the reported spelling of every documented filter value to \
             be one of {REPORTED_LIFECYCLE_STATES:?}, got {:?}",
            reported(state)
        );
    }
    for tier in TIERS {
        assert!(
            catalog
                .iter()
                .any(|branch| branch.supported_tiers.iter().any(|have| have == tier)),
            "case {tier:?}: expected the catalog to publish at least one branch supporting this \
             tier, got {catalog:?}"
        );
    }

    // "A larger BranchOrder value designates a newer platform branch within the
    // platform": the report is grouped by platform, newest branch first.
    for pair in catalog.windows(2) {
        let (left, right) = (&pair[0], &pair[1]);
        let case = format!("{} then {}", left.key(), right.key());
        if left.platform_name == right.platform_name {
            assert!(
                left.branch_order >= right.branch_order,
                "case {case:?}: within one platform expected a non-increasing BranchOrder, got \
                 {} then {}",
                left.branch_order,
                right.branch_order
            );
        } else {
            assert!(
                left.platform_name < right.platform_name,
                "case {case:?}: expected platforms in ascending name order, got {:?} then {:?}",
                left.platform_name,
                right.platform_name
            );
        }
    }
}

#[tokio::test]
async fn the_catalog_is_the_same_on_every_call_and_for_every_caller() {
    let emulator = Emulator::new();
    let first = full_catalog(&emulator).await;
    let second = full_catalog(&emulator).await;
    assert_eq!(
        first, second,
        "case \"repeat\": a report must not change what a later caller sees"
    );

    // A second emulator starts from the same published catalog: it is service
    // reference data, not something a caller's earlier calls created.
    let other = Emulator::new();
    let fresh = full_catalog(&other).await;
    assert_eq!(
        first, fresh,
        "case \"fresh store\": expected the same published catalog from a fresh store"
    );
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// One filter term: the attribute it reads, the operator it applies and the
/// values it applies it to.
struct Term {
    attribute: &'static str,
    operator: &'static str,
    values: Vec<String>,
}

impl Term {
    fn new(attribute: &'static str, operator: &'static str, values: Vec<String>) -> Self {
        Self {
            attribute,
            operator,
            values,
        }
    }

    /// Whether `branch` satisfies this term (see [`term_matches`]).
    fn matches(&self, branch: &Branch) -> bool {
        term_matches(branch, self.attribute, self.operator, &self.values)
    }

    /// This term as the awsQuery parameters of filter term `index`.
    fn params(&self, index: usize) -> Vec<(String, String)> {
        filter_params(index, self.attribute, self.operator, &self.values)
    }
}

impl std::fmt::Display for Term {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} {} {:?}",
            self.attribute, self.operator, self.values
        )
    }
}

/// One row of the single-term filter table: the term, and whether the
/// published catalog guarantees it selects something.
struct FilterCase {
    case: String,
    term: Term,
    expect_non_empty: bool,
}

/// One [`FilterCase`], written as a row.
fn filter_case(
    case: impl Into<String>,
    attribute: &'static str,
    operator: &'static str,
    values: Vec<String>,
    expect_non_empty: bool,
) -> FilterCase {
    FilterCase {
        case: case.into(),
        term: Term::new(attribute, operator, values),
        expect_non_empty,
    }
}

#[tokio::test]
async fn one_filter_term_is_applied_server_side_for_every_documented_operator() {
    let emulator = Emulator::new();
    let catalog = full_catalog(&emulator).await;

    // Every value below is taken from the catalog the server just reported, so
    // the table spans the real value space instead of pinning one recorded
    // request's strings.
    let platform = catalog[0].platform_name.clone();
    let other_platform = catalog
        .iter()
        .map(|branch| branch.platform_name.clone())
        .find(|name| *name != platform)
        .expect("a catalog with more than one platform");
    let branch = catalog[0].branch_name.clone();
    let middle_branch = catalog[catalog.len() / 2].branch_name.clone();
    let last_branch = catalog[catalog.len() - 1].branch_name.clone();
    // Ten values — the documented ceiling for one term's `Values` array.
    let ten_branches: Vec<String> = catalog
        .iter()
        .take(MAX_FILTER_ITEMS)
        .map(|branch| branch.branch_name.clone())
        .collect();
    let two_platforms = vec![platform.clone(), other_platform.clone()];
    let two_branches = vec![branch.clone(), last_branch.clone()];

    let cases = vec![
        // `BranchName` and `PlatformName` accept all seven operators.
        filter_case("PlatformName =", "PlatformName", "=", vec![platform.clone()], true),
        filter_case("PlatformName !=", "PlatformName", "!=", vec![platform.clone()], true),
        filter_case("PlatformName in", "PlatformName", "in", two_platforms.clone(), true),
        filter_case("PlatformName not_in", "PlatformName", "not_in", two_platforms, true),
        filter_case(
            "PlatformName begins_with",
            "PlatformName",
            "begins_with",
            vec![prefix(&platform, 2)],
            true,
        ),
        filter_case(
            "PlatformName ends_with",
            "PlatformName",
            "ends_with",
            vec![suffix(&platform, 3)],
            true,
        ),
        filter_case(
            "PlatformName contains",
            "PlatformName",
            "contains",
            vec![infix(&platform, 1, 3)],
            true,
        ),
        // A name no catalog publishes selects nothing — and must not fall back
        // to reporting everything.
        filter_case(
            "PlatformName = a platform no catalog publishes",
            "PlatformName",
            "=",
            owned(&["Fortran on 64bit Punch Cards"]),
            false,
        ),
        filter_case("BranchName =", "BranchName", "=", vec![branch.clone()], true),
        filter_case("BranchName !=", "BranchName", "!=", vec![branch.clone()], true),
        filter_case(
            "BranchName begins_with",
            "BranchName",
            "begins_with",
            vec![prefix(&middle_branch, 4)],
            true,
        ),
        filter_case(
            "BranchName ends_with",
            "BranchName",
            "ends_with",
            vec![suffix(&middle_branch, 12)],
            true,
        ),
        filter_case(
            "BranchName contains",
            "BranchName",
            "contains",
            vec![infix(&middle_branch, 2, 6)],
            true,
        ),
        filter_case("BranchName in", "BranchName", "in", two_branches.clone(), true),
        filter_case("BranchName not_in", "BranchName", "not_in", two_branches, true),
        filter_case(
            format!("BranchName in {MAX_FILTER_ITEMS} branches"),
            "BranchName",
            "in",
            ten_branches,
            true,
        ),
        // `LifecycleState` accepts `=` | `!=` | `in` | `not_in`.
        filter_case("LifecycleState !=", "LifecycleState", "!=", owned(&["retired"]), true),
        filter_case(
            "LifecycleState in",
            "LifecycleState",
            "in",
            owned(&["beta", "supported"]),
            true,
        ),
        filter_case(
            "LifecycleState not_in",
            "LifecycleState",
            "not_in",
            owned(&["deprecated", "retired"]),
            true,
        ),
        // `TierType` accepts `=` | `!=`, over a list-valued attribute.
        filter_case(
            "TierType = WebServer/Standard",
            "TierType",
            "=",
            owned(&["WebServer/Standard"]),
            true,
        ),
        filter_case(
            "TierType = Worker/SQS/HTTP",
            "TierType",
            "=",
            owned(&["Worker/SQS/HTTP"]),
            true,
        ),
        filter_case(
            "TierType != Worker/SQS/HTTP",
            "TierType",
            "!=",
            owned(&["Worker/SQS/HTTP"]),
            true,
        ),
    ]
    .into_iter()
    // One `=` row per documented life-cycle state, so every state in the
    // published value set is exercised, not only the ones a recording used. A
    // state the catalog publishes no branch in must answer an EMPTY page rather
    // than an unfiltered one, so the row's expectation follows the catalog.
    .chain(LIFECYCLE_STATES.iter().map(|state| {
        let published = catalog
            .iter()
            .any(|branch| branch.lifecycle_state == reported(state));
        filter_case(
            format!("LifecycleState = {state}"),
            "LifecycleState",
            "=",
            owned(&[state]),
            published,
        )
    }))
    .collect::<Vec<FilterCase>>();

    for FilterCase {
        case,
        term,
        expect_non_empty,
    } in &cases
    {
        let expected: Vec<Branch> = catalog
            .iter()
            .filter(|branch| term.matches(branch))
            .cloned()
            .collect();
        assert_eq!(
            !expected.is_empty(),
            *expect_non_empty,
            "case {case:?}: the catalog must make this case {}, got {} match(es) for {term}",
            if *expect_non_empty { "non-empty" } else { "empty" },
            expected.len()
        );

        let answer = list(&emulator, &term.params(0)).await;
        let reported = branches(&answer, case);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the server to report exactly the branches satisfying \
             {term}, got {reported:?}"
        );
        assert_eq!(
            answer.value("NextToken"),
            None,
            "case {case:?}: a filtered call with no MaxRecords must answer in one page, got body {}",
            answer.body
        );
    }
}

/// One row of the conjunction table: two terms that are sent together.
struct ConjunctionCase {
    case: &'static str,
    first: Term,
    second: Term,
}

#[tokio::test]
async fn filter_terms_are_combined_as_a_conjunction() {
    let emulator = Emulator::new();
    let catalog = full_catalog(&emulator).await;
    let platform = catalog[0].platform_name.clone();
    let other_platform = catalog
        .iter()
        .map(|branch| branch.platform_name.clone())
        .find(|name| *name != platform)
        .expect("a catalog with more than one platform");

    // The second term always narrows the first, and the last row's two terms
    // cannot hold together, so that conjunction is empty.
    let cases = vec![
        ConjunctionCase {
            case: "platform and life-cycle state",
            first: Term::new("PlatformName", "=", vec![platform.clone()]),
            second: Term::new("LifecycleState", "!=", owned(&["retired"])),
        },
        ConjunctionCase {
            case: "life-cycle state and tier",
            first: Term::new("LifecycleState", "in", owned(&["beta", "supported"])),
            second: Term::new("TierType", "=", owned(&["WebServer/Standard"])),
        },
        ConjunctionCase {
            case: "branch name substring and platform",
            first: Term::new("BranchName", "contains", owned(&["running on"])),
            second: Term::new(
                "PlatformName",
                "in",
                vec![platform.clone(), other_platform.clone()],
            ),
        },
        ConjunctionCase {
            case: "two platforms that cannot both hold",
            first: Term::new("PlatformName", "=", vec![platform.clone()]),
            second: Term::new("PlatformName", "=", vec![other_platform.clone()]),
        },
    ];

    for ConjunctionCase {
        case,
        first,
        second,
    } in &cases
    {
        let expected: Vec<Branch> = catalog
            .iter()
            .filter(|branch| first.matches(branch) && second.matches(branch))
            .cloned()
            .collect();

        let mut params = first.params(0);
        params.extend(second.params(1));
        let answer = list(&emulator, &params).await;
        let reported = branches(&answer, case);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the AND of both terms, got {reported:?}"
        );
    }
}

/// One row of the refusal table: the request, and the error code it must be
/// refused with.
struct RefusalCase {
    case: String,
    params: Vec<(String, String)>,
    code: &'static str,
}

impl RefusalCase {
    fn new(case: impl Into<String>, params: Vec<(String, String)>, code: &'static str) -> Self {
        Self {
            case: case.into(),
            params,
            code,
        }
    }
}

#[tokio::test]
async fn a_filter_outside_the_documented_grammar_is_refused() {
    let emulator = Emulator::new();

    // Every row is a request the filter grammar does not allow; the service
    // must refuse it rather than answer an unfiltered page.
    let mut cases: Vec<RefusalCase> = vec![
        RefusalCase::new(
            "unknown attribute",
            filter_params(0, "SolutionStackName", "=", &owned(&["anything"])),
            "InvalidParameterValue",
        ),
        RefusalCase::new(
            "operator the attribute does not accept: LifecycleState begins_with",
            filter_params(0, "LifecycleState", "begins_with", &owned(&["sup"])),
            "InvalidParameterValue",
        ),
        RefusalCase::new(
            "operator the attribute does not accept: TierType contains",
            filter_params(0, "TierType", "contains", &owned(&["Worker"])),
            "InvalidParameterValue",
        ),
        RefusalCase::new(
            "operator no attribute accepts",
            filter_params(0, "PlatformName", "like", &owned(&["Py%"])),
            "InvalidParameterValue",
        ),
        RefusalCase::new(
            "value outside the LifecycleState value set",
            filter_params(0, "LifecycleState", "=", &owned(&["unsupported"])),
            "InvalidParameterValue",
        ),
        RefusalCase::new(
            "value outside the TierType value set",
            filter_params(0, "TierType", "=", &owned(&["Worker"])),
            "InvalidParameterValue",
        ),
        RefusalCase::new(
            "two values for a single-value operator",
            filter_params(0, "LifecycleState", "=", &owned(&["beta", "supported"])),
            "InvalidParameterValue",
        ),
        RefusalCase::new(
            "no Attribute",
            vec![
                ("Filters.member.1.Operator".to_string(), "=".to_string()),
                (
                    "Filters.member.1.Values.member.1".to_string(),
                    "supported".to_string(),
                ),
            ],
            "MissingRequiredParameter",
        ),
        RefusalCase::new(
            "no Operator",
            vec![
                (
                    "Filters.member.1.Attribute".to_string(),
                    "LifecycleState".to_string(),
                ),
                (
                    "Filters.member.1.Values.member.1".to_string(),
                    "supported".to_string(),
                ),
            ],
            "MissingRequiredParameter",
        ),
        RefusalCase::new(
            "no Values",
            vec![
                (
                    "Filters.member.1.Attribute".to_string(),
                    "LifecycleState".to_string(),
                ),
                ("Filters.member.1.Operator".to_string(), "=".to_string()),
            ],
            "MissingRequiredParameter",
        ),
        RefusalCase::new(
            "an unknown NextToken",
            vec![("NextToken".to_string(), "not-a-page-boundary".to_string())],
            "InvalidParameterValue",
        ),
    ];

    // One past the published ceiling on each of the two arrays: 11 terms, and 11
    // values inside one term.
    let mut too_many_terms: Vec<(String, String)> = Vec::new();
    for index in 0..=MAX_FILTER_ITEMS {
        too_many_terms.extend(filter_params(
            index,
            "LifecycleState",
            "!=",
            &owned(&["retired"]),
        ));
    }
    cases.push(RefusalCase::new(
        format!("{} filter terms", MAX_FILTER_ITEMS + 1),
        too_many_terms,
        "InvalidParameterValue",
    ));
    let too_many_values: Vec<String> = (0..=MAX_FILTER_ITEMS)
        .map(|index| format!("branch-{index}"))
        .collect();
    cases.push(RefusalCase::new(
        format!("{} values in one term", MAX_FILTER_ITEMS + 1),
        filter_params(0, "BranchName", "in", &too_many_values),
        "InvalidParameterValue",
    ));

    for RefusalCase { case, params, code } in &cases {
        let answer = list(&emulator, params).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} and body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            *code,
            "case {case:?}: expected the error code {code}, got body {}",
            answer.body
        );
        assert!(
            elements(&answer.body, "PlatformBranchSummaryList").is_empty(),
            "case {case:?}: a refused filter must report no branches, got body {}",
            answer.body
        );
    }
}

#[tokio::test]
async fn a_page_size_below_the_modelled_minimum_is_refused() {
    let emulator = Emulator::new();
    // `MaxRecords` is modelled `PlatformBranchMaxRecords, range min 1`, so the
    // boundary validator refuses 0 before the handler runs. The error CODE a
    // boundary refusal carries is protocol-specific (see `crate::scaffolded`),
    // so only the status is asserted here.
    for max_records in ["0", "-1"] {
        let answer = list(
            &emulator,
            &[("MaxRecords".to_string(), max_records.to_string())],
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {max_records:?}: expected HTTP 400 for a MaxRecords below the modelled \
             minimum, got {} and body {}",
            answer.status, answer.body
        );
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Walk every page at `max_records`, with the filter terms in `filters`,
/// returning the branches in the order they were reported.
async fn walk_pages(
    emulator: &Emulator,
    case: &str,
    max_records: usize,
    filters: &[(String, String)],
) -> Vec<Branch> {
    let mut collected: Vec<Branch> = Vec::new();
    let mut next_token: Option<String> = None;
    // The catalog is finite and every page carries at least one branch, so this
    // bound can only be reached by a token that fails to advance.
    for page in 0..=1_000 {
        let mut params = filters.to_vec();
        params.push(("MaxRecords".to_string(), max_records.to_string()));
        if let Some(token) = &next_token {
            params.push(("NextToken".to_string(), token.clone()));
        }
        let answer = list(emulator, &params).await;
        let reported = branches(&answer, case);
        assert!(
            reported.len() <= max_records,
            "case {case:?}: page {page} returned {} branches for MaxRecords={max_records}",
            reported.len()
        );
        let token = answer.value("NextToken").map(str::to_string);
        if token.is_some() {
            assert_eq!(
                reported.len(),
                max_records,
                "case {case:?}: page {page} returned a NextToken on a partial page of {} \
                 branch(es)",
                reported.len()
            );
        }
        collected.extend(reported);
        match token {
            Some(token) => next_token = Some(token),
            None => return collected,
        }
    }
    panic!("case {case:?}: paging did not terminate at MaxRecords={max_records}");
}

#[tokio::test]
async fn paging_at_any_page_size_yields_the_whole_catalog_once_in_one_order() {
    let emulator = Emulator::new();
    let catalog = full_catalog(&emulator).await;
    let total = catalog.len();

    // Page sizes spanning the range: the smallest the model allows, sizes that
    // do and do not divide the catalog, and the two sizes either side of its
    // length.
    let sizes = [1usize, 2, 3, 7, total - 1, total, total + 1];
    for max_records in sizes {
        let case = format!("MaxRecords={max_records}");
        let walked = walk_pages(&emulator, &case, max_records, &[]).await;
        assert_eq!(
            walked, catalog,
            "case {case:?}: expected paging to report the unpaged catalog exactly once, in the \
             same order, got {} branch(es)",
            walked.len()
        );
    }
}

#[tokio::test]
async fn a_page_boundary_is_stable_across_calls() {
    let emulator = Emulator::new();
    let catalog = full_catalog(&emulator).await;

    // The same request answers with the same page and the same token every
    // time, and the token names the branch the next page starts at.
    for max_records in [1usize, 4] {
        let case = format!("stable MaxRecords={max_records}");
        let first = list(
            &emulator,
            &[("MaxRecords".to_string(), max_records.to_string())],
        )
        .await;
        let again = list(
            &emulator,
            &[("MaxRecords".to_string(), max_records.to_string())],
        )
        .await;
        assert_eq!(
            branches(&first, &case),
            branches(&again, &case),
            "case {case:?}: the same request must answer with the same page"
        );
        let token = first
            .value("NextToken")
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a NextToken on a partial page, got body {}",
                    first.body
                )
            })
            .to_string();

        let second = list(
            &emulator,
            &[
                ("MaxRecords".to_string(), max_records.to_string()),
                ("NextToken".to_string(), token.clone()),
            ],
        )
        .await;
        let second_page = branches(&second, &case);
        let expected_start = &catalog[max_records];
        assert_eq!(
            second_page.first(),
            Some(expected_start),
            "case {case:?}: expected the token {token:?} to resume at the branch after the \
             first page, got {second_page:?}"
        );
    }
}

#[tokio::test]
async fn paging_and_filtering_compose() {
    let emulator = Emulator::new();
    let catalog = full_catalog(&emulator).await;

    // Each term selects a strict, non-empty subset, which is then paged one and
    // two branches at a time.
    let cases = vec![
        (
            "supported branches",
            Term::new("LifecycleState", "=", owned(&["supported"])),
        ),
        (
            "worker-tier branches",
            Term::new("TierType", "=", owned(&["Worker/SQS/HTTP"])),
        ),
        (
            "branches not retired",
            Term::new("LifecycleState", "not_in", owned(&["retired"])),
        ),
    ];

    for (case, term) in &cases {
        let expected: Vec<Branch> = catalog
            .iter()
            .filter(|branch| term.matches(branch))
            .cloned()
            .collect();
        assert!(
            !expected.is_empty() && expected.len() < catalog.len(),
            "case {case:?}: expected a strict non-empty subset of the catalog, got {} of {}",
            expected.len(),
            catalog.len()
        );
        let filters = term.params(0);
        for max_records in [1usize, 2] {
            let case = format!("{case} at MaxRecords={max_records}");
            let walked = walk_pages(&emulator, &case, max_records, &filters).await;
            assert_eq!(
                walked, expected,
                "case {case:?}: expected paging a filtered list to report exactly the matching \
                 branches once, got {} branch(es)",
                walked.len()
            );
        }
    }
}
