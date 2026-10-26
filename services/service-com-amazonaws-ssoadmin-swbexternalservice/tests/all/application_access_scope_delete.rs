// SPDX-License-Identifier: CC0-1.0

//! `DeleteApplicationAccessScope`, end to end over the `awsJson1_1` wire.
//!
//! THE RULE under test: an access scope of an application is identified by its `Scope`
//! NAME, and the delete REMOVES the one scope the request names from the application the
//! request names ("Deletes an IAM Identity Center access scope from an application"), with
//! the authorized targets it carried — so afterwards `GetApplicationAccessScope` of it is
//! a `ResourceNotFoundException` and `ListApplicationAccessScopes` no longer reports it,
//! while every other scope of that application, and every other application, is left
//! exactly as it was. A scope nothing holds is nothing to remove: an `ApplicationArn` this
//! account holds no application for, and a `Scope` the application carries no access scope
//! for (never put, or already deleted), are refused `ResourceNotFoundException` and change
//! nothing — never an empty 200 claiming a scope was deleted. The application itself
//! survives the delete, so a later put stores the scope afresh.
//!
//! Each rule is asserted over a TABLE whose rows differ along what the rule generalises
//! over: the NUMBER of scopes the application carries (one, several, the ten one page of
//! `ListApplicationAccessScopes` can report, more than one page's worth), the segment
//! counts, charset and published 50-character segment bound the model's `Scope` pattern
//! admits, the number of `AuthorizedTargets` each scope was put with (none, one, several),
//! the POSITION of the deleted scope in the collection, and the number of applications
//! holding scopes at once.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes, so the modelled constraints, the `awsJson1_1` envelope and the
//! `X-Amz-Target` routing are exercised rather than bypassed. The scopes are seeded over
//! the wire through `PutApplicationAccessScope`, so nothing here reaches into the store.
//! The harness is the one `ssoadmin_permission_sets.rs` owns.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{assert_refused, call, router, seeded_instance_arn};

/// The OAuth 2.0 application provider a customer managed application is created under.
///
/// SOURCE: `state::BUILT_IN_APPLICATION_PROVIDERS`, the catalogue
/// `ListApplicationProviders`/`DescribeApplicationProvider` report, whose `custom` entry
/// federates over `OAUTH`.
const CUSTOM_PROVIDER_ARN: &str = "arn:aws:sso::aws:applicationProvider/custom";

/// An `ApplicationArn` of the modelled form that no create minted, for the not-found rows.
///
/// SOURCE of the form:
/// `smithy_query(query="operation",operation="DeleteApplicationAccessScope")` —
/// `^arn:aws(-[a-z]{1,5}){0,3}:sso::\d{12}:application/(sso)?ins-[a-zA-Z0-9-.]{16}/apl-[a-zA-Z0-9]{16}$`.
const UNMINTED_APPLICATION_ARN: &str =
    "arn:aws:sso::123456789012:application/ssoins-1111111111111111/apl-1111111111111111";

/// The most scopes one `ListApplicationAccessScopes` page can report, which is the bound
/// of its `Scopes` member's own `length(min: 0, max: 10)`.
///
/// SOURCE: `smithy_query(query="operation",operation="ListApplicationAccessScopes")` —
/// `MaxResults` is `range(min: 1, max: 10)` with `default: 10`.
const SCOPES_PER_PAGE: usize = 10;

/// The `Scope` names the rows are built from, in the order the puts are made.
///
/// They span the forms the model's `Scope` pattern admits
/// (`smithy_query(query="shape",shape="Scope")`:
/// `^([A-Za-z0-9_]{1,50})(:[A-Za-z0-9_]{1,50}){0,1}(:[A-Za-z0-9_]{1,50}){0,1}$`) — one
/// segment, two, the three it allows at most, the whole `[A-Za-z0-9_]` charset, a
/// one-character segment and a segment at the published 50-character bound — and are
/// twelve in number, more than one page of the enumeration can report. Several are names
/// the issue did not cite.
fn scope_catalog() -> Vec<String> {
    let longest_segment = "s".repeat(50);
    [
        "sso:account:access",
        "profile",
        "openid",
        "aws_Z9_0",
        "a",
        longest_segment.as_str(),
        "ab:cd",
        "Z9_:a:B",
        "email",
        "user_impersonation",
        "Read",
        "write_0",
    ]
    .iter()
    .map(|scope| (*scope).to_string())
    .collect()
}

