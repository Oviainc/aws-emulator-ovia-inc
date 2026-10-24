// SPDX-License-Identifier: CC0-1.0

//! What the ACCOUNT FIELD of a resource ARN may name, over the wire, for every reader of
//! this service that takes one.
//!
//! An ARN names the account that owns the resource it identifies. This emulator presents
//! exactly ONE standalone account (`engine::emulator_account_id`) and records no
//! organization, so an ARN whose account field is any other account asks about a resource
//! this service cannot answer about: it is refused with `InvalidParameterValueException`
//! (HTTP 400) carrying the service's cross-account message — the same refusal `accountIds`
//! naming that account answers with (`tests/all/cross_account_requests.rs`) — rather than
//! answered with preferences or a selection resolved under that other account.
//!
//! The rows below are the CLASS of ARN-valued members (one ARN member per reader, in each
//! ARN form that member documents) crossed with the class of account fields (the caller's
//! own account, and well-formed 12-digit ids naming another account), so no reader can
//! answer what another refuses.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    ANALYSIS_DELAY, DEFAULT_LOOK_BACK_PERIOD,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, emulator_with_analysed_services, error_code, opt_in, ACCOUNT, REGION};

/// The refusal a request about another account is answered with: the constant this
/// service's findings readers record for the condition, which names neither the member nor
/// the account — it states the permission that is missing and how an organization account
/// gets it.
const CROSS_ACCOUNT_MESSAGE: &str = "Invalid permission to access another account. If this \
     is an organization account, request the management account to enable service access.";

/// Well-formed 12-digit account ids that are NOT the caller's: the id the report cites, the
/// documentation's own example account, an id one digit from the emulator's own, and the
/// extremes of the 12-digit range.
const OTHER_ACCOUNTS: [&str; 5] = [
    "210987654321",
    "111122223333",
    "123456789012",
    "000000000001",
    "999999999999",
];

/// An EC2 instance ARN, one of the two forms the preference API accepts
/// (`GetEffectiveRecommendationPreferences$resourceArn`).
fn instance_arn_of(account_id: &str) -> String {
    format!("arn:aws:ec2:{REGION}:{account_id}:instance/i-0123456789abcdef0")
}

/// An Auto Scaling group ARN — the other form the preference API accepts, and the form
/// `GetAutoScalingGroupRecommendations$autoScalingGroupArns` takes.
fn group_arn_of(account_id: &str) -> String {
    format!(
        "arn:aws:autoscaling:{REGION}:{account_id}:autoScalingGroup:\
         12345678-1234-1234-1234-123456789012:autoScalingGroupName/web-tier"
    )
}

/// An Amazon EBS volume ARN (`GetEBSVolumeRecommendations$volumeArns`).
fn volume_arn_of(account_id: &str) -> String {
    format!("arn:aws:ec2:{REGION}:{account_id}:volume/vol-0123456789abcdef0")
}

/// A Lambda function ARN (`GetLambdaFunctionRecommendations$functionArns`).
fn function_arn_of(account_id: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{account_id}:function:checkout")
}

/// An Amazon ECS service ARN,
/// `arn:aws:ecs:region:aws_account_id:service/cluster-name/service-name`
/// (`GetECSServiceRecommendations$serviceArns`).
fn ecs_service_arn_of(account_id: &str) -> String {
    format!("arn:aws:ecs:{REGION}:{account_id}:service/web/checkout")
}

/// One ARN-valued member of one reader: the operation, the member, the ARN form it takes,
/// and whether the member is a LIST of ARNs or a single one.
struct ArnMember {
    operation: &'static str,
    member: &'static str,
    arn_of: fn(&str) -> String,
    list: bool,
}

/// Every reader of this service that takes a resource ARN, in each ARN form its member
/// documents.
const ARN_MEMBERS: [ArnMember; 6] = [
    ArnMember {
        operation: "GetEffectiveRecommendationPreferences",
        member: "resourceArn",
        arn_of: instance_arn_of,
        list: false,
    },
    ArnMember {
        operation: "GetEffectiveRecommendationPreferences",
        member: "resourceArn",
        arn_of: group_arn_of,
        list: false,
    },
    ArnMember {
        operation: "GetAutoScalingGroupRecommendations",
        member: "autoScalingGroupArns",
        arn_of: group_arn_of,
        list: true,
    },
    ArnMember {
        operation: "GetEBSVolumeRecommendations",
        member: "volumeArns",
        arn_of: volume_arn_of,
        list: true,
    },
    ArnMember {
        operation: "GetLambdaFunctionRecommendations",
        member: "functionArns",
        arn_of: function_arn_of,
        list: true,
    },
    ArnMember {
        operation: "GetECSServiceRecommendations",
        member: "serviceArns",
        arn_of: ecs_service_arn_of,
        list: true,
    },
];

