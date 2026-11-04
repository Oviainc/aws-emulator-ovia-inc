// SPDX-License-Identifier: CC0-1.0

//! The paging rule EVERY listing of this service answers with, driven through each
//! listing in one table, end to end over the wire.
//!
//! THE RULE UNDER TEST, single-homed in `crate::state` (`resume_after_sequence` /
//! `resume_after_name`, `page_size`, `list_page`) and reached by every one of these
//! operations: a listing reports its stored records in that family's own total order,
//! narrowed by the filters the request carries, in pages of at most `MaxResults`
//! (default 5); a page carries a `NextToken` only when a FURTHER record the filters
//! keep exists past it, and that token resumes the next page exactly where this one
//! stopped — so walking the pages visits every matching record once, in order; and a
//! `NextToken` this service never handed out is refused with `BadRequestException`
//! naming the listing that would have handed one out.
//!
//! The rows differ along what the rule generalises over: the listing (and so the
//! record family, the shape each record is projected onto, the member carrying the
//! page and the order it is taken in), the page size, and whether a name filter
//! narrows the page. The creates are the family table
//! `crate::tagging_resource_families::families` already holds, so no second copy of
//! them lives here.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, reusing the transcription-job suite's harness
//! (`crate::transcription_jobs`), so these tests exercise the awsJson1_1 envelope and
//! the boundary constraint validation too.

use serde_json::{json, Value};

use crate::tagging_resource_families::families;
use crate::transcription_jobs::{Reply, Transcribe};

/// The total order a listing takes its page in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Order {
    /// "Jobs are ordered by creation date, with the newest job first" — the order
    /// every sequence-paged listing of this service documents.
    NewestFirst,
    /// The store's own key order, the record name ascending: the order the one
    /// listing whose records carry no creation sequence is paged in.
    NameAscending,
}

/// One listing of this service: the operation, the create that establishes a record
/// it reports, and how to read the page it answers with.
///
/// `pub(crate)`, like [`listings`] and [`CREATED_IN_ORDER`]: `crate::list_cursor_rule`
/// drives the SAME listings through the same creates rather than writing a second copy
/// of this table.
pub(crate) struct Listing {
    /// The list operation — also the name its refusal of an alien cursor must name.
    pub(crate) operation: &'static str,
    /// The create that establishes one record this listing reports, as
    /// `crate::tagging_resource_families::families` spells it.
    create: &'static str,
    /// The output member carrying the page.
    page_member: &'static str,
    /// The member of each listed record that names it.
    name_member: &'static str,
    /// The request member this listing's case-insensitive substring search is spelled
    /// with, or `None` for the one listing that publishes no such filter.
    name_contains_member: Option<&'static str>,
    pub(crate) order: Order,
}