/// One seeded access scope: its `Scope` name and the `AuthorizedTargets` it was put with,
/// `None` for a put that sent none.
type ScopeRow = (String, Option<Vec<String>>);

/// One application the tables seed: its minted `ApplicationArn` and the scopes put on it.
type SeededApplication = (String, Vec<ScopeRow>);

/// One row of the delete-then-restore table: the case name, the `AuthorizedTargets` the
/// scope was first put with, and the ones it is put back with after the delete.
type RedeleteRow = (&'static str, Option<Vec<String>>, Option<Vec<String>>);

/// A router over a fresh engine and the ARN of the instance it seeded.
async fn fixture() -> (MultiServiceRouter, String) {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    (router, instance_arn)
}

/// Create one customer managed application over the wire and return its minted ARN.
async fn create_application(
    router: &MultiServiceRouter,
    instance_arn: &str,
    name: &str,
) -> String {
    let (status, created) = call(
        router,
        "CreateApplication",
        &json!({
            "InstanceArn": instance_arn,
            "ApplicationProviderArn": CUSTOM_PROVIDER_ARN,
            "Name": name,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateApplication {name:?}: expected 200, got {status} {created}"
    );
    created["ApplicationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateApplication {name:?} minted no ARN: {created}"))
        .to_string()
}

/// `PutApplicationAccessScope`: `targets` is `None` for a put that sends no
/// `AuthorizedTargets` at all.
async fn put_scope(
    router: &MultiServiceRouter,
    application_arn: &str,
    scope: &str,
    targets: Option<&[String]>,
) -> (u16, Value) {
    let mut request = json!({"ApplicationArn": application_arn, "Scope": scope});
    if let Some(targets) = targets {
        request["AuthorizedTargets"] = json!(targets);
    }
    call(router, "PutApplicationAccessScope", &request).await
}

/// The same put, which must succeed.
async fn put_scope_ok(
    router: &MultiServiceRouter,
    application_arn: &str,
    scope: &str,
    targets: Option<&[String]>,
) {
    let (status, body) = put_scope(router, application_arn, scope, targets).await;
    assert_eq!(
        status, 200,
        "the put of {scope:?} must answer 200, got {status} {body}"
    );
}

/// `GetApplicationAccessScope` for one scope of one application.
async fn get_scope(
    router: &MultiServiceRouter,
    application_arn: &str,
    scope: &str,
) -> (u16, Value) {
    call(
        router,
        "GetApplicationAccessScope",
        &json!({"ApplicationArn": application_arn, "Scope": scope}),
    )
    .await
}

/// `DeleteApplicationAccessScope` for one scope of one application.
async fn delete_scope(
    router: &MultiServiceRouter,
    application_arn: &str,
    scope: &str,
) -> (u16, Value) {
    call(
        router,
        "DeleteApplicationAccessScope",
        &json!({"ApplicationArn": application_arn, "Scope": scope}),
    )
    .await
}

/// The body a get of `scope` must answer when `targets` were put for it.
fn expected_scope_body(scope: &str, targets: Option<&[String]>) -> Value {
    match targets {
        // `ScopeTargets` is `length 1..10`: a scope put without authorized targets is
        // reported with the member ABSENT, not with an empty list.
        None => json!({"Scope": scope}),
        Some(targets) => json!({"Scope": scope, "AuthorizedTargets": targets}),
    }
}

/// Every `ScopeDetails` `ListApplicationAccessScopes` reports for an application, following
/// `NextToken` to the end of the enumeration.
async fn list_all_scopes(
    router: &MultiServiceRouter,
    application_arn: &str,
    case: &str,
) -> Vec<Value> {
    let mut collected: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    for _ in 0..64 {
        let mut request = json!({"ApplicationArn": application_arn});
        if let Some(token) = &next_token {
            request["NextToken"] = json!(token);
        }
        let (status, body) = call(router, "ListApplicationAccessScopes", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListApplicationAccessScopes expected 200, got {status} {body}"
        );
        collected.extend(
            body["Scopes"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: Scopes must be a list, got {body}"))
                .clone(),
        );
        match body["NextToken"].as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => return collected,
        }
    }
    panic!("case {case:?}: the scope enumeration must terminate");
}

/// Assert one call was refused `ResourceNotFoundException`, through the crate's shared
/// refusal assertion.
fn assert_not_found(case: &str, what: &str, status: u16, body: &Value) {
    assert_refused(case, what, "ResourceNotFoundException", status, body);
}

/// A pool of `ScopeTarget` values of the modelled form, built from resources this account
/// really holds: the instance itself and applications created here.
///
/// SOURCE of the form: `smithy_query(query="shape",shape="ScopeTarget")` —
/// `^arn:aws(-[a-z]{1,5}){0,3}:sso::(\d{12}:application/(sso)?ins-[a-zA-Z0-9-.]{16}/apl-[a-zA-Z0-9]{16}|:instance/(sso)?ins-[a-zA-Z0-9-.]{16})$`.
async fn target_pool(
    router: &MultiServiceRouter,
    instance_arn: &str,
    label: &str,
) -> Vec<String> {
    vec![
        instance_arn.to_string(),
        create_application(router, instance_arn, &format!("{label}-target-a")).await,
        create_application(router, instance_arn, &format!("{label}-target-b")).await,
    ]
}

/// The scopes one application is seeded with: the first `count` names of the catalog, each
/// with a different number of authorized targets (none, one, two, three), so no row shares
/// its target shape with its neighbours.
fn scope_rows(count: usize, pool: &[String]) -> Vec<ScopeRow> {
    scope_catalog()
        .into_iter()
        .take(count)
        .enumerate()
        .map(|(index, scope)| {
            let targets = index % 4;
            let targets = (targets > 0).then(|| pool[..targets].to_vec());
            (scope, targets)
        })
        .collect()
}

/// Seed one application with `rows` and return its ARN.
async fn application_with_scopes(
    router: &MultiServiceRouter,
    instance_arn: &str,
    name: &str,
    rows: &[ScopeRow],
) -> String {
    let application_arn = create_application(router, instance_arn, name).await;
    for (scope, targets) in rows {
        put_scope_ok(router, &application_arn, scope, targets.as_deref()).await;
    }
    application_arn
}

/// THE RULE: the delete removes exactly the scope it names — the get of it becomes a
/// not-found and the enumeration drops it, while every scope left keeps the targets its
/// put stored. Asserted while deleting the scopes of one application ONE AT A TIME, so
/// every position in the collection is the deleted one in some row, over collections of
/// one, several, a full page and more than a page of scopes.
#[tokio::test]
async fn a_delete_removes_exactly_the_scope_it_names() {
    let (router, instance_arn) = fixture().await;
    let pool = target_pool(&router, &instance_arn, "removes").await;

    for count in [1usize, 3, SCOPES_PER_PAGE, 12] {
        let rows = scope_rows(count, &pool);
        let application_arn = application_with_scopes(
            &router,
            &instance_arn,
            &format!("scope-delete-{count}"),
            &rows,
        )
        .await;

        // Delete in the order the puts were made; after each one, every scope deleted so
        // far must be gone and every scope not yet deleted must read back unchanged.
        for (deleted, (scope, targets)) in rows.iter().enumerate() {
            let case = format!("{count} scopes, deleting {scope:?}");

            // The scope resolves right up to the delete.
            let (status, before) = get_scope(&router, &application_arn, scope).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the get before the delete expected 200, got {status} {before}"
            );
            assert_eq!(
                before,
                expected_scope_body(scope, targets.as_deref()),
                "case {case:?}: the get before the delete must report the stored targets, \
                 got {before}"
            );

            let (status, body) = delete_scope(&router, &application_arn, scope).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the delete expected 200, got {status} {body}"
            );
            assert_eq!(
                body,
                json!({}),
                "case {case:?}: the delete's output is modelled as Unit, got {body}"
            );

            // The deleted scope is GONE for every reader.
            let (status, body) = get_scope(&router, &application_arn, scope).await;
            assert_not_found(&case, "the get after the delete", status, &body);

            let listed = list_all_scopes(&router, &application_arn, &case).await;
            let mut expected: Vec<Value> = rows[deleted + 1..]
                .iter()
                .map(|(scope, targets)| expected_scope_body(scope, targets.as_deref()))
                .collect();
            // The enumeration reports the survivors in ascending `Scope` order.
            expected.sort_by_key(|entry| entry["Scope"].as_str().unwrap_or_default().to_string());
            assert_eq!(
                listed, expected,
                "case {case:?}: the enumeration must report exactly the surviving scopes, \
                 got {listed:?}"
            );

            // Every survivor keeps the targets its own put stored: the delete reaches one
            // key, not the neighbours' values.
            for (other, other_targets) in &rows[deleted + 1..] {
                let (status, body) = get_scope(&router, &application_arn, other).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the surviving scope {other:?} expected 200, got {status} \
                     {body}"
                );
                assert_eq!(
                    body,
                    expected_scope_body(other, other_targets.as_deref()),
                    "case {case:?}: the surviving scope {other:?} must keep its targets, got \
                     {body}"
                );
            }
        }

        // Every scope deleted: the application carries none, which is an EMPTY page
        // rather than a refusal — the application itself survived the deletes.
        let case = format!("{count} scopes, all deleted");
        let listed = list_all_scopes(&router, &application_arn, &case).await;
        assert!(
            listed.is_empty(),
            "case {case:?}: an application whose every scope was deleted carries none, got \
             {listed:?}"
        );
    }
}

