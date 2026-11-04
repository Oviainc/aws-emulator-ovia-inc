// SPDX-License-Identifier: CC0-1.0

//! Permissions granted on a private CA, driven over the wire: `CreatePermission` records
//! a grant, `ListPermissions` reports back exactly what was granted, and
//! `DeletePermission` revokes one.
//!
//! The harness — the router, the signed request and the success/refusal helpers — is the
//! one `private_ca_lifecycle` drives the rest of this service with; only the cases here
//! are new.

use std::time::SystemTime;

use serde_json::{json, Value};

use crate::private_ca_lifecycle::{
    root_ca_request, string, wait_for_the_clock, Emulator, ACCOUNT, REGION,
};

/// `seconds` rounded DOWN to the millisecond the wire's `Timestamp` encoding carries.
fn floor_to_millisecond(seconds: f64) -> f64 {
    (seconds * 1_000.0).floor() / 1_000.0
}

/// `seconds` rounded UP to the millisecond the wire's `Timestamp` encoding carries.
fn ceil_to_millisecond(seconds: f64) -> f64 {
    (seconds * 1_000.0).ceil() / 1_000.0
}

/// Every `ActionType` member the model declares, from
/// `smithy_query(query="shape",shape="ActionType")`: `IssueCertificate`,
/// `GetCertificate`, `ListPermissions`. A grant names all of them, in any order
/// (`ActionList`, len 1..3); the refusal of a grant that names fewer is covered by
/// `permission_action_set`.
const ACTIONS: [&str; 3] = ["IssueCertificate", "GetCertificate", "ListPermissions"];

/// Create a private CA and return its ARN.
async fn certificate_authority(emulator: &Emulator, case: &str) -> String {
    let created = emulator
        .ok(case, "CreateCertificateAuthority", &root_ca_request(case))
        .await;
    string(&created, "CertificateAuthorityArn", case)
}

/// A `CreatePermission` / `DeletePermission`-shaped body: the CA, the principal and, when
/// the case names one, the source account.
fn grant(arn: &str, principal: &str, source_account: Option<&str>, actions: &[&str]) -> Value {
    let mut body = json!({
        "CertificateAuthorityArn": arn,
        "Principal": principal,
        "Actions": actions,
    });
    if let Some(account) = source_account {
        body["SourceAccount"] = json!(account);
    }
    body
}

/// One `ListPermissions` page.
async fn list_page(
    emulator: &Emulator,
    case: &str,
    arn: &str,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> Value {
    let mut body = json!({"CertificateAuthorityArn": arn});
    if let Some(max) = max_results {
        body["MaxResults"] = json!(max);
    }
    if let Some(token) = next_token {
        body["NextToken"] = json!(token);
    }
    emulator.ok(case, "ListPermissions", &body).await
}

/// The `Permissions` member of a page, as a list.
fn permissions_of(page: &Value, case: &str) -> Vec<Value> {
    page.get("Permissions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no Permissions list in {page:?}"))
        .clone()
}

/// Every permission one CA holds, read in a single page.
async fn all_permissions(emulator: &Emulator, case: &str, arn: &str) -> Vec<Value> {
    let page = list_page(emulator, case, arn, None, None).await;
    assert!(
        page.get("NextToken").is_none(),
        "case {case:?}: a page that holds every grant must carry no NextToken, got {page:?}"
    );
    permissions_of(&page, case)
}

/// The permission a list reports for `principal` and `source_account`, if any.
fn reported<'a>(
    permissions: &'a [Value],
    principal: &str,
    source_account: &str,
) -> Option<&'a Value> {
    permissions.iter().find(|permission| {
        permission["Principal"] == json!(principal)
            && permission["SourceAccount"] == json!(source_account)
    })
}

