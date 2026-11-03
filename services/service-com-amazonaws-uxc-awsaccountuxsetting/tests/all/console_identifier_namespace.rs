// SPDX-License-Identifier: CC0-1.0

//! The catalog holds ONE namespace in both directions, and the discovery list and
//! the admission list are the same list.
//!
//! The rule, in one sentence: an identifier is in this catalog because the
//! published catalog names it, so a service is named ONCE and the near spellings
//! of it the published catalog does not name — a SigV4 signing name / endpoint
//! prefix for a service published under its console home, or the identifier of a
//! service the console files under ANOTHER service's home — are neither reported
//! nor settable (`smithy_query(query="doc", shape="ListServices")`: the returned
//! identifiers are the ones "you can use as values for the `visibleServices`
//! setting", which "controls only the appearance of services in the Amazon Web
//! Services Management Console").
//!
//! Which of a service's own spellings is the published one is the published
//! catalog's choice and not a rule — it names Amazon EMR `emr` and Amazon MSK
//! `kafka` — and `catalog_published_spellings` owns that; the rows below are the
//! services it names by their console home, so for each of them the home is the
//! entry and the prefix is not.
//!
//! Why both directions in one suite: `ListServices` and
//! `UpdateAccountCustomizations` read the same catalog — the one is a discovery
//! list and the other an admission list — so an entry that is discoverable but not
//! settable, or an identifier the crate's own documentation presents as a console
//! home while the catalog refuses it, is the same defect seen from two sides.
//! `service_catalog` asserts presence and absence over the reported pages; what
//! this suite adds is that each reported identifier is also ACCEPTED and each
//! unreported one also REFUSED, over rows the other suites do not carry.
//!
//! Source of the rows: the console-home namespace the catalog documents itself as
//! holding (`crate::wire::whole_catalog`'s subject, stated in
//! `src/state.rs`) — each row names a service whose console home identifier
//! differs from its endpoint prefix, or a service the console publishes no home of
//! its own for. Every row is re-checked against the model's `Service` constraints
//! and against the catalog the emulator actually publishes before it is relied on.
//!
//! [`Row`] and [`assert_row`] are this crate's ONE statement of "exactly one of
//! these two spellings is published, settable and read back", so
//! `catalog_published_spellings` asserts its own table through them rather than
//! restating the pair of assertions in its own words.

use crate::wire::{
    build_router, get_customizations, is_legal_service, update_customizations, whole_catalog,
};
use serde_json::{json, Value};

/// The error code this service's model declares for a `visibleServices` member the
/// catalog does not name (`smithy_query(query="operation",
/// operation="UpdateAccountCustomizations")`).
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The wording of that refusal.
const INVALID_SERVICES: &str = "Invalid services provided";

/// One row of either table: the identifier the console publishes (so the catalog
/// must report it and the setting must accept it) and the identifier it must be
/// published INSTEAD OF (so the catalog must not report it and the setting must
/// refuse it).
pub(crate) struct Row {
    pub(crate) case: &'static str,
    pub(crate) published: &'static str,
    pub(crate) withheld: &'static str,
}

/// The rule, over one row: both identifiers are values the model admits, exactly
/// the published one is in the catalog, it is settable and reads back, and the
/// withheld one is refused with the modelled `ValidationException` without
/// disturbing what is stored.
pub(crate) async fn assert_row(catalog: &[String], row: &Row) {
    let Row {
        case,
        published,
        withheld,
    } = *row;

    assert!(
        is_legal_service(published) && is_legal_service(withheld),
        "case {case:?}: both identifiers must satisfy Service @length(1, 64) and \
         @pattern ^[a-z0-9]+(-[a-z0-9]+)*$, got {published:?} and {withheld:?}"
    );
    assert!(
        catalog.iter().any(|entry| entry == published),
        "case {case:?}: expected the console identifier {published:?} in the catalog, got {} \
         entries starting {:?}",
        catalog.len(),
        &catalog[..catalog.len().min(5)]
    );
    assert!(
        !catalog.iter().any(|entry| entry == withheld),
        "case {case:?}: expected {withheld:?} NOT to be reported as a console service \
         identifier, got it at index {:?}",
        catalog.iter().position(|entry| entry == withheld)
    );

    let router = build_router();
    let (status, error_type, accepted) =
        update_customizations(&router, json!({"visibleServices": [published]})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: everything the catalog reports must be settable, got {status} \
         [{error_type}] ({accepted}) for {published:?}"
    );
    assert_eq!(
        accepted.get("visibleServices"),
        Some(&json!([published])),
        "case {case:?}: the accepted identifier {published:?} must be read back, got {accepted:?}"
    );

    let (status, error_type, refused) =
        update_customizations(&router, json!({"visibleServices": [withheld]})).await;
    assert_eq!(
        status, 400,
        "case {case:?}: {withheld:?} names no catalog entry, so it must be refused, got {status} \
         ({refused})"
    );
    assert_eq!(
        error_type, VALIDATION_EXCEPTION,
        "case {case:?}: expected the modelled {VALIDATION_EXCEPTION} for {withheld:?}, got \
         {error_type:?} ({refused})"
    );
    assert_eq!(
        refused.get("message").and_then(Value::as_str),
        Some(INVALID_SERVICES),
        "case {case:?}: expected the message {INVALID_SERVICES:?} for {withheld:?}, got {refused:?}"
    );

    let (status, _, fetched) = get_customizations(&router).await;
    assert_eq!(
        status, 200,
        "case {case:?}: reading the record back must succeed, got {status} ({fetched})"
    );
    assert_eq!(
        fetched.get("visibleServices"),
        Some(&json!([published])),
        "case {case:?}: the refused update must leave {published:?} stored, got {fetched:?}"
    );
}