/// THE RULE: a scope already deleted is a scope nothing holds, so a SECOND delete of it is
/// refused `ResourceNotFoundException` rather than answered with an empty 200 — and the
/// application survives, so a put after the delete stores the scope afresh.
#[tokio::test]
async fn a_second_delete_of_the_same_scope_is_refused_and_a_put_restores_it() {
    let (router, instance_arn) = fixture().await;
    let pool = target_pool(&router, &instance_arn, "second").await;

    // Rows differing in the targets the scope was put with, and in the targets it is put
    // back with: no targets, one, and the three the pool holds.
    let rows: Vec<RedeleteRow> = vec![
        ("no targets, restored with none", None, None),
        (
            "no targets, restored with one",
            None,
            Some(pool[..1].to_vec()),
        ),
        (
            "one target, restored with none",
            Some(pool[..1].to_vec()),
            None,
        ),
        (
            "three targets, restored with two",
            Some(pool[..3].to_vec()),
            Some(pool[..2].to_vec()),
        ),
    ];

    for (index, (case, first, again)) in rows.iter().enumerate() {
        let scope = &scope_catalog()[index];
        let application_arn = application_with_scopes(
            &router,
            &instance_arn,
            &format!("scope-redelete-{index}"),
            &[(scope.clone(), first.clone())],
        )
        .await;

        let (status, body) = delete_scope(&router, &application_arn, scope).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first delete expected 200, got {status} {body}"
        );

        let (status, body) = delete_scope(&router, &application_arn, scope).await;
        assert_not_found(case, "a second delete of the same scope", status, &body);

        // The refused second delete changed nothing: the scope is still absent.
        let (status, body) = get_scope(&router, &application_arn, scope).await;
        assert_not_found(case, "the get after the second delete", status, &body);

        // The APPLICATION survived, so the scope can be put afresh — with targets of its
        // own, not the ones the deleted scope carried.
        put_scope_ok(&router, &application_arn, scope, again.as_deref()).await;
        let (status, body) = get_scope(&router, &application_arn, scope).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the get after the re-put expected 200, got {status} {body}"
        );
        assert_eq!(
            body,
            expected_scope_body(scope, again.as_deref()),
            "case {case:?}: the re-put must store the scope afresh, got {body}"
        );
    }
}

