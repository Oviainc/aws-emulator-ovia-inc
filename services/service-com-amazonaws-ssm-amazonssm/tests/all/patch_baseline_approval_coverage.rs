// SPDX-License-Identifier: CC0-1.0

//! What a patch baseline must approve, end to end: `CreatePatchBaseline` admits a baseline
//! that approves patches by naming them or by a rule that selects them, and refuses one
//! that would approve nothing.
//!
//! Every case goes through the front door a client uses — the shared harness in
//! `all/parameter_store.rs` ([`router`], [`call_in_region`], [`error_code`]) dispatches
//! `POST /` with `X-Amz-Target: AmazonSSM.<Operation>` through the multi-service router.
//! Nothing reaches into the state struct: an admitted baseline is read back through
//! `RegisterDefaultPatchBaseline` + `GetDefaultPatchBaseline`, and a refused one is shown
//! to have left no record behind by what the store does with its idempotency token.
//!
//! The rule under test is a cross-member one the model cannot state, since it makes both
//! members optional on their own (`smithy_query(query="operation",
//! operation="CreatePatchBaseline")`): `ApprovedPatches` and `ApprovalRules` cannot BOTH
//! be empty. The suites are tables whose rows differ along what the rule generalises over
//! — which of the two members is supplied, how many entries it carries, whether a supplied
//! member is empty, which other members the create fills in, the operating system and the
//! Region — rather than the single create a recorded unit happened to make.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call_in_region, error_code, router, REGION};

/// The sentence Patch Manager words for a baseline that would approve no patch: fixed
/// text naming the two attributes and interpolating nothing from the request.
const APPROVAL_COVERAGE_REQUIRED: &str =
    "Patch Baseline attribute: Approved Patches and Approval Rule, at least one must be non empty.";

/// The published ceiling of `ApprovedPatches` (`smithy_query(query="shape",
/// shape="PatchIdList")`: `length 0..50`).
const MAX_APPROVED_PATCHES: usize = 50;

/// The published ceiling of a rule group's own list (`smithy_query(query="shape",
/// shape="PatchRuleGroup")`: `PatchRules`, `length 0..10`).
const MAX_PATCH_RULES: usize = 10;

/// `CreatePatchBaseline` at `region`'s endpoint with `body`, answering `(status, body)`.
async fn create(router: &MultiServiceRouter, region: &str, body: &Value) -> (u16, Value) {
    call_in_region(router, region, "CreatePatchBaseline", body).await
}

/// One approval rule that selects patches of `classification` and approves them after a
/// day — the shape `PatchRule` requires: a filter group, with `ApproveAfterDays` for the
/// member a rule "must include a value for" ("your request must include a value for either
/// `ApproveAfterDays` or `ApproveUntilDate`").
fn patch_rule(classification: &str) -> Value {
    json!({
        "PatchFilterGroup": {
            "PatchFilters": [
                { "Key": "CLASSIFICATION", "Values": [classification] },
            ],
        },
        "ApproveAfterDays": 1,
        "ComplianceLevel": "CRITICAL",
    })
}

/// A rule group of `count` rules, each selecting its own classification so no two rules of
/// a group are one rule twice.
fn rule_group(count: usize) -> Value {
    // The classifications a Windows patch filter accepts, from `PatchFilter`'s own doc
    // (`smithy_query(query="doc", shape="PatchFilter")`).
    const CLASSIFICATIONS: &[&str] = &[
        "CriticalUpdates",
        "SecurityUpdates",
        "DefinitionUpdates",
        "Drivers",
        "FeaturePacks",
        "ServicePacks",
        "Tools",
        "UpdateRollups",
        "Updates",
        "Upgrades",
    ];
    json!({
        "PatchRules": (0..count)
            .map(|index| patch_rule(CLASSIFICATIONS[index % CLASSIFICATIONS.len()]))
            .collect::<Vec<Value>>(),
    })
}

/// `name` approved patches, in the `KB…` form a Windows patch id takes.
fn approved_patches(count: usize) -> Value {
    json!((0..count)
        .map(|index| format!("KB{:07}", 2_124_261 + index))
        .collect::<Vec<String>>())
}

/// One row of the refusal table: a create that would approve nothing.
struct RefusedCase<'a> {
    case: &'a str,
    /// The members the create supplies beside `Name` and `OperatingSystem`.
    extra: Value,
}

