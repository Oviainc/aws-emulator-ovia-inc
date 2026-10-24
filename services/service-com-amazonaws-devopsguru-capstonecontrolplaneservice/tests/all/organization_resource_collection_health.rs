// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeOrganizationResourceCollectionHealth`.
//!
//! The harness (`Emulator`, `Reply`, the coverage-filter builders) and the health-entry
//! extractors are the ones `configuration_lifecycle` and `resource_collection_health`
//! already use: one buffered HTTP request through the same
//! [`smithy_server_runtime::MultiServiceRouter`] the emulator boots, so every assertion
//! is about wire behaviour.
//!
//! RULES under test:
//!
//! 1. An organization-wide health read belongs to the account that administers DevOps
//!    Guru for that organization, so any other caller is refused with the operation's
//!    HTTP 403 `AccessDeniedException` — which is every caller a client of this
//!    emulator can present, because nothing registers an administrator.
//! 2. For that administering caller the report carries, for the requested
//!    `OrganizationResourceCollectionType`, one entry per member of the scoped member
//!    accounts' persisted coverage — the same member sets and tallies
//!    `DescribeResourceCollectionHealth` reports over the same store, plus one
//!    `AccountHealth` per scoped member account for `AWS_ACCOUNT` — paged at the page
//!    size the request asks for and leaving the store unchanged.
//! 3. `AccountIds` and `OrganizationalUnitIds` NARROW that report to the member accounts
//!    they name; a filter naming none restricts nothing.

use serde_json::{json, Value};

use service_com_amazonaws_devopsguru_capstonecontrolplaneservice::state::PAGE_SIZE;

use crate::configuration_lifecycle::{cloud_formation, tags, Emulator, Reply};
use crate::resource_collection_health::{
    describe_health, entries, insight, member_without_insights, Entry, CLOSED, ONGOING, PROACTIVE,
    REACTIVE,
};

/// `OrganizationResourceCollectionType`'s full value list, from
/// `smithy_query(query="shape",shape="OrganizationResourceCollectionType")`.
const COLLECTION_TYPES: [&str; 4] = [
    "AWS_ACCOUNT",
    "AWS_CLOUD_FORMATION",
    "AWS_SERVICE",
    "AWS_TAGS",
];

/// Every response member that can carry a report, so a test can assert the three the
/// read was not asked about stay absent.
const REPORT_MEMBERS: [&str; 4] = ["/Account", "/CloudFormation", "/Service", "/Tags"];

/// The response member one collection type's report is carried on.
fn report_member(collection_type: &str) -> &'static str {
    match collection_type {
        "AWS_ACCOUNT" => "/Account",
        "AWS_CLOUD_FORMATION" => "/CloudFormation",
        "AWS_SERVICE" => "/Service",
        "AWS_TAGS" => "/Tags",
        other => panic!("case {other:?}: not an OrganizationResourceCollectionType"),
    }
}

/// `DescribeOrganizationResourceCollectionHealth`'s HTTP binding, from
/// `smithy_query(query="operation",operation="DescribeOrganizationResourceCollectionHealth")`:
/// `POST /organization/health/resource-collection` with every member in the body.
async fn describe_org_health(emulator: &Emulator, body: &Value) -> Reply {
    emulator
        .call("POST", "/organization/health/resource-collection", Some(body))
        .await
}

/// The simplest well-formed request: one collection type and no filter.
fn request(collection_type: &str) -> Value {
    json!({ "OrganizationResourceCollectionType": collection_type })
}

/// The account this emulator serves, which is the one member account whose coverage and
/// insights the store holds. `AwsAccountId` is modelled `^\d{12}$`.
fn served_account() -> String {
    engine::emulator_account_id().to_string()
}

/// Twelve-digit account ids that are NOT the served account — `AwsAccountId`'s pattern,
/// from `smithy_query(query="shape",shape="AccountIdList")`. The list is as long as
/// `AccountIdList`'s modelled maximum of 5.
const OTHER_ACCOUNTS: [&str; 5] = [
    "210987654321",
    "111122223333",
    "444455556666",
    "777788889999",
    "999988887777",
];