/// THE RULE: a scope nothing holds is nothing to delete — an `ApplicationArn` this account
/// holds no application for, and a `Scope` the application carries no access scope for,
/// are refused `ResourceNotFoundException` and remove nothing.
#[tokio::test]
async fn a_delete_of_a_scope_nothing_holds_is_refused_and_removes_nothing() {
    let (router, instance_arn) = fixture().await;
    let pool = target_pool(&router, &instance_arn, "unknown").await;

    let stored = scope_rows(3, &pool);
    let application_arn =
        application_with_scopes(&router, &instance_arn, "scope-unknown", &stored).await;
    // A second application carrying one of the SAME scope names, so a delete aimed at the
    // wrong application is seen to leave it alone.
    let neighbour_scope = stored[0].0.clone();
    // It also carries one scope name this application holds none of, so a delete of THAT
    // name against this application is a not-found while the neighbour keeps it.
    let neighbour_only_scope = scope_catalog()[11].clone();
    let neighbour = application_with_scopes(
        &router,
        &instance_arn,
        "scope-unknown-neighbour",
        &[
            (neighbour_scope.clone(), Some(pool[..2].to_vec())),
            (neighbour_only_scope.clone(), None),
        ],
    )
    .await;

    let unstored_scope = scope_catalog()[SCOPES_PER_PAGE].clone();
    let cases: Vec<(&str, String, String)> = vec![
        (
            "an ApplicationArn no create minted",
            UNMINTED_APPLICATION_ARN.to_string(),
            stored[0].0.clone(),
        ),
        (
            "a scope the application never carried",
            application_arn.clone(),
            unstored_scope.clone(),
        ),
        (
            "a scope only the OTHER application carries",
            application_arn.clone(),
            neighbour_only_scope.clone(),
        ),
        (
            "an unminted application and a scope nothing stored",
            UNMINTED_APPLICATION_ARN.to_string(),
            unstored_scope,
        ),
    ];

    for (case, arn, scope) in &cases {
        let (status, body) = delete_scope(&router, arn, scope).await;
        assert_not_found(case, "the delete", status, &body);
    }

    // Every scope both applications hold is still exactly where it was.
    for (scope, targets) in &stored {
        let (status, body) = get_scope(&router, &application_arn, scope).await;
        assert_eq!(
            status, 200,
            "the refused deletes must leave {scope:?} readable, got {status} {body}"
        );
        assert_eq!(
            body,
            expected_scope_body(scope, targets.as_deref()),
            "the refused deletes must leave {scope:?} unchanged, got {body}"
        );
    }
    for (scope, targets) in [
        (&neighbour_scope, Some(&pool[..2])),
        (&neighbour_only_scope, None),
    ] {
        let (status, body) = get_scope(&router, &neighbour, scope).await;
        assert_eq!(
            status, 200,
            "the refused deletes must leave the neighbour's {scope:?} readable, got {status} \
             {body}"
        );
        assert_eq!(
            body,
            expected_scope_body(scope, targets),
            "the refused deletes must leave the neighbour's {scope:?} unchanged, got {body}"
        );
    }
}

