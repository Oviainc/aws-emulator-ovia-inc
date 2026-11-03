// SPDX-License-Identifier: CC0-1.0

//! WHICH spelling of a service the catalog publishes.
//!
//! The rule, in one sentence: the catalog `ListServices` pages over holds the
//! identifiers the published catalog names — each entry is discoverable there and
//! settable as a `visibleServices` value — and a near spelling of a service it
//! does not name is neither reported nor settable, however plausible that spelling
//! is (`smithy_query(query="doc", shape="ListServices")`: the returned identifiers
//! are the ones "you can use as values for the `visibleServices` setting").
//!
//! Which spelling that is cannot be derived from either namespace. The published
//! catalog names Amazon MSK by its service namespace `kafka` and not by the console
//! word `msk`, Amazon Location Service `geo` and not `location`, AWS Health
//! `health-dashboard` and not `health`, and AWS Migration Hub `mgh` and not
//! `migrationhub`, while naming Amazon EMR `emr` and not `elasticmapreduce` —
//! which is why the seed is an observation of that catalog rather than a set of
//! service names derived from one namespace, and why this suite pins the spellings
//! service by service. `service_catalog` and `console_identifier_namespace` own the
//! namespace contrast itself; what this suite adds is the published-versus-near-miss
//! decision, on rows those suites do not carry.
//!
//! Every row drives the real wire path through the shared harness
//! ([`crate::wire`]) and is asserted through
//! [`crate::console_identifier_namespace::assert_row`], which re-checks each
//! spelling against the model's `Service` constraints, then requires the published
//! one to be reported, settable and readable back and the withheld one to be
//! refused with the modelled `ValidationException` without disturbing what is
//! stored.

use crate::console_identifier_namespace::{assert_row, Row};
use crate::support::reported;
use crate::wire::{build_router, get_customizations, strings, update_customizations, whole_catalog};
use serde_json::json;

/// The rule, over the services whose published spelling and withheld spelling are
/// both legal, plausible identifiers.
///
/// Rows differ along WHY the two spellings differ — the published one is the
/// service namespace where the withheld one is the console word (`kafka`/`msk`,
/// `geo`/`location`, `mgh`/`migrationhub`), the published one names the product
/// where the withheld one names the bare service (`health-dashboard`/`health`), the
/// published one is unsegmented where the withheld one hyphenates the same words
/// (`gameliftservers`/`gamelift-servers`), the published one is the current product
/// name where the withheld one is the former one
/// (`infrastructure-composer`/`application-composer`), and the published one is a
/// two-character identifier the withheld one prefixes (`iq`/`aws-iq`) — and in
/// size, from two characters to twenty-four.
#[tokio::test]
async fn a_service_is_published_under_one_spelling_and_not_under_a_near_miss_of_it() {
    let catalog = whole_catalog(&build_router()).await;

    let rows: [Row; 9] = [
        Row {
            case: "Amazon Managed Streaming for Apache Kafka",
            published: "kafka",
            withheld: "msk",
        },
        Row {
            case: "Amazon Location Service",
            published: "geo",
            withheld: "location",
        },
        Row {
            case: "AWS Migration Hub",
            published: "mgh",
            withheld: "migrationhub",
        },
        Row {
            case: "AWS Health Dashboard",
            published: "health-dashboard",
            withheld: "health",
        },
        Row {
            case: "Amazon GameLift Servers",
            published: "gameliftservers",
            withheld: "gamelift-servers",
        },
        Row {
            case: "Amazon GameLift Streams",
            published: "gameliftstreams",
            withheld: "gamelift-streams",
        },
        Row {
            case: "AWS Infrastructure Composer",
            published: "infrastructure-composer",
            withheld: "application-composer",
        },
        Row {
            case: "AWS IQ",
            published: "iq",
            withheld: "aws-iq",
        },
        Row {
            case: "Amazon Managed Service for Prometheus",
            published: "prometheus",
            withheld: "managed-prometheus",
        },
    ];

    for row in &rows {
        assert_row(&catalog, row).await;
    }
}

/// Siblings are NOT one service: where the published catalog names several homes of
/// one brand, each of them is an entry in its own right, discoverable and settable
/// beside the others — which is what distinguishes a brand's several homes from one
/// service's several spellings.
///
/// Driven end to end: the three homes are discovered through `ListServices`, set in
/// ONE `UpdateAccountCustomizations` request, and read back through
/// `GetAccountCustomizations`.
#[tokio::test]
async fn several_homes_of_one_brand_are_each_an_entry() {
    let router = build_router();
    let catalog = whole_catalog(&router).await;

    // The GameLift homes: the legacy one and the two product homes beside it.
    let siblings = ["gamelift", "gameliftservers", "gameliftstreams"];
    for identifier in siblings {
        assert!(
            catalog.iter().any(|entry| entry == identifier),
            "case {identifier:?}: each home of the brand must be its own catalog entry, got a \
             catalog of {} entries",
            catalog.len()
        );
    }

    let expected = reported(&siblings);
    let (status, error_type, updated) =
        update_customizations(&router, json!({"visibleServices": strings(&siblings)})).await;
    assert_eq!(
        status, 200,
        "case {siblings:?}: every entry the catalog reports must be settable, got {status} \
         [{error_type}] ({updated})"
    );
    assert_eq!(
        updated.get("visibleServices"),
        Some(&expected),
        "case {siblings:?}: expected {expected}, got {updated:?}"
    );

    let (status, _, fetched) = get_customizations(&router).await;
    assert_eq!(
        status, 200,
        "case {siblings:?}: reading the record back must succeed, got {status} ({fetched})"
    );
    assert_eq!(
        fetched.get("visibleServices"),
        Some(&expected),
        "case {siblings:?}: Get must report what Update stored, got {fetched:?}"
    );
}