/// A service whose console home identifier and endpoint prefix are DIFFERENT
/// strings: the console identifier is the entry, the prefix is not an entry too.
///
/// Rows differ along how the two spellings diverge — the console identifier
/// longer than the prefix (`systems-manager`/`ssm`, `singlesignon`/`sso`,
/// `directoryservice`/`ds`), one segment and two, an unrelated word
/// (`keyspaces`/`cassandra`), an abbreviation of the same words, and a SECOND
/// signing name of one service — its data plane (`sagemaker-runtime`) or a
/// companion API (`cognito-identity`) — which is no more an entry than the first;
/// and in size, from the three characters of `emr` to the seventeen of
/// `sagemaker-runtime`.
#[tokio::test]
async fn one_service_is_named_once_by_its_console_identifier_and_never_by_its_prefix() {
    let catalog = whole_catalog(&build_router()).await;

    let rows: [Row; 8] = [
        Row {
            case: "AWS Systems Manager",
            published: "systems-manager",
            withheld: "ssm",
        },
        Row {
            case: "IAM Identity Center",
            published: "singlesignon",
            withheld: "sso",
        },
        Row {
            case: "Amazon SageMaker's runtime signing name",
            published: "sagemaker",
            withheld: "sagemaker-runtime",
        },
        Row {
            case: "Amazon Cognito identity pools",
            published: "cognito",
            withheld: "cognito-identity",
        },
        Row {
            case: "Amazon Keyspaces",
            published: "keyspaces",
            withheld: "cassandra",
        },
        Row {
            case: "AWS Directory Service",
            published: "directoryservice",
            withheld: "ds",
        },
        Row {
            case: "Amazon Cognito",
            published: "cognito",
            withheld: "cognito-idp",
        },
        Row {
            case: "Amazon EMR",
            published: "emr",
            withheld: "elasticmapreduce",
        },
    ];

    for row in &rows {
        assert_row(&catalog, row).await;
    }
}

/// A service the console files under ANOTHER service's home: that home is the
/// entry, and the filed service's own identifier is not — which also means every
/// home the catalog's own omission rationale redirects a caller to is itself
/// discoverable and settable there.
///
/// Rows differ along what is filed where: a feature of another service
/// (`access-analyzer`, `tagging`), a separately-branded service with its own
/// endpoint prefix (`appconfig`, `logs`), and a second service filed under one
/// home already covered by another row (`monitoring` beside `logs`).
#[tokio::test]
async fn a_service_filed_under_another_console_home_is_published_as_that_home() {
    let catalog = whole_catalog(&build_router()).await;

    let rows: [Row; 5] = [
        Row {
            case: "IAM Access Analyzer, under the IAM console home",
            published: "iam",
            withheld: "access-analyzer",
        },
        Row {
            case: "AWS AppConfig, under the Systems Manager console home",
            published: "systems-manager",
            withheld: "appconfig",
        },
        Row {
            case: "the Resource Groups Tagging API, under the Resource Groups console home",
            published: "resource-groups",
            withheld: "tagging",
        },
        Row {
            case: "CloudWatch Logs, under the CloudWatch console home",
            published: "cloudwatch",
            withheld: "logs",
        },
        Row {
            case: "CloudWatch metrics, under the CloudWatch console home",
            published: "cloudwatch",
            withheld: "monitoring",
        },
    ];

    for row in &rows {
        assert_row(&catalog, row).await;
    }
}