/// Every listing this service serves, each reached through the same paging rule.
///
/// The page member, the member naming each record and the search member are the ones
/// each operation models (`smithy_query(query="operation", operation="\
/// ListTranscriptionJobs,ListMedicalTranscriptionJobs,ListMedicalScribeJobs,\
/// ListCallAnalyticsJobs,ListVocabularies,ListMedicalVocabularies,\
/// ListVocabularyFilters,ListLanguageModels,ListCallAnalyticsCategories")`).
pub(crate) fn listings() -> Vec<Listing> {
    vec![
        Listing {
            operation: "ListTranscriptionJobs",
            create: "StartTranscriptionJob",
            page_member: "TranscriptionJobSummaries",
            name_member: "TranscriptionJobName",
            name_contains_member: Some("JobNameContains"),
            order: Order::NewestFirst,
        },
        Listing {
            operation: "ListMedicalTranscriptionJobs",
            create: "StartMedicalTranscriptionJob",
            page_member: "MedicalTranscriptionJobSummaries",
            name_member: "MedicalTranscriptionJobName",
            name_contains_member: Some("JobNameContains"),
            order: Order::NewestFirst,
        },
        Listing {
            operation: "ListMedicalScribeJobs",
            create: "StartMedicalScribeJob",
            page_member: "MedicalScribeJobSummaries",
            name_member: "MedicalScribeJobName",
            name_contains_member: Some("JobNameContains"),
            order: Order::NewestFirst,
        },
        Listing {
            operation: "ListCallAnalyticsJobs",
            create: "StartCallAnalyticsJob",
            page_member: "CallAnalyticsJobSummaries",
            name_member: "CallAnalyticsJobName",
            name_contains_member: Some("JobNameContains"),
            order: Order::NewestFirst,
        },
        Listing {
            operation: "ListVocabularies",
            create: "CreateVocabulary",
            page_member: "Vocabularies",
            name_member: "VocabularyName",
            name_contains_member: Some("NameContains"),
            order: Order::NewestFirst,
        },
        Listing {
            operation: "ListMedicalVocabularies",
            create: "CreateMedicalVocabulary",
            page_member: "Vocabularies",
            name_member: "VocabularyName",
            name_contains_member: Some("NameContains"),
            order: Order::NewestFirst,
        },
        Listing {
            operation: "ListVocabularyFilters",
            create: "CreateVocabularyFilter",
            page_member: "VocabularyFilters",
            name_member: "VocabularyFilterName",
            name_contains_member: Some("NameContains"),
            order: Order::NewestFirst,
        },
        Listing {
            operation: "ListLanguageModels",
            create: "CreateLanguageModel",
            page_member: "Models",
            name_member: "ModelName",
            name_contains_member: Some("NameContains"),
            order: Order::NewestFirst,
        },
        Listing {
            operation: "ListCallAnalyticsCategories",
            create: "CreateCallAnalyticsCategory",
            page_member: "Categories",
            name_member: "CategoryName",
            // `ListCallAnalyticsCategoriesRequest` carries only `NextToken` and
            // `MaxResults`: this family publishes no search member.
            name_contains_member: None,
            order: Order::NameAscending,
        },
    ]
}

/// The names this suite creates, in creation order.
///
/// Neither ascending nor descending by name, and mixed in case, so a newest-first
/// listing cannot be confused with a name-ordered one and a case-insensitive search
/// has something to fold: `ET` is a substring of exactly two of them, and of neither
/// the first nor the last of either order.
pub(crate) const CREATED_IN_ORDER: &[&str] = &[
    "Zeta-1", "alpha-2", "Mid-3", "beta-4", "Gamma-5", "delta-6",
];

/// The needle the search rows use, in a case no stored name is spelled in.
const NEEDLE: &str = "ET";

impl Listing {
    /// This listing's create request for a record named `name`.
    ///
    /// Reuses the family table rather than writing a second set of create requests: it
    /// holds a row for every family this service stores records of, each family's ARN
    /// resource type included.
    fn create_request(&self, name: &str) -> Value {
        self.family().create_request(name, &[])
    }

    /// The family row this listing reports the records of.
    fn family(&self) -> crate::tagging_resource_families::Family {
        families()
            .into_iter()
            .find(|family| family.create == self.create)
            .unwrap_or_else(|| panic!("no family row creates with {}", self.create))
    }

    /// Whether a request served in `region` reaches this listing at all: the availability
    /// of the FAMILY it reports
    /// (`crate::tagging_resource_families::Family::is_served_in`), so a row driving one
    /// listing through two Regions asks this before it uses either.
    pub(crate) fn is_served_in(&self, region: &str) -> bool {
        self.family().is_served_in(region)
    }

    /// Establish one record per name in [`CREATED_IN_ORDER`], in that order, in the
    /// Region this emulator serves a request carrying no SigV4 credential scope in.
    pub(crate) async fn create_the_records(&self, transcribe: &Transcribe) {
        self.create_records(transcribe, CREATED_IN_ORDER, None).await;
    }

