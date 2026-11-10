// SPDX-License-Identifier: CC0-1.0

//! What an identifier-addressed operation answers for an identifier written in the
//! service's alphabet whose symbols spell a value above the range this service
//! mints identifiers from.
//!
//! RULE under test: an identifier is 26 characters of the base32 alphabet
//! (`tests/all/identifier_format_refusal.rs` pins that half, which every route
//! requires), and 26 such symbols carry 130 bits while a minted identifier
//! occupies 128 of them — so the alphabet admits strings above every value this
//! service could have minted. Whether that range is a PRECONDITION is stated per
//! operation, not by the model: `ClusterId` and `StreamId` carry one `@pattern`
//! each and every one of these operations models both `ValidationException` (400)
//! and `ResourceNotFoundException` (404) (`smithy_query(query="digest")`), so the
//! answer comes from each operation's own wire behaviour. `GetCluster`,
//! `DeleteCluster` and `GetClusterPolicy` state it: such a value is refused as
//! input, before any lookup, with the same sentence as any other malformed cluster
//! identifier. Every other identifier-addressed operation states no range
//! precondition: such a value is an identifier this account does not hold, so it is
//! resolved against the store and reported as the modelled
//! `ResourceNotFoundException` — HTTP 404, the `ResourceNotFoundException` wire
//! code, and all three `@required` members filled: `message` naming the ARN the
//! call addressed, `resourceId` the identifier the request carried verbatim and
//! `resourceType` the type of the resource the lookup stopped at. The per-operation
//! answer lives on the route table as `Route::above_the_minted_range`, and this
//! suite asserts BOTH answers over that one table.
//!
//! The value table spans how a 26-symbol value can sit above that range: each
//! leading symbol the range leaves out, a single symbol repeated across the whole
//! value, the smallest such value and the largest the alphabet spells. Every row
//! runs over every route, for a cluster identifier and for a stream identifier
//! under a cluster that is really held.
//!
//! Every case drives the real request path through the `Dsql` harness in
//! `tests/all/harness.rs`, this crate's one test client.

use crate::harness::Dsql;
use crate::identifier_format_refusal::{refused_as_input, the_store_is_untouched};
use crate::resource_identifier_absence::{
    reported_absent, routes, AboveTheMintedRange, IdCase, Route, IDENTIFIER_LEN,
    IDENTIFIER_SYMBOLS, LEADING_SYMBOL_MAX,
};

/// Identifiers written in the alphabet whose symbols spell a value above the
/// range a minted identifier is drawn from: the leading symbol decides that, so
/// the rows walk every symbol past [`LEADING_SYMBOL_MAX`], then repeat single
/// symbols across the whole value and take the two ends of the range above it.
fn values_above_the_minted_range() -> Vec<IdCase> {
    let body: String = IDENTIFIER_SYMBOLS
        .chars()
        .skip(1)
        .take(IDENTIFIER_LEN - 1)
        .collect();
    let mut cases = Vec::new();

    for leading in IDENTIFIER_SYMBOLS
        .chars()
        .filter(|symbol| *symbol > LEADING_SYMBOL_MAX)
    {
        cases.push(IdCase {
            name: format!("a leading {leading:?}, above the minted range"),
            value: format!("{leading}{body}"),
        });
    }

    // One symbol repeated across the whole value: the leading symbol carries it
    // above the range, and nothing about the rest of the value changes that.
    for symbol in ['a', 'b', 'j', 'z'] {
        cases.push(IdCase {
            name: format!("{symbol:?} repeated across the whole value"),
            value: symbol.to_string().repeat(IDENTIFIER_LEN),
        });
    }

    // The two ends of the range the alphabet spells above a minted identifier:
    // the smallest such value, and the largest the alphabet spells at all.
    let above_the_ceiling = IDENTIFIER_SYMBOLS
        .chars()
        .find(|symbol| *symbol > LEADING_SYMBOL_MAX)
        .expect("the alphabet carries symbols above the minted range");
    cases.push(IdCase {
        name: "the smallest value above the minted range".to_string(),
        value: format!("{above_the_ceiling}{}", "0".repeat(IDENTIFIER_LEN - 1)),
    });
    cases.push(IdCase {
        name: "the largest value the alphabet spells".to_string(),
        value: "z".repeat(IDENTIFIER_LEN),
    });

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
                .all(|symbol| IDENTIFIER_SYMBOLS.contains(symbol)),
            "case {:?}: every row must be written in the identifier alphabet, got {:?}",
            case.name,
            case.value
        );
        assert!(
            case.value
                .chars()
                .next()
                .is_some_and(|leading| leading > LEADING_SYMBOL_MAX),
            "case {:?}: every row must spell a value above the minted range, got {:?}",
            case.name,
            case.value
        );
    }
    cases
}

