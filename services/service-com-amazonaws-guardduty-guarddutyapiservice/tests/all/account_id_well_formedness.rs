// SPDX-License-Identifier: CC0-1.0

//! The account-id well-formedness refusal, asserted through EVERY operation whose
//! input carries account ids — the member operations that take an `AccountIds` list,
//! the invitation decline and delete that take one, the create that takes an `AccountDetails`
//! list, the organization-configuration pair whose include/exclude lists together
//! name one account scope, and the usage report whose criteria carry the list.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch` (the
//! harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rule under test, asserted over a SET of inputs rather than one recorded call:
//! an account id is 12 decimal digits, and an operation handed a value that cannot be
//! an account id at all refuses the WHOLE call with HTTP 400 under the service's
//! `InvalidInputException` class — with GuardDuty's fixed format-rejection text for
//! the parameter that carried the value (the LIST, where the request names one, and
//! the `accountId` member where the id sits inside a structure), echoing nothing of
//! the caller's value — before the store is read or written, while a well-formed id
//! the administrator holds no member record for is not a refusal at all but a
//! per-account result. The rows differ along the malformed value's shape, along where
//! in the list it sits, along the list's length and along which operation (and which
//! of the two scope lists) carried it.

use serde_json::{json, Value};

use crate::client_rejections::{account_ids_format, ACCOUNT_ID_FORMAT};
use crate::detector_lifecycle::{
    as_delegated_admin, call, enabled_detector, error_code, error_shape, new_router, Router,
};
// The malformed, non-member and member tables the member suites share, so no suite
// can drift from another on what a malformed account id is.
use crate::member_association::{
    create_members, get_members, MALFORMED_ACCOUNT_IDS, MEMBERS, NON_MEMBER_ACCOUNT_IDS,
};

/// The error CLASS every client rejection on this service carries in the body's
/// `__type` discriminator and in `x-amzn-errortype`'s class position.
const CLASS: &str = "InvalidInputException";

/// The modelled error SHAPE that class is enveloped in, as `x-amzn-errortype`
/// carries it.
const ENVELOPE: &str = "BadRequestException";

/// The wire name of the LIST member most of these operations carry the account ids
/// in: `AccountIds` renders `accountIds`
/// (`smithy_query(operation="StopMonitoringMembers")`), and `GetUsageStatistics`
/// nests the same list under its criteria
/// (`smithy_query(query="shape",shape="UsageCriteria")`).
const ACCOUNT_IDS_MEMBER: &str = "accountIds";

/// The two account lists an organization configuration's scope is carried by
/// (`smithy_query(shape="DetectionRuleOrgConfiguration")`).
const INCLUDE_ACCOUNT_IDS_MEMBER: &str = "includeAccountIds";
/// See [`INCLUDE_ACCOUNT_IDS_MEMBER`].
const EXCLUDE_ACCOUNT_IDS_MEMBER: &str = "excludeAccountIds";

/// The message `operation` refuses a malformed account id with.
///
/// GuardDuty words the format rejection in terms of the parameter that carried the
/// malformed value: the LIST, where the request names one, and the `accountId`
/// member itself where the id sits inside a structure — which is what
/// `CreateMembers` sends (`AccountDetail.AccountId` —
/// `smithy_query(shape="AccountDetail")`).
fn expected_message(operation: &str) -> String {
    match operation {
        "CreateMembers" => ACCOUNT_ID_FORMAT.to_string(),
        "CreateCustomDetectionRuleOrgConfiguration" => {
            account_ids_format(INCLUDE_ACCOUNT_IDS_MEMBER)
        }
        "UpdateCustomDetectionRuleOrgConfiguration" => {
            account_ids_format(EXCLUDE_ACCOUNT_IDS_MEMBER)
        }
        _ => account_ids_format(ACCOUNT_IDS_MEMBER),
    }
}

/// `ListCustomDetectionRules` is `POST /custom-detection-rule/rule`
/// (`smithy_query(operation="ListCustomDetectionRules")`).
const RULES_PATH: &str = "/custom-detection-rule/rule";

/// The create is `POST /custom-detection-rule/org-configuration`; the read and the
/// update are `/custom-detection-rule/org-configuration/{RuleId}`
/// (`smithy_query(operation="CreateCustomDetectionRuleOrgConfiguration,GetCustomDetectionRuleOrgConfiguration,UpdateCustomDetectionRuleOrgConfiguration")`).
const ORG_CONFIGURATION_PATH: &str = "/custom-detection-rule/org-configuration";