/// Organizational unit ids matching `OrganizationalUnitId`'s pattern
/// `^ou-[0-9a-z]{4,32}-[a-z0-9]{8,32}$`, from
/// `smithy_query(query="shape",shape="OrganizationalUnitIdList")`.
const ORGANIZATIONAL_UNITS: [&str; 2] = ["ou-abc1-12345678", "ou-9xyz-abcdef0123456789"];

/// The `AccountHealth` entries of one reply: the member account and the two open-insight
/// counts `AccountInsightHealth` models.
fn account_entries(reply: &Reply) -> Vec<(String, Option<i64>, Option<i64>)> {
    reply
        .at("/Account")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    let insight = item.get("Insight");
                    (
                        item.get("AccountId")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        insight
                            .and_then(|i| i.get("OpenProactiveInsights"))
                            .and_then(Value::as_i64),
                        insight
                            .and_then(|i| i.get("OpenReactiveInsights"))
                            .and_then(Value::as_i64),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `ServiceName`s one reply's `Service` report names, in the order reported.
fn service_names(reply: &Reply) -> Vec<String> {
    reply
        .at("/Service")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item.get("ServiceName")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// An emulator in which the served account administers DevOps Guru for the organization,
/// as the Organizations-side registration no operation of this service performs would
/// record. Without it every organization-wide read is refused — rule 1 — so this is what
/// makes the reporting path reachable at all.
fn emulator_with_organization_administration() -> Emulator {
    let emulator = Emulator::new();
    emulator.delegate_organization_administration(&served_account());
    emulator
}

/// Walk every page of one organization-wide report, concatenating what `extract` reads
/// off each page, and asserting the page size the request asked for is respected.
async fn walk_report<T, F>(
    emulator: &Emulator,
    body: &Value,
    page_size: usize,
    extract: F,
) -> Vec<T>
where
    F: Fn(&Reply) -> Vec<T>,
{
    let mut all: Vec<T> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = body.clone();
        match token.as_deref() {
            Some(next) => {
                request["NextToken"] = json!(next);
            }
            None => {
                request.as_object_mut().expect("the request is an object").remove("NextToken");
            }
        }
        let reply = describe_org_health(emulator, &request).await;
        assert_eq!(
            reply.status, 200,
            "case {request}: the organization health read must succeed, got {}",
            reply.raw
        );
        let page = extract(&reply);
        assert!(
            page.len() <= page_size,
            "case {request}: expected at most {page_size} entries per page, got {}",
            page.len()
        );
        all.extend(page);
        token = reply
            .at("/NextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if token.is_none() {
            return all;
        }
    }
}

/// RULE 1: an organization-wide report is a view of an organization's member accounts,
/// which belongs to the account administering DevOps Guru for that organization. No
/// account administers one here, so every caller is refused with the operation's HTTP
/// 403 `AccessDeniedException` — not a 200 with an empty report, and not the scaffold's
/// 501.
///
/// The rows span every `OrganizationResourceCollectionType` and, for each, the filter
/// forms a client can send: none, named accounts, named organizational units, and a
/// chosen page size.
#[tokio::test]
async fn an_organization_read_is_denied_while_no_account_administers_devops_guru() {
    let emulator = Emulator::new();
    // Coverage and insights exist, so the refusal is not just an empty store answering.
    emulator
        .update_resource_collection("ADD", cloud_formation(&["alpha", "beta"]))
        .await;
    emulator
        .update_resource_collection("ADD", tags(&[("Devops-guru-app", &["web"])]))
        .await;
    emulator.seed_insights(vec![insight(
        "i1",
        PROACTIVE,
        ONGOING,
        None,
        &["alpha"],
        &[],
        &["RDS"],
    )]);

    for collection_type in COLLECTION_TYPES {
        let filters: [(&str, Value); 4] = [
            ("no filter", request(collection_type)),
            (
                "the served account named",
                json!({
                    "OrganizationResourceCollectionType": collection_type,
                    "AccountIds": [served_account()],
                }),
            ),
            (
                "an organizational unit named",
                json!({
                    "OrganizationResourceCollectionType": collection_type,
                    "OrganizationalUnitIds": [ORGANIZATIONAL_UNITS[0]],
                }),
            ),
            (
                "a chosen page size",
                json!({
                    "OrganizationResourceCollectionType": collection_type,
                    "MaxResults": 1,
                }),
            ),
        ];
        for (name, body) in filters {
            let reply = describe_org_health(&emulator, &body).await;
            assert_eq!(
                reply.status, 403,
                "case {collection_type:?}/{name:?}: expected a 403 refusal, got status {} body {}",
                reply.status, reply.raw
            );
            assert!(
                reply.names_error("AccessDeniedException"),
                "case {collection_type:?}/{name:?}: expected an AccessDeniedException, got {}",
                reply.raw
            );
            for member in REPORT_MEMBERS {
                assert!(
                    reply.at(member).is_none(),
                    "case {collection_type:?}/{name:?}: a refusal must report no {member}, got {}",
                    reply.raw
                );
            }
        }
    }
}

/// One coverage case: the stack names and (key, values) pairs a client covers, and the
/// members each half is then expected to have an entry for.
type CoverageCase<'a> = (
    &'a str,
    &'a [&'a str],
    &'a [(&'a str, &'a [&'a str])],
    &'a [&'a str],
    &'a [&'a str],
);