/// A grant is keyed by the CA, the principal and the source account, and
/// `ListPermissions` reports each one with exactly the actions it was granted with.
#[tokio::test]
async fn a_granted_permission_is_listed_back_as_it_was_granted() {
    let emulator = Emulator::new();
    let arn = certificate_authority(&emulator, "granted").await;
    let other_arn = certificate_authority(&emulator, "granted-other").await;

    // The rows differ along what the rule generalises over: the order a grant lists the
    // actions in, whether the request names a source account, and which (principal,
    // source account) pair identifies the grant.
    let rows: [(&str, &str, Option<&str>, &[&str]); 4] = [
        (
            "the model's order, no source account",
            "acm.amazonaws.com",
            None,
            &ACTIONS,
        ),
        (
            "another order, an explicit source account",
            "acm.amazonaws.com",
            Some("123456789012"),
            &[ACTIONS[1], ACTIONS[2], ACTIONS[0]],
        ),
        (
            "every action, in the order the request named them",
            "ram.amazonaws.com",
            Some("210987654321"),
            &[ACTIONS[2], ACTIONS[0], ACTIONS[1]],
        ),
        (
            "the same principal under another source account",
            "ram.amazonaws.com",
            Some("123456789012"),
            &[ACTIONS[2], ACTIONS[1], ACTIONS[0]],
        ),
    ];

    // `awsJson1_1` serialises a `Timestamp` as epoch seconds with MILLISECOND precision,
    // so the `CreatedAt` reported back is the grant's instant truncated to the
    // millisecond and can sit just below a nanosecond-precision bound. Widen each bound
    // to the millisecond the wire carries, so the window brackets the reported value
    // rather than the clock's full resolution.
    let before = floor_to_millisecond(
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_secs_f64(),
    );

    for (case, principal, source_account, actions) in rows {
        let answer = emulator
            .ok(
                case,
                "CreatePermission",
                &grant(&arn, principal, source_account, actions),
            )
            .await;
        assert!(
            answer.as_object().is_none_or(|members| members.is_empty()),
            "case {case:?}: CreatePermission models no output members, got {answer:?}"
        );
    }

    // A grant is held under the CA it was made on: another CA holds none of them.
    let others = all_permissions(&emulator, "another CA", &other_arn).await;
    assert!(
        others.is_empty(),
        "case \"another CA\": a CA nobody granted on must list no permission, got {others:?}"
    );

    let permissions = all_permissions(&emulator, "granted", &arn).await;
    assert_eq!(
        permissions.len(),
        rows.len(),
        "case \"granted\": expected one permission per grant, got {permissions:?}"
    );

    let after = ceil_to_millisecond(
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_secs_f64(),
    );

    // A `TStamp` is serialized with MILLISECOND resolution, so an instant taken inside
    // this window comes back truncated to its millisecond — which reads marginally
    // BEFORE `before` whenever the grant was made in the same millisecond the window
    // opened. Widen each bound to the millisecond it falls in, so the assertion is
    // about the window and not about the wire format's resolution.
    let before = (before * 1_000.0).floor() / 1_000.0;
    let after = (after * 1_000.0).ceil() / 1_000.0;

    for (case, principal, source_account, actions) in rows {
        // A request that names no source account is the calling account's grant, and the
        // calling account is the one every ARN of this emulator carries.
        let expected_account = source_account.unwrap_or(ACCOUNT);
        let permission = reported(&permissions, principal, expected_account).unwrap_or_else(|| {
            panic!("case {case:?}: no grant for {principal} / {expected_account} in {permissions:?}")
        });
        assert_eq!(
            permission["CertificateAuthorityArn"],
            json!(arn),
            "case {case:?}: expected the grant to name the CA it was made on, got {permission:?}"
        );
        assert_eq!(
            permission["Actions"],
            json!(crate::policy_documents::in_reported_order(actions)),
            "case {case:?}: expected the granted actions in reported order, got {permission:?}"
        );
        assert!(
            permission.get("Policy").is_some(),
            "case {case:?}: a grant reports the policy it expresses, got {permission:?}"
        );
        let created_at = permission["CreatedAt"].as_f64().unwrap_or_default();
        // The reported instant is the clock read TRUNCATED to milliseconds
        // (`state::to_date_time` builds it with `DateTime::from_millis`), so it
        // can sit just before `before`, which is read at full precision. What is
        // under test is that `CreatedAt` is the instant the grant was made, not
        // its sub-millisecond precision, so the window carries ten of those
        // rendering steps of slack at each end.
        const RENDERING_SLACK: f64 = 0.01;
        let window = (before - RENDERING_SLACK)..=(after + RENDERING_SLACK);
        assert!(
            window.contains(&created_at),
            "case {case:?}: expected CreatedAt within [{}, {}], got {created_at}",
            window.start(),
            window.end()
        );
    }
}