/// A create that supplies neither a non-empty approved-patch list nor a non-empty approval
/// rule group is refused, with the status, the error class and the sentence Patch Manager
/// words — and nothing is written: the token it carried is still free for a create that
/// approves something.
#[tokio::test]
async fn a_baseline_that_would_approve_nothing_is_refused() {
    let cases = [
        RefusedCase {
            case: "neither member supplied",
            extra: json!({}),
        },
        RefusedCase {
            case: "an empty approved-patch list",
            extra: json!({ "ApprovedPatches": [] }),
        },
        RefusedCase {
            case: "a rule group holding no rule",
            extra: json!({ "ApprovalRules": { "PatchRules": [] } }),
        },
        RefusedCase {
            case: "both members supplied and both empty",
            extra: json!({
                "ApprovedPatches": [],
                "ApprovalRules": { "PatchRules": [] },
            }),
        },
        RefusedCase {
            case: "patches rejected but none approved",
            extra: json!({
                "ApprovedPatches": [],
                "RejectedPatches": ["KB2124261"],
                "RejectedPatchesAction": "BLOCK",
            }),
        },
        RefusedCase {
            case: "every other member filled in",
            extra: json!({
                "Description": "a baseline that approves nothing",
                "ApprovedPatchesComplianceLevel": "CRITICAL",
                "ApprovedPatchesEnableNonSecurity": true,
                "GlobalFilters": {
                    "PatchFilters": [{ "Key": "PRODUCT", "Values": ["WindowsServer2022"] }],
                },
                "Tags": [{ "Key": "Owner", "Value": "SysAdmin" }],
            }),
        },
    ];

    for RefusedCase { case, extra } in &cases {
        let router = router();
        let token = "9c1f6d2b-0000-4000-8000-00000000000a";
        let mut body = json!({
            "Name": "Approves.Nothing.Baseline",
            "OperatingSystem": "WINDOWS",
            "ClientToken": token,
        });
        for (member, value) in extra.as_object().expect("a row's extra members are an object") {
            body[member] = value.clone();
        }

        let (status, response) = create(&router, REGION, &body).await;
        assert_eq!(
            status, 400,
            "case {case}: {body} must be refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case}: expected ValidationException, got {response}"
        );
        assert_eq!(
            response["message"].as_str(),
            Some(APPROVAL_COVERAGE_REQUIRED),
            "case {case}: expected the sentence Patch Manager words, got {response}"
        );
        assert!(
            response["BaselineId"].is_null(),
            "case {case}: a refused create mints no BaselineId, got {response}"
        );

        // Nothing was written: the SAME token now carries a create that approves
        // something, which a stored record of the refused one would have turned into an
        // idempotency mismatch instead of a baseline.
        let admitted = json!({
            "Name": "Approves.Nothing.Baseline",
            "OperatingSystem": "WINDOWS",
            "ClientToken": token,
            "ApprovedPatches": approved_patches(1),
        });
        let (status, response) = create(&router, REGION, &admitted).await;
        assert_eq!(
            status, 200,
            "case {case}: the refused create left the token free, got {status} {response}"
        );
        assert!(
            response["BaselineId"].as_str().is_some(),
            "case {case}: the create that approves a patch mints a BaselineId, got {response}"
        );
    }
}

/// One row of the admission table: a create that approves something.
struct AdmittedCase<'a> {
    case: &'a str,
    name: &'a str,
    operating_system: &'a str,
    region: &'a str,
    /// The members that make the baseline approve something.
    approves: Value,
}

/// A create is admitted as soon as one of the two members approves something, whichever it
/// is and however many entries it carries — and the baseline it mints is the one a read
/// reports.
#[tokio::test]
async fn a_baseline_that_approves_something_is_admitted() {
    let cases = [
        AdmittedCase {
            case: "one approved patch",
            name: "One.Patch.Baseline",
            operating_system: "WINDOWS",
            region: REGION,
            approves: json!({ "ApprovedPatches": approved_patches(1) }),
        },
        AdmittedCase {
            case: "as many approved patches as the member holds",
            name: "Full.Patch.List.Baseline",
            operating_system: "AMAZON_LINUX_2023",
            region: REGION,
            approves: json!({ "ApprovedPatches": approved_patches(MAX_APPROVED_PATCHES) }),
        },
        AdmittedCase {
            case: "one approval rule and no approved patch",
            name: "One.Rule.Baseline",
            operating_system: "UBUNTU",
            region: REGION,
            approves: json!({ "ApprovalRules": rule_group(1) }),
        },
        AdmittedCase {
            case: "an empty approved-patch list beside one approval rule",
            name: "Empty.List.One.Rule.Baseline",
            operating_system: "SUSE",
            region: REGION,
            approves: json!({
                "ApprovedPatches": [],
                "ApprovalRules": rule_group(1),
            }),
        },
        AdmittedCase {
            case: "as many approval rules as a group holds",
            name: "Full.Rule.Group.Baseline",
            operating_system: "WINDOWS",
            region: "eu-central-1",
            approves: json!({ "ApprovalRules": rule_group(MAX_PATCH_RULES) }),
        },
        AdmittedCase {
            case: "both members non-empty",
            name: "Both.Members.Baseline",
            operating_system: "REDHAT_ENTERPRISE_LINUX",
            region: "ap-southeast-1",
            approves: json!({
                "ApprovedPatches": approved_patches(3),
                "ApprovalRules": rule_group(2),
            }),
        },
    ];

    for AdmittedCase {
        case,
        name,
        operating_system,
        region,
        approves,
    } in &cases
    {
        let router = router();
        let mut body = json!({
            "Name": name,
            "OperatingSystem": operating_system,
            "Description": format!("the baseline of case {case}"),
        });
        for (member, value) in approves
            .as_object()
            .expect("a row's approving members are an object")
        {
            body[member] = value.clone();
        }

        let (status, response) = create(&router, region, &body).await;
        assert_eq!(
            status, 200,
            "case {case}: {body} must be admitted, got {status} {response}"
        );
        let baseline_id = response["BaselineId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: a create reports a BaselineId, got {response}"))
            .to_string();

        // The baseline the create minted is one the store holds: a register of it records
        // it as the default of ITS operating system, and the read of that default reports
        // the id the create handed back.
        let (status, registered) = call_in_region(
            &router,
            region,
            "RegisterDefaultPatchBaseline",
            &json!({ "BaselineId": baseline_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: the minted baseline must register, got {status} {registered}"
        );
        let (status, reported) = call_in_region(
            &router,
            region,
            "GetDefaultPatchBaseline",
            &json!({ "OperatingSystem": operating_system }),
        )
        .await;
        assert_eq!(
            (status, reported["BaselineId"].as_str()),
            (200, Some(baseline_id.as_str())),
            "case {case}: the default of {operating_system} is the baseline the create \
             minted {baseline_id:?}, got {status} {reported}"
        );
        assert_eq!(
            reported["OperatingSystem"].as_str(),
            Some(*operating_system),
            "case {case}: the read reports the operating system it was asked for, got \
             {reported}"
        );
    }
}
