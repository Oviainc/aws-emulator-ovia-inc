// SPDX-License-Identifier: CC0-1.0

//! The CUSTOMER MANAGED POLICY REFERENCES of a permission set, end to end over the
//! `awsJson1_1` wire.
//!
//! THE RULE under test: a permission set carries a SET of customer managed policy
//! references, each identified by its (`Name`, `Path`) pair with `Path` materialised to the
//! value the member documents as its default (`/`) when a request names none;
//! `AttachCustomerManagedPolicyReferenceToPermissionSet` adds one entry to that set (so
//! attaching the same pair twice attaches it once),
//! `DetachCustomerManagedPolicyReferenceFromPermissionSet` removes exactly the entry whose
//! identity the request names and answers `ResourceNotFoundException` for a pair the
//! permission set does not carry, and
//! `ListCustomerManagedPolicyReferencesInPermissionSet` reports exactly the entries that
//! are left, once each, for any page size the model admits. An `InstanceArn` or
//! `PermissionSetArn` nothing is held under is a `ResourceNotFoundException`, and the whole
//! surface is withdrawn on an account instance.
//!
//! Each rule is asserted over a TABLE whose rows differ along what the rule generalises
//! over: the presence and the form of `Path`, the published length bounds of `Name`
//! (1..128) and of `Path` (1..512) and one value past each, the character classes both
//! members admit, the NUMBER of references attached, the page size, and the order the
//! attaches and detaches are made in. No row is the one a recording happened to carry.
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
    account_instance_router, call, error_code, organization_router, UNKNOWN_INSTANCE_ARN,
};

/// A pattern-valid `PermissionSetArn` that was never minted, for the not-found rows.
/// Shaped by the model's `PermissionSetArn` pattern
/// `^arn:aws(-[a-z]{1,5}){0,3}:sso:::permissionSet/(sso)?ins-[a-zA-Z0-9-.]{16}/ps-[a-zA-Z0-9-./]{16}$`.
const UNKNOWN_PERMISSION_SET_ARN: &str =
    "arn:aws:sso:::permissionSet/ssoins-0000000000000000/ps-0000000000000000";

/// `CustomerManagedPolicyReference$Path`'s documented default: "The default is `/`."
const DEFAULT_PATH: &str = "/";

/// `ManagedPolicyName`'s published upper length bound (`length(min: 1, max: 128)`).
const NAME_MAX: usize = 128;

/// `ManagedPolicyPath`'s published upper length bound (`length(min: 1, max: 512)`).
const PATH_MAX: usize = 512;

/// One row of the reference table: what a request sends as
/// `CustomerManagedPolicyReference`, and the (`Name`, `Path`) pair a later list must
/// report for it.
struct ReferenceCase {
    label: &'static str,
    /// What the attach and the detach send.
    sent: Value,
    /// The `Name` the list must report.
    name: String,
    /// The `Path` the list must report — materialised, so always present.
    path: String,
}

