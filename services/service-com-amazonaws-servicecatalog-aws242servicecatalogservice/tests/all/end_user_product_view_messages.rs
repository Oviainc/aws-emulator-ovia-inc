// SPDX-License-Identifier: CC0-1.0

//! The END-USER product view's refusal wording, over the wire as awsJson1_1 requests
//! through the same `MultiServiceRouter` the production emulator mounts.
//!
//! `DescribeProduct` reads a product through the catalog an end user can reach — a
//! launch path — so it can fail for two reasons that it deliberately does not tell
//! apart: no product carries that identifier, or one does and this view cannot reach it.
//! The service answers both with ONE sentence: the identifier the request addressed the
//! product by, followed by the fixed text `" does not exist or access was denied"`. The
//! identifier is the only variable part.
//!
//! The ADMIN view, `DescribeProductAsAdmin`, sees every stored product, so it words its
//! own not-found differently; this suite pins that the two are not interchangeable.
//!
//! Every case reads STORED state back through `DescribeProductAsAdmin` and
//! `DescribeProduct`, never a value this file hands the service and takes straight out
//! of the same response.

use serde_json::{json, Value};

use crate::harness::{
    associate_product_with_portfolio, call, create_portfolio, create_product, emulator, error_code,
    first_provisioning_artifact_id, message,
};

/// The fixed text of the end-user view's not-found sentence: everything but the
/// identifier echoed from the request.
const END_USER_SENTENCE: &str = " does not exist or access was denied";

/// The modelled upper bound on an error message: `ResourceNotFoundException$Message`
/// targets `ErrorMessage`, `@length 1..1024`
/// (`smithy_query query="shape" shape="ErrorMessage"`).
const MAX_ERROR_MESSAGE: usize = 1024;

/// Assert one `DescribeProduct` refusal carries exactly the end-user sentence for
/// `identifier`.
async fn assert_end_user_refusal(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    request: &Value,
    identifier: &str,
) {
    let (status, refused) = call(router, "DescribeProduct", request).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ResourceNotFoundException",
        "case {case:?}: unexpected error in {refused}"
    );
    let reported = message(&refused);
    assert_eq!(
        reported,
        format!("{identifier}{END_USER_SENTENCE}"),
        "case {case:?}: unexpected message, got {reported:?}"
    );
    assert!(
        !reported.is_empty() && reported.len() <= MAX_ERROR_MESSAGE,
        "case {case:?}: the message must satisfy the modelled length, got {} characters in \
         {reported:?}",
        reported.len()
    );
}

/// An identifier that names no stored product at all is refused with the end-user
/// sentence, whichever member carried it and whatever shape it has.
///
/// Rows differ along the member the product is addressed by and the shape of the
/// identifier. `Id` is `@length 1..100`, pattern `^[a-zA-Z0-9_\-]*$`, and `Name` is
/// `ProductViewName` `@length 0..8191`
/// (`smithy_query query="operation" operation="DescribeProduct"`), so each member is
/// driven at its shortest admitted value, at a long one, at the prefixed form the
/// service mints, and at a value of another resource kind. Anything outside those
/// constraints never reaches the handler: the framework refuses it first.
///
/// `Name`'s long row is the LONGEST identifier whose sentence still fits the modelled
/// `ErrorMessage` bound — 1024 less the fixed text — so the length assertion below is
/// driven right at that bound rather than comfortably inside it.
#[tokio::test]
async fn an_unknown_product_is_refused_with_the_end_user_sentence() {
    let router = emulator_with_a_reachable_product().await;

    let longest_id = format!("prod-{}", "a".repeat(95));
    let long_name = "n".repeat(MAX_ERROR_MESSAGE - END_USER_SENTENCE.len());
    let rows: [(&str, &str, String); 9] = [
        ("Id", "the minimum length", "q".to_string()),
        (
            "Id",
            "the prefixed form the service mints",
            "prod-zzzzzzzzzzzz".to_string(),
        ),
        ("Id", "underscores and dashes", "prod_view-7_X".to_string()),
        ("Id", "not an id of any kind", "lorem".to_string()),
        (
            "Id",
            "an id of another resource kind",
            "port-zzzzzzzzzzzz".to_string(),
        ),
        ("Id", "the maximum length", longest_id.clone()),
        ("Name", "the minimum length", "q".to_string()),
        (
            "Name",
            "a name with the punctuation a product name admits",
            "no such product (v2)".to_string(),
        ),
        (
            "Name",
            "the longest name whose refusal fits the message bound",
            long_name.clone(),
        ),
    ];
    assert!(
        long_name.len() <= 8191,
        "the long name must satisfy the modelled ProductViewName length, got {} characters",
        long_name.len()
    );

    for (member, shape, identifier) in rows {
        let case = format!("{member}: {shape}");
        assert_end_user_refusal(
            &router,
            &case,
            &json!({member: identifier.clone()}),
            &identifier,
        )
        .await;
    }
}