impl ArnMember {
    /// The request that carries `arn` in this member.
    fn request(&self, arn: &str) -> Value {
        if self.list {
            json!({self.member: [arn]})
        } else {
            json!({self.member: arn})
        }
    }

    /// How a case names this row: the operation and the member it carries the ARN in.
    fn case(&self, account_id: &str) -> String {
        format!("{}${} of {account_id}", self.operation, self.member)
    }
}

/// Opt the account in and let the analysis its enrollment starts come due — the gate every
/// recommendation reader of this service applies. The span is the service's own
/// ([`ANALYSIS_DELAY`]); the extra margin covers scheduler jitter.
async fn analysed(router: &MultiServiceRouter) {
    opt_in(router).await;
    tokio::time::sleep(ANALYSIS_DELAY + Duration::from_millis(100)).await;
}

/// [`OTHER_ACCOUNTS`] is only a set of OTHER accounts while none of its ids is the one this
/// service answers for: asserted here, so a change to the emulator's account id fails
/// loudly instead of turning a row below into a refusal of the caller's own account.
fn assert_other_accounts_are_other() {
    assert!(
        !OTHER_ACCOUNTS.contains(&ACCOUNT),
        "the other-account rows must not name the account this service answers for \
         ({ACCOUNT}), got {OTHER_ACCOUNTS:?}"
    );
}

/// An ARN of the caller's own account is ANSWERED (whether or not this emulator holds a
/// resource by that ARN); an ARN of any other account is refused with the service's
/// cross-account message, by every reader that takes one.
#[tokio::test]
async fn a_resource_arn_of_another_account_is_refused_by_every_reader() {
    assert_other_accounts_are_other();
    let router = emulator_with_analysed_services();
    analysed(&router).await;

    for row in &ARN_MEMBERS {
        let own = (row.arn_of)(ACCOUNT);
        let case = row.case(ACCOUNT);
        let (status, body) = call(&router, row.operation, row.request(&own)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an ARN of the account this service answers for is answered, got \
             {status}: {body}"
        );

        for account_id in OTHER_ACCOUNTS {
            let arn = (row.arn_of)(account_id);
            let case = row.case(account_id);
            let (status, body) = call(&router, row.operation, row.request(&arn)).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidParameterValueException",
                "case {case:?}: expected InvalidParameterValueException, got {body}"
            );
            assert_eq!(
                body["message"].as_str().unwrap_or_default(),
                CROSS_ACCOUNT_MESSAGE,
                "case {case:?}: expected the cross-account refusal, got {body}"
            );
        }
    }
}

/// The refusal is about the REQUEST, not about what the store holds: an account that never
/// opted in reads nothing (403), but an ARN of another account is answered with the 400
/// whether or not the caller is enrolled.
#[tokio::test]
async fn the_cross_account_arn_refusal_precedes_the_opt_in_gate() {
    assert_other_accounts_are_other();
    let router = emulator_with_analysed_services();

    for row in &ARN_MEMBERS {
        let own = (row.arn_of)(ACCOUNT);
        let case = row.case(ACCOUNT);
        let (status, body) = call(&router, row.operation, row.request(&own)).await;
        assert_eq!(
            status, 403,
            "case {case:?}: not opted in, expected 403, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "OptInRequiredException",
            "case {case:?}: not opted in, expected OptInRequiredException, got {body}"
        );

        for account_id in OTHER_ACCOUNTS {
            let arn = (row.arn_of)(account_id);
            let case = row.case(account_id);
            let (status, body) = call(&router, row.operation, row.request(&arn)).await;
            assert_eq!(
                status, 400,
                "case {case:?}: not opted in, expected 400, got {status}: {body}"
            );
            assert_eq!(
                body["message"].as_str().unwrap_or_default(),
                CROSS_ACCOUNT_MESSAGE,
                "case {case:?}: not opted in, expected the cross-account refusal, got {body}"
            );
        }
    }
}