/// One `AssociationMode` member, which both organization-configuration operations
/// require (`smithy_query(shape="AssociationMode")`).
const MODE: &str = "LIVE";

/// The Region the harness signs every request below for, and so the Region the
/// organization-configuration operations judge the delegated-administrator standing
/// they require against.
const HOME_REGION: &str = "us-east-1";

/// One `UsageStatisticType` member, which `GetUsageStatistics` requires
/// (`smithy_query(shape="UsageStatisticType")`); the usage suite asserts the rule
/// holds for every member of that enum.
const USAGE_STATISTIC_TYPE: &str = "SUM_BY_ACCOUNT";

/// Every operation whose input carries account ids, with the list that carries them.
///
/// The last two carry the scope as a PAIR of lists ("mutually exclusive" —
/// `smithy_query(query="doc",shape="DetectionRuleOrgConfiguration",member="IncludeAccountIds")`),
/// so the create is driven through `includeAccountIds` and the update through
/// `excludeAccountIds`: both sides of the pair are covered.
const OPERATIONS: [&str; 12] = [
    "GetMembers",
    "GetUsageStatistics",
    "InviteMembers",
    "DeclineInvitations",
    "DeleteInvitations",
    "DisassociateMembers",
    "StopMonitoringMembers",
    "GetMemberDetectors",
    "UpdateMemberDetectors",
    "CreateMembers",
    "CreateCustomDetectionRuleOrgConfiguration",
    "UpdateCustomDetectionRuleOrgConfiguration",
];

/// Where the malformed entry sits, and how long the list is: the rule is about the
/// list's CONTENT, so neither may change the answer. Every length stays inside
/// `AccountIds`' `len 1..50` (`smithy_query(operation="StopMonitoringMembers")`).
const PLACEMENTS: [(&str, usize, usize); 4] = [
    ("the only entry", 0, 1),
    ("first of three", 0, 3),
    ("in the middle of three", 1, 3),
    ("last of four", 3, 4),
];

/// A list of `len` account ids, well formed but for the entry at `index`.
fn account_ids_with(malformed: &str, index: usize, len: usize) -> Vec<&str> {
    let mut ids: Vec<&str> = (0..len)
        .map(|position| NON_MEMBER_ACCOUNT_IDS[position % NON_MEMBER_ACCOUNT_IDS.len()].0)
        .collect();
    ids[index] = malformed;
    ids
}

/// A list of `len` well-formed account ids the administrator holds no member record
/// for: the control the refusal must NOT fire on.
fn well_formed_account_ids(len: usize) -> Vec<&'static str> {
    (0..len)
        .map(|position| NON_MEMBER_ACCOUNT_IDS[position % NON_MEMBER_ACCOUNT_IDS.len()].0)
        .collect()
}