/// The table. Rows span the presence of `Path`, both published length bounds of `Name`
/// (1..128) and the bounds and nesting forms of `Path` (1..512), the full character classes
/// `^[\w+=,.@-]+$` (`Name`) and `^((/[A-Za-z0-9\.,\+@=_-]+)*)/$` (`Path`), and the same
/// `Name` under two different paths — which the identity rule makes two references.
fn reference_cases() -> Vec<ReferenceCase> {
    vec![
        ReferenceCase {
            label: "no Path at all, so the documented default is stored",
            sent: json!({"Name": "ReadOnlyPolicy"}),
            name: "ReadOnlyPolicy".to_string(),
            path: DEFAULT_PATH.to_string(),
        },
        ReferenceCase {
            // `Path` at its lower length bound, which is the default spelled out.
            label: "the default Path spelled out",
            sent: json!({"Name": "ExplicitRootPolicy", "Path": "/"}),
            name: "ExplicitRootPolicy".to_string(),
            path: "/".to_string(),
        },
        ReferenceCase {
            // `Name` at its lower length bound.
            label: "a one-character Name",
            sent: json!({"Name": "a", "Path": "/"}),
            name: "a".to_string(),
            path: "/".to_string(),
        },
        ReferenceCase {
            // `Name` at its upper length bound.
            label: "a Name of 128 characters",
            sent: json!({"Name": "n".repeat(NAME_MAX)}),
            name: "n".repeat(NAME_MAX),
            path: DEFAULT_PATH.to_string(),
        },
        ReferenceCase {
            label: "a Name using the full ^[\\w+=,.@-]+$ class",
            sent: json!({"Name": "a.b,c+d=e@f_g-H9", "Path": "/"}),
            name: "a.b,c+d=e@f_g-H9".to_string(),
            path: "/".to_string(),
        },
        ReferenceCase {
            label: "a nested Path",
            sent: json!({"Name": "NestedPolicy", "Path": "/engineering/platform/"}),
            name: "NestedPolicy".to_string(),
            path: "/engineering/platform/".to_string(),
        },
        ReferenceCase {
            label: "a Path using the full ^((/[A-Za-z0-9\\.,\\+@=_-]+)*)/$ class",
            sent: json!({"Name": "CharsetPolicy", "Path": "/a.b,c+d@e=f_g-H9/"}),
            name: "CharsetPolicy".to_string(),
            path: "/a.b,c+d@e=f_g-H9/".to_string(),
        },
        ReferenceCase {
            // `Path` at its upper length bound: a leading `/`, 510 segment characters and
            // the trailing `/` the pattern requires.
            label: "a Path of 512 characters",
            sent: json!({
                "Name": "LongPathPolicy",
                "Path": format!("/{}/", "p".repeat(PATH_MAX - 2)),
            }),
            name: "LongPathPolicy".to_string(),
            path: format!("/{}/", "p".repeat(PATH_MAX - 2)),
        },
        ReferenceCase {
            // The SAME `Name` as the first row under another `Path`: a different policy,
            // because a reference's identity is the pair.
            label: "the first row's Name under another Path",
            sent: json!({"Name": "ReadOnlyPolicy", "Path": "/team/"}),
            name: "ReadOnlyPolicy".to_string(),
            path: "/team/".to_string(),
        },
    ]
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

/// `AttachCustomerManagedPolicyReferenceToPermissionSet`, as `(status, body)`.
async fn attach(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    reference: &Value,
) -> (u16, Value) {
    call(
        router,
        "AttachCustomerManagedPolicyReferenceToPermissionSet",
        &json!({
            "InstanceArn": instance_arn,
            "PermissionSetArn": permission_set_arn,
            "CustomerManagedPolicyReference": reference,
        }),
    )
    .await
}

/// `DetachCustomerManagedPolicyReferenceFromPermissionSet`, as `(status, body)`.
async fn detach(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    reference: &Value,
) -> (u16, Value) {
    call(
        router,
        "DetachCustomerManagedPolicyReferenceFromPermissionSet",
        &json!({
            "InstanceArn": instance_arn,
            "PermissionSetArn": permission_set_arn,
            "CustomerManagedPolicyReference": reference,
        }),
    )
    .await
}

/// `ListCustomerManagedPolicyReferencesInPermissionSet`, as `(status, body)`.
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
    call(
        router,
        "ListCustomerManagedPolicyReferencesInPermissionSet",
        &request,
    )
    .await
}