/// A CA accepts a grant whatever status its own creation has reached: a permission says
/// who may call the CA's actions, not whether the CA holds a certificate yet.
#[tokio::test]
async fn a_grant_is_accepted_before_and_after_creation_settles() {
    let emulator = Emulator::new();
    let arn = certificate_authority(&emulator, "settling").await;

    // Immediately after create, while the CA still reads CREATING.
    let described = emulator
        .ok(
            "while creating",
            "DescribeCertificateAuthority",
            &json!({"CertificateAuthorityArn": arn}),
        )
        .await;
    assert_eq!(
        string(&described["CertificateAuthority"], "Status", "while creating"),
        "CREATING",
        "case \"while creating\": the CA must still be settling, got {described:?}"
    );
    emulator
        .ok(
            "while creating",
            "CreatePermission",
            &grant(&arn, "acm.amazonaws.com", None, &ACTIONS),
        )
        .await;

    wait_for_the_clock();
    emulator
        .ok(
            "after settling",
            "CreatePermission",
            &grant(&arn, "ram.amazonaws.com", None, &ACTIONS),
        )
        .await;

    let permissions = all_permissions(&emulator, "settling", &arn).await;
    assert_eq!(
        permissions.len(),
        2,
        "case \"settling\": both grants must survive the status change, got {permissions:?}"
    );
}

/// One repeated-grant row: the case, the principal that holds the grant, the account that
/// assigned it (when the request names one), the actions the grant named and the actions
/// the repeat of it names.
type RepeatRow<'a> = (
    &'a str,
    &'a str,
    Option<&'a str>,
    &'a [&'a str],
    &'a [&'a str],
);

/// A second grant naming the same CA, principal and source account asks for the access
/// that already holds, so it answers success and leaves that grant exactly as it was.
#[tokio::test]
async fn a_repeated_grant_succeeds_and_changes_nothing() {
    let emulator = Emulator::new();
    let arn = certificate_authority(&emulator, "repeated").await;

    // The rows differ along whether the request names a source account, which (principal,
    // source account) pair identifies the grant, and the order the repeat lists the
    // actions in — the grant is the SET of actions, so a reordered repeat is the same ask.
    let rows: [RepeatRow; 3] = [
        (
            "no source account",
            "acm.amazonaws.com",
            None,
            &ACTIONS,
            &ACTIONS,
        ),
        (
            "an explicit source account, the repeat reordered",
            "acm.amazonaws.com",
            Some("123456789012"),
            &ACTIONS,
            &[ACTIONS[2], ACTIONS[1], ACTIONS[0]],
        ),
        (
            "another principal",
            "ram.amazonaws.com",
            Some("210987654321"),
            &[ACTIONS[1], ACTIONS[0], ACTIONS[2]],
            &ACTIONS,
        ),
    ];

    for (case, principal, source_account, actions, repeat) in rows {
        emulator
            .ok(
                case,
                "CreatePermission",
                &grant(&arn, principal, source_account, actions),
            )
            .await;
        // The repeat asks for exactly the access already in effect for that triple.
        let answer = emulator
            .ok(
                case,
                "CreatePermission",
                &grant(&arn, principal, source_account, repeat),
            )
            .await;
        assert!(
            answer.as_object().is_none_or(|members| members.is_empty()),
            "case {case:?}: a repeated grant answers an empty body, got {answer:?}"
        );
    }

    let permissions = all_permissions(&emulator, "repeated", &arn).await;
    assert_eq!(
        permissions.len(),
        rows.len(),
        "case \"repeated\": a repeat must record no second grant, got {permissions:?}"
    );
    for (case, principal, source_account, actions, _) in rows {
        let expected_account = source_account.unwrap_or(ACCOUNT);
        let permission = reported(&permissions, principal, expected_account)
            .unwrap_or_else(|| panic!("case {case:?}: no grant for {principal} in {permissions:?}"));
        assert_eq!(
            permission["Actions"],
            json!(crate::policy_documents::in_reported_order(actions)),
            "case {case:?}: the grant's actions must stand, got {permission:?}"
        );
    }
}