/// Send `operation` over the wire with `account_ids` in whichever member that
/// operation carries account ids in.
async fn send(
    router: &Router,
    operation: &str,
    detector_id: &str,
    rule_id: &str,
    account_ids: &[&str],
) -> (u16, Value) {
    let ids = json!(account_ids);
    match operation {
        "GetMembers" => {
            call(
                router,
                "POST",
                &format!("/detector/{detector_id}/member/get"),
                Some(json!({"accountIds": ids})),
            )
            .await
        }
        "InviteMembers" => {
            call(
                router,
                "POST",
                &format!("/detector/{detector_id}/member/invite"),
                Some(json!({"accountIds": ids})),
            )
            .await
        }
        "DeclineInvitations" => {
            // One of the two that name no detector: the inbox it answers from is the
            // one the request's Region serves (`POST /invitation/decline` —
            // `smithy_query(operation="DeclineInvitations")`).
            call(
                router,
                "POST",
                "/invitation/decline",
                Some(json!({"accountIds": ids})),
            )
            .await
        }
        "DeleteInvitations" => {
            // The other one that names no detector: the inbox it deletes
            // from is the one the request's Region serves
            // (`POST /invitation/delete` —
            // `smithy_query(operation="DeleteInvitations")`).
            call(
                router,
                "POST",
                "/invitation/delete",
                Some(json!({"accountIds": ids})),
            )
            .await
        }
        "DisassociateMembers" => {
            call(
                router,
                "POST",
                &format!("/detector/{detector_id}/member/disassociate"),
                Some(json!({"accountIds": ids})),
            )
            .await
        }
        "StopMonitoringMembers" => {
            call(
                router,
                "POST",
                &format!("/detector/{detector_id}/member/stop"),
                Some(json!({"accountIds": ids})),
            )
            .await
        }
        "GetMemberDetectors" => {
            call(
                router,
                "POST",
                &format!("/detector/{detector_id}/member/detector/get"),
                Some(json!({"accountIds": ids})),
            )
            .await
        }
        "UpdateMemberDetectors" => {
            call(
                router,
                "POST",
                &format!("/detector/{detector_id}/member/detector/update"),
                Some(json!({"accountIds": ids})),
            )
            .await
        }
        "CreateMembers" => {
            // `AccountDetail` carries the account id beside the email the account is
            // recorded under, so each entry gets an email of its own.
            let details: Vec<Value> = account_ids
                .iter()
                .enumerate()
                .map(|(position, account_id)| {
                    json!({"accountId": account_id, "email": format!("member-{position}@example.com")})
                })
                .collect();
            call(
                router,
                "POST",
                &format!("/detector/{detector_id}/member"),
                Some(json!({"accountDetails": details})),
            )
            .await
        }
        "CreateCustomDetectionRuleOrgConfiguration" => {
            // The operation is available only to the organization's delegated
            // administrator, so the standing is established first: the account-id
            // rule is what is under test here, not the authorization gate.
            as_delegated_admin(router, HOME_REGION).await;
            call(
                router,
                "POST",
                ORG_CONFIGURATION_PATH,
                Some(json!({"ruleId": rule_id, "mode": MODE, "includeAccountIds": ids})),
            )
            .await
        }
        "UpdateCustomDetectionRuleOrgConfiguration" => {
            as_delegated_admin(router, HOME_REGION).await;
            call(
                router,
                "PUT",
                &format!("{ORG_CONFIGURATION_PATH}/{rule_id}"),
                Some(json!({"mode": MODE, "excludeAccountIds": ids})),
            )
            .await
        }
        "GetUsageStatistics" => {
            // The one operation that carries the list inside a criteria structure
            // rather than at the top level, and the same `AccountIds` shape either
            // way (`smithy_query(query="shape",shape="UsageCriteria")`).
            call(
                router,
                "POST",
                &format!("/detector/{detector_id}/usage/statistics"),
                Some(json!({
                    "usageStatisticsType": USAGE_STATISTIC_TYPE,
                    "usageCriteria": {"accountIds": ids},
                })),
            )
            .await
        }
        other => panic!("no request builder for operation {other:?}"),
    }
}

/// The id of a rule the catalogue publishes, read over the wire so the test cannot
/// drift from it.
async fn published_rule_id(router: &Router) -> String {
    let (status, body) = call(router, "POST", RULES_PATH, Some(json!({}))).await;
    assert_eq!(
        status, 200,
        "list-custom-detection-rules: expected 200, got {status} {body:?}"
    );
    body["rules"]
        .as_array()
        .and_then(|rules| rules.first())
        .and_then(|rule| rule["ruleId"].as_str())
        .map(str::to_string)
        .unwrap_or_else(|| {
            panic!("list-custom-detection-rules: expected a published rule, got {body:?}")
        })
}

/// Assert one refusal: the status, the class in all three places it appears, the
/// message GuardDuty words the format rejection with for the parameter that carried
/// the value, and that the caller's value is nowhere in the response.
fn assert_refused(case: &str, status: u16, body: &Value, malformed: &str, expected: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        body["__type"].as_str(),
        Some(CLASS),
        "case {case:?}: expected {CLASS} in __type, got {body:?}"
    );
    assert_eq!(
        body["type"].as_str(),
        Some(CLASS),
        "case {case:?}: expected {CLASS} in the type member, got {body:?}"
    );
    assert_eq!(
        error_code(body),
        CLASS,
        "case {case:?}: expected {CLASS} on the wire, got {body:?}"
    );
    assert_eq!(
        error_shape(body),
        ENVELOPE,
        "case {case:?}: expected {ENVELOPE} in x-amzn-errortype, got {body:?}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(expected),
        "case {case:?}: expected GuardDuty's format-rejection text, got {body:?}"
    );
    let rendered = body.to_string();
    assert!(
        !rendered.contains(malformed),
        "case {case:?}: expected the refusal not to echo the caller's value, got {rendered}"
    );
}