/// RULE 2: for the administering caller, the report of a collection type carries exactly
/// one entry per member of the scoped member accounts' persisted coverage — a stack name
/// for `AWS_CLOUD_FORMATION`, an (`AppBoundaryKey`, `TagValue`) pair for `AWS_TAGS` — and
/// those entries are the ones `DescribeResourceCollectionHealth` reports over the same
/// store, so the account-scoped and organization-scoped reads cannot disagree. Only the
/// requested type's member is present.
///
/// The rows differ along which half of the collection is covered, the member count, the
/// number of values per key, and whether coverage was later removed.
#[tokio::test]
async fn the_report_has_one_entry_per_member_of_the_scoped_coverage() {
    let cases: [CoverageCase; 5] = [
        ("no coverage at all", &[], &[], &[], &[]),
        (
            "one stack added",
            &["my-stack"],
            &[],
            &["my-stack"],
            &[],
        ),
        (
            "three stacks are reported sorted",
            &["zeta", "alpha", "middle"],
            &[],
            &["alpha", "middle", "zeta"],
            &[],
        ),
        (
            "one app boundary key with two tag values",
            &[],
            &[("Devops-guru-production", &["web", "batch"])],
            &[],
            &["Devops-guru-production=batch", "Devops-guru-production=web"],
        ),
        (
            "both halves at once",
            &["payments", "ingest"],
            &[("Devops-guru-web", &["edge", "primary"])],
            &["ingest", "payments"],
            &["Devops-guru-web=edge", "Devops-guru-web=primary"],
        ),
    ];

    for (name, stacks, tag_entries, expect_stacks, expect_tags) in cases {
        let emulator = emulator_with_organization_administration();
        if !stacks.is_empty() {
            emulator
                .update_resource_collection("ADD", cloud_formation(stacks))
                .await;
        }
        if !tag_entries.is_empty() {
            emulator
                .update_resource_collection("ADD", tags(tag_entries))
                .await;
        }

        for (collection_type, expected_members) in [
            ("AWS_CLOUD_FORMATION", expect_stacks),
            ("AWS_TAGS", expect_tags),
        ] {
            let member = report_member(collection_type);
            let reply = describe_org_health(&emulator, &request(collection_type)).await;
            assert_eq!(
                reply.status, 200,
                "case {name:?}/{collection_type:?}: the read must succeed, got {}",
                reply.raw
            );
            let expected: Vec<Entry> = expected_members
                .iter()
                .map(|m| member_without_insights(m))
                .collect();
            let reported = entries(&reply, member);
            assert_eq!(
                reported, expected,
                "case {name:?}/{collection_type:?}: expected {expected:?}, got {reported:?}"
            );

            // The same member set and tallies the account-scoped read reports over the
            // same store.
            let account_scoped_type = collection_type;
            let account_scoped = describe_health(&emulator, account_scoped_type, None).await;
            assert_eq!(
                reported,
                entries(&account_scoped, member),
                "case {name:?}/{collection_type:?}: the organization-wide and account-scoped reads must agree, got {} and {}",
                reply.raw,
                account_scoped.raw
            );

            // Only the requested type's list is reported.
            for absent in REPORT_MEMBERS.iter().filter(|m| **m != member) {
                assert!(
                    reply.at(absent).is_none(),
                    "case {name:?}/{collection_type:?}: the read must not report {absent}, got {}",
                    reply.raw
                );
            }
        }
    }
}