/// The (`Name`, `Path`) pairs one list page reports, in the order the page carries them.
fn pairs(body: &Value) -> Vec<(String, String)> {
    body["CustomerManagedPolicyReferences"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|reference| {
            (
                reference["Name"].as_str().unwrap_or_default().to_string(),
                reference["Path"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// Every reference attached to `permission_set_arn`, read over the wire in one page and
/// sorted, so a comparison is about the SET rather than about a page order.
async fn attached_set(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
) -> Vec<(String, String)> {
    let (status, body) = list(router, instance_arn, permission_set_arn, None, None).await;
    assert_eq!(
        status, 200,
        "ListCustomerManagedPolicyReferencesInPermissionSet: expected 200, got {status} {body}"
    );
    let mut found = pairs(&body);
    found.sort();
    found
}

#[tokio::test]
async fn an_attach_stores_the_reference_and_the_list_answers_from_it() {
    let (router, instance_arn) = organization_router();
    for (index, case) in reference_cases().into_iter().enumerate() {
        // One permission set per row, so each row's list reports its row alone.
        let permission_set_arn =
            create_permission_set(&router, &instance_arn, &format!("Attach{index}")).await;

        let empty = attached_set(&router, &instance_arn, &permission_set_arn).await;
        assert!(
            empty.is_empty(),
            "case {:?}: a fresh permission set carries no references, got {empty:?}",
            case.label
        );

        let (status, body) = attach(&router, &instance_arn, &permission_set_arn, &case.sent).await;
        assert_eq!(
            status, 200,
            "case {:?}: attach expected 200, got {status} {body}",
            case.label
        );

        let found = attached_set(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            found,
            vec![(case.name.clone(), case.path.clone())],
            "case {:?}: the list must report the stored pair, got {found:?}",
            case.label
        );
    }
}

#[tokio::test]
async fn attaching_the_same_reference_twice_attaches_it_once() {
    let (router, instance_arn) = organization_router();
    for (index, case) in reference_cases().into_iter().enumerate() {
        let permission_set_arn =
            create_permission_set(&router, &instance_arn, &format!("Twice{index}")).await;
        for attempt in 1..=3 {
            let (status, body) =
                attach(&router, &instance_arn, &permission_set_arn, &case.sent).await;
            assert_eq!(
                status, 200,
                "case {:?}: attach #{attempt} expected 200, got {status} {body}",
                case.label
            );
        }
        let found = attached_set(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            found,
            vec![(case.name.clone(), case.path.clone())],
            "case {:?}: three attaches of one pair attach it once, got {found:?}",
            case.label
        );
    }
}

#[tokio::test]
async fn a_detach_removes_exactly_the_reference_it_names() {
    let (router, instance_arn) = organization_router();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "DetachOne").await;

    let cases = reference_cases();
    for case in &cases {
        let (status, body) = attach(&router, &instance_arn, &permission_set_arn, &case.sent).await;
        assert_eq!(
            status, 200,
            "case {:?}: attach expected 200, got {status} {body}",
            case.label
        );
    }

    let mut remaining: Vec<(String, String)> = cases
        .iter()
        .map(|case| (case.name.clone(), case.path.clone()))
        .collect();
    remaining.sort();
    let found = attached_set(&router, &instance_arn, &permission_set_arn).await;
    assert_eq!(
        found, remaining,
        "the list must report every attached reference, got {found:?}"
    );

    // Detach the rows in a different order from the one they were attached in, so the
    // assertion is about identity rather than about position.
    for case in cases.iter().rev() {
        let (status, body) = detach(&router, &instance_arn, &permission_set_arn, &case.sent).await;
        assert_eq!(
            status, 200,
            "case {:?}: detach expected 200, got {status} {body}",
            case.label
        );
        remaining.retain(|pair| pair != &(case.name.clone(), case.path.clone()));
        let found = attached_set(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            found, remaining,
            "case {:?}: a detach removes exactly that pair, got {found:?}",
            case.label
        );
    }
    assert!(
        remaining.is_empty(),
        "every row was detached, got {remaining:?}"
    );
}

#[tokio::test]
async fn a_detach_that_omits_path_removes_the_reference_stored_under_the_default() {
    let (router, instance_arn) = organization_router();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "PathDefault").await;

    // The pair is the same reference whichever side spells `Path` out, because both sides
    // materialise an absent one to the documented default.
    let spellings = [
        ("attached with Path, detached without", json!({"Name": "P1", "Path": "/"}), json!({"Name": "P1"})),
        ("attached without Path, detached with", json!({"Name": "P2"}), json!({"Name": "P2", "Path": "/"})),
        ("attached without Path, detached without", json!({"Name": "P3"}), json!({"Name": "P3"})),
    ];
    for (label, attached, detached) in spellings {
        let (status, body) = attach(&router, &instance_arn, &permission_set_arn, &attached).await;
        assert_eq!(
            status, 200,
            "case {label:?}: attach expected 200, got {status} {body}"
        );
        let (status, body) = detach(&router, &instance_arn, &permission_set_arn, &detached).await;
        assert_eq!(
            status, 200,
            "case {label:?}: detach expected 200, got {status} {body}"
        );
        let found = attached_set(&router, &instance_arn, &permission_set_arn).await;
        assert!(
            found.is_empty(),
            "case {label:?}: the detach must remove the stored reference, got {found:?}"
        );
    }
}

#[tokio::test]
async fn a_detach_of_a_reference_the_permission_set_does_not_hold_is_not_found() {
    let (router, instance_arn) = organization_router();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "NotAttached").await;
    let other_permission_set_arn =
        create_permission_set(&router, &instance_arn, "NotAttachedOther").await;

    // The one reference this permission set does hold, so every row below is about a pair
    // it does NOT hold while the set is non-empty.
    let held = json!({"Name": "HeldPolicy", "Path": "/held/"});
    let (status, body) = attach(&router, &instance_arn, &permission_set_arn, &held).await;
    assert_eq!(status, 200, "attach of the held pair: got {status} {body}");

    // The pair attached to the OTHER permission set: an attach is scoped to the permission
    // set it was made on, so detaching it here is a missing reference.
    let elsewhere = json!({"Name": "ElsewherePolicy", "Path": "/"});
    let (status, body) = attach(&router, &instance_arn, &other_permission_set_arn, &elsewhere).await;
    assert_eq!(status, 200, "attach on the other set: got {status} {body}");

    // Detach the held pair once (success), so the SAME request is a missing reference the
    // second time round.
    let (status, body) = detach(&router, &instance_arn, &permission_set_arn, &held).await;
    assert_eq!(status, 200, "first detach of the held pair: {status} {body}");
    let (status, body) = attach(&router, &instance_arn, &permission_set_arn, &held).await;
    assert_eq!(status, 200, "re-attach of the held pair: {status} {body}");

    let missing = [
        ("a Name never attached", json!({"Name": "NeverAttached", "Path": "/held/"})),
        (
            "the held Name under another Path",
            json!({"Name": "HeldPolicy", "Path": "/other/"}),
        ),
        (
            "the held Name under the default Path",
            json!({"Name": "HeldPolicy"}),
        ),
        (
            "the held Path under another Name",
            json!({"Name": "HeldPolicyToo", "Path": "/held/"}),
        ),
        ("a pair attached to another permission set", elsewhere.clone()),
    ];
    let before = attached_set(&router, &instance_arn, &permission_set_arn).await;
    for (label, reference) in missing {
        let (status, body) = detach(&router, &instance_arn, &permission_set_arn, &reference).await;
        // HTTP 400: this service answers every modelled 4xx refusal with 400 whatever
        // `@httpError` the shape declares — `crate::scaffolded::modeled_error_status` —
        // so the ERROR CODE is what tells the refusals apart.
        assert_eq!(
            status, 400,
            "case {label:?}: detach of an unattached pair expected 400, got {status} {body}"
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

    // And the second detach of a pair that WAS attached, after it is gone.
    let (status, body) = detach(&router, &instance_arn, &permission_set_arn, &held).await;
    assert_eq!(status, 200, "second detach of the held pair: {status} {body}");
    let (status, body) = detach(&router, &instance_arn, &permission_set_arn, &held).await;
    assert_eq!(
        status, 400,
        "a repeated detach of a removed pair expected 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "a repeated detach expected ResourceNotFoundException, got {body}"
    );
}

#[tokio::test]
async fn an_unknown_instance_or_permission_set_is_not_found() {
    let (router, instance_arn) = organization_router();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "Known").await;
    let reference = json!({"Name": "AnyPolicy", "Path": "/"});

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
        (
            "a known permission set under an unknown instance",
            UNKNOWN_INSTANCE_ARN.to_string(),
            permission_set_arn.clone(),
        ),
    ];
    for (label, instance, permission_set) in rows {
        for operation in [
            "AttachCustomerManagedPolicyReferenceToPermissionSet",
            "DetachCustomerManagedPolicyReferenceFromPermissionSet",
        ] {
            let (status, body) = call(
                &router,
                operation,
                &json!({
                    "InstanceArn": instance,
                    "PermissionSetArn": permission_set,
                    "CustomerManagedPolicyReference": reference,
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
        let (status, body) = list(&router, &instance, &permission_set, None, None).await;
        assert_eq!(
            status, 400,
            "case {label:?}: list expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {label:?}: list expected ResourceNotFoundException, got {body}"
        );
    }
}

#[tokio::test]
async fn the_whole_surface_is_refused_on_an_account_instance() {
    // An instance created through `CreateInstance` is the instance of a standalone
    // account, on which no permission set can exist, so every operation of this surface is
    // refused before any lookup.
    let (router, instance_arn) = account_instance_router().await;
    let reference = json!({"Name": "AnyPolicy", "Path": "/"});

    for operation in [
        "AttachCustomerManagedPolicyReferenceToPermissionSet",
        "DetachCustomerManagedPolicyReferenceFromPermissionSet",
    ] {
        let (status, body) = call(
            &router,
            operation,
            &json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": UNKNOWN_PERMISSION_SET_ARN,
                "CustomerManagedPolicyReference": reference,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {operation:?}: expected 400 on an account instance, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {operation:?}: expected ValidationException, got {body}"
        );
    }

    let (status, body) = list(&router, &instance_arn, UNKNOWN_PERMISSION_SET_ARN, None, None).await;
    assert_eq!(
        status, 400,
        "case \"list\": expected 400 on an account instance, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case \"list\": expected ValidationException, got {body}"
    );
}

#[tokio::test]
async fn the_list_enumerates_the_attached_set_once_per_reference_at_every_page_size() {
    let (router, instance_arn) = organization_router();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "Paged").await;

    let cases = reference_cases();
    for case in &cases {
        let (status, body) = attach(&router, &instance_arn, &permission_set_arn, &case.sent).await;
        assert_eq!(
            status, 200,
            "case {:?}: attach expected 200, got {status} {body}",
            case.label
        );
    }
    let mut expected: Vec<(String, String)> = cases
        .iter()
        .map(|case| (case.name.clone(), case.path.clone()))
        .collect();
    expected.sort();

    // Every page size the model's `MaxResults` range (1..100) admits that is interesting
    // here: both its lower bound, sizes that do and do not divide the set, one larger than
    // the set, the published upper bound, and no `MaxResults` at all.
    for max_results in [Some(1), Some(2), Some(4), Some(9), Some(50), Some(100), None] {
        let case = format!("MaxResults {max_results:?}");
        let mut walked: Vec<(String, String)> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let (status, body) = list(
                &router,
                &instance_arn,
                &permission_set_arn,
                max_results,
                token.as_deref(),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: list expected 200, got {status} {body}"
            );
            let page = pairs(&body);
            if let Some(limit) = max_results {
                assert!(
                    page.len() as i64 <= limit,
                    "case {case:?}: a page carries at most MaxResults items, got {}",
                    page.len()
                );
            }
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= cases.len() + 1,
                "case {case:?}: the enumeration must terminate, walked {walked:?}"
            );
            token = body["NextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        let mut sorted = walked.clone();
        sorted.sort();
        assert_eq!(
            sorted, expected,
            "case {case:?}: the enumeration reports every reference exactly once, got {walked:?}"
        );
    }

    // A `NextToken` this service never issued ends an enumeration early if it is answered
    // with an empty page, so it is refused instead.
    let (status, body) = list(
        &router,
        &instance_arn,
        &permission_set_arn,
        None,
        Some("not-a-token-this-service-issued"),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"a foreign NextToken\": expected 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case \"a foreign NextToken\": expected ValidationException, got {body}"
    );

    // And a `MaxResults` outside the model's published range is refused by the modelled
    // constraints, on each bound's far side.
    for max_results in [0_i64, 101] {
        let (status, body) = list(
            &router,
            &instance_arn,
            &permission_set_arn,
            Some(max_results),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case \"MaxResults {max_results}\": expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case \"MaxResults {max_results}\": expected ValidationException, got {body}"
        );
    }
}

#[tokio::test]
async fn a_list_changes_nothing_a_later_caller_sees() {
    let (router, instance_arn) = organization_router();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "ReadOnly").await;
    for case in &reference_cases() {
        let (status, body) = attach(&router, &instance_arn, &permission_set_arn, &case.sent).await;
        assert_eq!(
            status, 200,
            "case {:?}: attach expected 200, got {status} {body}",
            case.label
        );
    }

    let first = attached_set(&router, &instance_arn, &permission_set_arn).await;
    for round in 1..=3 {
        let again = attached_set(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            again, first,
            "round {round}: a list must leave the stored set alone, got {again:?}"
        );
    }
}

#[tokio::test]
async fn a_reference_outside_the_modeled_constraints_is_refused_and_stores_nothing() {
    let (router, instance_arn) = organization_router();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "Constrained").await;

    // One value past each published bound, and one outside each member's pattern.
    let refused = [
        ("a Name of 129 characters", json!({"Name": "n".repeat(NAME_MAX + 1)})),
        ("an empty Name", json!({"Name": ""})),
        (
            "a Path of 513 characters",
            json!({"Name": "Ok", "Path": format!("/{}/", "p".repeat(PATH_MAX - 1))}),
        ),
        ("an empty Path", json!({"Name": "Ok", "Path": ""})),
        (
            "a Name outside ^[\\w+=,.@-]+$",
            json!({"Name": "slash/in/name"}),
        ),
        (
            "a Path with no trailing slash",
            json!({"Name": "Ok", "Path": "/engineering"}),
        ),
        (
            "a Path with no leading slash",
            json!({"Name": "Ok", "Path": "engineering/"}),
        ),
    ];
    for (label, reference) in refused {
        for operation in [
            "AttachCustomerManagedPolicyReferenceToPermissionSet",
            "DetachCustomerManagedPolicyReferenceFromPermissionSet",
        ] {
            let (status, body) = call(
                &router,
                operation,
                &json!({
                    "InstanceArn": instance_arn,
                    "PermissionSetArn": permission_set_arn,
                    "CustomerManagedPolicyReference": reference,
                }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {label:?}: {operation} expected 400, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {label:?}: {operation} expected ValidationException, got {body}"
            );
        }
        let found = attached_set(&router, &instance_arn, &permission_set_arn).await;
        assert!(
            found.is_empty(),
            "case {label:?}: a refused attach stores nothing, got {found:?}"
        );
    }
}

#[tokio::test]
async fn an_attach_beyond_the_emulator_ceiling_is_refused_and_stores_nothing() {
    // The ceiling is the emulator's own per-permission-set one, so the test reads it from
    // the crate rather than restating a number.
    let ceiling = service_com_amazonaws_ssoadmin_swbexternalservice::state::
        MAX_CUSTOMER_MANAGED_POLICIES_PER_PERMISSION_SET;
    let (router, instance_arn) = organization_router();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "AtCeiling").await;

    for index in 0..ceiling {
        let reference = json!({"Name": format!("Policy{index:03}"), "Path": "/"});
        let (status, body) = attach(&router, &instance_arn, &permission_set_arn, &reference).await;
        assert_eq!(
            status, 200,
            "case \"reference {index}\": attach expected 200, got {status} {body}"
        );
    }
    let at_ceiling = attached_set(&router, &instance_arn, &permission_set_arn).await;
    assert_eq!(
        at_ceiling.len(),
        ceiling,
        "the set must hold every attached reference, got {at_ceiling:?}"
    );

    // One past the ceiling: refused, and the stored set is untouched.
    let beyond = json!({"Name": "OnePastTheCeiling", "Path": "/"});
    let (status, body) = attach(&router, &instance_arn, &permission_set_arn, &beyond).await;
    assert_eq!(
        status, 400,
        "case \"one past the ceiling\": expected 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ServiceQuotaExceededException",
        "case \"one past the ceiling\": expected ServiceQuotaExceededException, got {body}"
    );
    let after = attached_set(&router, &instance_arn, &permission_set_arn).await;
    assert_eq!(
        after, at_ceiling,
        "case \"one past the ceiling\": a refused attach stores nothing, got {after:?}"
    );

    // Re-attaching a reference the set ALREADY holds adds no entry, so it is not refused
    // even at the ceiling.
    let held = json!({"Name": "Policy000", "Path": "/"});
    let (status, body) = attach(&router, &instance_arn, &permission_set_arn, &held).await;
    assert_eq!(
        status, 200,
        "case \"re-attach at the ceiling\": expected 200, got {status} {body}"
    );
    let after = attached_set(&router, &instance_arn, &permission_set_arn).await;
    assert_eq!(
        after, at_ceiling,
        "case \"re-attach at the ceiling\": the set is unchanged, got {after:?}"
    );

    // And a detach frees one place, so the reference that was refused fits.
    let (status, body) = detach(&router, &instance_arn, &permission_set_arn, &held).await;
    assert_eq!(
        status, 200,
        "case \"detach to free a place\": expected 200, got {status} {body}"
    );
    let (status, body) = attach(&router, &instance_arn, &permission_set_arn, &beyond).await;
    assert_eq!(
        status, 200,
        "case \"attach into the freed place\": expected 200, got {status} {body}"
    );
    let after = attached_set(&router, &instance_arn, &permission_set_arn).await;
    assert_eq!(
        after.len(),
        ceiling,
        "case \"attach into the freed place\": the set is back at the ceiling, got {after:?}"
    );
    assert!(
        after.contains(&("OnePastTheCeiling".to_string(), "/".to_string())),
        "case \"attach into the freed place\": the new reference is stored, got {after:?}"
    );
}
