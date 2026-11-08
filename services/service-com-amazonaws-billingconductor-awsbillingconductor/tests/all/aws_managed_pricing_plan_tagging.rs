// SPDX-License-Identifier: CC0-1.0

//! The tagging surface over the pricing plans AMAZON WEB SERVICES owns, driven end to end
//! over the service's real `restJson1` wire through the harness in
//! `tests/all/tagging_surface.rs`.
//!
//! RULE under test: this service's tagging plane answers its not-found only for a
//! `ResourceArn` that names NO resource. A resource Amazon Web Services owns rather than
//! this account is present without the caller creating it, so the plane does not report
//! it absent: every tagging operation addressing one is refused with the modelled
//! `AccessDeniedException` (HTTP 403) — the tags of a resource the caller does not own
//! are not the caller's to read or write — and the refusal stores nothing.
//!
//! The pricing plans Amazon Web Services owns are this service's instance of such a
//! resource: the `PricingPlanFullArn` pattern admits them with the account segment `aws`
//! and names their ids beside the caller-minted form, which is why a billing group may
//! compute against one the caller never created
//! (`tests/all/billing_group_core_loop.rs` asserts that door) — the same recognition the
//! tagging plane now makes.
//!
//! The rows differ along: which managed plan is addressed (both ids the pattern
//! enumerates), which of the three tagging operations addresses it, and — for the
//! not-found half — which single ARN field (partition, account, resource kind, resource
//! id, region, the bare-id spelling) an ARN that names no resource differs from a managed
//! plan's in.

use crate::tagging_surface::{
    assert_tagging_not_found, error_code, expected, tag_map, Emulator, TAGGING_STATUS,
};

/// The pricing plans Amazon Web Services owns, as the model enumerates them beside the
/// caller-minted id form
/// (`smithy_query(query="shape",shape="PricingPlanFullArn")`:
/// `^arn:aws(-cn)?:billingconductor::(aws|[0-9]{12}):pricingplan/(BasicPricingPlan|Passthrough|[a-zA-Z0-9]{10})$`).
const AWS_MANAGED_PLAN_IDS: &[&str] = &["BasicPricingPlan", "Passthrough"];

/// The status the model binds to this service's `AccessDeniedException`
/// (`smithy_query(query="operation",operation="ListTagsForResource")`: `httpError` 403).
const ACCESS_DENIED_STATUS: u16 = 403;

/// The wire error code of that modelled shape.
const ACCESS_DENIED_CODE: &str = "AccessDeniedException";

/// The sentence that refusal carries: the text the model documents for this service's
/// `AccessDeniedException`
/// (`smithy_query(query="operation",operation="ListTagsForResource")`: "You do not have
/// sufficient access to perform this action."). Wholly fixed, with nothing from the request
/// in it — the same shape this surface's not-found sentence has.
const ACCESS_DENIED_MESSAGE: &str = "You do not have sufficient access to perform this action.";

/// The ARN of one AWS-managed plan: the account segment the pattern spells `aws`, in the
/// partition this emulator serves, with this global service's empty region segment.
fn managed_plan_arn(id: &str) -> String {
    format!("arn:aws:billingconductor::aws:pricingplan/{id}")
}

/// RULE: a tagging call that addresses a pricing plan Amazon Web Services owns is refused
/// 403 `AccessDeniedException` — never the surface's not-found, which would report a
/// resource of every account as absent.
#[tokio::test]
async fn every_tagging_operation_refuses_an_aws_owned_plan_as_access_denied() {
    let em = Emulator::new();

    for id in AWS_MANAGED_PLAN_IDS {
        let arn = managed_plan_arn(id);
        // Every operation of the surface, so the decision is the plane's and not one
        // handler's: a write, a removal and a read.
        let attempts = [
            ("TagResource", em.tag(&arn, &tag_map(&[("env", "prod")])).await),
            ("UntagResource", em.untag(&arn, &["env"]).await),
            ("ListTagsForResource", em.list_tags(&arn).await),
        ];
        for (operation, (status, body)) in &attempts {
            assert_eq!(
                *status, ACCESS_DENIED_STATUS,
                "case {id:?} / {operation}: expected HTTP {ACCESS_DENIED_STATUS} for {arn:?}, \
                 got {status} with {body}"
            );
            assert_eq!(
                error_code(body),
                ACCESS_DENIED_CODE,
                "case {id:?} / {operation}: expected the error code {ACCESS_DENIED_CODE} for a \
                 resource Amazon Web Services owns, got {body}"
            );
            assert_eq!(
                body.get("Message")
                    .or_else(|| body.get("message"))
                    .and_then(serde_json::Value::as_str),
                Some(ACCESS_DENIED_MESSAGE),
                "case {id:?} / {operation}: expected the documented sentence for this error \
                 class, got {body}"
            );
        }
    }
}