/// One tally row: the case name, the insights seeded, and the open proactive / open
/// reactive counts expected for the member account.
type AccountTallyCase = (&'static str, Vec<(&'static str, &'static str, &'static str)>, i64, i64);

/// RULE 2, for `AWS_ACCOUNT`: the report carries one `AccountHealth` per scoped member
/// account, naming that account and counting ITS open insights by `InsightType` —
/// `AccountInsightHealth` models those two counts and nothing else, so no recovery time
/// is reported on it.
///
/// The rows differ along insight type, status and count, including closed insights, which
/// are no longer open and so count towards neither total.
#[tokio::test]
async fn the_account_report_counts_the_member_accounts_open_insights() {
    let cases: Vec<AccountTallyCase> = vec![
        ("no insight at all", vec![], 0, 0),
        (
            "one ongoing proactive insight",
            vec![("i1", PROACTIVE, ONGOING)],
            1,
            0,
        ),
        (
            "two ongoing proactive and one ongoing reactive insight",
            vec![
                ("i1", PROACTIVE, ONGOING),
                ("i2", PROACTIVE, ONGOING),
                ("i3", REACTIVE, ONGOING),
            ],
            2,
            1,
        ),
        (
            "closed insights are no longer open",
            vec![
                ("i1", PROACTIVE, CLOSED),
                ("i2", REACTIVE, CLOSED),
                ("i3", REACTIVE, ONGOING),
            ],
            0,
            1,
        ),
    ];

    for (name, seeded, proactive, reactive) in cases {
        let emulator = emulator_with_organization_administration();
        emulator.seed_insights(
            seeded
                .iter()
                .map(|(id, insight_type, status)| {
                    insight(id, insight_type, status, Some(1_000), &["watched"], &[], &[])
                })
                .collect(),
        );

        let reply = describe_org_health(&emulator, &request("AWS_ACCOUNT")).await;
        assert_eq!(
            reply.status, 200,
            "case {name:?}: the read must succeed, got {}",
            reply.raw
        );
        let reported = account_entries(&reply);
        assert_eq!(
            reported,
            vec![(served_account(), Some(proactive), Some(reactive))],
            "case {name:?}: expected the member account with ({proactive}, {reactive}) open insights, got {reported:?}"
        );
        let entry = reply
            .at("/Account/0/Insight")
            .cloned()
            .unwrap_or(Value::Null);
        assert!(
            entry.get("MeanTimeToRecoverInMilliseconds").is_none(),
            "case {name:?}: AccountInsightHealth models no recovery time, got {entry}"
        );
    }
}

/// RULE 2, for `AWS_SERVICE`: the report names one entry per Amazon Web Services service
/// the stored insights name — the store's record of a service DevOps Guru analysed — with
/// that service's open counts, exactly as the account-scoped read reports them.
#[tokio::test]
async fn the_service_report_names_the_services_the_stored_insights_name() {
    let emulator = emulator_with_organization_administration();
    emulator.seed_insights(vec![
        insight("i1", PROACTIVE, ONGOING, None, &[], &[], &["RDS", "LAMBDA"]),
        insight("i2", REACTIVE, ONGOING, None, &[], &[], &["RDS"]),
        insight("i3", REACTIVE, CLOSED, Some(7_000), &[], &[], &["S3"]),
    ]);

    let reply = describe_org_health(&emulator, &request("AWS_SERVICE")).await;
    assert_eq!(
        reply.status, 200,
        "the AWS_SERVICE read must succeed, got {}",
        reply.raw
    );
    assert_eq!(
        service_names(&reply),
        vec!["LAMBDA".to_string(), "RDS".to_string(), "S3".to_string()],
        "expected the services the insights name, sorted, got {}",
        reply.raw
    );
    let account_scoped = describe_health(&emulator, "AWS_SERVICE", None).await;
    assert_eq!(
        reply.at("/Service"),
        account_scoped.at("/Service"),
        "the organization-wide and account-scoped service reports must agree, got {} and {}",
        reply.raw,
        account_scoped.raw
    );
}

