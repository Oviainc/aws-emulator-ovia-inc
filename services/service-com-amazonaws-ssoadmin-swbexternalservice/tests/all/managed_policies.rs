// SPDX-License-Identifier: CC0-1.0

//! The ATTACH and DETACH of AWS managed policies on a permission set, end to end over the
//! `awsJson1_1` wire. (`ListManagedPoliciesInPermissionSet`'s own enumeration and
//! pagination rules are `managed_policies_in_permission_set.rs`; it is used here as the
//! READER that the attach and the detach are observed through.)
//!
//! THE RULE under test: a permission set carries a SET of AWS managed policy ARNs, each
//! identified by its `ManagedPolicyArn`; `AttachManagedPolicyToPermissionSet` adds one
//! entry to that set (so attaching the same ARN twice attaches it once),
//! `DetachManagedPolicyFromPermissionSet` removes exactly the entry whose ARN the request
//! names and answers `ResourceNotFoundException` for an ARN the permission set does not
//! carry, and `ListManagedPoliciesInPermissionSet` reports exactly the entries that are
//! left — once each, in ARN order, as the ARN together with the policy NAME that ARN names
//! (its last `/`-separated segment). An `InstanceArn` or `PermissionSetArn` nothing is held
//! under is a `ResourceNotFoundException`, and the whole surface is withdrawn on an account
//! instance.
//!
//! Each rule is asserted over a TABLE whose rows differ along what the rule generalises
//! over: the ARN partition, the presence, depth and character class of the path between
//! `policy` and the policy name, the published length bounds of `ManagedPolicyArn`
//! (20..2048) and of `AttachedManagedPolicy$Name` (1..100) and one value past each, the
//! NUMBER of policies attached, and the order the attaches and detaches are made in.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes, so the modelled constraints and the `awsJson1_1` envelope are exercised
//! rather than bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns, and the
//! calls run against an ORGANIZATION instance because the permission-set surface is
//! withdrawn on an account instance (asserted here too).
//!
//! Every modelled 4xx refusal of this service is answered with HTTP 400 whatever
//! `@httpError` its shape declares (`crate::scaffolded::modeled_error_status` records why,
//! from the service's own recorded responses), so the assertions below pin the status AND
//! the wire error code, which is what tells one refusal from another.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    call, error_code, organization_router, router, seeded_instance_arn, UNKNOWN_INSTANCE_ARN,
};

/// A pattern-valid `PermissionSetArn` that was never minted, for the not-found rows.
/// Shaped by the model's `PermissionSetArn` pattern
/// `^arn:aws(-[a-z]{1,5}){0,3}:sso:::permissionSet/(sso)?ins-[a-zA-Z0-9-.]{16}/ps-[a-zA-Z0-9-./]{16}$`.
const UNKNOWN_PERMISSION_SET_ARN: &str =
    "arn:aws:sso:::permissionSet/ssoins-0000000000000000/ps-0000000000000000";

/// `ManagedPolicyArn`'s published upper length bound (`length(min: 20, max: 2048)`).
const ARN_MAX: usize = 2048;

/// `AttachedManagedPolicy$Name`'s published upper length bound
/// (`length(min: 1, max: 100)`).
const NAME_MAX: usize = 100;

/// The prefix every `ManagedPolicyArn` of the `aws` partition starts with, per the member's
/// pattern
/// `^arn:aws(-[a-z]{1,5}){0,3}:iam::aws:policy((/[A-Za-z0-9\.,\+@=_-]+)*)/([A-Za-z0-9\.,\+=@_-]+)$`.
const POLICY_PREFIX: &str = "arn:aws:iam::aws:policy/";

/// One row of the managed-policy table: the ARN a request sends, and the (`Name`, `Arn`)
/// pair a later list must report for it.
struct PolicyCase {
    label: &'static str,
    /// What the attach and the detach send as `ManagedPolicyArn`.
    arn: String,
    /// The `Name` the list must report: the ARN's last `/`-separated segment, cut to the
    /// member's published upper bound.
    name: String,
}