    /// Establish one record per name in `names`, in that order, through the endpoint of
    /// `region` when one is given.
    ///
    /// What [`Self::create_the_records`] is for every row of this suite; a suite that
    /// drives one family over two Regions, or over a subset of the names, reaches the
    /// same creates through this.
    pub(crate) async fn create_records(
        &self,
        transcribe: &Transcribe,
        names: &[&str],
        region: Option<&str>,
    ) {
        for name in names {
            let case = format!("{} creates {name}", self.create);
            transcribe
                .call_in_region(self.create, self.create_request(name), region)
                .await
                .ok(&case);
        }
    }

    /// Every created name in the order this listing reports them.
    pub(crate) fn expected_order(&self) -> Vec<String> {
        self.order_of(CREATED_IN_ORDER)
    }

    /// `created`, in the order this listing reports the records created under those
    /// names: the creation order reversed for a listing documented newest-first, and the
    /// names ascending for the one listing paged by name.
    pub(crate) fn order_of(&self, created: &[&str]) -> Vec<String> {
        let mut names: Vec<String> = created.iter().map(|n| (*n).to_string()).collect();
        match self.order {
            // The reverse of the creation order.
            Order::NewestFirst => names.reverse(),
            // The store's key order, which is the name ascending.
            Order::NameAscending => names.sort(),
        }
        names
    }

    /// The names of one page, in the order it reported them.
    fn names_of(&self, body: &Value, case: &str) -> Vec<String> {
        body[self.page_member]
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a {} list, got {body:?}",
                    self.page_member
                )
            })
            .iter()
            .map(|record| {
                record[self.name_member]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!(
                            "case {case:?}: every listed record names itself in {}, got \
                             {record:?}",
                            self.name_member
                        )
                    })
                    .to_string()
            })
            .collect()
    }

    /// One call of this listing, through the endpoint of the Region this emulator serves
    /// a request carrying no SigV4 credential scope in.
    pub(crate) async fn list(&self, transcribe: &Transcribe, request: Value) -> Reply {
        self.list_in(transcribe, request, None).await
    }

    /// One call of this listing, through `region`'s endpoint when one is given.
    pub(crate) async fn list_in(
        &self,
        transcribe: &Transcribe,
        request: Value,
        region: Option<&str>,
    ) -> Reply {
        transcribe
            .call_in_region(self.operation, request, region)
            .await
    }

    /// One page: the names on it and the cursor it handed out, if any.
    pub(crate) async fn page(
        &self,
        transcribe: &Transcribe,
        request: Value,
        case: &str,
    ) -> (Vec<String>, Option<String>) {
        self.page_in(transcribe, request, None, case).await
    }

    /// [`Self::page`] through `region`'s endpoint when one is given.
    pub(crate) async fn page_in(
        &self,
        transcribe: &Transcribe,
        request: Value,
        region: Option<&str>,
        case: &str,
    ) -> (Vec<String>, Option<String>) {
        let reply = self.list_in(transcribe, request, region).await;
        let body = reply.ok(case);
        (
            self.names_of(body, case),
            body["NextToken"].as_str().map(str::to_string),
        )
    }

    /// Walk every page of `request` with `page_size`, answering the names in the order
    /// the walk saw them.
    async fn walk(&self, transcribe: &Transcribe, request: &Value, page_size: u64) -> Vec<String> {
        let case = format!("{} MaxResults={page_size}", self.operation);
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut page_request = request.clone();
            page_request["MaxResults"] = json!(page_size);
            if let Some(token) = &token {
                page_request["NextToken"] = json!(token);
            }
            let (names, next) = self.page(transcribe, page_request, &case).await;
            assert!(
                names.len() as u64 <= page_size,
                "case {case:?}: a page holds at most {page_size} records, got {names:?}"
            );
            seen.extend(names);
            token = next;
            if token.is_none() {
                break;
            }
            assert!(
                seen.len() <= CREATED_IN_ORDER.len(),
                "case {case:?}: paging must terminate, got {seen:?}"
            );
        }
        seen
    }
}

