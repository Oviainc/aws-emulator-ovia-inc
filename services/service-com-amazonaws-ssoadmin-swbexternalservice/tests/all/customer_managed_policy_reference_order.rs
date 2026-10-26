// SPDX-License-Identifier: CC0-1.0

//! The ORDER and the PAGINATION of
//! `ListCustomerManagedPolicyReferencesInPermissionSet`, end to end over the `awsJson1_1`
//! wire.
//!
//! THE RULE under test: the operation enumerates exactly the customer managed policy
//! references a permission set carries, in ONE total order — ascending by each
//! reference's (`Name`, `Path`) identity, which is the key the stored set is held under
//! (`state::CustomerManagedPolicyReferenceRecord::identity`) and the key a cursor resumes
//! after — so the sequence a caller reads is a function of the SET and never of the order
//! the attaches arrived in; a `MaxResults` the model admits partitions that one sequence
//! into consecutive pages, each page that carries a `NextToken` being full and that token
//! resuming strictly after its last reference, with the final page carrying no token; and
//! a `NextToken` belongs to the enumeration it was issued for, so one issued for another
//! permission set's references, or by another paginated operation, is refused rather than
//! resuming inside a sequence whose keys mean something else.
//!
//! Each rule is asserted over a TABLE whose rows differ along what the rule generalises
//! over: the ORDER the attaches are made in (six permutations of one reference set,
//! including the identity order and its reverse), the PAGE SIZE (sizes that do and do not
//! divide the set, the set's own size, one past it, the model's published upper bound, and
//! no `MaxResults` at all), and the POSITION of the reference a detach removes (first,
//! middle, last). The reference set itself spans what the identity orders by: one name
//! under two paths, two names differing only in case, a nested path, and a name that is
//! another name with a suffix.
//!
//! The SET the operations store and the refusals they answer are
//! `customer_managed_policy_references.rs`; this suite is about the SEQUENCE a caller
//! walks, which that suite compares sorted. Its harness (and the organization-instance
//! fixture the permission-set surface needs) is reused rather than rebuilt.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ssoadmin_swbexternalservice::state::{
    CustomerManagedPolicyReferenceRecord, BAD_CURSOR_MESSAGE,
};

use crate::customer_managed_policy_references::{
    attach, create_permission_set, detach, list, pairs,
};
use crate::ssoadmin_permission_sets::{assert_refused, call, organization_router};

/// The references every table below attaches, as the (`Name`, `Path`) pairs a list must
/// report for them.
///
/// Declared in an order that is NOT the identity order, and spanning what that identity
/// orders by: one `Name` under two different paths (two references, by the identity rule),
/// two names differing only in case, a nested path, and a name that is another name with a
/// suffix. Every value is inside the published character classes of `ManagedPolicyName`
/// (`^[\w+=,.@-]+$`) and `ManagedPolicyPath` (`^((/[A-Za-z0-9\.,\+@=_-]+)*)/$`), and the
/// set is well inside the emulator's per-permission-set ceiling.
const REFERENCES: [(&str, &str); 7] = [
    ("Zeta", "/"),
    ("Alpha", "/"),
    ("Alpha", "/eng/"),
    ("alpha", "/"),
    ("Beta", "/ops/team/"),
    ("Beta", "/ops/"),
    ("Beta_2", "/"),
];

/// [`REFERENCES`] in the order it is declared.
fn declared() -> Vec<(String, String)> {
    REFERENCES
        .iter()
        .map(|(name, path)| (name.to_string(), path.to_string()))
        .collect()
}

/// `references` in the order an enumeration of them must report: ascending by each
/// reference's identity.
///
/// The identity is built by the crate's own
/// [`CustomerManagedPolicyReferenceRecord::identity`] — the key the stored set is held
/// under and the key a `NextToken` resumes after — so a page order and a cursor cannot
/// disagree about what "after" means, and this expectation restates no key format.
fn identity_order(references: &[(String, String)]) -> Vec<(String, String)> {
    let mut ordered = references.to_vec();
    ordered.sort_by_key(|(name, path)| {
        CustomerManagedPolicyReferenceRecord {
            name: name.clone(),
            path: path.clone(),
        }
        .identity()
    });
    ordered
}

/// The attach-order table: six permutations of [`REFERENCES`], each labelled.
fn attach_orders() -> Vec<(&'static str, Vec<(String, String)>)> {
    let declared = declared();
    let mut reversed = declared.clone();
    reversed.reverse();
    let mut rotated = declared.clone();
    rotated.rotate_left(3);
    let ascending = identity_order(&declared);
    let mut descending = ascending.clone();
    descending.reverse();
    let interleaved: Vec<(String, String)> = declared
        .iter()
        .skip(1)
        .step_by(2)
        .chain(declared.iter().step_by(2))
        .cloned()
        .collect();
    vec![
        ("as declared", declared),
        ("reversed", reversed),
        ("rotated left by three", rotated),
        ("in the identity order itself", ascending),
        ("in reverse identity order", descending),
        ("odd positions first", interleaved),
    ]
}