/// Every operation that takes account ids refuses a malformed entry the same way,
/// wherever in the list it sits, and writes nothing while doing so.
#[tokio::test]
async fn malformed_account_id_refuses_every_operation_that_takes_one() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let rule_id = published_rule_id(&router).await;

    // One recorded member, so "the store was not touched" is asserted against
    // something the store actually holds.
    let (status, body) = create_members(&router, &detector_id, &MEMBERS[..1]).await;
    assert_eq!(
        status, 200,
        "create-members: expected 200, got {status} {body:?}"
    );
    let (status, baseline) = get_members(&router, &detector_id, &[MEMBERS[0].0]).await;
    assert_eq!(
        status, 200,
        "get-members baseline: expected 200, got {status} {baseline:?}"
    );

    for operation in OPERATIONS {
        let expected = expected_message(operation);
        for (malformed, why) in MALFORMED_ACCOUNT_IDS {
            for (placement, index, len) in PLACEMENTS {
                let ids = account_ids_with(malformed, index, len);
                let case = format!("{operation}: {why} as {placement}");
                let (status, body) = send(&router, operation, &detector_id, &rule_id, &ids).await;
                assert_refused(&case, status, &body, malformed, &expected);

                // Nothing was read into existence and nothing was written: the
                // recorded member reads back exactly as it did before the call.
                let (status, after) = get_members(&router, &detector_id, &[MEMBERS[0].0]).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: get-members after the refusal expected 200, got {status} {after:?}"
                );
                assert_eq!(
                    after, baseline,
                    "case {case:?}: expected the member record untouched, got {after:?}"
                );
            }
        }
    }

    // Neither refused organization-configuration call left a configuration behind:
    // the read still answers the modelled 404.
    as_delegated_admin(&router, HOME_REGION).await;
    let (status, body) = call(
        &router,
        "GET",
        &format!("{ORG_CONFIGURATION_PATH}/{rule_id}?mode={MODE}"),
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "get-org-configuration after the refusals: expected 404, got {status} {body:?}"
    );

    // Nor did any of them record a member for the well-formed ids they carried
    // beside the malformed one.
    let bystanders = well_formed_account_ids(NON_MEMBER_ACCOUNT_IDS.len());
    let (status, body) = get_members(&router, &detector_id, &bystanders).await;
    assert_eq!(
        status, 200,
        "get-members for the bystanders: expected 200, got {status} {body:?}"
    );
    assert_eq!(
        body["members"].as_array().map(Vec::len),
        Some(0),
        "expected no member recorded for the well-formed ids of a refused call, got {body:?}"
    );
}

/// The same operations, handed a list of WELL-FORMED account ids no member record
/// exists for, are not refused at all: the shared reader answers on the form only, so
/// each call is served and reports the accounts per-account instead.
#[tokio::test]
async fn well_formed_account_ids_are_not_refused_by_any_of_them() {
    for operation in OPERATIONS {
        for len in [1usize, 2, NON_MEMBER_ACCOUNT_IDS.len()] {
            let router = new_router();
            let detector_id = enabled_detector(&router).await;
            let rule_id = published_rule_id(&router).await;
            if operation == "UpdateCustomDetectionRuleOrgConfiguration" {
                // The update addresses a configuration that exists; the account-id
                // rule is what is under test here, not the modelled 404.
                as_delegated_admin(&router, HOME_REGION).await;
                let (status, body) = call(
                    &router,
                    "POST",
                    ORG_CONFIGURATION_PATH,
                    Some(json!({"ruleId": rule_id, "mode": MODE})),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "create-org-configuration: expected 200, got {status} {body:?}"
                );
            }
            let ids = well_formed_account_ids(len);
            let case = format!("{operation}: {len} well-formed account ids");
            let (status, body) = send(&router, operation, &detector_id, &rule_id, &ids).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} {body:?}"
            );
            assert_eq!(
                body["message"].as_str(),
                None,
                "case {case:?}: expected no refusal message, got {body:?}"
            );
        }
    }
}
