// SPDX-License-Identifier: CC0-1.0

//! `UpdateOpenIDConnectProviderThumbprint`, end to end over the awsQuery wire, read back
//! through `GetOpenIDConnectProvider`.
//!
//! RULE under test: the call REPLACES the named provider's thumbprint list with exactly
//! the list the request carried, in the order it carried it — "the list that you pass with
//! this operation completely replaces the existing list of thumbprints. (The lists are not
//! merged.)" — leaving every other member of that provider, and every other provider the
//! account holds, as it was; a request the service refuses (a value that is not an ARN, an
//! ARN the account holds no provider under, a list past the provider's thumbprint quota, a
//! thumbprint outside the length its shape declares) replaces nothing, so the provider
//! still reports the list it held.
//!
//! Each suite is a TABLE whose rows differ along what that rule generalises over: how many
//! thumbprints the provider held and how many the replacement carries (none, one, several,
//! and the quota's full list), how the two lists overlap (disjoint, identical, partially
//! shared), the ORDER the replacement arrives in (every permutation of one three-element
//! list, so the order a row reports cannot be an accident of sorting), which provider of
//! several is addressed, and how a refused request is outside the rule. No single request
//! is the specification.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour (status, awsQuery error code, XML members) of a provider that was
//! PERSISTED rather than a handler called directly. The awsQuery harness is the one
//! `tests/all/user_lifecycle.rs` owns, the parameter builders are
//! `tests/all/role_lifecycle.rs`'s and the provider ARN, thumbprint and list readers come
//! from `tests/all/open_id_connect_providers.rs`, so a request is built and read exactly
//! one way across this crate.

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state::MAX_THUMBPRINTS_PER_OPEN_ID_CONNECT_PROVIDER;
use smithy_server_runtime::MultiServiceRouter;

use crate::open_id_connect_providers::{
    absence_message, client_ids, expected_arn, issuer_of, list_members, thumbprint,
};
use crate::role_lifecycle::{borrowed, param};
use crate::user_lifecycle::{account_id, call, element, error_code, router};

/// The client IDs every fixture provider is created with, so a replacement can be shown to
/// leave the provider's other list alone.
const FIXTURE_CLIENT_ID_COUNT: usize = 2;

/// The members of one provider, as `GetOpenIDConnectProvider` reports them.
#[derive(Debug, PartialEq, Eq)]
struct Reported {
    url: String,
    client_ids: Vec<String>,
    thumbprints: Vec<String>,
    create_date: String,
}

/// `OpenIDConnectProviderArn` plus the `ThumbprintList` members, in `list`'s order.
fn update_params(arn: &str, list: &[String]) -> Vec<(String, String)> {
    let mut params = vec![param("OpenIDConnectProviderArn", arn)];
    for (index, value) in list.iter().enumerate() {
        params.push(param(
            &format!("ThumbprintList.member.{}", index + 1),
            value,
        ));
    }
    params
}

/// Create the provider for `url` holding `thumbprints` (and [`FIXTURE_CLIENT_ID_COUNT`]
/// client IDs), answering the ARN it is held under.
async fn create_provider(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    thumbprints: &[String],
) -> String {
    let mut params = vec![param("Url", url)];
    for (index, client_id) in client_ids(FIXTURE_CLIENT_ID_COUNT).iter().enumerate() {
        params.push(param(
            &format!("ClientIDList.member.{}", index + 1),
            client_id,
        ));
    }
    for (index, value) in thumbprints.iter().enumerate() {
        params.push(param(
            &format!("ThumbprintList.member.{}", index + 1),
            value,
        ));
    }
    let (status, body) = call(router, "CreateOpenIDConnectProvider", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateOpenIDConnectProvider must succeed, got \
         {status}: {body}"
    );
    let arn = element(&body, "OpenIDConnectProviderArn");
    assert_eq!(
        arn,
        expected_arn(url),
        "case {case:?}: the provider must be held under the ARN derived from its URL, got \
         {arn:?}"
    );
    arn
}