/// An ARN that is not in the form its member takes is refused for its FORM, not as a
/// cross-account request: the two refusals stay distinguishable, and the form is judged
/// first whichever account the ARN names.
#[tokio::test]
async fn a_malformed_arn_is_refused_for_its_form_whichever_account_it_names() {
    let router = emulator_with_analysed_services();
    analysed(&router).await;

    // Rows: a member paired with an ARN form it does NOT take (a volume ARN is not an EC2
    // instance or Auto Scaling group ARN; an EC2 instance ARN is neither an Auto Scaling
    // group's nor a volume's). Each is sent for the caller's OWN account and for another
    // account, so the form check is shown to come first.
    let rows: [ArnMember; 3] = [
        ArnMember {
            operation: "GetEffectiveRecommendationPreferences",
            member: "resourceArn",
            arn_of: volume_arn_of,
            list: false,
        },
        ArnMember {
            operation: "GetAutoScalingGroupRecommendations",
            member: "autoScalingGroupArns",
            arn_of: instance_arn_of,
            list: true,
        },
        ArnMember {
            operation: "GetEBSVolumeRecommendations",
            member: "volumeArns",
            arn_of: instance_arn_of,
            list: true,
        },
    ];

    for row in &rows {
        for account_id in [ACCOUNT, OTHER_ACCOUNTS[0]] {
            let arn = (row.arn_of)(account_id);
            let case = row.case(account_id);
            let (status, body) = call(&router, row.operation, row.request(&arn)).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status}: {body}"
            );
            let message = body["message"].as_str().unwrap_or_default();
            assert_ne!(
                message, CROSS_ACCOUNT_MESSAGE,
                "case {case:?}: an ARN of the wrong form is refused for its form, got {body}"
            );
        }
    }
}

/// An account-level preference record written for ANOTHER account governs nothing this
/// service answers about: the effective preferences of a resource of the account this
/// service answers for are resolved from that account's records, never from a record scoped
/// to a different account id.
#[tokio::test]
async fn a_preference_scoped_to_another_account_is_not_effective_here() {
    assert_other_accounts_are_other();
    let router = emulator_with_analysed_services();
    opt_in(&router).await;
    let instance = instance_arn_of(ACCOUNT);

    /// The look-back period reported as effective for `resource_arn`.
    async fn effective_look_back(router: &MultiServiceRouter, resource_arn: &str) -> String {
        let (status, body) = call(
            router,
            "GetEffectiveRecommendationPreferences",
            json!({"resourceArn": resource_arn}),
        )
        .await;
        assert_eq!(
            status, 200,
            "effective preferences for {resource_arn}: expected 200, got {status}: {body}"
        );
        body["lookBackPeriod"].as_str().unwrap_or_default().to_string()
    }

    // Every other account, each writing a DIFFERENT look-back period at `AccountId` scope:
    // none of them may become the effective value for this account's instance.
    for (account_id, look_back_period) in OTHER_ACCOUNTS
        .iter()
        .zip(["DAYS_93", "DAYS_32", "DAYS_93", "DAYS_32", "DAYS_93"])
    {
        let request = json!({
            "resourceType": "Ec2Instance",
            "scope": {"name": "AccountId", "value": account_id},
            "lookBackPeriod": look_back_period,
        });
        let (status, body) = call(&router, "PutRecommendationPreferences", request.clone()).await;
        // `state::scope_rejection` checks an `AccountId` scope value for the documented
        // 12-digit form only, so this record IS stored; what the read below pins is that a
        // stored record naming another account governs none of this account's resources.
        assert_eq!(
            status, 200,
            "case {account_id:?}: writing {request} expected 200, got {status}: {body}"
        );
        let effective = effective_look_back(&router, &instance).await;
        assert_eq!(
            effective, DEFAULT_LOOK_BACK_PERIOD,
            "case {account_id:?}: a preference scoped to another account must leave this \
             account's instance at the documented default, got {effective:?}"
        );
    }

    // The same record written at THIS account's scope is effective — so the rows above are
    // inert because of the account they name, not because the write did nothing.
    let (status, body) = call(
        &router,
        "PutRecommendationPreferences",
        json!({
            "resourceType": "Ec2Instance",
            "scope": {"name": "AccountId", "value": ACCOUNT},
            "lookBackPeriod": "DAYS_93",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "writing this account's own AccountId-scoped preference: expected 200, got {status}: \
         {body}"
    );
    let effective = effective_look_back(&router, &instance).await;
    assert_eq!(
        effective, "DAYS_93",
        "this account's own AccountId-scoped preference is the effective one, got {effective:?}"
    );
}