/// RULE: a refusal stores nothing — a managed plan is not created, tagged or made
/// readable by a tagging call addressed at it, and the caller's own resources are
/// untouched.
#[tokio::test]
async fn a_refused_tagging_call_on_an_aws_owned_plan_stores_nothing() {
    let em = Emulator::new();
    let plan = em.create_plan("CallerOwnedPlan", None).await;

    for id in AWS_MANAGED_PLAN_IDS {
        let arn = managed_plan_arn(id);
        let (status, body) = em.tag(&arn, &tag_map(&[("env", "prod")])).await;
        assert_eq!(
            status, ACCESS_DENIED_STATUS,
            "case {id:?}: TagResource expected HTTP {ACCESS_DENIED_STATUS}, got {status} with \
             {body}"
        );
        // The refused write left no record behind: the read is refused the SAME way, not
        // served with the pairs the refused call carried.
        let (status, body) = em.list_tags(&arn).await;
        assert_eq!(
            status, ACCESS_DENIED_STATUS,
            "case {id:?}: a refused write must not make the plan readable, got {status} with \
             {body}"
        );
        assert!(
            body.get("Tags").is_none(),
            "case {id:?}: a refused call must report no tags, got {body}"
        );
    }

    // The caller's own plan is still the surface's own business, and still untagged.
    let stored = em.stored_tags("the caller's own plan", &plan).await;
    assert_eq!(
        stored,
        expected(&[]),
        "expected the plan the caller created to carry no tag, got {stored:?}"
    );
    let (status, body) = em.tag(&plan, &tag_map(&[("env", "prod")])).await;
    assert_eq!(
        status, TAGGING_STATUS,
        "expected TagResource on the caller's own plan to be served with HTTP \
         {TAGGING_STATUS}, got {status} with {body}"
    );
    let stored = em.stored_tags("the caller's own plan, tagged", &plan).await;
    assert_eq!(
        stored,
        expected(&[("env", "prod")]),
        "expected the caller's own plan to carry the pair just written, got {stored:?}"
    );
}

/// RULE: the surface's not-found still answers an ARN that names NO resource — a managed
/// plan is owned by Amazon Web Services only as the ARN Amazon Web Services owns, so the
/// same id under another partition, account, region or resource kind, and the bare-id
/// spelling (which the modelled pattern gives to a custom line item), each name nothing
/// and are refused 404 `NotFoundException`.
#[tokio::test]
async fn an_arn_that_names_no_resource_is_still_the_surfaces_not_found() {
    let em = Emulator::new();
    let account = engine::emulator_account_id();
    let unknown: Vec<(&str, String)> = vec![
        (
            "a managed plan id in the China partition",
            "arn:aws-cn:billingconductor::aws:pricingplan/Passthrough".to_string(),
        ),
        (
            "a managed plan id under a 12-digit account",
            "arn:aws:billingconductor::123456789012:pricingplan/BasicPricingPlan".to_string(),
        ),
        (
            "a managed plan id under this caller's own account",
            format!("arn:aws:billingconductor::{account}:pricingplan/Passthrough"),
        ),
        (
            "an id no managed plan has, in the aws account",
            "arn:aws:billingconductor::aws:pricingplan/NOTMANAGED".to_string(),
        ),
        (
            "a managed plan id under another resource KIND",
            "arn:aws:billingconductor::aws:pricingrule/Passthrough".to_string(),
        ),
        (
            "a managed plan ARN carrying a region segment",
            "arn:aws:billingconductor:us-east-1:aws:pricingplan/Passthrough".to_string(),
        ),
        (
            "the bare managed id, which the pattern gives to a custom line item",
            "Passthrough".to_string(),
        ),
    ];

    for (case, arn) in &unknown {
        assert_tagging_not_found(&em, case, arn).await;
    }

    // And the plans Amazon Web Services DOES own are still answered for their owner
    // rather than reported absent, so the two classifications stay apart.
    for id in AWS_MANAGED_PLAN_IDS {
        let (status, body) = em.list_tags(&managed_plan_arn(id)).await;
        assert_eq!(
            status, ACCESS_DENIED_STATUS,
            "case {id:?}: expected HTTP {ACCESS_DENIED_STATUS} for the plan Amazon Web Services \
             owns, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            ACCESS_DENIED_CODE,
            "case {id:?}: expected {ACCESS_DENIED_CODE}, got {body}"
        );
    }
}