/// The table. Rows span the partitions `arn:aws(-[a-z]{1,5}){0,3}` admits, the presence,
/// depth and character class of the path between `policy` and the policy name, both
/// published length bounds of `ManagedPolicyArn`, the upper bound of
/// `AttachedManagedPolicy$Name`, and the same policy name under two different paths — which
/// the identity rule makes two policies.
fn policy_cases() -> Vec<PolicyCase> {
    vec![
        PolicyCase {
            label: "an AWS managed policy with no path",
            arn: format!("{POLICY_PREFIX}ReadOnlyAccess"),
            name: "ReadOnlyAccess".to_string(),
        },
        PolicyCase {
            // The shortest ARN the pattern admits, which is well inside the member's
            // lower length bound of 20 (the prefix alone is longer than that).
            label: "a one-character policy name",
            arn: format!("{POLICY_PREFIX}A"),
            name: "A".to_string(),
        },
        PolicyCase {
            label: "the job-function path AWS publishes",
            arn: format!("{POLICY_PREFIX}job-function/ViewOnlyAccess"),
            name: "ViewOnlyAccess".to_string(),
        },
        PolicyCase {
            label: "a deeply nested path",
            arn: format!("{POLICY_PREFIX}a/b/c/DeepPolicy"),
            name: "DeepPolicy".to_string(),
        },
        PolicyCase {
            label: "the aws-cn partition",
            arn: "arn:aws-cn:iam::aws:policy/ReadOnlyAccess".to_string(),
            name: "ReadOnlyAccess".to_string(),
        },
        PolicyCase {
            label: "the aws-us-gov partition",
            arn: "arn:aws-us-gov:iam::aws:policy/ReadOnlyAccess".to_string(),
            name: "ReadOnlyAccess".to_string(),
        },
        PolicyCase {
            label: "the full ^[A-Za-z0-9\\.,\\+=@_-]+$ class in the policy name",
            arn: format!("{POLICY_PREFIX}a.b,c+d=e@f_g-H9"),
            name: "a.b,c+d=e@f_g-H9".to_string(),
        },
        PolicyCase {
            label: "the full ^[A-Za-z0-9\\.,\\+@=_-]+$ class in the path",
            arn: format!("{POLICY_PREFIX}a.b,c+d@e=f_g-H9/CharsetPolicy"),
            name: "CharsetPolicy".to_string(),
        },
        PolicyCase {
            // `AttachedManagedPolicy$Name` at its upper length bound.
            label: "a policy name of 100 characters",
            arn: format!("{POLICY_PREFIX}{}", "n".repeat(NAME_MAX)),
            name: "n".repeat(NAME_MAX),
        },
        PolicyCase {
            // `ManagedPolicyArn` at its upper length bound, reached with a long PATH so the
            // name segment stays inside `AttachedManagedPolicy$Name`'s own 1..100 bound.
            label: "an ARN of 2048 characters",
            arn: max_length_arn(),
            name: BOUND_POLICY_NAME.to_string(),
        },
    ]
}

/// The policy name the longest ARN below ends in.
const BOUND_POLICY_NAME: &str = "MaxLengthAccess";

/// An ARN of exactly [`ARN_MAX`] characters: the bound is reached with a long path, so its
/// name segment stays inside `AttachedManagedPolicy$Name`'s `length(min: 1, max: 100)`.
fn max_length_arn() -> String {
    let path = "p".repeat(ARN_MAX - POLICY_PREFIX.len() - BOUND_POLICY_NAME.len() - 1);
    format!("{POLICY_PREFIX}{path}/{BOUND_POLICY_NAME}")
}

/// Create a permission set named `name` on `instance_arn` and return its minted ARN.
async fn create_permission_set(
    router: &MultiServiceRouter,
    instance_arn: &str,
    name: &str,
) -> String {
    let (status, body) = call(
        router,
        "CreatePermissionSet",
        &json!({"InstanceArn": instance_arn, "Name": name}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreatePermissionSet expected 200, got {status} {body}"
    );
    body["PermissionSet"]["PermissionSetArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: no minted PermissionSetArn in {body}"))
        .to_string()
}

