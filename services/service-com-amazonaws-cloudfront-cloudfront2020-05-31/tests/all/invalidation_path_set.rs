// SPDX-License-Identifier: CC0-1.0

//! An invalidation batch's paths are the SET of objects to invalidate, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes.
//!
//! The rule under test: `InvalidationBatch$Paths` is "a complex type that contains
//! information about the objects that you want to invalidate", and `Paths$Quantity` "the
//! number of invalidation paths specified for the objects that you want to invalidate"
//! (`smithy_query(query="shape",shape="InvalidationBatch,Paths")`) — so naming one object
//! twice asks for nothing more than naming it once: the stored batch carries each distinct
//! path once, at the position the request first named it, and `Quantity` states how many
//! that is. A path is taken VERBATIM, so two paths differing only in the case of a character
//! name different objects and both stay in the set.
//!
//! The rows span what the rule generalises over: how many paths a batch names, where the
//! repeats sit (adjacent, separated, at the end), how often one path repeats, whether the
//! paths differ only in case, whether a path is a wildcard, and whether anything repeats at
//! all. Each row is asserted from the create AND from `GetInvalidation`, so the set is what
//! was stored rather than what one answer rendered; and because a repeat under one caller
//! reference is the same request again, a batch naming an object twice and one naming it once
//! are asserted to be the same invalidation.
//!
//! The wire harness, the configuration builder and the batch builder are the lifecycle
//! suite's, reused rather than copied.

use crate::distribution_lifecycle::{
    batch_xml, config, create_distribution, create_invalidation, emulator, get_invalidation,
    Answer, Node,
};

/// One batch under test: the paths submitted, and the set they describe.
struct Row {
    case: &'static str,
    submitted: &'static [&'static str],
    /// The distinct paths, in the order the request first names them.
    expected: &'static [&'static str],
}

/// The table every assertion below runs over.
const ROWS: &[Row] = &[
    Row {
        case: "no repeat at all",
        submitted: &["/a.html", "/b.html", "/c.html"],
        expected: &["/a.html", "/b.html", "/c.html"],
    },
    Row {
        case: "an adjacent repeat",
        submitted: &["/a.html", "/a.html"],
        expected: &["/a.html"],
    },
    Row {
        case: "a separated repeat",
        submitted: &["/a.html", "/b.html", "/a.html"],
        expected: &["/a.html", "/b.html"],
    },
    Row {
        case: "one path named four times",
        submitted: &["/only.html", "/only.html", "/only.html", "/only.html"],
        expected: &["/only.html"],
    },
    Row {
        case: "paths differing only in case, with one exact repeat",
        submitted: &[
            "/images/logo.png",
            "/images/Logo.png",
            "/IMAGES/logo.png",
            "/images/logo.png",
        ],
        expected: &["/images/logo.png", "/images/Logo.png", "/IMAGES/logo.png"],
    },
    Row {
        case: "a repeated wildcard beside a repeated path",
        submitted: &["/*", "/index.html", "/*", "/index.html"],
        expected: &["/*", "/index.html"],
    },
    Row {
        case: "paths differing only in a trailing slash",
        submitted: &["/assets/", "/assets", "/assets/"],
        expected: &["/assets/", "/assets"],
    },
    Row {
        case: "a single path",
        submitted: &["/*"],
        expected: &["/*"],
    },
];

/// The text of every `Path` the batch of `answer` reports, in order.
fn reported_paths(document: &Node) -> Vec<String> {
    document
        .at("InvalidationBatch/Paths/Items")
        .map(|items| {
            items
                .children_named("Path")
                .into_iter()
                .map(|path| path.text.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// Assert `answer` reports exactly `expected`, as a set with its own stated length.
fn assert_set(answer: &Answer, case: &str, context: &str, expected: &[&str]) {
    let document = answer.root();
    let paths = reported_paths(document);
    assert_eq!(
        paths, expected,
        "case {case:?} ({context}): expected paths {expected:?}, got {paths:?} in {}",
        answer.raw
    );
    let quantity = document.text_at("InvalidationBatch/Paths/Quantity");
    assert_eq!(
        quantity,
        Some(expected.len().to_string().as_str()),
        "case {case:?} ({context}): expected Paths.Quantity {}, got {quantity:?} in {}",
        expected.len(),
        answer.raw
    );
}

/// `CreateInvalidation` stores the batch as the set of objects it names, and every later read
/// of that invalidation answers with the same set.
#[tokio::test]
async fn an_invalidation_batch_stores_its_paths_as_a_set() {
    let router = emulator();
    let created = create_distribution(&router, &config("paths", "ref-path-set").xml()).await;
    let distribution = created
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();

    for row in ROWS {
        let case = row.case;
        let reference = format!("inv-{}", case.replace(' ', "-"));
        let answer =
            create_invalidation(&router, &distribution, &batch_xml(&reference, row.submitted))
                .await;
        assert_eq!(
            answer.status, 201,
            "case {case:?}: expected HTTP 201, got {} with {}",
            answer.status, answer.raw
        );
        assert_set(&answer, case, "CreateInvalidation", row.expected);

        let id = answer.root().text_at("Id").unwrap_or_default().to_string();
        let read = get_invalidation(&router, &distribution, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 reading the invalidation back, got {} with {}",
            read.status, read.raw
        );
        assert_set(&read, case, "GetInvalidation", row.expected);

        // "If you make a second invalidation request with the same value for CallerReference,
        // and if the rest of the request is the same, CloudFront doesn't create a new
        // invalidation request": two batches naming the same SET ask for the same thing, so
        // the submitted list and the distinct one are the same request again.
        for (context, paths) in [
            ("the same list again", row.submitted),
            ("the distinct list", row.expected),
        ] {
            let repeat =
                create_invalidation(&router, &distribution, &batch_xml(&reference, paths)).await;
            assert_eq!(
                repeat.status, 201,
                "case {case:?} ({context}): expected the repeat to be answered, got {} with {}",
                repeat.status, repeat.raw
            );
            assert_eq!(
                repeat.root().text_at("Id"),
                Some(id.as_str()),
                "case {case:?} ({context}): expected the repeat to answer with the invalidation \
                 already created, got {}",
                repeat.raw
            );
            assert_set(&repeat, case, context, row.expected);
        }
    }
}
