// SPDX-License-Identifier: CC0-1.0

//! The connection-alias lifecycle, end to end over the awsJson1_1 wire.
//!
//! Every case drives a real `MultiServiceRouter` with this service mounted on one shared
//! engine (the router, the wire helper and the clock helper are
//! [`crate::workspace_lifecycle`]'s), so each assertion is about what a client would see:
//! the request is serialized, routed, validated, handled and serialized back.
//!
//! The rules under test, all of them about aliases this emulator persisted rather than
//! about any one recorded request:
//!
//! * `CreateConnectionAlias` mints an `AliasId` matching the model's
//!   `^wsca-[0-9a-z]{8,63}$` for ANY `ConnectionString` the model admits, persists the
//!   connection string as supplied, and hands the alias back ready: a describe issued
//!   straight after the create reports `CREATED`, and reading it again does not move it.
//! * Every described alias reports a non-empty `Associations` list
//!   (`ConnectionAliasAssociationList` is `len 1..25`), whose entries carry a modelled
//!   `AssociationStatus`; an alias associated with no directory reports exactly one
//!   entry, `NOT_ASSOCIATED`, with no member naming a directory, an account or a
//!   connection identifier.
//! * A connection string a stored alias already holds cannot be taken by a second alias
//!   (`ResourceAlreadyExistsException`), and the hold is released when the alias is gone.
//! * `DeleteConnectionAlias` moves the persisted alias to `DELETING` and the clock drops
//!   it; an `AliasId` naming no stored alias is `ResourceNotFoundException`.
//! * `DescribeConnectionAliases` reports stored aliases filtered by the identifiers the
//!   request names, paginated over a stable creation order.

use serde_json::{json, Value};
use service_com_amazonaws_workspaces_workspacesservice::ops::types::ASSOCIATION_STATUS_VALUES;

use crate::workspace_lifecycle::{call, emulator_with_engine, settle};