/// `AttachManagedPolicyToPermissionSet`, as `(status, body)`.
async fn attach(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    managed_policy_arn: &str,
) -> (u16, Value) {
    call(
        router,
        "AttachManagedPolicyToPermissionSet",
        &json!({
            "InstanceArn": instance_arn,
            "PermissionSetArn": permission_set_arn,
            "ManagedPolicyArn": managed_policy_arn,
        }),
    )
    .await
}

/// `DetachManagedPolicyFromPermissionSet`, as `(status, body)`.
async fn detach(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    managed_policy_arn: &str,
) -> (u16, Value) {
    call(
        router,
        "DetachManagedPolicyFromPermissionSet",
        &json!({
            "InstanceArn": instance_arn,
            "PermissionSetArn": permission_set_arn,
            "ManagedPolicyArn": managed_policy_arn,
        }),
    )
    .await
}

/// `ListManagedPoliciesInPermissionSet`, as `(status, body)`.
async fn list(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> (u16, Value) {
    let mut request = json!({
        "InstanceArn": instance_arn,
        "PermissionSetArn": permission_set_arn,
    });
    if let Some(max_results) = max_results {
        request["MaxResults"] = json!(max_results);
    }
    if let Some(next_token) = next_token {
        request["NextToken"] = json!(next_token);
    }
    call(router, "ListManagedPoliciesInPermissionSet", &request).await
}

/// The (`Name`, `Arn`) pairs one list page reports, in the order the page carries them.
fn pairs(body: &Value) -> Vec<(String, String)> {
    body["AttachedManagedPolicies"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|policy| {
            (
                policy["Name"].as_str().unwrap_or_default().to_string(),
                policy["Arn"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// Every policy attached to `permission_set_arn`, read over the wire in one page, in the
/// order the page reports them.
async fn attached_page(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
) -> Vec<(String, String)> {
    let (status, body) = list(router, instance_arn, permission_set_arn, None, None).await;
    assert_eq!(
        status, 200,
        "ListManagedPoliciesInPermissionSet: expected 200, got {status} {body}"
    );
    assert!(
        body["NextToken"].is_null(),
        "a page that holds the whole set carries no NextToken, got {body}"
    );
    pairs(&body)
}

/// The same, sorted, so a comparison is about the SET rather than about a page order.
async fn attached_set(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
) -> Vec<(String, String)> {
    let mut found = attached_page(router, instance_arn, permission_set_arn).await;
    found.sort();
    found
}

#[tokio::test]
async fn an_attach_stores_the_policy_and_the_list_answers_from_it() {
    let (router, instance_arn) = organization_router();
    for (index, case) in policy_cases().into_iter().enumerate() {
        // One permission set per row, so each row's list reports its row alone.
        let permission_set_arn =
            create_permission_set(&router, &instance_arn, &format!("Attach{index}")).await;

        let empty = attached_set(&router, &instance_arn, &permission_set_arn).await;
        assert!(
            empty.is_empty(),
            "case {:?}: a fresh permission set carries no managed policy, got {empty:?}",
            case.label
        );

        let (status, body) = attach(&router, &instance_arn, &permission_set_arn, &case.arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: attach expected 200, got {status} {body}",
            case.label
        );

        let found = attached_set(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            found,
            vec![(case.name.clone(), case.arn.clone())],
            "case {:?}: the list must report the stored ARN and the name it names, got \
             {found:?}",
            case.label
        );
    }
}

#[tokio::test]
async fn attaching_the_same_policy_twice_attaches_it_once() {
    let (router, instance_arn) = organization_router();
    for (index, case) in policy_cases().into_iter().enumerate() {
        let permission_set_arn =
            create_permission_set(&router, &instance_arn, &format!("Twice{index}")).await;
        for attempt in 1..=3 {
            let (status, body) =
                attach(&router, &instance_arn, &permission_set_arn, &case.arn).await;
            assert_eq!(
                status, 200,
                "case {:?}: attach #{attempt} expected 200, got {status} {body}",
                case.label
            );
        }
        let found = attached_set(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            found,
            vec![(case.name.clone(), case.arn.clone())],
            "case {:?}: three attaches of one ARN attach it once, got {found:?}",
            case.label
        );
    }
}

#[tokio::test]
async fn a_detach_removes_exactly_the_policy_it_names() {
    let (router, instance_arn) = organization_router();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "DetachOne").await;

    let cases = policy_cases();
    for case in &cases {
        let (status, body) = attach(&router, &instance_arn, &permission_set_arn, &case.arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: attach expected 200, got {status} {body}",
            case.label
        );
    }

    let mut remaining: Vec<(String, String)> = cases
        .iter()
        .map(|case| (case.name.clone(), case.arn.clone()))
        .collect();
    remaining.sort();
    let found = attached_set(&router, &instance_arn, &permission_set_arn).await;
    assert_eq!(
        found, remaining,
        "the list must report every attached policy, got {found:?}"
    );

    // Detach the rows in a different order from the one they were attached in, so the
    // assertion is about identity rather than about position.
    for case in cases.iter().rev() {
        let (status, body) = detach(&router, &instance_arn, &permission_set_arn, &case.arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: detach expected 200, got {status} {body}",
            case.label
        );
        remaining.retain(|pair| pair.1 != case.arn);
        let found = attached_set(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            found, remaining,
            "case {:?}: a detach removes exactly that ARN, got {found:?}",
            case.label
        );
    }
    assert!(
        remaining.is_empty(),
        "every row was detached, got {remaining:?}"
    );
}

#[tokio::test]
async fn a_detach_of_a_policy_the_permission_set_does_not_hold_is_not_found() {
    let (router, instance_arn) = organization_router();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "NotAttached").await;
    let other_permission_set_arn =
        create_permission_set(&router, &instance_arn, "NotAttachedOther").await;

    // The one policy this permission set does hold, so every row below is about an ARN it
    // does NOT hold while the set is non-empty.
    let held = format!("{POLICY_PREFIX}held/HeldPolicy");
    let (status, body) = attach(&router, &instance_arn, &permission_set_arn, &held).await;
    assert_eq!(status, 200, "attach of the held ARN: got {status} {body}");

    // The policy attached to the OTHER permission set: an attach is scoped to the
    // permission set it was made on, so detaching it here is a missing attachment.
    let elsewhere = format!("{POLICY_PREFIX}ElsewherePolicy");
    let (status, body) = attach(
        &router,
        &instance_arn,
        &other_permission_set_arn,
        &elsewhere,
    )
    .await;
    assert_eq!(status, 200, "attach on the other set: got {status} {body}");

    let missing = [
        (
            "an ARN never attached",
            format!("{POLICY_PREFIX}held/NeverAttached"),
        ),
        (
            "the held policy name under another path",
            format!("{POLICY_PREFIX}other/HeldPolicy"),
        ),
        (
            "the held policy name under no path at all",
            format!("{POLICY_PREFIX}HeldPolicy"),
        ),
        (
            "the held ARN in another partition",
            "arn:aws-cn:iam::aws:policy/held/HeldPolicy".to_string(),
        ),
        ("an ARN attached to another permission set", elsewhere),
    ];
    let before = attached_set(&router, &instance_arn, &permission_set_arn).await;
    for (label, managed_policy_arn) in missing {
        let (status, body) =
            detach(&router, &instance_arn, &permission_set_arn, &managed_policy_arn).await;
        // HTTP 400: this service answers every modelled 4xx refusal with 400 whatever
        // `@httpError` the shape declares, so the ERROR CODE is what tells the refusals
        // apart.
        assert_eq!(
            status, 400,
            "case {label:?}: detach of an unattached ARN expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {body}"
        );
        let after = attached_set(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            after, before,
            "case {label:?}: a refused detach removes nothing, got {after:?}"
        );
    }

    // And the second detach of an ARN that WAS attached, after it is gone.
    let (status, body) = detach(&router, &instance_arn, &permission_set_arn, &held).await;
    assert_eq!(status, 200, "first detach of the held ARN: {status} {body}");
    let (status, body) = detach(&router, &instance_arn, &permission_set_arn, &held).await;
    assert_eq!(
        status, 400,
        "a repeated detach of a removed ARN expected 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "a repeated detach expected ResourceNotFoundException, got {body}"
    );
}

#[tokio::test]
async fn the_list_pages_the_whole_attached_set_for_any_page_size() {
    let (router, instance_arn) = organization_router();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "Paged").await;

    let cases = policy_cases();
    for case in &cases {
        let (status, body) = attach(&router, &instance_arn, &permission_set_arn, &case.arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: attach expected 200, got {status} {body}",
            case.label
        );
    }
    // The whole set, in the order a list reports it: ascending by ARN.
    let mut expected: Vec<(String, String)> = cases
        .iter()
        .map(|case| (case.name.clone(), case.arn.clone()))
        .collect();
    expected.sort_by(|left, right| left.1.cmp(&right.1));

    // Page sizes spanning the model's `MaxResults` range (1..100): below, at and above the
    // number of attached policies, plus the published upper bound and no `MaxResults` at
    // all.
    let page_sizes = [None, Some(1), Some(2), Some(3), Some(9), Some(10), Some(100)];
    for max_results in page_sizes {
        let mut walked: Vec<(String, String)> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let (status, body) = list(
                &router,
                &instance_arn,
                &permission_set_arn,
                max_results,
                next_token.as_deref(),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {max_results:?}: list expected 200, got {status} {body}"
            );
            let page = pairs(&body);
            if let Some(limit) = max_results {
                assert!(
                    page.len() as i64 <= limit,
                    "case {max_results:?}: a page must hold at most MaxResults entries, got \
                     {}",
                    page.len()
                );
            }
            walked.extend(page);
            next_token = body["NextToken"].as_str().map(str::to_string);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {max_results:?}: the enumeration must terminate, got {pages} pages"
            );
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, expected,
            "case {max_results:?}: the pages together must report the whole set once each, \
             in ARN order, got {walked:?}"
        );
    }
}

#[tokio::test]
async fn an_unknown_instance_or_permission_set_is_not_found() {
    let (router, instance_arn) = organization_router();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "Known").await;
    let managed_policy_arn = format!("{POLICY_PREFIX}ReadOnlyAccess");

    let rows = [
        (
            "an instance ARN nothing is held under",
            UNKNOWN_INSTANCE_ARN.to_string(),
            permission_set_arn.clone(),
        ),
        (
            "a permission-set ARN nothing is held under",
            instance_arn.clone(),
            UNKNOWN_PERMISSION_SET_ARN.to_string(),
        ),
    ];
    for (label, instance, permission_set) in rows {
        for operation in [
            "AttachManagedPolicyToPermissionSet",
            "DetachManagedPolicyFromPermissionSet",
            "ListManagedPoliciesInPermissionSet",
        ] {
            let (status, body) = call(
                &router,
                operation,
                &json!({
                    "InstanceArn": instance,
                    "PermissionSetArn": permission_set,
                    "ManagedPolicyArn": managed_policy_arn,
                }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {label:?}: {operation} expected 400, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {label:?}: {operation} expected ResourceNotFoundException, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn the_managed_policy_surface_is_withdrawn_on_an_account_instance() {
    // The account's own instance is an ACCOUNT instance, and permission sets are a feature
    // of an organization instance.
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let managed_policy_arn = format!("{POLICY_PREFIX}ReadOnlyAccess");

    for operation in [
        "AttachManagedPolicyToPermissionSet",
        "DetachManagedPolicyFromPermissionSet",
        "ListManagedPoliciesInPermissionSet",
    ] {
        let (status, body) = call(
            &router,
            operation,
            &json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": UNKNOWN_PERMISSION_SET_ARN,
                "ManagedPolicyArn": managed_policy_arn,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {operation}: expected 400 on an account instance, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {operation}: expected ValidationException, got {body}"
        );
        let message = body["message"]
            .as_str()
            .or_else(|| body["Message"].as_str())
            .unwrap_or_default();
        assert!(
            message.contains("account instances"),
            "case {operation}: the refusal must say the operation is unsupported for \
             account instances, got {body}"
        );
    }
}

#[tokio::test]
async fn a_managed_policy_arn_outside_the_modeled_constraints_is_refused() {
    let (router, instance_arn) = organization_router();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "Constraints").await;

    let refused: Vec<(&str, String)> = vec![
        // One past the upper length bound of `ManagedPolicyArn`.
        (
            "an ARN of 2049 characters",
            format!(
                "{POLICY_PREFIX}{}",
                "p".repeat(ARN_MAX + 1 - POLICY_PREFIX.len())
            ),
        ),
        // One past its lower bound.
        ("an empty ARN", String::new()),
        // Inside the length bounds, outside the pattern: a CUSTOMER managed policy ARN,
        // which this member does not admit (`iam::aws:policy` is AWS-managed only).
        (
            "a customer managed policy ARN",
            "arn:aws:iam::123456789012:policy/CustomerPolicy".to_string(),
        ),
        // The prefix with no policy name after it.
        ("the prefix with no policy name", POLICY_PREFIX.to_string()),
        // A space, which neither the path nor the name class admits.
        (
            "a policy name with a space",
            format!("{POLICY_PREFIX}not a policy"),
        ),
    ];
    for (label, managed_policy_arn) in refused {
        for operation in [
            "AttachManagedPolicyToPermissionSet",
            "DetachManagedPolicyFromPermissionSet",
        ] {
            let (status, body) = call(
                &router,
                operation,
                &json!({
                    "InstanceArn": instance_arn,
                    "PermissionSetArn": permission_set_arn,
                    "ManagedPolicyArn": managed_policy_arn,
                }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {label:?}: {operation} expected a 400 refusal, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {label:?}: {operation} expected ValidationException, got {body}"
            );
        }
    }

    // And the in-bounds neighbour of the upper bound is accepted and read back.
    let at_bound = max_length_arn();
    let (status, body) = attach(&router, &instance_arn, &permission_set_arn, &at_bound).await;
    assert_eq!(
        status, 200,
        "an ARN of {ARN_MAX} characters expected 200, got {status} {body}"
    );
    let found = attached_set(&router, &instance_arn, &permission_set_arn).await;
    assert_eq!(
        found,
        vec![(BOUND_POLICY_NAME.to_string(), at_bound)],
        "the list must report the ARN at the bound and the name it ends in, got {found:?}"
    );
}

#[tokio::test]
async fn a_caller_cannot_attach_past_the_per_permission_set_ceiling() {
    use service_com_amazonaws_ssoadmin_swbexternalservice::state::MAX_MANAGED_POLICIES_PER_PERMISSION_SET;

    let (router, instance_arn) = organization_router();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "Ceiling").await;

    for index in 0..MAX_MANAGED_POLICIES_PER_PERMISSION_SET {
        let (status, body) = attach(
            &router,
            &instance_arn,
            &permission_set_arn,
            &format!("{POLICY_PREFIX}Policy{index}"),
        )
        .await;
        assert_eq!(
            status, 200,
            "attach #{index} expected 200, got {status} {body}"
        );
    }
    // One more DISTINCT ARN crosses the ceiling.
    let (status, body) = attach(
        &router,
        &instance_arn,
        &permission_set_arn,
        &format!("{POLICY_PREFIX}OneTooMany"),
    )
    .await;
    assert_eq!(
        status, 400,
        "an attach past the ceiling expected 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ServiceQuotaExceededException",
        "an attach past the ceiling expected ServiceQuotaExceededException, got {body}"
    );
    // Re-attaching one the set already holds adds nothing, so it is still accepted.
    let (status, body) = attach(
        &router,
        &instance_arn,
        &permission_set_arn,
        &format!("{POLICY_PREFIX}Policy0"),
    )
    .await;
    assert_eq!(
        status, 200,
        "a re-attach at the ceiling expected 200, got {status} {body}"
    );
    let found = attached_set(&router, &instance_arn, &permission_set_arn).await;
    assert_eq!(
        found.len(),
        MAX_MANAGED_POLICIES_PER_PERMISSION_SET,
        "the set must hold exactly the ceiling, got {found:?}"
    );
}
