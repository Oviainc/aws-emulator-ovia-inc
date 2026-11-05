// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests that an id list a request carries names a SET, driven over the
//! wire through [`smithy_server_runtime::MultiServiceRouter::dispatch`] with the
//! harness in [`super::detector_lifecycle`].
//!
//! The rule under test: an operation given a list of resource ids resolves each
//! DISTINCT id once, so an id the request names more than once is processed and
//! reported exactly once and the answer is the one the same request with the
//! repetitions removed gets. It is asserted over a TABLE of id lists — rows differing
//! along how many times an id repeats, whether the repetitions are adjacent or
//! separated, how many ids repeat, whether an id the store does not hold repeats too,
//! the list's length up to the modelled bound, and the ordering the request asks for
//! — and across the operations the rule spans: the batch read of findings by id
//! (`GetFindings`), the batch read of members by account id (`GetMembers`), and the
//! account scope an organization configuration stores and reports
//! (`CreateCustomDetectionRuleOrgConfiguration` /
//! `UpdateCustomDetectionRuleOrgConfiguration` read back through
//! `GetCustomDetectionRuleOrgConfiguration`).

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::custom_detection_rule_org_configuration_update::{
    create_org_configuration, get_org_configuration, update_org_configuration,
};
use crate::detector_lifecycle::{
    all_finding_ids, call, enabled_detector, new_router, published_rule_ids, Router,
};
use crate::member_association::{
    create_members, get_members, MEMBERS, NOT_AN_ASSOCIATED_MEMBER,
};

/// The finding types the detector under test is populated with. Source: the finding
/// types this service's sample catalogue publishes (`CreateSampleFindings`). The rule
/// is about the id list, not about the type, so the rows only need the findings to be
/// distinguishable from one another.
const SAMPLE_TYPES: [&str; 4] = [
    "Backdoor:EC2/DenialOfService.Tcp",
    "Discovery:S3/MaliciousIPCaller",
    "Execution:Runtime/ReverseShell",
    "Policy:Kubernetes/ExposedDashboard",
];

/// The most ids a `GetFindings` request may carry: `FindingIds` is `len 0..50`
/// (`smithy_query(operation="GetFindings")`).
const MAX_FINDING_IDS: usize = 50;

/// Well-formed finding ids (32 lowercase hex characters) no `CreateSampleFindings`
/// minted, so the detector holds no finding for either.
const UNHELD_FINDING_IDS: [&str; 2] = [
    "00000000000000000000000000000000",
    "ffffffffffffffffffffffffffffffff",
];

/// A well-formed account id (12 decimal digits) no `CreateMembers` here records.
const NON_MEMBER_ACCOUNT_ID: &str = "424242424242";

/// `GetFindings` is `POST /detector/{DetectorId}/findings/get`
/// (`smithy_query(operation="GetFindings")`).
async fn get_findings(router: &Router, detector_id: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/findings/get"),
        Some(body),
    )
    .await
}

/// An enabled detector holding one finding per entry of [`SAMPLE_TYPES`], and its
/// finding ids in list order.
async fn detector_with_findings(router: &Router) -> (String, Vec<String>) {
    let detector_id = enabled_detector(router).await;
    let (status, body) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/findings/create"),
        Some(json!({"findingTypes": SAMPLE_TYPES})),
    )
    .await;
    assert_eq!(
        status, 200,
        "create-sample-findings: expected 200, got {status} {body:?}"
    );
    let ids = all_finding_ids(router, &detector_id, MAX_FINDING_IDS as i64).await;
    assert_eq!(
        ids.len(),
        SAMPLE_TYPES.len(),
        "the detector must hold one finding per sample type, got {ids:?}"
    );
    (detector_id, ids)
}

