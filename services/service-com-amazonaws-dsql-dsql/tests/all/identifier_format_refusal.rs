// SPDX-License-Identifier: CC0-1.0

//! What an identifier-addressed operation answers for a value that is inside
//! `ClusterId`'s and `StreamId`'s pattern yet is no identifier this service could
//! ever have issued.
//!
//! RULE under test: an identifier of this service is the 26-character
//! base32 rendering of a 128-bit value — the alphabet is the digits plus the
//! letters with `i`, `l`, `o` and `u` left out, and 26 symbols carry 130 bits, so
//! the leading symbol can only be one of the eight that keep the value inside 128
//! bits. A value that `^[a-z0-9]{26}$` admits but that encoding does not render
//! names no resource, so it is refused as INPUT before any state is read: HTTP 400
//! with the service's own modelled `ValidationException`, its `@required` `reason`
//! filled and its `fieldList` naming the member the value arrived in — never 404
//! with a report that some resource is absent, which is what a client reads as a
//! resource that is gone (`tests/all/resource_identifier_absence.rs` pins that
//! other half, for the values the encoding DOES render).
//!
//! The sentence is per KIND of identifier: a cluster identifier's refusal quotes
//! the member and the value it rejected and says the cluster identifier format is
//! invalid, while a stream identifier's names the member and the form it had to be
//! in without echoing the value.
//!
//! The value table spans the two ways a 26-symbol lowercase-alphanumeric value can
//! fail to be such a rendering — a symbol the alphabet leaves out, at the front, in
//! the middle and at the end, and a leading symbol past the 128-bit ceiling, over
//! every symbol past it — and the second test walks the whole leading-symbol
//! alphabet so both sides of that bound are asserted. The alphabet and the ceiling
//! are the ones `resource_identifier_absence` spells, and every row runs over the
//! ONE route table that module spells, so every operation that carries an
//! identifier is asserted on the member it binds.
//!
//! Every case drives the real request path through the `Dsql` harness in
//! `tests/all/harness.rs`, this crate's one test client.

use serde_json::Value;

use crate::harness::Dsql;
use crate::resource_identifier_absence::{
    routes, Class, IdCase, Route, IDENTIFIER_LEN, IDENTIFIER_SYMBOLS, LEADING_SYMBOL_MAX,
};

/// The symbols `[a-z0-9]` carries that [`IDENTIFIER_SYMBOLS`] leaves out.
const OUTSIDE_THE_ALPHABET: [char; 4] = ['i', 'l', 'o', 'u'];

/// A value of the service's own form, which every row below breaks in exactly one
/// way: [`IDENTIFIER_LEN`] symbols of [`IDENTIFIER_SYMBOLS`] whose leading symbol
/// is inside the ceiling [`LEADING_SYMBOL_MAX`] fixes.
fn rendered_identifier() -> String {
    let tail: String = IDENTIFIER_SYMBOLS
        .chars()
        .skip(1)
        .take(IDENTIFIER_LEN - 1)
        .collect();
    format!("0{tail}")
}

/// Whether `leading` is a leading symbol the 128-bit ceiling admits: 26 symbols
/// carry 130 bits, so the leading one spells only the value's top 3 bits and the
/// symbols up to [`LEADING_SYMBOL_MAX`] are the whole range.
fn inside_the_ceiling(leading: char) -> bool {
    leading <= LEADING_SYMBOL_MAX
}

/// Values INSIDE `^[a-z0-9]{26}$` that the encoding does not render: a symbol the
/// alphabet leaves out, each at the front, in the middle and at the end, and every
/// leading symbol past the 128-bit ceiling.
fn values_the_encoding_does_not_render() -> Vec<IdCase> {
    let base = rendered_identifier();
    let mut cases = Vec::new();

    for symbol in OUTSIDE_THE_ALPHABET {
        for (position, where_it_is) in [
            (0, "first"),
            (IDENTIFIER_LEN / 2, "middle"),
            (IDENTIFIER_LEN - 1, "last"),
        ] {
            let mut value: Vec<char> = base.chars().collect();
            value[position] = symbol;
            cases.push(IdCase {
                name: format!("{symbol:?}, outside the alphabet, as the {where_it_is} symbol"),
                value: value.into_iter().collect(),
            });
        }
    }

    for leading in IDENTIFIER_SYMBOLS
        .chars()
        .filter(|leading| !inside_the_ceiling(*leading))
    {
        let mut value: Vec<char> = base.chars().collect();
        value[0] = leading;
        cases.push(IdCase {
            name: format!("a leading {leading:?}, past the 128-bit ceiling"),
            value: value.into_iter().collect(),
        });
    }

    for case in &cases {
        assert_eq!(
            case.value.len(),
            IDENTIFIER_LEN,
            "case {:?}: every row must be {IDENTIFIER_LEN} symbols, got {:?}",
            case.name,
            case.value
        );
        assert!(
            case.value
                .chars()
                .all(|symbol| symbol.is_ascii_lowercase() || symbol.is_ascii_digit()),
            "case {:?}: every row must be inside the modelled pattern, got {:?}",
            case.name,
            case.value
        );
    }
    cases
}