/// RULE: an identifier written in the alphabet whose symbols spell a value above
/// the minted range is refused as input by the operations that state that range as
/// a precondition, and resolved against the store by every other — so those answer
/// 404 `ResourceNotFoundException` with the ARN-naming sentence, `resourceId` and
/// `resourceType`. Nothing is read or written either way.
#[tokio::test]
async fn an_identifier_above_the_minted_range_is_answered_per_operation() {
    let dsql = Dsql::new();
    // One live cluster, so a row that puts the identifier under test in the stream
    // label is answered for the stream and not for a missing cluster.
    let live = dsql.create_settled().await;

    let routes = routes();
    for case in values_above_the_minted_range() {
        for route in &routes {
            match route.above_the_minted_range {
                AboveTheMintedRange::Refused => refused_as_input(&dsql, route, &live, &case).await,
                AboveTheMintedRange::Absent => reported_absent(&dsql, route, &live, &case).await,
            }
        }
    }

    the_store_is_untouched(&dsql, &live).await;
}

/// RULE, split by operation rather than by value: ONE identifier above the minted
/// range gets both answers on the same store, so the split is a property of the
/// operation and not of the value — the routes that state the range refuse it,
/// the routes that do not report it absent, and every row asserts both for the
/// same string.
#[tokio::test]
async fn the_same_identifier_gets_both_answers_on_different_operations() {
    let dsql = Dsql::new();
    let live = dsql.create_settled().await;

    let routes = routes();
    let gated: Vec<&Route> = routes
        .iter()
        .filter(|route| route.above_the_minted_range == AboveTheMintedRange::Refused)
        .collect();
    let resolving: Vec<&Route> = routes
        .iter()
        .filter(|route| route.above_the_minted_range == AboveTheMintedRange::Absent)
        .collect();
    assert!(
        !gated.is_empty() && !resolving.is_empty(),
        "the route table must carry both answers, got {} gated and {} resolving",
        gated.len(),
        resolving.len()
    );

    for case in values_above_the_minted_range() {
        for route in &gated {
            refused_as_input(&dsql, route, &live, &case).await;
        }
        for route in &resolving {
            reported_absent(&dsql, route, &live, &case).await;
        }
    }

    the_store_is_untouched(&dsql, &live).await;
}

/// RULE, the other way round: an identifier above the minted range is absent
/// rather than a resource, so the resources that DO exist are untouched by one —
/// a stream minted on the live cluster still reads back while the identifier above
/// the range keeps answering 404 on that same stream route.
#[tokio::test]
async fn an_absent_identifier_does_not_disturb_the_resources_that_exist() {
    let dsql = Dsql::new();
    let live = dsql.create_settled().await;

    let created = dsql
        .create_stream(&live, crate::resource_identifier_absence::stream_request())
        .await;
    assert_eq!(
        created.status, 200,
        "CreateStream must be served, got {} {:?}",
        created.status, created.body
    );
    let stream = created.str("streamIdentifier");

    for case in values_above_the_minted_range() {
        let answer = dsql.get_stream(&live, &case.value).await;
        assert_eq!(
            answer.status,
            404,
            "case {:?}: a stream identifier above the minted range must answer 404, got {} {:?}",
            case.name,
            answer.status,
            answer.body
        );
        let read = dsql.get_stream(&live, &stream).await;
        assert_eq!(
            read.status,
            200,
            "case {:?}: the minted stream must still be read back, got {} {:?}",
            case.name,
            read.status,
            read.body
        );
    }
}