/// The `id` of each finding an answer carries, in answer order.
fn finding_ids(body: &Value) -> Vec<String> {
    body["findings"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|finding| finding["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The DISTINCT entries of `ids`, in the order they first appear — what the request
/// the rule says the service answers carries.
fn distinct(ids: &[String]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    ids.iter()
        .filter(|id| seen.insert((*id).clone()))
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// GetFindings: a repeated findingId is reported once.
// ---------------------------------------------------------------------------

/// One row of the table: how the request's `findingIds` list is built.
struct Case {
    label: &'static str,
    /// Indices into the detector's finding ids, in request order. An index may
    /// repeat, which is what the rule is about.
    held: Vec<usize>,
    /// Ids the detector holds no finding for, appended in request order.
    unheld: Vec<&'static str>,
    /// The `sortCriteria` the request carries, if any.
    sort: Option<Value>,
}

fn case(label: &'static str, held: Vec<usize>, unheld: Vec<&'static str>, sort: Option<Value>) -> Case {
    Case {
        label,
        held,
        unheld,
        sort,
    }
}

/// A request body for `findingIds`, plus the row's `sortCriteria`.
fn request_body(ids: &[String], sort: Option<&Value>) -> Value {
    let mut body = json!({"findingIds": ids});
    if let Some(sort) = sort {
        body["sortCriteria"] = sort.clone();
    }
    body
}

#[tokio::test]
async fn get_findings_reports_a_repeated_finding_id_once() {
    let router = new_router();
    let (detector_id, held) = detector_with_findings(&router).await;

    let cases = vec![
        case("a single id, the control row", vec![0], Vec::new(), None),
        case("every id once, the control row", vec![0, 1, 2, 3], Vec::new(), None),
        case("one id twice, adjacent", vec![0, 0], Vec::new(), None),
        case("one id five times", vec![1, 1, 1, 1, 1], Vec::new(), None),
        case("a repeat separated by another id", vec![0, 1, 0], Vec::new(), None),
        case(
            "every id twice, adjacent",
            vec![0, 0, 1, 1, 2, 2, 3, 3],
            Vec::new(),
            None,
        ),
        case(
            "every id twice, interleaved",
            vec![0, 1, 2, 3, 0, 1, 2, 3],
            Vec::new(),
            None,
        ),
        case(
            "a repeat beside a repeated id the detector does not hold",
            vec![2, 2],
            vec![UNHELD_FINDING_IDS[0], UNHELD_FINDING_IDS[0]],
            None,
        ),
        case(
            "only ids the detector does not hold, each repeated",
            Vec::new(),
            vec![
                UNHELD_FINDING_IDS[0],
                UNHELD_FINDING_IDS[1],
                UNHELD_FINDING_IDS[0],
                UNHELD_FINDING_IDS[1],
            ],
            None,
        ),
        case(
            "the modelled bound filled by one id",
            vec![3; MAX_FINDING_IDS],
            Vec::new(),
            None,
        ),
        case(
            "the modelled bound filled by every id in turn",
            (0..MAX_FINDING_IDS).map(|i| i % 4).collect(),
            Vec::new(),
            None,
        ),
        case(
            "a repeat under an ascending sort",
            vec![2, 2, 0, 1],
            Vec::new(),
            Some(json!({"attributeName": "severity", "orderBy": "ASC"})),
        ),
        case(
            "a repeat under a descending sort",
            vec![1, 3, 1, 3],
            Vec::new(),
            Some(json!({"attributeName": "createdAt", "orderBy": "DESC"})),
        ),
    ];

    for Case {
        label,
        held: indices,
        unheld,
        sort,
    } in cases
    {
        let mut requested: Vec<String> = indices.iter().map(|i| held[*i].clone()).collect();
        requested.extend(unheld.iter().map(|id| (*id).to_string()));
        let expected: Vec<String> = distinct(&requested)
            .into_iter()
            .filter(|id| held.contains(id))
            .collect();

        let (status, body) =
            get_findings(&router, &detector_id, request_body(&requested, sort.as_ref())).await;
        assert_eq!(
            status, 200,
            "case {label:?}: get-findings expected 200, got {status} {body:?}"
        );

        let answered = finding_ids(&body);
        assert_eq!(
            answered.len(),
            expected.len(),
            "case {label:?}: expected {} findings for {requested:?}, got {answered:?}",
            expected.len()
        );
        assert_eq!(
            answered.iter().collect::<BTreeSet<_>>(),
            expected.iter().collect::<BTreeSet<_>>(),
            "case {label:?}: expected the findings {expected:?}, got {answered:?}"
        );

        // The answer to a list with repetitions IS the answer to the same list
        // without them — every member of every finding, not just the ids.
        let deduplicated = distinct(&requested);
        let (status, without_repeats) = get_findings(
            &router,
            &detector_id,
            request_body(&deduplicated, sort.as_ref()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: get-findings for {deduplicated:?} expected 200, got {status} {without_repeats:?}"
        );
        assert_eq!(
            body, without_repeats,
            "case {label:?}: the answer for {requested:?} must be the answer for {deduplicated:?}, got {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// GetMembers: the same rule on the sibling batch read.
// ---------------------------------------------------------------------------

/// The members, and the accounts with no member record, an answer reports — each as
/// the ids it carries, in answer order.
fn member_and_unprocessed_ids(body: &Value) -> (Vec<String>, Vec<String>) {
    let ids = |member: &str, key: &str| -> Vec<String> {
        body[member]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|entry| entry[key].as_str().unwrap_or_default().to_string())
            .collect()
    };
    (
        ids("members", "accountId"),
        ids("unprocessedAccounts", "accountId"),
    )
}

/// One row of the `GetMembers` table.
struct MemberCase {
    label: &'static str,
    /// The `accountIds` the request carries, in request order; an id may repeat.
    requested: Vec<&'static str>,
    /// The member account ids the answer must report, in answer order.
    expected_members: Vec<&'static str>,
    /// The account ids the answer must report as not processed, in answer order.
    expected_unprocessed: Vec<&'static str>,
}

fn member_case(
    label: &'static str,
    requested: Vec<&'static str>,
    expected_members: Vec<&'static str>,
    expected_unprocessed: Vec<&'static str>,
) -> MemberCase {
    MemberCase {
        label,
        requested,
        expected_members,
        expected_unprocessed,
    }
}

#[tokio::test]
async fn get_members_reports_a_repeated_account_id_once() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let (status, body) = create_members(&router, &detector_id, &MEMBERS[..2]).await;
    assert_eq!(
        status, 200,
        "create-members: expected 200, got {status} {body:?}"
    );

    let (first, second) = (MEMBERS[0].0, MEMBERS[1].0);
    let cases = vec![
        member_case(
            "every account once, the control row",
            vec![first, second],
            vec![first, second],
            Vec::new(),
        ),
        member_case(
            "one member twice, adjacent",
            vec![first, first],
            vec![first],
            Vec::new(),
        ),
        member_case(
            "a member repeated four times",
            vec![second, second, second, second],
            vec![second],
            Vec::new(),
        ),
        member_case(
            "both members repeated, interleaved",
            vec![first, second, first, second],
            vec![first, second],
            Vec::new(),
        ),
        member_case(
            "a repeated account with no member record",
            vec![first, NON_MEMBER_ACCOUNT_ID, NON_MEMBER_ACCOUNT_ID, first],
            vec![first],
            vec![NON_MEMBER_ACCOUNT_ID],
        ),
    ];

    for MemberCase {
        label,
        requested,
        expected_members,
        expected_unprocessed,
    } in cases
    {
        let (status, body) = get_members(&router, &detector_id, &requested).await;
        assert_eq!(
            status, 200,
            "case {label:?}: get-members expected 200, got {status} {body:?}"
        );
        let (members, unprocessed) = member_and_unprocessed_ids(&body);
        assert_eq!(
            members, expected_members,
            "case {label:?}: expected the members {expected_members:?} for {requested:?}, got {members:?}"
        );
        assert_eq!(
            unprocessed, expected_unprocessed,
            "case {label:?}: expected the unprocessed accounts {expected_unprocessed:?} for {requested:?}, got {unprocessed:?}"
        );
        for entry in body["unprocessedAccounts"].as_array().cloned().unwrap_or_default() {
            assert_eq!(
                entry["result"],
                json!(NOT_AN_ASSOCIATED_MEMBER),
                "case {label:?}: expected the documented reason, got {entry:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The organization configuration's account scope: stored once per distinct id.
// ---------------------------------------------------------------------------

/// The accounts an organization configuration may be scoped to: 12 decimal digits
/// each, the form `AccountId` takes (`len 12..12`,
/// `smithy_query(shape="AccountId")`).
const SCOPE_ACCOUNTS: [&str; 3] = ["111122223333", "222233334444", "333344445555"];

/// The modes the scope rows are configured in
/// (`smithy_query(shape="AssociationMode")`). A rule holds one configuration per
/// mode, so the rows below are spread over `(rule, mode)` pairs rather than over
/// rules alone.
const MODES: [&str; 2] = ["LIVE", "DRY_RUN"];

#[tokio::test]
async fn an_organization_configuration_scopes_a_repeated_account_once() {
    let router = new_router();
    let rules = published_rule_ids(&router).await;

    let cases: [(&str, Vec<&str>, Vec<&str>); 4] = [
        ("every account once, the control row", vec![SCOPE_ACCOUNTS[0], SCOPE_ACCOUNTS[1]], vec![SCOPE_ACCOUNTS[0], SCOPE_ACCOUNTS[1]]),
        ("one account twice, adjacent", vec![SCOPE_ACCOUNTS[0], SCOPE_ACCOUNTS[0]], vec![SCOPE_ACCOUNTS[0]]),
        (
            "a repeat separated by another account",
            vec![SCOPE_ACCOUNTS[0], SCOPE_ACCOUNTS[1], SCOPE_ACCOUNTS[0]],
            vec![SCOPE_ACCOUNTS[0], SCOPE_ACCOUNTS[1]],
        ),
        (
            "every account twice, adjacent",
            vec![
                SCOPE_ACCOUNTS[0],
                SCOPE_ACCOUNTS[0],
                SCOPE_ACCOUNTS[1],
                SCOPE_ACCOUNTS[1],
                SCOPE_ACCOUNTS[2],
                SCOPE_ACCOUNTS[2],
            ],
            vec![SCOPE_ACCOUNTS[0], SCOPE_ACCOUNTS[1], SCOPE_ACCOUNTS[2]],
        ),
    ];

    // Each row needs a `(rule, mode)` pair of its own, since a rule holds one
    // configuration per mode: the catalogue has to publish enough rules for the rows
    // to pair off against both modes.
    assert!(
        rules.len() * MODES.len() >= cases.len(),
        "the catalogue must publish enough rules for {} distinct (rule, mode) pairs, got {rules:?}",
        cases.len()
    );

    for (index, (label, requested, expected)) in cases.into_iter().enumerate() {
        // One `(rule, mode)` pair per row: a rule holds one configuration per mode,
        // so the rows cannot share one.
        let rule_id = rules.get(index % rules.len()).unwrap_or_else(|| {
            panic!("case {label:?}: the catalogue must publish a rule, got {rules:?}")
        });
        let mode = MODES[(index / rules.len()) % MODES.len()];

        // The create stores the scope the request names, on the INCLUDE side.
        let (status, body) = create_org_configuration(
            &router,
            json!({"ruleId": rule_id, "mode": mode, "includeAccountIds": requested}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: create-org-configuration expected 200, got {status} {body:?}"
        );
        let (status, body) = get_org_configuration(&router, rule_id, mode).await;
        assert_eq!(
            status, 200,
            "case {label:?}: get-org-configuration expected 200, got {status} {body:?}"
        );
        assert_eq!(
            body["configuration"]["includeAccountIds"],
            json!(expected),
            "case {label:?}: expected includeAccountIds {expected:?} for {requested:?}, got {body:?}"
        );

        // The update writes the same scope through the EXCLUDE side — the other
        // half of the one mutually exclusive scope — and the read reports it the
        // same way.
        let (status, body) = update_org_configuration(
            &router,
            rule_id,
            json!({"mode": mode, "excludeAccountIds": requested}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: update-org-configuration expected 200, got {status} {body:?}"
        );
        let (status, body) = get_org_configuration(&router, rule_id, mode).await;
        assert_eq!(
            status, 200,
            "case {label:?}: get-org-configuration expected 200, got {status} {body:?}"
        );
        assert_eq!(
            body["configuration"]["excludeAccountIds"],
            json!(expected),
            "case {label:?}: expected excludeAccountIds {expected:?} for {requested:?}, got {body:?}"
        );
    }
}