/// Every listing walks its whole family, in that family's order, over every page size:
/// no record repeated, none missed, and the last page carrying no cursor.
#[tokio::test]
async fn every_listing_pages_through_its_whole_family_once() {
    for listing in listings() {
        let transcribe = Transcribe::new();
        listing.create_the_records(&transcribe).await;
        let expected = listing.expected_order();

        // 1 and 100 are the published bounds of `MaxResults` (`@range(min: 1, max:
        // 100)`); 5 is the documented default; 4 is a size the record count is not a
        // multiple of, so the last page is a partial one.
        for page_size in [1_u64, 4, 5, 100] {
            let case = format!("{} MaxResults={page_size}", listing.operation);
            let seen = listing.walk(&transcribe, &json!({}), page_size).await;
            assert_eq!(
                seen, expected,
                "case {case:?}: paging must walk every record once, in order, got \
                 {seen:?}"
            );
        }
    }
}

/// A page is cursored only when a FURTHER record the filters keep exists past it —
/// never because the page happened to fill.
#[tokio::test]
async fn a_cursor_is_handed_out_only_when_a_further_matching_record_exists() {
    let total = CREATED_IN_ORDER.len() as u64;
    for listing in listings() {
        let transcribe = Transcribe::new();
        listing.create_the_records(&transcribe).await;
        let expected = listing.expected_order();

        // A page that holds the whole family is the last page, even though it is
        // exactly full.
        let case = format!("{} MaxResults={total}", listing.operation);
        let (names, token) = listing
            .page(&transcribe, json!({"MaxResults": total}), &case)
            .await;
        assert_eq!(
            names, expected,
            "case {case:?}: one page must hold the whole family, got {names:?}"
        );
        assert!(
            token.is_none(),
            "case {case:?}: a page with nothing past it carries no cursor, got {token:?}"
        );

        // One short of the whole family leaves exactly one record past the page, so
        // this page IS cursored, and the cursor resumes at that record — never at the
        // one that overflowed the previous page.
        let case = format!("{} MaxResults={}", listing.operation, total - 1);
        let (names, token) = listing
            .page(&transcribe, json!({"MaxResults": total - 1}), &case)
            .await;
        assert_eq!(
            names,
            expected[..expected.len() - 1].to_vec(),
            "case {case:?}: the page must hold all but the last record, got {names:?}"
        );
        let token = token.unwrap_or_else(|| {
            panic!("case {case:?}: a page with a record past it must carry a cursor")
        });
        let (names, token) = listing
            .page(&transcribe, json!({"NextToken": token}), &case)
            .await;
        assert_eq!(
            names,
            vec![expected[expected.len() - 1].clone()],
            "case {case:?}: the resumed page must hold exactly the last record, got \
             {names:?}"
        );
        assert!(
            token.is_none(),
            "case {case:?}: the final page carries no cursor, got {token:?}"
        );

        // An empty store answers an empty page and no cursor rather than refusing.
        let fresh = Transcribe::new();
        let case = format!("{} over an empty store", listing.operation);
        let (names, token) = listing.page(&fresh, json!({}), &case).await;
        assert!(
            names.is_empty() && token.is_none(),
            "case {case:?}: an empty store lists nothing and hands out no cursor, got \
             {names:?} and {token:?}"
        );
    }
}