/// Does `alias_id` match the model's `ConnectionAliasId`?
///
/// From `smithy_query(query="operation", operation="CreateConnectionAlias")`:
/// `AliasId` is `len 13..68, pattern ^wsca-[0-9a-z]{8,63}$`.
fn is_modelled_alias_id(alias_id: &str) -> bool {
    let Some(suffix) = alias_id.strip_prefix("wsca-") else {
        return false;
    };
    (8..=63).contains(&suffix.len())
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

/// Every connection string the round-trip table creates an alias for, with the case
/// name each assertion reports.
///
/// The rows span what `CreateConnectionAliasRequest$ConnectionString` admits — from
/// `smithy_query(query="operation", operation="CreateConnectionAlias")`, `len 1..255`
/// with `pattern ^[.0-9a-zA-Z\-]{1,255}$` — along label count, length (both published
/// bounds), digits, hyphens and letter case.
fn connection_string_table() -> Vec<(&'static str, String)> {
    vec![
        ("a fully qualified domain name", "www.example.com".to_string()),
        (
            "several labels",
            "a.b.c.d.workspaces.example.com".to_string(),
        ),
        ("the shortest the model admits", "x".to_string()),
        (
            "digits and hyphens",
            "cross-region-01.example-site.com".to_string(),
        ),
        ("mixed case", "WWW.Example-TEST.COM".to_string()),
        // The longest the model admits: 255 characters exactly.
        ("the longest the model admits", format!("{}.com", "a".repeat(251))),
    ]
}

/// Create one alias for `connection_string` and return the minted `AliasId`.
async fn create_alias(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    connection_string: &str,
) -> String {
    let (status, body) = call(
        router,
        "CreateConnectionAlias",
        &json!({ "ConnectionString": connection_string }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateConnectionAlias must answer 200, got {status} body {body:?}"
    );
    let alias_id = body["AliasId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a minted AliasId, got {body:?}"))
        .to_string();
    assert!(
        is_modelled_alias_id(&alias_id),
        "case {case:?}: minted id must match ^wsca-[0-9a-z]{{8,63}}$, got {alias_id:?}"
    );
    alias_id
}

/// Describe with `filters` and return the reported `ConnectionAlias` list.
async fn describe_aliases(
    router: &smithy_server_runtime::MultiServiceRouter,
    filters: &Value,
) -> Vec<Value> {
    let (status, body) = call(router, "DescribeConnectionAliases", filters).await;
    assert_eq!(
        status, 200,
        "case {filters:?}: DescribeConnectionAliases must answer 200, body {body:?}"
    );
    body["ConnectionAliases"]
        .as_array()
        .unwrap_or_else(|| panic!("case {filters:?}: a ConnectionAliases list, got {body:?}"))
        .clone()
}

/// The one alias `alias_id` names, or `None` once it is no longer stored.
async fn describe_one(
    router: &smithy_server_runtime::MultiServiceRouter,
    alias_id: &str,
) -> Option<Value> {
    let reported = describe_aliases(router, &json!({ "AliasIds": [alias_id] })).await;
    assert!(
        reported.len() <= 1,
        "case {alias_id}: one AliasId can match at most one alias, got {reported:?}"
    );
    reported.into_iter().next()
}

/// Assert `described` reports the `Associations` list AWS reports for an alias that is
/// associated with no directory.
///
/// The list is never absent and never empty: `ConnectionAlias$Associations` targets
/// `ConnectionAliasAssociationList`, `len 1..25`. For an alias no directory association
/// names, the single entry's `AssociationStatus` is `NOT_ASSOCIATED` — one of the
/// model's `AssociationStatus` values — and the three members that would name the
/// association (`AssociatedAccountId`, `ResourceId`, `ConnectionIdentifier`) are absent,
/// because there is no association to name one.
fn assert_not_associated(case: &str, described: &Value) {
    let associations = described["Associations"].as_array().unwrap_or_else(|| {
        panic!("case {case:?}: every described alias reports an Associations list, got {described:?}")
    });
    assert!(
        (1..=25).contains(&associations.len()),
        "case {case:?}: Associations is len 1..25, got {associations:?}"
    );
    for association in associations {
        let status = association["AssociationStatus"].as_str().unwrap_or_default();
        assert!(
            ASSOCIATION_STATUS_VALUES.contains(&status),
            "case {case:?}: AssociationStatus must be one of {ASSOCIATION_STATUS_VALUES:?}, got {association:?}"
        );
    }
    assert_eq!(
        associations.len(),
        1,
        "case {case:?}: an alias associated with no directory reports one association, got {associations:?}"
    );
    let only = &associations[0];
    assert_eq!(
        only["AssociationStatus"].as_str(),
        Some("NOT_ASSOCIATED"),
        "case {case:?}: an alias associated with no directory is NOT_ASSOCIATED, got {only:?}"
    );
    for member in ["AssociatedAccountId", "ResourceId", "ConnectionIdentifier"] {
        assert!(
            only[member].is_null(),
            "case {case:?}: {member} names an association this alias does not have, got {only:?}"
        );
    }
}

/// The wire error code of a refused call (`__type` carries `<namespace>#<Shape>`).
fn error_code(body: &Value) -> String {
    body["__type"].as_str().unwrap_or_default().to_string()
}

#[tokio::test]
async fn created_aliases_are_persisted_and_described_back() {
    let (router, engine) = emulator_with_engine();
    let mut created: Vec<(&str, String, String)> = Vec::new();

    for (case, connection_string) in connection_string_table() {
        let alias_id = create_alias(&router, case, &connection_string).await;

        // The alias is handed back ready: the describe that follows the create
        // reports it CREATED, with no transient state to poll through.
        let described = describe_one(&router, &alias_id)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: alias {alias_id} must be stored"));
        assert_eq!(
            described["State"].as_str(),
            Some("CREATED"),
            "case {case:?}: a created alias is CREATED straight away, got {described:?}"
        );
        assert_eq!(
            described["ConnectionString"].as_str(),
            Some(connection_string.as_str()),
            "case {case:?}: the connection string must be reported as supplied, got {described:?}"
        );
        assert_eq!(
            described["AliasId"].as_str(),
            Some(alias_id.as_str()),
            "case {case:?}: the alias must report the id the create minted, got {described:?}"
        );
        assert_eq!(
            described["OwnerAccountId"].as_str(),
            Some(engine::emulator_account_id()),
            "case {case:?}: the owner is the emulator's account, got {described:?}"
        );
        assert_not_associated(case, &described);

        // Describing again reports the same alias: a read moves nothing.
        let reread = describe_one(&router, &alias_id)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: alias {alias_id} must still be stored"));
        assert_eq!(
            reread, described,
            "case {case:?}: describing an alias twice reports the same alias"
        );

        created.push((case, connection_string, alias_id));
    }

    let minted: std::collections::BTreeSet<&String> =
        created.iter().map(|(_, _, id)| id).collect();
    assert_eq!(
        minted.len(),
        created.len(),
        "every minted AliasId must be distinct, got {created:?}"
    );

    settle(&engine);

    for (case, connection_string, alias_id) in &created {
        let described = describe_one(&router, alias_id)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: alias {alias_id} must still be stored"));
        assert_eq!(
            described["State"].as_str(),
            Some("CREATED"),
            "case {case:?}: a created alias stays CREATED, got {described:?}"
        );
        assert_eq!(
            described["ConnectionString"].as_str(),
            Some(connection_string.as_str()),
            "case {case:?}: the connection string must survive settling, got {described:?}"
        );
        assert_not_associated(case, &described);
    }

    // No filter: every stored alias, in creation order.
    let reported: Vec<String> = describe_aliases(&router, &json!({}))
        .await
        .iter()
        .map(|alias| alias["AliasId"].as_str().unwrap_or_default().to_string())
        .collect();
    let expected: Vec<String> = created.iter().map(|(_, _, id)| id.clone()).collect();
    assert_eq!(
        reported, expected,
        "DescribeConnectionAliases with no filter reports every stored alias in creation order"
    );
}

#[tokio::test]
async fn a_connection_string_is_held_by_at_most_one_alias() {
    let (router, engine) = emulator_with_engine();

    for (case, connection_string) in connection_string_table() {
        let alias_id = create_alias(&router, case, &connection_string).await;

        // The same connection string, and the same one spelled in another case: a
        // fully qualified domain name is case-insensitive (RFC 4343), so both name
        // the string the stored alias already holds.
        for collision in [
            connection_string.clone(),
            connection_string.to_uppercase(),
            connection_string.to_lowercase(),
        ] {
            let (status, body) = call(
                &router,
                "CreateConnectionAlias",
                &json!({ "ConnectionString": collision }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}/{collision:?}: a taken connection string must be refused, body {body:?}"
            );
            assert!(
                error_code(&body).ends_with("ResourceAlreadyExistsException"),
                "case {case:?}/{collision:?}: expected ResourceAlreadyExistsException, got {body:?}"
            );
        }

        // Once the alias is gone the connection string is free again, and a fresh
        // alias for it is a DIFFERENT alias.
        let (status, body) = call(
            &router,
            "DeleteConnectionAlias",
            &json!({ "AliasId": alias_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: deleting a stored alias must answer 200, body {body:?}"
        );
        settle(&engine);

        let recreated = create_alias(&router, case, &connection_string).await;
        assert_ne!(
            recreated, alias_id,
            "case {case:?}: recreating a deleted alias mints a new id"
        );
    }
}

#[tokio::test]
async fn deleting_an_alias_moves_it_through_deleting_and_drops_it() {
    let (router, engine) = emulator_with_engine();
    let alias_id = create_alias(&router, "delete lifecycle", "redirect.example.com").await;
    settle(&engine);
    let described = describe_one(&router, &alias_id)
        .await
        .expect("a created alias is reported");
    assert_eq!(
        described["State"].as_str(),
        Some("CREATED"),
        "an alias is CREATED before its deletion starts, got {described:?}"
    );

    let (status, body) = call(
        &router,
        "DeleteConnectionAlias",
        &json!({ "AliasId": alias_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteConnectionAlias must answer 200 for a stored alias, body {body:?}"
    );

    // Deletion is asynchronous: DELETING until the clock drops the alias.
    let described = describe_one(&router, &alias_id)
        .await
        .expect("an alias being deleted is still reported");
    assert_eq!(
        described["State"].as_str(),
        Some("DELETING"),
        "an alias being deleted is DELETING, got {described:?}"
    );
    // Deleting the alias does not associate it with anything: the list is still the
    // one NOT_ASSOCIATED entry.
    assert_not_associated("an alias being deleted", &described);

    settle(&engine);
    assert_eq!(
        describe_one(&router, &alias_id).await,
        None,
        "a deleted alias is no longer reported"
    );

    // And it is gone from the store: deleting it again, and deleting an id that was
    // never minted, are both ResourceNotFoundException.
    for (case, missing) in [
        ("an alias already deleted", alias_id.as_str()),
        ("an id never minted", "wsca-ffffffff"),
    ] {
        let (status, body) = call(
            &router,
            "DeleteConnectionAlias",
            &json!({ "AliasId": missing }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: deleting {missing} must be refused, body {body:?}"
        );
        assert!(
            error_code(&body).ends_with("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {body:?}"
        );
        assert_eq!(
            body["ResourceId"].as_str(),
            Some(missing),
            "case {case:?}: the refusal names the alias asked for, got {body:?}"
        );
    }
}

#[tokio::test]
async fn describe_filters_and_paginates_over_creation_order() {
    let (router, engine) = emulator_with_engine();
    let mut alias_ids = Vec::new();
    for index in 0..7 {
        let connection_string = format!("alias-{index}.example.com");
        alias_ids.push(create_alias(&router, "pagination", &connection_string).await);
    }
    settle(&engine);

    // Every page size from one to more than the store holds reports the same aliases,
    // in creation order, exactly once.
    for limit in [1usize, 2, 3, 7, 25] {
        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut filters = json!({ "Limit": limit });
            if let Some(token) = &next_token {
                filters["NextToken"] = json!(token);
            }
            let (status, body) = call(&router, "DescribeConnectionAliases", &filters).await;
            assert_eq!(
                status, 200,
                "case limit {limit}: page {pages} must answer 200, body {body:?}"
            );
            let page = body["ConnectionAliases"]
                .as_array()
                .unwrap_or_else(|| panic!("case limit {limit}: a list, got {body:?}"))
                .clone();
            assert!(
                page.len() <= limit,
                "case limit {limit}: a page must hold at most Limit aliases, got {page:?}"
            );
            walked.extend(
                page.iter()
                    .map(|alias| alias["AliasId"].as_str().unwrap_or_default().to_string()),
            );
            next_token = body["NextToken"].as_str().map(str::to_string);
            pages += 1;
            assert!(
                pages <= alias_ids.len() + 1,
                "case limit {limit}: pagination must terminate, walked {walked:?}"
            );
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, alias_ids,
            "case limit {limit}: paginating reports every stored alias once, in creation order"
        );
    }

    // `AliasIds` reports exactly the aliases it names, in creation order.
    let wanted = vec![alias_ids[4].clone(), alias_ids[1].clone()];
    let reported: Vec<String> = describe_aliases(&router, &json!({ "AliasIds": wanted }))
        .await
        .iter()
        .map(|alias| alias["AliasId"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        reported,
        vec![alias_ids[1].clone(), alias_ids[4].clone()],
        "AliasIds reports the named aliases in creation order"
    );

    // `ResourceId` filters on the directory an alias is associated with. Nothing
    // associates an alias in this emulator (`AssociateConnectionAlias` is still a
    // scaffold stub), so no stored alias matches any directory.
    assert_eq!(
        describe_aliases(&router, &json!({ "ResourceId": "d-1234567890" })).await,
        Vec::<Value>::new(),
        "no stored alias is associated with a directory"
    );

    // A token this service did not mint is not a page boundary.
    let (status, body) = call(
        &router,
        "DescribeConnectionAliases",
        &json!({ "NextToken": "not-a-token" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "an unminted NextToken must be refused, body {body:?}"
    );
    assert!(
        error_code(&body).ends_with("InvalidParameterValuesException"),
        "expected InvalidParameterValuesException, got {body:?}"
    );
}

#[tokio::test]
async fn a_connection_string_outside_the_modelled_constraint_is_refused() {
    let (router, _engine) = emulator_with_engine();

    // One past each published bound of `ConnectionString` (`len 1..255`,
    // `pattern ^[.0-9a-zA-Z\-]{1,255}$`), plus a character the pattern excludes.
    for (case, connection_string) in [
        ("empty", String::new()),
        ("256 characters", "a".repeat(256)),
        ("an underscore", "www.exa_mple.com".to_string()),
    ] {
        let (status, body) = call(
            &router,
            "CreateConnectionAlias",
            &json!({ "ConnectionString": connection_string }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a connection string outside the constraint must be refused, body {body:?}"
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {case:?}: expected ValidationException, got {body:?}"
        );
    }
}