/// One `UpdateOpenIDConnectProviderThumbprint` call: the HTTP status and the XML body.
async fn update(
    router: &MultiServiceRouter,
    arn: &str,
    list: &[String],
) -> (u16, String) {
    let params = update_params(arn, list);
    call(
        router,
        "UpdateOpenIDConnectProviderThumbprint",
        &borrowed(&params),
    )
    .await
}

/// What `GetOpenIDConnectProvider` reports for `arn`.
async fn reported(router: &MultiServiceRouter, case: &str, arn: &str) -> Reported {
    let (status, body) = call(
        router,
        "GetOpenIDConnectProvider",
        &[("OpenIDConnectProviderArn", arn)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetOpenIDConnectProvider must report the held provider, got \
         {status}: {body}"
    );
    Reported {
        url: element(&body, "Url"),
        client_ids: list_members(&body, "ClientIDList"),
        thumbprints: list_members(&body, "ThumbprintList"),
        create_date: element(&body, "CreateDate"),
    }
}

/// One replacement row: the list the provider is created with, and the list that replaces
/// it.
struct ReplacementCase {
    case: &'static str,
    url: &'static str,
    held: Vec<String>,
    replacement: Vec<String>,
}

/// The rows: replacement lists shorter than, longer than and the same length as the held
/// one; a replacement sharing every entry with the held list, some of them, and none of
/// them; the quota's full list; and a provider created carrying no thumbprint at all.
fn replacement_cases() -> Vec<ReplacementCase> {
    vec![
        ReplacementCase {
            case: "one thumbprint replaces one",
            url: "https://thumb.example.com/one",
            held: vec![thumbprint(1)],
            replacement: vec![thumbprint(2)],
        },
        ReplacementCase {
            case: "one thumbprint replaces three",
            url: "https://thumb.example.com/shrink",
            held: vec![thumbprint(3), thumbprint(4), thumbprint(5)],
            replacement: vec![thumbprint(6)],
        },
        ReplacementCase {
            case: "three thumbprints replace one",
            url: "https://thumb.example.com/grow",
            held: vec![thumbprint(7)],
            replacement: vec![thumbprint(8), thumbprint(9), thumbprint(10)],
        },
        ReplacementCase {
            case: "the quota's full list replaces two",
            url: "https://thumb.example.com/full",
            held: vec![thumbprint(11), thumbprint(12)],
            replacement: (0..MAX_THUMBPRINTS_PER_OPEN_ID_CONNECT_PROVIDER as u32)
                .map(|index| thumbprint(20 + index))
                .collect(),
        },
        ReplacementCase {
            case: "a list identical to the held one",
            url: "https://thumb.example.com/identical",
            held: vec![thumbprint(30), thumbprint(31)],
            replacement: vec![thumbprint(30), thumbprint(31)],
        },
        ReplacementCase {
            case: "a list sharing one entry with the held one",
            url: "https://thumb.example.com/overlapping",
            held: vec![thumbprint(40), thumbprint(41), thumbprint(42)],
            replacement: vec![thumbprint(41), thumbprint(43)],
        },
        ReplacementCase {
            case: "a provider created carrying no thumbprint",
            url: "https://thumb.example.com/none",
            held: Vec::new(),
            replacement: vec![thumbprint(50), thumbprint(51)],
        },
    ]
}

/// RULE: the provider reports exactly the list the request carried, whatever it held
/// before and however the two lists overlap — the lists are not merged — and the
/// provider's other members are untouched.
#[tokio::test]
async fn a_replacement_is_the_whole_list_the_request_carried() {
    let router = router();

    for ReplacementCase {
        case,
        url,
        held,
        replacement,
    } in replacement_cases()
    {
        let arn = create_provider(&router, case, url, &held).await;
        let before = reported(&router, case, &arn).await;
        assert_eq!(
            before.thumbprints, held,
            "case {case:?}: the fixture must hold {held:?}, got {:?}",
            before.thumbprints
        );

        let (status, body) = update(&router, &arn, &replacement).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the replacement must succeed, got {status}: {body}"
        );

        let after = reported(&router, case, &arn).await;
        assert_eq!(
            after.thumbprints, replacement,
            "case {case:?}: expected the reported list to be exactly {replacement:?}, got \
             {:?}",
            after.thumbprints
        );
        assert_eq!(
            after.client_ids, before.client_ids,
            "case {case:?}: expected the client IDs {:?} to be untouched, got {:?}",
            before.client_ids, after.client_ids
        );
        assert_eq!(
            after.url,
            issuer_of(url),
            "case {case:?}: expected the URL {:?} to be untouched, got {:?}",
            issuer_of(url),
            after.url
        );
        assert_eq!(
            after.create_date, before.create_date,
            "case {case:?}: expected the create instant {:?} to be untouched, got {:?}",
            before.create_date, after.create_date
        );
    }
}