/// THE RULE: a scope is keyed by its name ON ONE APPLICATION, so deleting it from one
/// application leaves the same scope name on every other application untouched — asserted
/// with several applications carrying the same names at once, deleted one at a time.
#[tokio::test]
async fn a_delete_reaches_one_application_only() {
    let (router, instance_arn) = fixture().await;
    let pool = target_pool(&router, &instance_arn, "isolation").await;

    // The same three scope names on each of four applications, with per-application
    // targets so a leak between them is visible as a value and not only as a presence.
    let names = &scope_catalog()[..3];
    let mut applications: Vec<SeededApplication> = Vec::new();
    for index in 0..4usize {
        let rows: Vec<ScopeRow> = names
            .iter()
            .enumerate()
            .map(|(position, scope)| {
                let count = (index + position) % 4;
                (
                    scope.clone(),
                    (count > 0).then(|| pool[..count].to_vec()),
                )
            })
            .collect();
        let arn = application_with_scopes(
            &router,
            &instance_arn,
            &format!("scope-isolation-{index}"),
            &rows,
        )
        .await;
        applications.push((arn, rows));
    }

    // The (application, scope) pairs deleted so far: everything else must still read back
    // exactly as its own put stored it.
    let mut deleted: Vec<(usize, String)> = Vec::new();
    for index in 0..applications.len() {
        let scope = names[index % names.len()].clone();
        let case = format!("deleting {scope:?} from application {index}");
        let (arn, _) = &applications[index];
        let (status, body) = delete_scope(&router, arn, &scope).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the delete expected 200, got {status} {body}"
        );
        deleted.push((index, scope.clone()));

        // Every pair the deletes have NOT reached still reads back with its own targets,
        // and only the deleted pairs are gone — a delete is keyed by (application, scope).
        for (other, (other_arn, rows)) in applications.iter().enumerate() {
            for (name, targets) in rows {
                let (status, body) = get_scope(&router, other_arn, name).await;
                if deleted.iter().any(|(app, gone)| *app == other && gone == name) {
                    assert_not_found(
                        &case,
                        &format!("the get of {name:?} on application {other}"),
                        status,
                        &body,
                    );
                    continue;
                }
                assert_eq!(
                    status, 200,
                    "case {case:?}: application {other} must still carry {name:?}, got \
                     {status} {body}"
                );
                assert_eq!(
                    body,
                    expected_scope_body(name, targets.as_deref()),
                    "case {case:?}: application {other} must keep its own targets for \
                     {name:?}, got {body}"
                );
            }
        }
    }
}