/// The sentence a refusal of `value` in `route`'s member carries: the cluster
/// identifier's, which quotes the member and the value, or the stream
/// identifier's, which names the member and the form without echoing the value.
fn expected_sentence(route: &Route, value: &str) -> String {
    match route.class {
        Class::Cluster => format!(
            "Invalid input '{}: {value}'. The cluster identifier format is invalid. \
             Provide a valid cluster identifier and try again.",
            route.member
        ),
        Class::Stream => format!(
            "Aurora DSQL can't process the request because the {member} that you specified \
             isn't valid. Provide the {member} in the format \
             'a valid base32-encoded 26-character string' and try again.",
            member = route.member
        ),
    }
}

/// Assert that `route` refuses `value` as input, with the body the modelled
/// `ValidationException` owes and the sentence its kind of identifier is refused
/// with.
async fn refused_as_input(dsql: &Dsql, route: &Route, live: &str, case: &IdCase) {
    let (method, path, body) = (route.request)(&case.value, live);
    let answer = dsql.call(method, &path, body).await;
    let name = format!("{} on {}", case.name, route.operation);

    assert_eq!(
        answer.status, 400,
        "case {name}: a value the encoding does not render must answer 400, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error.as_deref(),
        Some("ValidationException"),
        "case {name}: the refusal must be a ValidationException, got {:?}",
        answer.error
    );
    assert!(
        !answer.str("reason").is_empty(),
        "case {name}: ValidationException models reason as @required, got {:?}",
        answer.body
    );
    let field_list = answer.get("fieldList");
    let fields = field_list.as_array().map(Vec::as_slice).unwrap_or_default();
    assert!(
        fields
            .iter()
            .any(|field| field.get("name").and_then(Value::as_str) == Some(route.member)),
        "case {name}: the refusal must name {:?} in fieldList, got {field_list:?}",
        route.member
    );
    assert_eq!(
        answer.message(),
        expected_sentence(route, &case.value),
        "case {name}: expected {:?}, got {:?}",
        expected_sentence(route, &case.value),
        answer.message()
    );
    assert_eq!(
        answer.get("resourceId"),
        Value::Null,
        "case {name}: a refusal of the request names no resource, got {:?}",
        answer.body
    );
}

/// Assert that nothing any refused call addressed was created, removed or changed:
/// `live` is still the only cluster, still readable, still without a policy and
/// still holding no stream.
async fn the_store_is_untouched(dsql: &Dsql, live: &str) {
    let read = dsql.get(live).await;
    assert_eq!(
        read.status, 200,
        "the live cluster must be untouched by every refusal, got {} {:?}",
        read.status, read.body
    );
    let clusters = dsql.list("").await.get("clusters");
    assert_eq!(
        clusters.as_array().map(Vec::len),
        Some(1),
        "no refused request may have created a cluster, got {clusters:?}"
    );
    assert_eq!(
        dsql.get_policy(live).await.status,
        404,
        "no refused request may have attached a policy"
    );
    let streams = dsql.list_streams(live, "").await.get("streams");
    assert_eq!(
        streams.as_array().map(Vec::len),
        Some(0),
        "no refused request may have created a stream, got {streams:?}"
    );
}

/// RULE: a value inside the modelled pattern that the identifier encoding does not
/// render is refused as input — 400 `ValidationException`, with the modelled body
/// and the sentence its kind of identifier is refused with — on every route that
/// carries a resource identifier, and nothing is read or written.
#[tokio::test]
async fn a_value_the_identifier_encoding_does_not_render_is_refused_as_input() {
    let dsql = Dsql::new();
    // One live cluster, so a row that puts the value under test in the stream
    // label is refused for that label rather than for a missing cluster.
    let live = dsql.create_settled().await;

    let routes = routes();
    for case in values_the_encoding_does_not_render() {
        for route in &routes {
            refused_as_input(&dsql, route, &live, &case).await;
        }
    }

    the_store_is_untouched(&dsql, &live).await;
}

/// RULE at the bound: the leading symbol decides which refusal a 26-symbol
/// lowercase-alphanumeric value gets, so the whole alphabet splits in two — the
/// eight symbols the 128-bit ceiling admits make the value an identifier, which an
/// account that does not hold it reports as an absent resource (404), and every
/// symbol past the bound makes it no identifier at all, refused as input (400).
#[tokio::test]
async fn the_leading_symbol_decides_absence_from_a_refused_input() {
    let dsql = Dsql::new();
    let live = dsql.create_settled().await;

    let routes = routes();
    let base = rendered_identifier();
    for leading in IDENTIFIER_SYMBOLS.chars() {
        let mut value: Vec<char> = base.chars().collect();
        value[0] = leading;
        let case = IdCase {
            name: format!("a leading {leading:?}"),
            value: value.into_iter().collect(),
        };

        for route in &routes {
            if inside_the_ceiling(leading) {
                let (method, path, body) = (route.request)(&case.value, &live);
                let answer = dsql.call(method, &path, body).await;
                assert_eq!(
                    answer.status,
                    404,
                    "case {} on {}: a leading symbol inside the ceiling is an identifier, \
                     so an account that does not hold it must answer 404, got {} {:?}",
                    case.name,
                    route.operation,
                    answer.status,
                    answer.body
                );
                assert_eq!(
                    answer.error.as_deref(),
                    Some("ResourceNotFoundException"),
                    "case {} on {}: expected ResourceNotFoundException, got {:?}",
                    case.name,
                    route.operation,
                    answer.error
                );
            } else {
                refused_as_input(&dsql, route, &live, &case).await;
            }
        }
    }

    the_store_is_untouched(&dsql, &live).await;
}
