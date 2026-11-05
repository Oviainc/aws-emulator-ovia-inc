// SPDX-License-Identifier: CC0-1.0

//! The account alias, end to end over the awsQuery wire:
//! `CreateAccountAlias` -> `ListAccountAliases` -> `DeleteAccountAlias`.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one
//! shared `InMemoryEngine`, exactly as the production router does, so what is under
//! test is the HTTP behaviour (status, awsQuery error code, XML members) of the
//! alias the account PERSISTED, never a handler called directly. The awsQuery
//! harness is the one `tests/all/user_lifecycle.rs` owns and the other suites in
//! this crate share, so a request is built and read exactly one way.
//!
//! Each suite is a TABLE whose rows differ along what the rule being asserted
//! generalises over: the alias alphabet (letters, digits, dashes), its length at
//! and past each published bound, how many aliases have been created in turn, the
//! page size, and which optional pagination parameters the listing request
//! carries. No single request is the specification.

use smithy_server_runtime::MultiServiceRouter;

use crate::role_lifecycle::{borrowed, param};
use crate::user_lifecycle::{call, element, elements, error_code, router};

/// The alias catalog: rows spanning the whole alphabet `accountAliasType` admits
/// and both published length bounds. Source: `AccountAlias` on
/// `smithy_query(query="operation", operation="CreateAccountAlias")` —
/// `accountAliasType`, length 3..63, pattern
/// `^[a-z0-9]([a-z0-9]|-(?!-)){1,61}[a-z0-9]$` ("lowercase letters, digits, and
/// dashes", no leading, trailing or doubled dash).
fn alias_catalog() -> Vec<String> {
    vec![
        // The published minimum length, letters only.
        "abc".to_string(),
        // Digits only, and a mix.
        "0123456789".to_string(),
        "a1b2c3".to_string(),
        // Single dashes, at one and at several positions.
        "a-1".to_string(),
        "my-account-alias".to_string(),
        // The published maximum length: 63 characters.
        format!("a{}c", "b".repeat(61)),
    ]
}

/// The aliases a `ListAccountAliases` response reports, in document order.
///
/// `accountAliasListType` is a wrapped list, so each entry arrives as the one
/// `<member>` element inside `<AccountAliases>`, and that list is the only one in
/// this response shape.
fn listed_aliases(body: &str) -> Vec<String> {
    elements(body, "member")
}

/// Create one alias, asserting the fixture create succeeded.
async fn create_alias(router: &MultiServiceRouter, alias: &str) {
    let (status, body) = call(router, "CreateAccountAlias", &[("AccountAlias", alias)]).await;
    assert_eq!(
        status, 200,
        "case {alias:?}: the fixture CreateAccountAlias must succeed, got {status}: {body}"
    );
}

/// The aliases a plain `ListAccountAliases` reports, asserting the listing
/// succeeded.
async fn current_aliases(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (status, body) = call(router, "ListAccountAliases", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListAccountAliases to answer 200, got {status}: {body}"
    );
    listed_aliases(&body)
}