/// One scope row: the case name, the `AccountIds` and `OrganizationalUnitIds` filters,
/// and whether the member account is expected to remain in scope.
type ScopeCase<'a> = (&'a str, Option<Vec<String>>, Option<Vec<String>>, bool);

/// RULE 3: `AccountIds` and `OrganizationalUnitIds` narrow the report to the member
/// accounts they name — an account is in scope exactly while every filter the request
/// carries selects it, a filter naming nothing restricts nothing, and an out-of-scope
/// member contributes no entry to any collection type.
///
/// The rows differ along which filter is sent, how many ids it carries, whether the
/// member account is among them, and whether both filters are combined. The store records
/// no organizational-unit membership for its member account, so any unit filter leaves it
/// out of scope.
#[tokio::test]
async fn the_filters_narrow_the_report_to_the_accounts_they_name() {
    let ids = |values: &[&str]| Some(values.iter().map(ToString::to_string).collect::<Vec<_>>());
    assert!(
        !OTHER_ACCOUNTS.contains(&served_account().as_str()),
        "the other-account ids must name no account this emulator serves, got {:?} and {}",
        OTHER_ACCOUNTS,
        served_account()
    );
    let cases: Vec<ScopeCase> = vec![
        ("no filter at all", None, None, true),
        ("an empty AccountIds restricts nothing", ids(&[]), None, true),
        (
            "the member account named",
            ids(&[&served_account()]),
            None,
            true,
        ),
        (
            "the member account among other accounts",
            ids(&[OTHER_ACCOUNTS[0], &served_account(), OTHER_ACCOUNTS[1]]),
            None,
            true,
        ),
        (
            "one other account only",
            ids(&[OTHER_ACCOUNTS[0]]),
            None,
            false,
        ),
        (
            "five other accounts, the modelled maximum",
            ids(&OTHER_ACCOUNTS),
            None,
            false,
        ),
        (
            "an empty OrganizationalUnitIds restricts nothing",
            None,
            ids(&[]),
            true,
        ),
        (
            "one organizational unit",
            None,
            ids(&[ORGANIZATIONAL_UNITS[0]]),
            false,
        ),
        (
            "two organizational units",
            None,
            ids(&ORGANIZATIONAL_UNITS),
            false,
        ),
        (
            "the member account and an organizational unit",
            ids(&[&served_account()]),
            ids(&[ORGANIZATIONAL_UNITS[0]]),
            false,
        ),
    ];

    for (name, account_ids, organizational_unit_ids, in_scope) in cases {
        let emulator = emulator_with_organization_administration();
        emulator
            .update_resource_collection("ADD", cloud_formation(&["alpha", "beta"]))
            .await;
        emulator
            .update_resource_collection("ADD", tags(&[("Devops-guru-app", &["web"])]))
            .await;
        emulator.seed_insights(vec![insight(
            "i1",
            PROACTIVE,
            ONGOING,
            None,
            &["alpha"],
            &[],
            &["RDS"],
        )]);

        for collection_type in COLLECTION_TYPES {
            let mut body = request(collection_type);
            if let Some(values) = account_ids.clone() {
                body["AccountIds"] = json!(values);
            }
            if let Some(values) = organizational_unit_ids.clone() {
                body["OrganizationalUnitIds"] = json!(values);
            }
            let reply = describe_org_health(&emulator, &body).await;
            assert_eq!(
                reply.status, 200,
                "case {name:?}/{collection_type:?}: the read must succeed, got {}",
                reply.raw
            );
            let reported = reply
                .at(report_member(collection_type))
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or_default();
            if in_scope {
                assert!(
                    reported > 0,
                    "case {name:?}/{collection_type:?}: the member account is in scope, so its members must be reported, got {}",
                    reply.raw
                );
            } else {
                assert_eq!(
                    reported, 0,
                    "case {name:?}/{collection_type:?}: no account is in scope, so no entry may be reported, got {}",
                    reply.raw
                );
            }
        }
    }
}