/// Every page size reports the same grants once each, in one order, and issues a
/// `NextToken` only while the list is truncated.
#[tokio::test]
async fn permissions_are_paged_and_resumed_in_one_order() {
    let emulator = Emulator::new();
    let arn = certificate_authority(&emulator, "paged").await;

    // Seven grants, so no published page-size bound divides the count evenly. Each names
    // every action, which is what a grant records; what identifies them is the principal.
    let granted = 7usize;
    for index in 0..granted {
        emulator
            .ok(
                "paged",
                "CreatePermission",
                &grant(
                    &arn,
                    &format!("principal-{index}.amazonaws.com"),
                    None,
                    &ACTIONS,
                ),
            )
            .await;
    }

    // `MaxResults` is `range 1..1000`: both published bounds, two sizes that truncate,
    // and one that exactly covers the grants. An omitted `MaxResults` is the last row.
    let mut orders: Vec<(String, Vec<String>)> = Vec::new();
    for max_results in [Some(1), Some(2), Some(3), Some(7), Some(1000), None] {
        let case = format!("MaxResults={max_results:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let page = list_page(&emulator, &case, &arn, max_results, token.as_deref()).await;
            let items = permissions_of(&page, &case);
            pages += 1;
            let limit = usize::try_from(max_results.unwrap_or(1000))
                .expect("a page size is a positive count");
            assert!(
                items.len() <= limit,
                "case {case:?}: a page may not exceed MaxResults, got {} items",
                items.len()
            );
            for item in &items {
                seen.push(string(item, "Principal", &case));
            }
            token = page
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            match token.as_deref() {
                // A truncated page is full: a token means grants were left over.
                Some(_) => assert_eq!(
                    items.len(),
                    limit,
                    "case {case:?}: a page that issues a NextToken must be full, got {items:?}"
                ),
                None => break,
            }
            assert!(
                pages <= granted + 1,
                "case {case:?}: paging must terminate, got {pages} pages for {granted} grants"
            );
        }
        assert_eq!(
            seen.len(),
            granted,
            "case {case:?}: every grant must be reported exactly once, got {seen:?}"
        );
        let expected_pages = granted.div_ceil(
            usize::try_from(max_results.unwrap_or(1000))
                .expect("a page size is a positive count"),
        );
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages, got {pages}"
        );
        orders.push((case, seen));
    }

    // One order, whatever the page size: the position a NextToken names is the same on
    // every call.
    let (first_case, first_order) = &orders[0];
    for (case, order) in &orders[1..] {
        assert_eq!(
            order, first_order,
            "case {case:?}: expected the order of {first_case:?}, got {order:?}"
        );
    }
}

/// A `NextToken` this service did not issue is refused, rather than answered with a page.
#[tokio::test]
async fn a_forged_next_token_is_refused() {
    let emulator = Emulator::new();
    let arn = certificate_authority(&emulator, "tokens").await;
    emulator
        .ok(
            "tokens",
            "CreatePermission",
            &grant(&arn, "acm.amazonaws.com", None, &ACTIONS),
        )
        .await;

    // Hex of a pair, of a pair whose account is not twelve digits, of text carrying no
    // separator, plus bodies that are not hex and not UTF-8 at all.
    let separator = '\u{1f}';
    for (case, token) in [
        ("not hex", "zz".to_string()),
        ("no separator", hex::encode("acm.amazonaws.com")),
        (
            "a source account that is not twelve digits",
            hex::encode(format!("acm.amazonaws.com{separator}12")),
        ),
        (
            "an empty principal",
            hex::encode(format!("{separator}{ACCOUNT}")),
        ),
        ("not utf-8", "ff".to_string()),
    ] {
        emulator
            .refused(
                case,
                "ListPermissions",
                &json!({"CertificateAuthorityArn": arn, "NextToken": token}),
                "InvalidNextTokenException",
            )
            .await;
    }

    // The token this service DID issue resumes the list.
    let page = list_page(&emulator, "tokens", &arn, Some(1), None).await;
    assert!(
        page.get("NextToken").is_none(),
        "case \"tokens\": one grant fits in one page of one, got {page:?}"
    );
}

/// Every permission operation reads the CA ARN by the same rule the rest of the service
/// does: refused by form first, then by existence.
#[tokio::test]
async fn permission_operations_refuse_a_ca_arn_by_form_and_then_by_existence() {
    let emulator = Emulator::new();
    let malformed = [
        (
            "no ca id",
            format!("arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority"),
        ),
        (
            "an extra resource segment",
            format!("arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority/abc/extra"),
        ),
        (
            "another resource type",
            format!("arn:aws:acm-pca:{REGION}:{ACCOUNT}:private-ca/abc"),
        ),
    ];
    let unknown = format!(
        "arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority/\
         00000000-0000-4000-8000-000000000000"
    );

    for (operation, extra) in [
        (
            "CreatePermission",
            json!({"Principal": "acm.amazonaws.com", "Actions": ACTIONS}),
        ),
        ("ListPermissions", json!({})),
        (
            "DeletePermission",
            json!({"Principal": "acm.amazonaws.com"}),
        ),
    ] {
        for (case, arn) in &malformed {
            let case = format!("{operation}: {case}");
            let mut body = extra.clone();
            body["CertificateAuthorityArn"] = json!(arn);
            emulator
                .refused(&case, operation, &body, "InvalidArnException")
                .await;
        }
        let case = format!("{operation}: a CA that was never created");
        let mut body = extra.clone();
        body["CertificateAuthorityArn"] = json!(unknown);
        emulator
            .refused(&case, operation, &body, "ResourceNotFoundException")
            .await;
    }
}