/// The search filter narrows the page case-insensitively, composes with paging, and
/// decides on its own whether a cursor is owed: a page holding every MATCH is the last
/// page even when non-matching records follow it.
#[tokio::test]
async fn a_filtered_listing_pages_over_its_matches_alone() {
    let matching: Vec<&str> = CREATED_IN_ORDER
        .iter()
        .copied()
        .filter(|name| name.to_lowercase().contains(&NEEDLE.to_lowercase()))
        .collect();
    assert_eq!(
        matching.len(),
        2,
        "the table must hold exactly two names the needle matches, got {matching:?}"
    );

    for listing in listings() {
        let Some(search_member) = listing.name_contains_member else {
            continue;
        };
        let transcribe = Transcribe::new();
        listing.create_the_records(&transcribe).await;
        let expected: Vec<String> = listing
            .expected_order()
            .into_iter()
            .filter(|name| matching.contains(&name.as_str()))
            .collect();

        // The search is not case sensitive, so the needle matches whatever case it is
        // spelled in; a needle no name holds matches nothing.
        for needle in [NEEDLE.to_string(), NEEDLE.to_lowercase(), "Et".to_string()] {
            let case = format!("{} {search_member}={needle}", listing.operation);
            let request = json!({search_member: needle, "MaxResults": 100});
            let (names, token) = listing.page(&transcribe, request, &case).await;
            assert_eq!(
                names, expected,
                "case {case:?}: the search must narrow the page to its matches, got \
                 {names:?}"
            );
            assert!(
                token.is_none(),
                "case {case:?}: an unfilled page carries no cursor, got {token:?}"
            );
        }
        let case = format!("{} {search_member}=no-such-name", listing.operation);
        let (names, token) = listing
            .page(
                &transcribe,
                json!({search_member: "no-such-name", "MaxResults": 100}),
                &case,
            )
            .await;
        assert!(
            names.is_empty() && token.is_none(),
            "case {case:?}: a needle nothing holds matches nothing, got {names:?} and \
             {token:?}"
        );

        // A page holding every match is the LAST page, even though records the filter
        // drops follow it in the order.
        let case = format!("{} {search_member}={NEEDLE} MaxResults=2", listing.operation);
        let (names, token) = listing
            .page(
                &transcribe,
                json!({search_member: NEEDLE, "MaxResults": matching.len()}),
                &case,
            )
            .await;
        assert_eq!(
            names, expected,
            "case {case:?}: the page must hold both matches, got {names:?}"
        );
        assert!(
            token.is_none(),
            "case {case:?}: a page past which no MATCH exists carries no cursor, got \
             {token:?}"
        );

        // The filter composes with paging: walking a filtered listing one record at a
        // time visits the matches, in order, and nothing else.
        let request = json!({search_member: NEEDLE});
        let seen = listing.walk(&transcribe, &request, 1).await;
        assert_eq!(
            seen, expected,
            "case {:?}: a filtered walk must visit the matches alone, got {seen:?}",
            listing.operation
        );
    }
}

/// A `NextToken` this service never handed out is refused — with the same words from
/// every listing, each naming the listing that would have handed one out.
#[tokio::test]
async fn every_listing_refuses_a_cursor_it_did_not_hand_out() {
    let transcribe = Transcribe::new();
    for listing in listings() {
        // Creation sequences start at 1 and every record name is non-empty, so none of
        // these is a cursor this service ever wrote — in either of its two token
        // formats. Listing from the top instead would hide a caller's paging bug.
        for token in [
            "not-a-cursor",
            "0|absent",
            "-1|absent",
            "name|",
            "|",
            "99999999999999999999999999",
        ] {
            let case = format!("{} resumed with {token:?}", listing.operation);
            let reply = listing
                .list(&transcribe, json!({"NextToken": token}))
                .await;
            // Every client error of this service answers HTTP 400 and names its shape
            // in the body (the class is spanned by `crate::client_error_wire_status`).
            reply.refused(&case, 400, "BadRequestException");
            assert_eq!(
                reply.error_message(&case),
                format!(
                    "The NextToken in your request is not valid. Use the NextToken \
                     returned by your previous {} request.",
                    listing.operation
                ),
                "case {case:?}: unexpected refusal, got {:?}",
                reply.body
            );
        }

        // A `MaxResults` one past each published bound is refused by the boundary
        // validator before the handler runs (`@range(min: 1, max: 100)`).
        for max_results in [0, 101] {
            let case = format!("{} MaxResults={max_results}", listing.operation);
            listing
                .list(&transcribe, json!({"MaxResults": max_results}))
                .await
                .refused(&case, 400, "ValidationException");
        }
    }
}