/// RULE 2's paging half: the pages of one report concatenated are every member exactly
/// once, in order, with no repeats and no gaps; no page carries more than the
/// `MaxResults` the request asked for; and the last page carries no `NextToken`.
///
/// The rows are the page sizes a client may ask for — the modelled range of
/// `OrganizationResourceCollectionMaxResults` is 1..500 — plus the absent member, over a
/// member count that spans the service's own page size.
#[tokio::test]
async fn pages_walk_every_member_exactly_once_at_the_requested_page_size() {
    let emulator = emulator_with_organization_administration();
    let total = PAGE_SIZE + 3;
    let mut covered = 0;
    while covered < total {
        let batch: Vec<String> = (covered..(covered + 100).min(total))
            .map(|index| format!("stack-{index:05}"))
            .collect();
        covered += batch.len();
        let reply = emulator
            .update_resource_collection("ADD", json!({ "CloudFormation": { "StackNames": batch } }))
            .await;
        assert_eq!(
            reply.status, 200,
            "covering {covered} stacks must succeed, got {}",
            reply.raw
        );
    }
    let expected: Vec<Entry> = (0..total)
        .map(|index| member_without_insights(&format!("stack-{index:05}")))
        .collect();

    // `None` is the service's own page size; 1, 7 and 500 are the modelled minimum, a
    // value between the bounds, and the modelled maximum.
    for max_results in [None, Some(1), Some(7), Some(500)] {
        let mut body = request("AWS_CLOUD_FORMATION");
        if let Some(requested) = max_results {
            body["MaxResults"] = json!(requested);
        }
        let page_size = max_results.map_or(PAGE_SIZE, |requested| requested as usize);
        let walked = walk_report(&emulator, &body, page_size, |reply| {
            entries(reply, "/CloudFormation")
        })
        .await;
        assert_eq!(
            walked, expected,
            "case MaxResults={max_results:?}: the walk must report every covered stack exactly once in order, got {:?}",
            walked.iter().map(|e| e.member.clone()).collect::<Vec<_>>()
        );
    }
}

/// RULE 2's token half: a `NextToken` names a page of ONE report, so a token this service
/// never issued for the report being read names no page of it and is refused with the
/// operation's HTTP 400 `ValidationException` rather than seeking elsewhere.
///
/// The rows differ along how the token fails to belong: another collection type's cursor,
/// the account-scoped read's cursor over the same type, a cursor issued under a different
/// account scope, and a rewrite outside the modelled lowercase-hex form.
#[tokio::test]
async fn a_token_from_another_report_is_refused() {
    let emulator = emulator_with_organization_administration();
    emulator
        .update_resource_collection("ADD", cloud_formation(&["alpha", "beta", "gamma"]))
        .await;
    emulator
        .update_resource_collection(
            "ADD",
            tags(&[("Devops-guru-app", &["batch", "edge", "web"])]),
        )
        .await;

    /// The `NextToken` a one-entry page of `body` offers.
    async fn first_token(emulator: &Emulator, body: &Value) -> String {
        let mut body = body.clone();
        body["MaxResults"] = json!(1);
        let reply = describe_org_health(emulator, &body).await;
        assert_eq!(
            reply.status, 200,
            "case {body}: the first page must succeed, got {}",
            reply.raw
        );
        reply
            .at("/NextToken")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {body}: a second page must be offered, got {}", reply.raw))
            .to_string()
    }

    let cloud_formation_token = first_token(&emulator, &request("AWS_CLOUD_FORMATION")).await;
    let tags_token = first_token(&emulator, &request("AWS_TAGS")).await;
    let scoped_token = first_token(
        &emulator,
        &json!({
            "OrganizationResourceCollectionType": "AWS_CLOUD_FORMATION",
            "AccountIds": [served_account()],
        }),
    )
    .await;
    // The account-scoped read's own cursor over the same collection type.
    let account_scoped_reply = describe_health(&emulator, "AWS_CLOUD_FORMATION", None).await;
    let account_scoped_token = account_scoped_reply
        .at("/NextToken")
        .and_then(Value::as_str)
        .map(ToString::to_string);

    let mut refused: Vec<(&str, Value)> = vec![
        (
            "another collection type's cursor",
            json!({
                "OrganizationResourceCollectionType": "AWS_CLOUD_FORMATION",
                "MaxResults": 1,
                "NextToken": tags_token,
            }),
        ),
        (
            "a cursor issued under a different account scope",
            json!({
                "OrganizationResourceCollectionType": "AWS_CLOUD_FORMATION",
                "MaxResults": 1,
                "NextToken": scoped_token,
            }),
        ),
        (
            "an uppercase rewrite of this report's own cursor",
            json!({
                "OrganizationResourceCollectionType": "AWS_CLOUD_FORMATION",
                "MaxResults": 1,
                "NextToken": cloud_formation_token.to_uppercase(),
            }),
        ),
    ];
    if let Some(token) = account_scoped_token {
        refused.push((
            "the account-scoped read's cursor over the same type",
            json!({
                "OrganizationResourceCollectionType": "AWS_CLOUD_FORMATION",
                "MaxResults": 1,
                "NextToken": token,
            }),
        ));
    }

    for (name, body) in refused {
        let reply = describe_org_health(&emulator, &body).await;
        assert_eq!(
            reply.status, 400,
            "case {name:?}: expected a 400 refusal, got status {} body {}",
            reply.status, reply.raw
        );
        assert!(
            reply.names_error("ValidationException"),
            "case {name:?}: expected a ValidationException, got {}",
            reply.raw
        );
    }

    // The cursor this report DID issue resolves against it.
    let reply = describe_org_health(
        &emulator,
        &json!({
            "OrganizationResourceCollectionType": "AWS_CLOUD_FORMATION",
            "MaxResults": 1,
            "NextToken": cloud_formation_token,
        }),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "the cursor this report issued must resolve, got {}",
        reply.raw
    );
    assert_eq!(
        entries(&reply, "/CloudFormation"),
        vec![member_without_insights("beta")],
        "expected the second covered stack on the second page, got {}",
        reply.raw
    );
}