/// RULE (ordering invariant): a thumbprint list is reported in the order the request
/// carried it, so EVERY ordering of one set of thumbprints round trips as itself. The rows
/// are the six permutations of a three-element list — exhaustive for that length, so no
/// row can pass by accident of sorting.
#[tokio::test]
async fn every_ordering_of_a_replacement_round_trips_as_itself() {
    let router = router();

    let values = [thumbprint(60), thumbprint(61), thumbprint(62)];
    // The six permutations of three positions, written out: a generator would be the same
    // list, longer.
    let orders: &[[usize; 3]] = &[
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];

    let arn = create_provider(
        &router,
        "the ordering fixture",
        "https://thumb.example.com/ordering",
        &values,
    )
    .await;

    for order in orders {
        let case = format!("the ordering {order:?}");
        let replacement: Vec<String> = order.iter().map(|index| values[*index].clone()).collect();
        let (status, body) = update(&router, &arn, &replacement).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the replacement must succeed, got {status}: {body}"
        );
        let after = reported(&router, &case, &arn).await;
        assert_eq!(
            after.thumbprints, replacement,
            "case {case:?}: expected {replacement:?} in that order, got {:?}",
            after.thumbprints
        );
    }
}

/// RULE: the call addresses ONE provider, so the thumbprints of every other provider the
/// account holds are left exactly as they were.
#[tokio::test]
async fn a_replacement_leaves_every_other_provider_alone() {
    let router = router();

    let providers: Vec<(&str, Vec<String>)> = vec![
        ("https://many.example.com/first", vec![thumbprint(70)]),
        (
            "https://many.example.com/second",
            vec![thumbprint(71), thumbprint(72)],
        ),
        ("https://many.example.com/third", vec![thumbprint(73)]),
    ];
    let mut arns = Vec::new();
    for (url, held) in &providers {
        arns.push(create_provider(&router, url, url, held).await);
    }

    // The addressed provider is the middle one, so a replacement that leaked would be
    // visible on a provider created before it and on one created after it.
    let addressed = 1;
    let replacement = vec![thumbprint(74), thumbprint(75), thumbprint(76)];
    let (status, body) = update(&router, &arns[addressed], &replacement).await;
    assert_eq!(
        status, 200,
        "the addressed provider's replacement must succeed, got {status}: {body}"
    );

    for (index, (url, held)) in providers.iter().enumerate() {
        let case = format!("the provider at {url}");
        let expected = if index == addressed {
            replacement.clone()
        } else {
            held.clone()
        };
        let after = reported(&router, &case, &arns[index]).await;
        assert_eq!(
            after.thumbprints, expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            after.thumbprints
        );
    }
}

/// One refused row: the ARN and list it carries, and what it must be refused with.
struct RefusalCase {
    case: String,
    arn: String,
    list: Vec<String>,
    status: u16,
    code: &'static str,
    message: String,
}

