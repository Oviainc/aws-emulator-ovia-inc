// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the blank-filter rule at the two listings that filter on a
//! `category`.
//!
//! `tests/all/blank_filter_value.rs` pins the same rule for the readers of the job set;
//! this suite pins it for the two `listBy` listings, whose `Category` is the model's bare
//! `__string` with no length and no pattern
//! (`smithy_query(query="operation",operation="ListPresets,ListJobTemplates",model_path="services/service-com-amazonaws-mediaconvert-mediaconvert/smithy-model/com-amazonaws-mediaconvert-mediaconvert.json")`),
//! so the framework's boundary validation passes the zero-length value and one made only
//! of whitespace through to the handler.
//!
//! The rule under test: a filter member the request CARRIES has to name something to
//! narrow by, and a value made entirely of whitespace names nothing — a category is stored
//! only when it carries characters, so no record could ever be in one — so each listing
//! answers it with `BadRequestException` (400) naming the member, rather than resolving it
//! into a comparison and reporting the empty page that comes out, which a caller could not
//! tell from "the account holds nothing in that category". Leaving the member out is a
//! different request: it asks for no narrowing at all.
//!
//! Its boundary is the other half of the same rule: a category that carries at least one
//! character is compared verbatim, surrounding whitespace included, so it narrows —
//! possibly to nothing — rather than being refused.
//!
//! The blank forms and the refusal sentence are the ones the suites about the other halves
//! of the rule drive their own rows through, so none of them can disagree about which
//! values are blank or how a refusal reads. Every record here is created over HTTP and
//! every case drives the real restJson1 wire path through the crate's one test harness, so
//! what is asserted is the bytes a client would see.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::blank_resource_name::{assert_blank_refusal, blanks};
use super::job_lifecycle::{Emulator, Reply};
use super::job_template_crud::settings as template_settings;

/// The member every refusal below names, spelled as the model spells it.
const MEMBER: &str = "Category";

/// One listing that filters on `category`: how it is created into, how it is asked, and the
/// output member its records come back in.
///
/// The bindings come from the model (`smithy_query(query="digest")`): `GET/POST
/// /2017-08-29/presets` and `GET/POST /2017-08-29/jobTemplates`.
struct Listing {
    /// How a failing assertion names this listing.
    label: &'static str,
    /// The collection URI: `POST` creates one record, `GET` lists them.
    collection: &'static str,
    /// The output member the records come back in.
    items: &'static str,
    /// What a valid create carries BESIDE `name` and `category`, so the only thing a case
    /// varies is the category.
    extra: fn() -> Value,
}

/// Both listings whose set a `category` narrows.
fn listings() -> Vec<Listing> {
    vec![
        Listing {
            label: "ListPresets",
            collection: "/2017-08-29/presets",
            items: "presets",
            // A preset's settings must name the container the output is written in.
            extra: || json!({ "settings": { "containerSettings": { "container": "MP4" } } }),
        },
        Listing {
            label: "ListJobTemplates",
            collection: "/2017-08-29/jobTemplates",
            items: "jobTemplates",
            extra: || json!({ "settings": template_settings(1, 1) }),
        },
    ]
}

impl Listing {
    /// Create one record under `name`, in `category` when a case names one.
    async fn create(&self, emulator: &Emulator, name: &str, category: Option<&str>) -> Reply {
        let mut body = (self.extra)();
        let members = body
            .as_object_mut()
            .expect("the fixture request is a JSON object");
        members.insert("name".to_string(), json!(name));
        if let Some(category) = category {
            members.insert("category".to_string(), json!(category));
        }
        emulator.send("POST", self.collection, Some(body)).await
    }

    /// One page of this listing for `query`.
    async fn list(&self, emulator: &Emulator, query: &str) -> Reply {
        let uri = if query.is_empty() {
            self.collection.to_string()
        } else {
            format!("{}?{}", self.collection, query)
        };
        emulator.send("GET", &uri, None).await
    }