/// RULE 2's read half: a read must not change what a later caller sees — neither the
/// report itself, nor the coverage the report is derived from, nor the refusal a
/// non-administering caller gets.
///
/// The rows are every `OrganizationResourceCollectionType`, read repeatedly and in turn,
/// in an emulator where the read reports and in one where it refuses.
#[tokio::test]
async fn reading_the_organization_health_leaves_the_store_unchanged() {
    for administered in [false, true] {
        let emulator = if administered {
            emulator_with_organization_administration()
        } else {
            Emulator::new()
        };
        emulator
            .update_resource_collection("ADD", cloud_formation(&["alpha", "beta"]))
            .await;
        emulator
            .update_resource_collection("ADD", tags(&[("Devops-guru-app", &["web", "batch"])]))
            .await;
        emulator.seed_insights(vec![insight(
            "i1",
            REACTIVE,
            ONGOING,
            None,
            &["alpha"],
            &[],
            &["RDS"],
        )]);

        let coverage_before = (
            emulator.all_stack_names().await,
            emulator.all_tag_collections().await,
        );
        let mut first_bodies: Vec<(u16, String)> = Vec::new();
        for collection_type in COLLECTION_TYPES {
            let reply = describe_org_health(&emulator, &request(collection_type)).await;
            assert_eq!(
                reply.status,
                if administered { 200 } else { 403 },
                "case administered={administered}/{collection_type:?}: unexpected status, got {}",
                reply.raw
            );
            first_bodies.push((reply.status, reply.body.to_string()));
        }

        for round in 1..=3 {
            for (index, collection_type) in COLLECTION_TYPES.iter().enumerate() {
                let reply = describe_org_health(&emulator, &request(collection_type)).await;
                assert_eq!(
                    (reply.status, reply.body.to_string()),
                    first_bodies[index],
                    "case administered={administered}/{collection_type:?} round {round}: a repeated read must answer the same, got {}",
                    reply.raw
                );
            }
        }

        let coverage_after = (
            emulator.all_stack_names().await,
            emulator.all_tag_collections().await,
        );
        assert_eq!(
            coverage_after, coverage_before,
            "case administered={administered}: reading the organization health must not change the coverage: {coverage_before:?} before, {coverage_after:?} after"
        );
    }
}