// ---------------------------------------------------------------------------
// DeletePermission: a grant exists only between its grant and its revocation
// ---------------------------------------------------------------------------

/// A `DeletePermission` body: the CA, the principal and, when the case names one, the
/// source account. `DeletePermission` models no `Actions` member — a revocation names the
/// grant, not what it granted.
fn revoke(arn: &str, principal: &str, source_account: Option<&str>) -> Value {
    let mut body = json!({"CertificateAuthorityArn": arn, "Principal": principal});
    if let Some(account) = source_account {
        body["SourceAccount"] = json!(account);
    }
    body
}

/// Revoking a grant removes exactly the grant it names: the CA stops reporting that one,
/// every other grant it holds stands, and the pair can be granted again afterwards.
#[tokio::test]
async fn a_revocation_removes_exactly_the_grant_it_names() {
    let emulator = Emulator::new();
    let arn = certificate_authority(&emulator, "revoked").await;
    let long_principal = "p".repeat(128);

    // The rows differ along what the rule generalises over: the order the grant listed
    // the actions in, whether the request names a source account, and which (principal,
    // source account) pair identifies it — including one principal held under two
    // accounts, so a revocation that ignored the account would take the wrong grant.
    // `Principal` is len 0..128
    // (`smithy_query(query="operation",operation="DeletePermission")`), so the last row
    // sits on that upper bound.
    let rows: Vec<(&str, &str, Option<&str>, Vec<&str>)> = vec![
        (
            "the model's order, no source account",
            "acm.amazonaws.com",
            None,
            ACTIONS.to_vec(),
        ),
        (
            "the same principal, an explicit source account",
            "acm.amazonaws.com",
            Some("123456789012"),
            vec![ACTIONS[1], ACTIONS[0], ACTIONS[2]],
        ),
        (
            "another principal, the actions reversed",
            "ram.amazonaws.com",
            Some("210987654321"),
            vec![ACTIONS[2], ACTIONS[1], ACTIONS[0]],
        ),
        (
            "a principal at the modelled length bound",
            long_principal.as_str(),
            None,
            ACTIONS.to_vec(),
        ),
    ];

    for (case, principal, source_account, actions) in &rows {
        emulator
            .ok(
                case,
                "CreatePermission",
                &grant(&arn, principal, *source_account, actions),
            )
            .await;
    }

    for revoked in 1..=rows.len() {
        let (case, principal, source_account, _) = &rows[revoked - 1];
        let answer = emulator
            .ok(case, "DeletePermission", &revoke(&arn, principal, *source_account))
            .await;
        assert!(
            answer.as_object().is_none_or(|members| members.is_empty()),
            "case {case:?}: DeletePermission models no output members, got {answer:?}"
        );

        let permissions = all_permissions(&emulator, case, &arn).await;
        assert_eq!(
            permissions.len(),
            rows.len() - revoked,
            "case {case:?}: one revocation removes one grant, got {permissions:?}"
        );
        assert!(
            reported(
                &permissions,
                principal,
                source_account.unwrap_or(ACCOUNT)
            )
            .is_none(),
            "case {case:?}: the revoked grant must be gone, got {permissions:?}"
        );
        // Every grant not yet revoked is still there, with the actions it was granted
        // with: a revocation touches no other grant.
        for (other, principal, source_account, actions) in &rows[revoked..] {
            let expected_account = source_account.unwrap_or(ACCOUNT);
            let permission = reported(&permissions, principal, expected_account)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: {other:?} must survive, got {permissions:?}")
                });
            assert_eq!(
                permission["Actions"],
                json!(crate::policy_documents::in_reported_order(actions)),
                "case {case:?}: {other:?} must keep its actions, got {permission:?}"
            );
        }
        // The grant no longer exists, so revoking it again is refused.
        emulator
            .refused(
                case,
                "DeletePermission",
                &revoke(&arn, principal, *source_account),
                "ResourceNotFoundException",
            )
            .await;
    }

    // Every grant is gone, and the pair the first row named can be granted again — with
    // the actions the NEW grant names, since the revoked one is no longer there.
    assert!(
        all_permissions(&emulator, "all revoked", &arn)
            .await
            .is_empty(),
        "case \"all revoked\": a CA whose grants were all revoked lists none"
    );
    emulator
        .ok(
            "granted again",
            "CreatePermission",
            &grant(&arn, "acm.amazonaws.com", None, &ACTIONS),
        )
        .await;
    let permissions = all_permissions(&emulator, "granted again", &arn).await;
    let permission = reported(&permissions, "acm.amazonaws.com", ACCOUNT).unwrap_or_else(|| {
        panic!("case \"granted again\": the new grant must be listed, got {permissions:?}")
    });
    assert_eq!(
        permission["Actions"],
        json!(crate::policy_documents::in_reported_order(&ACTIONS)),
        "case \"granted again\": expected the new grant's actions, got {permission:?}"
    );
}