    /// The names one reply reported, in the order they came back.
    fn names(&self, reply: &Reply) -> Vec<String> {
        reply
            .at(&format!("/{}", self.items))
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        item.pointer("/name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Seed the four records every case reads over: two sharing a category, one in a
    /// category made of a padded word (a value that DOES carry characters, so it is stored
    /// verbatim) and one stored under no category at all.
    async fn seed(&self, emulator: &Emulator) {
        for (name, category) in [
            ("alpha-category", Some("promos")),
            ("zeta-category", Some("promos")),
            ("padded-category", Some(" padded ")),
            ("uncategorised", None),
        ] {
            let created = self.create(emulator, name, category).await;
            assert_eq!(
                created.status, 201,
                "case {} fixture {name:?}: expected HTTP 201, got {} with body {}",
                self.label, created.status, created.body
            );
        }
    }
}

/// The ways a blank `category` can arrive beside the rest of a listing request: alone, with
/// each `listBy` value the model declares (`PresetListBy`/`JobTemplateListBy`:
/// `CREATION_DATE | NAME | SYSTEM`), and beside the ordering and paging members.
fn placements() -> Vec<&'static str> {
    vec![
        "category={blank}",
        "listBy=NAME&category={blank}",
        "listBy=CREATION_DATE&category={blank}",
        "listBy=SYSTEM&category={blank}",
        "category={blank}&order=DESCENDING",
        "category={blank}&maxResults=1",
    ]
}

/// Both listings refuse a `category` carrying no characters, naming the member — and report
/// no page for it.
#[tokio::test]
async fn a_blank_category_filter_is_refused_by_both_list_by_listings() {
    for listing in listings() {
        let emulator = Emulator::new();
        listing.seed(&emulator).await;

        for placement in placements() {
            for blank in blanks() {
                let query = placement.replace("{blank}", blank.path);
                let case = format!("{} {query:?} of {}", listing.label, blank.label);
                let refused = listing.list(&emulator, &query).await;

                assert_blank_refusal(&case, MEMBER, &refused);
                assert!(
                    refused.at(&format!("/{}", listing.items)).is_null()
                        && refused.at("/nextToken").is_null(),
                    "case {case:?}: a refused listing reports no page, got body {}",
                    refused.body
                );
            }
        }

        // A refusal leaves the store exactly as it was: every record is still reported, and
        // the category two of them share still narrows to those two.
        let all = listing.list(&emulator, "listBy=NAME").await;
        assert_eq!(
            listing.names(&all),
            vec![
                "alpha-category",
                "padded-category",
                "uncategorised",
                "zeta-category"
            ],
            "case {}: expected the stored records after the refusals, got {:?} from body {}",
            listing.label,
            listing.names(&all),
            all.body
        );
        let promos = listing.list(&emulator, "category=promos").await;
        assert_eq!(
            listing.names(&promos),
            vec!["alpha-category", "zeta-category"],
            "case {}: expected the category to still narrow after the refusals, got {:?}",
            listing.label,
            listing.names(&promos)
        );
    }
}

/// The boundary of the same rule: a `category` that carries at least one character is
/// compared verbatim — whitespace around it is part of the value, so it narrows, and
/// narrows to nothing when no record is stored under it.
#[tokio::test]
async fn a_category_filter_that_carries_characters_narrows_verbatim() {
    for listing in listings() {
        let emulator = Emulator::new();
        listing.seed(&emulator).await;

        let cases: Vec<(&str, &str, Vec<&str>)> = vec![
            (
                "a category two records share",
                "category=promos",
                vec!["alpha-category", "zeta-category"],
            ),
            (
                "a padded category one record holds",
                "category=%20padded%20",
                vec!["padded-category"],
            ),
            // The comparison is byte for byte: the unpadded form of a padded category is a
            // different category, and so are a different case and a prefix of a stored one.
            ("the unpadded form of a padded category", "category=padded", vec![]),
            ("a different case", "category=Promos", vec![]),
            ("a prefix of a stored category", "category=promo", vec![]),
            ("one character no record is in", "category=.", vec![]),
            (
                "no category at all, which narrows nothing",
                "listBy=NAME",
                vec![
                    "alpha-category",
                    "padded-category",
                    "uncategorised",
                    "zeta-category",
                ],
            ),
        ];

        for (label, query, expected) in cases {
            let case = format!("{} with {label}", listing.label);
            let reply = listing.list(&emulator, query).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected HTTP 200, got {} with body {}",
                reply.status, reply.body
            );
            let reported: BTreeSet<String> = listing.names(&reply).into_iter().collect();
            let expected: BTreeSet<String> =
                expected.into_iter().map(str::to_string).collect();
            assert_eq!(
                reported, expected,
                "case {case:?}: expected {expected:?}, got {reported:?} from body {}",
                reply.body
            );
        }
    }
}