/// Attach `order` to `permission_set_arn`, one call per reference, and answer the order a
/// later list must report them in.
async fn attach_all(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    order: &[(String, String)],
) -> Vec<(String, String)> {
    for (name, path) in order {
        let (status, body) = attach(
            router,
            instance_arn,
            permission_set_arn,
            &json!({"Name": name, "Path": path}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case ({name:?}, {path:?}): attach expected 200, got {status} {body}"
        );
    }
    identity_order(order)
}

/// One whole page of an enumeration, as `(the pairs it carries, its NextToken)`.
async fn one_page(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    case: &str,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> (Vec<(String, String)>, Option<String>) {
    let (status, body) = list(
        router,
        instance_arn,
        permission_set_arn,
        max_results,
        next_token,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: list expected 200, got {status} {body}"
    );
    (
        pairs(&body),
        body["NextToken"].as_str().map(str::to_string),
    )
}

#[tokio::test]
async fn the_list_reports_one_identity_order_whatever_order_the_attaches_arrived_in() {
    let (router, instance_arn) = organization_router();
    for (index, (label, order)) in attach_orders().into_iter().enumerate() {
        // One permission set per row, so each row's list reports its own attaches alone.
        let permission_set_arn =
            create_permission_set(&router, &instance_arn, &format!("Order{index}")).await;
        let expected = attach_all(&router, &instance_arn, &permission_set_arn, &order).await;

        let (walked, token) =
            one_page(&router, &instance_arn, &permission_set_arn, label, None, None).await;
        assert_eq!(
            walked, expected,
            "case {label:?}: the list reports the identity order, got {walked:?}"
        );
        assert!(
            token.is_none(),
            "case {label:?}: a page that carries the whole set carries no NextToken, got {token:?}"
        );

        // The order is a property of the stored set, so reading it again answers the same
        // sequence rather than one that depends on when it was read.
        let (again, _) =
            one_page(&router, &instance_arn, &permission_set_arn, label, None, None).await;
        assert_eq!(
            again, walked,
            "case {label:?}: a second list answers the same sequence, got {again:?}"
        );
    }
}

#[tokio::test]
async fn a_page_size_partitions_the_one_ordered_enumeration_into_consecutive_pages() {
    let (router, instance_arn) = organization_router();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "PagedOrder").await;
    let expected = attach_all(&router, &instance_arn, &permission_set_arn, &declared()).await;

    // Page sizes inside the model's `MaxResults` range (1..100): its lower bound, sizes
    // that do and do not divide the set, the set's own size, one past it, the published
    // upper bound, and no `MaxResults` at all.
    for max_results in [
        Some(1),
        Some(2),
        Some(3),
        Some(6),
        Some(7),
        Some(8),
        Some(100),
        None,
    ] {
        let case = format!("MaxResults {max_results:?}");
        // With no `MaxResults` the whole set fits one page, which is the same arithmetic
        // as a page size at least as large as the set.
        let limit = match max_results {
            Some(limit) => limit as usize,
            None => expected.len(),
        };
        let expected_pages = expected.len().div_ceil(limit);

        let mut walked: Vec<(String, String)> = Vec::new();
        let mut page_sizes: Vec<usize> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let (page, next) = one_page(
                &router,
                &instance_arn,
                &permission_set_arn,
                &case,
                max_results,
                token.as_deref(),
            )
            .await;
            assert!(
                page.len() <= limit,
                "case {case:?}: a page carries at most {limit} references, got {page:?}"
            );
            if next.is_some() {
                assert_eq!(
                    page.len(),
                    limit,
                    "case {case:?}: a page that hands back a NextToken is full, got {page:?}"
                );
            }
            page_sizes.push(page.len());
            walked.extend(page);
            assert!(
                page_sizes.len() <= expected.len() + 1,
                "case {case:?}: the enumeration must terminate, walked {walked:?}"
            );
            token = next;
            if token.is_none() {
                break;
            }
        }

        assert_eq!(
            walked, expected,
            "case {case:?}: the pages concatenate to the one identity order, got {walked:?}"
        );
        assert_eq!(
            page_sizes.len(),
            expected_pages,
            "case {case:?}: expected {expected_pages} pages, got page sizes {page_sizes:?}"
        );
    }
}