/// A revocation naming a grant the CA does not hold is refused, whichever way the name
/// misses — and the refusal revokes nothing.
#[tokio::test]
async fn revoking_a_grant_the_ca_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    let arn = certificate_authority(&emulator, "missing").await;
    let other_arn = certificate_authority(&emulator, "missing-other").await;
    emulator
        .ok(
            "the one grant the CA holds",
            "CreatePermission",
            &grant(&arn, "acm.amazonaws.com", None, &ACTIONS),
        )
        .await;

    // Each row names a (CA, principal, source account) triple the store does not hold:
    // another principal, a principal the granted one only starts with, the granted
    // principal under another account, the granted principal in another case, and the
    // granted triple on a CA it was not granted on.
    let rows = [
        (
            "a principal that was never granted anything",
            revoke(&arn, "s3.amazonaws.com", None),
        ),
        (
            "a principal the granted one only starts with",
            revoke(&arn, "acm.amazonaws.co", None),
        ),
        (
            "the granted principal under another source account",
            revoke(&arn, "acm.amazonaws.com", Some("210987654321")),
        ),
        (
            "the granted principal in another case",
            revoke(&arn, "ACM.amazonaws.com", None),
        ),
        (
            "the granted principal on a CA it was not granted on",
            revoke(&other_arn, "acm.amazonaws.com", None),
        ),
    ];

    for (case, body) in &rows {
        let refusal = emulator
            .refused(case, "DeletePermission", body, "ResourceNotFoundException")
            .await;
        assert!(
            !string(&refusal, "message", case).is_empty(),
            "case {case:?}: the refusal carries a message, got {refusal:?}"
        );
    }

    // None of those refusals touched the grant the CA does hold.
    let permissions = all_permissions(&emulator, "after the refusals", &arn).await;
    let permission = reported(&permissions, "acm.amazonaws.com", ACCOUNT).unwrap_or_else(|| {
        panic!("case \"after the refusals\": the grant must stand, got {permissions:?}")
    });
    assert_eq!(
        permission["Actions"],
        json!(crate::policy_documents::in_reported_order(&ACTIONS)),
        "case \"after the refusals\": the grant must keep its actions, got {permission:?}"
    );
}

/// `SourceAccount` left out means the calling account, on the grant and on the
/// revocation alike: either naming resolves to the one grant, in both directions.
#[tokio::test]
async fn a_grant_and_its_revocation_resolve_the_calling_account_the_same_way() {
    let emulator = Emulator::new();
    let arn = certificate_authority(&emulator, "defaults").await;

    for (case, granted_with, revoked_with) in [
        ("granted without, revoked with", None, Some(ACCOUNT)),
        ("granted with, revoked without", Some(ACCOUNT), None),
    ] {
        emulator
            .ok(
                case,
                "CreatePermission",
                &grant(&arn, "acm.amazonaws.com", granted_with, &ACTIONS),
            )
            .await;
        let permissions = all_permissions(&emulator, case, &arn).await;
        assert!(
            reported(&permissions, "acm.amazonaws.com", ACCOUNT).is_some(),
            "case {case:?}: the grant is assigned by the calling account, got {permissions:?}"
        );
        emulator
            .ok(
                case,
                "DeletePermission",
                &revoke(&arn, "acm.amazonaws.com", revoked_with),
            )
            .await;
        assert!(
            all_permissions(&emulator, case, &arn).await.is_empty(),
            "case {case:?}: the revocation names the same grant the create made"
        );
    }
}