/// A product that IS stored but sits in no portfolio gets the SAME sentence as one that
/// does not exist: the end-user view reaches a product only through a launch path, and
/// its refusal does not tell the two apart.
///
/// Rows differ along the member the product is addressed by and along what the
/// identifier names — a stored product out of view, and one nothing stored carries.
#[tokio::test]
async fn a_stored_product_out_of_view_is_refused_with_the_same_sentence() {
    let router = emulator();
    let hidden_id = create_product(&router, "hidden-product", "tok-hidden").await;

    // The product really is stored: the ADMIN view reports it all along.
    let (status, described) =
        call(&router, "DescribeProductAsAdmin", &json!({"Id": hidden_id})).await;
    assert_eq!(
        status, 200,
        "the stored product must be readable by the admin view, got {status} with {described}"
    );

    for (case, request, identifier) in [
        (
            "a stored product out of view, by Id",
            json!({"Id": hidden_id.clone()}),
            hidden_id.clone(),
        ),
        (
            "a stored product out of view, by Name",
            json!({"Name": "hidden-product"}),
            "hidden-product".to_string(),
        ),
        (
            "a product nothing stored carries, by Id",
            json!({"Id": "prod-000000000000"}),
            "prod-000000000000".to_string(),
        ),
        (
            "a product nothing stored carries, by Name",
            json!({"Name": "hidden-product-2"}),
            "hidden-product-2".to_string(),
        ),
    ] {
        assert_end_user_refusal(&router, case, &request, &identifier).await;
    }

    // Giving the product a launch path is what brings it into view, so the refusals
    // above really were the view's and not a broken lookup.
    let portfolio_id =
        create_portfolio(&router, "hidden-portfolio", "acme", &[], "tok-hidden-port").await;
    associate_product_with_portfolio(&router, &hidden_id, &portfolio_id).await;
    for (case, request) in [
        ("by Id", json!({"Id": hidden_id.clone()})),
        ("by Name", json!({"Name": "hidden-product"})),
    ] {
        let (status, described) = call(&router, "DescribeProduct", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a product on a launch path is in view, got {status} with {described}"
        );
        assert_eq!(
            described["ProductViewSummary"]["ProductId"].as_str(),
            Some(hidden_id.as_str()),
            "case {case:?}: it is the stored product, got {described}"
        );
    }
}

/// The end-user and the admin view word their refusals differently for the very same
/// unresolvable identifier: one asserts the access dimension, the other does not.
#[tokio::test]
async fn the_admin_view_words_its_refusal_differently() {
    let router = emulator_with_a_reachable_product().await;
    let unknown = "prod-zzzzzzzzzzzz";

    let (status, refused) = call(&router, "DescribeProduct", &json!({"Id": unknown})).await;
    assert_eq!(
        status, 400,
        "the end-user view: expected 400, got {status} with {refused}"
    );
    let end_user = message(&refused);

    let (status, refused) = call(&router, "DescribeProductAsAdmin", &json!({"Id": unknown})).await;
    assert_eq!(
        status, 400,
        "the admin view: expected 400, got {status} with {refused}"
    );
    let admin = message(&refused);

    assert_eq!(
        end_user,
        format!("{unknown}{END_USER_SENTENCE}"),
        "the end-user view's sentence, got {end_user:?}"
    );
    assert!(
        !admin.contains(END_USER_SENTENCE),
        "the admin view must not carry the end-user sentence, got {admin:?}"
    );
    assert_ne!(
        end_user, admin,
        "the two views word their refusals differently, got {end_user:?} for both"
    );
}

/// Naming neither identifier is a bad parameter rather than a missing resource, so the
/// end-user sentence is not the answer to every refusal of this operation.
#[tokio::test]
async fn naming_no_identifier_is_not_a_not_found() {
    let router = emulator_with_a_reachable_product().await;
    for (case, request) in [
        ("neither member", json!({})),
        ("an empty Name", json!({"Name": ""})),
    ] {
        let (status, refused) = call(&router, "DescribeProduct", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidParametersException",
            "case {case:?}: unexpected error in {refused}"
        );
        assert!(
            !message(&refused).contains(END_USER_SENTENCE),
            "case {case:?}: a bad parameter is not a not-found, got {:?}",
            message(&refused)
        );
    }
}

/// One emulator carrying a product that IS in view, so a refusal below is about the
/// identifier the request carried rather than about an empty catalog.
async fn emulator_with_a_reachable_product() -> smithy_server_runtime::MultiServiceRouter {
    let router = emulator();
    let portfolio_id =
        create_portfolio(&router, "view-portfolio", "acme", &[], "tok-view-port").await;
    let product_id = create_product(&router, "view-product", "tok-view-product").await;
    associate_product_with_portfolio(&router, &product_id, &portfolio_id).await;
    // The product has a version, so the view it serves is a complete one.
    let _ = first_provisioning_artifact_id(&router, &product_id).await;
    let (status, described) = call(&router, "DescribeProduct", &json!({"Id": product_id})).await;
    assert_eq!(
        status, 200,
        "the seeded product must be in view, got {status} with {described}"
    );
    router
}