#[tokio::test]
async fn a_next_token_resumes_only_the_enumeration_it_was_issued_for() {
    let (router, instance_arn) = organization_router();
    let first = create_permission_set(&router, &instance_arn, "CursorScopeOne").await;
    let second = create_permission_set(&router, &instance_arn, "CursorScopeTwo").await;
    let expected = attach_all(&router, &instance_arn, &first, &declared()).await;
    let twin = attach_all(&router, &instance_arn, &second, &declared()).await;
    assert_eq!(
        twin, expected,
        "the two permission sets carry the same references, got {twin:?}"
    );

    let (page, token) = one_page(
        &router,
        &instance_arn,
        &first,
        "the first permission set",
        Some(2),
        None,
    )
    .await;
    assert_eq!(
        page,
        expected[..2].to_vec(),
        "case \"the first permission set\": the first page is the order's head, got {page:?}"
    );
    let token = token.unwrap_or_else(|| {
        panic!("case \"the first permission set\": a truncated page hands back a NextToken")
    });

    // The positive control: on its OWN enumeration the token resumes strictly after the
    // page it was issued for.
    let (rest, _) = one_page(
        &router,
        &instance_arn,
        &first,
        "resuming its own enumeration",
        None,
        Some(&token),
    )
    .await;
    assert_eq!(
        rest,
        expected[2..].to_vec(),
        "case \"resuming its own enumeration\": expected the tail of the order, got {rest:?}"
    );

    // A token issued by ANOTHER paginated operation of this service, over the same
    // instance: a cursor carries the enumeration it belongs to, not just a position.
    let (status, body) = call(
        &router,
        "ListPermissionSets",
        &json!({"InstanceArn": instance_arn, "MaxResults": 1}),
    )
    .await;
    assert_eq!(
        status, 200,
        "ListPermissionSets expected 200, got {status} {body}"
    );
    let other_operation_token = body["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("ListPermissionSets handed back no NextToken: {body}"))
        .to_string();

    // A token of the SECOND set's enumeration too, so neither direction is the one that
    // happens to be refused.
    let (_, second_token) = one_page(
        &router,
        &instance_arn,
        &second,
        "the second permission set",
        Some(2),
        None,
    )
    .await;
    let second_token = second_token.unwrap_or_else(|| {
        panic!("case \"the second permission set\": a truncated page hands back a NextToken")
    });

    // Every token below was issued for a DIFFERENT enumeration than the one it is
    // presented to, so each is refused rather than answered with a page.
    let foreign = [
        (
            "a token issued for another permission set's references",
            second.as_str(),
            token.as_str(),
        ),
        (
            "that same mismatch the other way round",
            first.as_str(),
            second_token.as_str(),
        ),
        (
            "a token issued by another paginated operation",
            first.as_str(),
            other_operation_token.as_str(),
        ),
    ];
    for (case, permission_set_arn, next_token) in foreign {
        let (status, body) = list(
            &router,
            &instance_arn,
            permission_set_arn,
            None,
            Some(next_token),
        )
        .await;
        assert_refused(case, "the list", "ValidationException", status, &body);
        assert_eq!(
            body["Message"].as_str(),
            Some(BAD_CURSOR_MESSAGE),
            "case {case:?}: expected the cursor refusal's own wording, got {body}"
        );
    }
}

#[tokio::test]
async fn a_detach_and_a_reattach_leave_the_order_a_function_of_the_set() {
    let (router, instance_arn) = organization_router();
    let expected = identity_order(&declared());

    // The position the detached reference holds in the order: its head, its middle and
    // its tail — a re-attach has to put each one back where the SET puts it, not at the
    // end of the sequence.
    for position in [0_usize, expected.len() / 2, expected.len() - 1] {
        let case = format!("the reference at position {position}");
        let permission_set_arn =
            create_permission_set(&router, &instance_arn, &format!("Reattach{position}")).await;
        let stored = attach_all(&router, &instance_arn, &permission_set_arn, &declared()).await;
        assert_eq!(
            stored, expected,
            "case {case:?}: the attaches store the identity order, got {stored:?}"
        );

        let (name, path) = expected[position].clone();
        let reference = json!({"Name": name, "Path": path});
        let (status, body) = detach(&router, &instance_arn, &permission_set_arn, &reference).await;
        assert_eq!(
            status, 200,
            "case {case:?}: detach expected 200, got {status} {body}"
        );

        let mut without: Vec<(String, String)> = expected.clone();
        without.remove(position);
        let (walked, _) =
            one_page(&router, &instance_arn, &permission_set_arn, &case, None, None).await;
        assert_eq!(
            walked, without,
            "case {case:?}: the remaining references keep their order, got {walked:?}"
        );

        let (status, body) = attach(&router, &instance_arn, &permission_set_arn, &reference).await;
        assert_eq!(
            status, 200,
            "case {case:?}: re-attach expected 200, got {status} {body}"
        );
        let (walked, _) =
            one_page(&router, &instance_arn, &permission_set_arn, &case, None, None).await;
        assert_eq!(
            walked, expected,
            "case {case:?}: a re-attached reference returns to its ordered position, got {walked:?}"
        );
    }
}