/// RULE: a request outside the rule replaces nothing — the provider still reports the list
/// it held — and is refused with the modelled error for what was wrong with it.
#[tokio::test]
async fn a_refused_replacement_replaces_nothing() {
    let router = router();

    let case = "the refusal fixture";
    let url = "https://refused.example.com/provider";
    let held = vec![thumbprint(80), thumbprint(81)];
    let arn = create_provider(&router, case, url, &held).await;

    let quota = MAX_THUMBPRINTS_PER_OPEN_ID_CONNECT_PROVIDER;
    let quota_message =
        format!("Cannot exceed quota for ThumbprintsPerOpenIdConnectProvider: {quota}");
    // A value long enough to satisfy `arnType`'s length 20..2048
    // (`smithy_query(query="shape", shape="UpdateOpenIDConnectProviderThumbprintRequest")`),
    // so what refuses it is its FORM and not its length.
    let not_an_arn = "refused.example.com/provider";
    let unheld = format!("arn:aws:iam::{}:oidc-provider/never.example.org", account_id());
    let rows = vec![
        RefusalCase {
            case: "a value that is not an ARN".to_string(),
            arn: not_an_arn.to_string(),
            list: vec![thumbprint(90)],
            status: 400,
            code: "InvalidInput",
            message: format!(
                "Invalid OpenIDConnectProviderArn {not_an_arn}: the value is not an ARN"
            ),
        },
        RefusalCase {
            case: "an ARN the account holds no provider under".to_string(),
            arn: unheld.clone(),
            list: vec![thumbprint(91)],
            status: 404,
            code: "NoSuchEntity",
            message: absence_message(&unheld),
        },
        RefusalCase {
            case: "one thumbprint past the provider's quota".to_string(),
            arn: arn.clone(),
            list: (0..quota as u32 + 1)
                .map(|index| thumbprint(100 + index))
                .collect(),
            status: 400,
            code: "InvalidInput",
            message: quota_message.clone(),
        },
        RefusalCase {
            case: "several thumbprints past the provider's quota".to_string(),
            arn: arn.clone(),
            list: (0..quota as u32 + 4)
                .map(|index| thumbprint(110 + index))
                .collect(),
            status: 400,
            code: "InvalidInput",
            message: quota_message.clone(),
        },
    ];

    for RefusalCase {
        case,
        arn: addressed,
        list,
        status: expected_status,
        code,
        message,
    } in rows
    {
        let (status, body) = update(&router, &addressed, &list).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code}, got {:?}: {body}",
            error_code(&body)
        );
        let reported_message = element(&body, "Message");
        assert_eq!(
            reported_message, message,
            "case {case:?}: expected {message:?}, got {reported_message:?}"
        );
        let after = reported(&router, &case, &arn).await;
        assert_eq!(
            after.thumbprints, held,
            "case {case:?}: expected the held list {held:?} to survive the refusal, got {:?}",
            after.thumbprints
        );
    }
}

/// RULE: a thumbprint is "always a 40-character string" — the length `thumbprintType`
/// declares (`smithy_query(query="shape", shape="thumbprintType")`: len 40..40) — so a
/// value on either side of that bound is refused before anything is replaced, while the
/// bound itself is accepted.
#[tokio::test]
async fn a_thumbprint_outside_its_declared_length_is_refused() {
    let router = router();

    let case = "the length fixture";
    let url = "https://sized.example.com/provider";
    let held = vec![thumbprint(120)];
    let arn = create_provider(&router, case, url, &held).await;

    // One character short of the bound, and one past it.
    let rows = [
        ("one character under the declared length", "a".repeat(39)),
        ("one character over the declared length", "a".repeat(41)),
    ];
    for (case, value) in rows {
        let (status, body) = update(&router, &arn, std::slice::from_ref(&value)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a {}-character thumbprint, got {status}: \
             {body}",
            value.len()
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {case:?}: expected the constraint walk's refusal code, got {:?}: {body}",
            error_code(&body)
        );
        let after = reported(&router, case, &arn).await;
        assert_eq!(
            after.thumbprints, held,
            "case {case:?}: expected the held list {held:?} to survive the refusal, got {:?}",
            after.thumbprints
        );
    }

    // The bound itself: a thumbprint of exactly the declared length is accepted.
    let at_bound = vec!["b".repeat(40)];
    let (status, body) = update(&router, &arn, &at_bound).await;
    assert_eq!(
        status, 200,
        "case the declared length itself: expected the replacement to succeed, got \
         {status}: {body}"
    );
    let after = reported(&router, "the declared length itself", &arn).await;
    assert_eq!(
        after.thumbprints, at_bound,
        "case the declared length itself: expected {at_bound:?}, got {:?}",
        after.thumbprints
    );
}