/// RULE: a reporting operation over a single-valued account-wide resource reports
/// exactly what the account has persisted for it — the value a create stored, and
/// nothing once a delete has cleared it — rather than a value recomputed from the
/// reading request.
#[tokio::test]
async fn list_account_aliases_reports_the_alias_the_account_persisted() {
    for alias in alias_catalog() {
        let case = format!("{} chars: {alias}", alias.len());
        // A fresh account per row: the alias is account-wide, so each row starts
        // from an account holding none.
        let router = router();
        assert_eq!(
            current_aliases(&router, &case).await,
            Vec::<String>::new(),
            "case {case:?}: an account that created no alias holds none"
        );

        create_alias(&router, &alias).await;
        let (status, body) = call(&router, "ListAccountAliases", &[]).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {body}");
        assert_eq!(
            listed_aliases(&body),
            vec![alias.clone()],
            "case {case:?}: expected the stored alias, got {:?}",
            listed_aliases(&body)
        );
        // The account holds one alias, so the page is complete: "a flag that
        // indicates whether there are more items to return".
        assert_eq!(
            element(&body, "IsTruncated"),
            "false",
            "case {case:?}: expected IsTruncated false, got {:?}",
            element(&body, "IsTruncated")
        );
        assert_eq!(
            elements(&body, "Marker"),
            Vec::<String>::new(),
            "case {case:?}: an untruncated page carries no Marker, got {:?}",
            elements(&body, "Marker")
        );

        let (status, body) = call(
            &router,
            "DeleteAccountAlias",
            &[("AccountAlias", alias.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the stored alias to be deletable, got {status}: {body}"
        );
        assert_eq!(
            current_aliases(&router, &case).await,
            Vec::<String>::new(),
            "case {case:?}: expected no alias after the delete"
        );
    }
}

/// RULE: the account holds AT MOST ONE alias — "Lists the account alias associated
/// with the Amazon Web Services account (Note: you can have only one)" — so a
/// create REPLACES the one held ("if you create a new alias for your AWS account,
/// the new alias replaces the previous alias"), leaving the listing reporting just
/// the latest.
#[tokio::test]
async fn creating_an_alias_replaces_the_one_the_account_holds() {
    // Rows differ in the number of creates in turn and in the alias each one
    // names. No row names the alias just created, which is a create of a resource
    // that already exists rather than a replacement — see
    // `recreating_the_alias_the_account_already_holds_is_refused`.
    let sequences: &[&[&str]] = &[
        &["first-alias"],
        &["first-alias", "second-alias"],
        &["abc", "abd"],
        &["one", "two", "three", "two"],
        &["0123456789", "a-1", "my-account-alias"],
    ];

    for sequence in sequences {
        let router = router();
        for (position, alias) in sequence.iter().enumerate() {
            let case = format!("{sequence:?} step {position}: {alias}");
            create_alias(&router, alias).await;
            let held = current_aliases(&router, &case).await;
            assert_eq!(
                held,
                vec![(*alias).to_string()],
                "case {case:?}: expected the newest alias alone, got {held:?}"
            );
        }

        // And the aliases replaced along the way are gone: deleting one of them
        // refers to no entity.
        for replaced in sequence.iter().take(sequence.len() - 1) {
            if replaced == sequence.last().expect("a non-empty sequence") {
                continue;
            }
            let (status, body) =
                call(&router, "DeleteAccountAlias", &[("AccountAlias", replaced)]).await;
            assert_eq!(
                status, 404,
                "case {replaced:?}: a replaced alias is no longer held, got {status}: {body}"
            );
        }
    }
}

/// RULE: a create whose resource already exists is refused — the modelled
/// `EntityAlreadyExistsException` (HTTP 409, awsQuery code `EntityAlreadyExists`)
/// — rather than answered as a success, so a client that branches on that refusal
/// learns the resource was not minted again; the account keeps exactly what it
/// held.
#[tokio::test]
async fn recreating_the_alias_the_account_already_holds_is_refused() {
    // Rows span the whole alias catalog (alphabet and both published length
    // bounds), and each row repeats the create twice over, so the refusal is not
    // one-shot.
    for alias in alias_catalog() {
        let case = format!("{} chars: {alias}", alias.len());
        let router = router();
        create_alias(&router, &alias).await;

        for attempt in 0..2 {
            let (status, body) =
                call(&router, "CreateAccountAlias", &[("AccountAlias", alias.as_str())]).await;
            assert_eq!(
                status, 409,
                "case {case:?} attempt {attempt}: expected 409, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "EntityAlreadyExists",
                "case {case:?} attempt {attempt}: expected EntityAlreadyExists, got {:?}",
                error_code(&body)
            );
            assert_eq!(
                element(&body, "Message"),
                format!("The account alias {alias} already exists."),
                "case {case:?} attempt {attempt}: unexpected message, got {:?}",
                element(&body, "Message")
            );
            assert_eq!(
                current_aliases(&router, &case).await,
                vec![alias.clone()],
                "case {case:?} attempt {attempt}: the refusal must leave the held alias alone"
            );
        }
    }
}

/// RULE: the refusal is about what the account HOLDS NOW, so an alias the account
/// no longer holds — because a later create replaced it, or a delete cleared it —
/// is a resource that no longer exists and can be created again.
#[tokio::test]
async fn an_alias_the_account_no_longer_holds_can_be_created_again() {
    // Rows differ in how the account stopped holding the first alias: a create of
    // another alias replaced it, and a delete cleared it.
    for cleared_by_delete in [false, true] {
        let case = format!("cleared by delete: {cleared_by_delete}");
        let router = router();
        create_alias(&router, "released-alias").await;
        if cleared_by_delete {
            let (status, body) = call(
                &router,
                "DeleteAccountAlias",
                &[("AccountAlias", "released-alias")],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the fixture delete must succeed, got {status}: {body}"
            );
        } else {
            create_alias(&router, "holder-alias").await;
        }

        // The released alias is creatable again, and once held it is refused again.
        create_alias(&router, "released-alias").await;
        assert_eq!(
            current_aliases(&router, &case).await,
            vec!["released-alias".to_string()],
            "case {case:?}: expected the re-created alias to be held"
        );
        let (status, body) = call(
            &router,
            "CreateAccountAlias",
            &[("AccountAlias", "released-alias")],
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected the re-created alias to be refused once held, got \
             {status}: {body}"
        );
    }
}

/// RULE: a delete names the resource to remove, so a name the account does not
/// hold refers to no entity — the modelled `NoSuchEntityException` (HTTP 404,
/// awsQuery code `NoSuchEntity`) — and removes nothing, so the value the account
/// DOES hold survives a mistaken name.
#[tokio::test]
async fn deleting_an_alias_the_account_does_not_hold_is_refused() {
    // Rows: an account holding no alias at all, and an account holding one, each
    // asked to delete names from the catalog it does not hold.
    let held_cases: &[Option<&str>] = &[None, Some("held-alias")];

    for held in held_cases {
        let router = router();
        if let Some(alias) = held {
            create_alias(&router, alias).await;
        }
        let expected: Vec<String> = held.iter().map(|alias| (*alias).to_string()).collect();

        for absent in alias_catalog() {
            if Some(absent.as_str()) == *held {
                continue;
            }
            let case = format!("holding {held:?}, deleting {absent:?}");
            let (status, body) = call(
                &router,
                "DeleteAccountAlias",
                &[("AccountAlias", absent.as_str())],
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "NoSuchEntity",
                "case {case:?}: expected NoSuchEntity, got {:?}",
                error_code(&body)
            );
            assert_eq!(
                current_aliases(&router, &case).await,
                expected,
                "case {case:?}: the refusal must leave the held alias alone"
            );
        }
    }
}

/// RULE: a listing of an empty collection is an empty list with `IsTruncated`
/// false and no `Marker`, not an error — whatever pagination parameters the
/// request carries.
#[tokio::test]
async fn listing_an_account_holding_no_alias_is_an_empty_list() {
    let router = router();
    // Rows span the parameters a caller may add to an empty-collection listing,
    // including both published `MaxItems` bounds (range 1..1000).
    let cases: &[&[(&str, &str)]] = &[
        &[],
        &[("MaxItems", "1")],
        &[("MaxItems", "100")],
        &[("MaxItems", "1000")],
    ];
    for params in cases {
        let (status, body) = call(&router, "ListAccountAliases", params).await;
        assert_eq!(status, 200, "case {params:?}: expected 200, got {status}: {body}");
        assert_eq!(
            listed_aliases(&body),
            Vec::<String>::new(),
            "case {params:?}: expected no aliases, got {:?}",
            listed_aliases(&body)
        );
        assert_eq!(
            element(&body, "IsTruncated"),
            "false",
            "case {params:?}: expected IsTruncated false, got {:?}",
            element(&body, "IsTruncated")
        );
        assert_eq!(
            elements(&body, "Marker"),
            Vec::<String>::new(),
            "case {params:?}: an untruncated page carries no Marker, got {:?}",
            elements(&body, "Marker")
        );
    }
}

/// RULE: `MaxItems` is a page size inside its published range — a value outside it
/// is refused before the operation runs, and a value inside it never hides an
/// entry the collection holds nor truncates a page that fits.
#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let router = router();
    create_alias(&router, "paged-alias").await;

    // One past each published bound of `maxItemsType` (range 1..1000).
    for out_of_range in ["0", "1001", "-1"] {
        let (status, body) =
            call(&router, "ListAccountAliases", &[("MaxItems", out_of_range)]).await;
        assert_eq!(
            status, 400,
            "case {out_of_range:?}: expected 400, got {status}: {body}"
        );
    }

    // Each bound itself is accepted, and the account's one alias fits on a page of
    // any legal size, so no page is ever truncated.
    for in_range in ["1", "2", "100", "1000"] {
        let (status, body) = call(&router, "ListAccountAliases", &[("MaxItems", in_range)]).await;
        assert_eq!(
            status, 200,
            "case {in_range:?}: expected 200, got {status}: {body}"
        );
        assert_eq!(
            listed_aliases(&body),
            vec!["paged-alias".to_string()],
            "case {in_range:?}: expected the stored alias on the page, got {:?}",
            listed_aliases(&body)
        );
        assert_eq!(
            element(&body, "IsTruncated"),
            "false",
            "case {in_range:?}: a page that fits is not truncated, got {:?}",
            element(&body, "IsTruncated")
        );
        assert_eq!(
            elements(&body, "Marker"),
            Vec::<String>::new(),
            "case {in_range:?}: an untruncated page carries no Marker, got {:?}",
            elements(&body, "Marker")
        );
    }
}

/// RULE: a `Marker` this service did not issue for this collection names no
/// position in it, so the call is refused rather than silently answered from the
/// beginning.
#[tokio::test]
async fn a_marker_this_collection_did_not_issue_is_refused() {
    let router = router();
    create_alias(&router, "marker-alias").await;

    // Rows: a token that is not base64 at all, and two that decode to bytes
    // carrying none of this service's marker structure.
    for foreign in ["not-a-marker", "zz", "0g"] {
        let (status, body) = call(&router, "ListAccountAliases", &[("Marker", foreign)]).await;
        assert_eq!(
            status, 400,
            "case {foreign:?}: expected a foreign marker to be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {foreign:?}: expected the marker to be refused as invalid, got {:?}",
            error_code(&body)
        );
    }

    // A marker this service DID issue, for another collection: a different
    // position space, so it names no position among the aliases.
    for user in ["one", "two"] {
        let (status, body) = call(&router, "CreateUser", &[("UserName", user)]).await;
        assert_eq!(
            status, 200,
            "case {user:?}: the fixture CreateUser must succeed: {body}"
        );
    }
    let (status, body) = call(&router, "ListUsers", &[("MaxItems", "1")]).await;
    assert_eq!(status, 200, "expected a truncated user page, got {status}: {body}");
    let user_marker = element(&body, "Marker");
    let (status, body) = call(
        &router,
        "ListAccountAliases",
        &[("Marker", user_marker.as_str())],
    )
    .await;
    assert_eq!(
        status, 400,
        "expected a user marker to be refused by ListAccountAliases, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationError",
        "expected ValidationError, got {:?}",
        error_code(&body)
    );
}

/// RULE: an alias outside the published length of `accountAliasType` (3..63) is
/// refused before the operation runs, so a value the model forbids never reaches
/// the store — on the create and on the delete alike.
#[tokio::test]
async fn an_alias_outside_the_published_length_is_refused() {
    let router = router();

    // One past each published bound, and each bound itself.
    let too_short = "ab";
    let too_long = format!("a{}c", "b".repeat(62));
    assert_eq!(too_long.len(), 64, "the over-long row must be one past 63");
    for action in ["CreateAccountAlias", "DeleteAccountAlias"] {
        for out_of_range in [too_short.to_string(), too_long.clone()] {
            let case = format!("{action} with {} chars", out_of_range.len());
            let (status, body) = call(
                &router,
                action,
                &[("AccountAlias", out_of_range.as_str())],
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status}: {body}"
            );
        }
        // A required member the request omits is refused the same way.
        let (status, body) = call(&router, action, &[]).await;
        assert_eq!(
            status, 400,
            "case {action:?}: a missing AccountAlias is refused, got {status}: {body}"
        );
    }

    // Nothing was persisted by any of the refusals.
    assert_eq!(
        current_aliases(&router, "after the refusals").await,
        Vec::<String>::new(),
        "a refused request persists nothing"
    );

    // Each published bound itself is accepted.
    for in_range in ["abc".to_string(), format!("a{}c", "b".repeat(61))] {
        let case = format!("{} chars", in_range.len());
        let (status, body) = call(
            &router,
            "CreateAccountAlias",
            &[("AccountAlias", in_range.as_str())],
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {body}");
        assert_eq!(
            current_aliases(&router, &case).await,
            vec![in_range.clone()],
            "case {case:?}: expected the alias at the bound to be stored"
        );
    }
}

/// RULE: a listing is a REPORT — it changes nothing a later caller sees, however
/// many times and however it is paged.
#[tokio::test]
async fn listing_the_account_alias_changes_nothing_a_later_caller_sees() {
    let router = router();
    create_alias(&router, "stable-alias").await;
    let baseline = current_aliases(&router, "baseline").await;

    for params in [
        vec![],
        vec![param("MaxItems", "1")],
        vec![param("MaxItems", "1000")],
        vec![param("MaxItems", "7")],
    ] {
        for _ in 0..3 {
            let (status, body) =
                call(&router, "ListAccountAliases", &borrowed(&params)).await;
            assert_eq!(
                status, 200,
                "case {params:?}: expected 200, got {status}: {body}"
            );
            assert_eq!(
                listed_aliases(&body),
                baseline,
                "case {params:?}: expected the same alias on every read, got {:?}",
                listed_aliases(&body)
            );
        }
    }

    assert_eq!(
        current_aliases(&router, "after the repeated listings").await,
        baseline,
        "expected the repeated listings to leave the store alone"
    );
}
