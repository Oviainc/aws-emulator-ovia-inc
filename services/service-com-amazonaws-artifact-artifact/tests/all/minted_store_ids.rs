// SPDX-License-Identifier: CC0-1.0

//! The one rule every store this service keys by a MINTED id follows, driven end to end
//! over HTTP through one operation from each former copy of it.
//!
//! THE RULE: an id minted for such a store is a fixed prefix followed by 16 lower-case hex
//! digits of a counter that advances on every mint, skipping an id the store already holds
//! — so the ids of one store are distinct and rise with the counter. A store that releases
//! an entry to make room releases its FIRST key, which those fixed-width ids make its
//! OLDEST entry, and keeps minting: reaching the bound releases, it does not refuse.
//!
//! One rule, one implementation, whichever store asks it: the term grants
//! `GetTermForReport` mints, the compliance inquiries `CreateComplianceInquiry` mints and
//! the export records `ExportComplianceInquiry` mints.
//!
//! Every assertion goes over the router, so each case exercises routing, the handler, the
//! persisted state and serialization — the same path a client takes. Each rule is observed
//! where the emulator makes it observable: the `termToken` an acceptance answers, the
//! inquiry id a create answers and the export id inside the document URL an export
//! announces; and, for the bounded half, by what a minted id still reaches — the newest
//! entry resolves and the oldest, released to make room for it, no longer does.
//!
//! The rows differ along what the rule generalises over: which store is keyed, its id
//! prefix, whether that store releases an entry to make room, and how many ids are minted
//! into it (the first few of a fresh store, every id up to its bound, and one past it).

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_artifact_artifact::state::{
    INQUIRY_EXPORT_CAPACITY, TERM_GRANT_CAPACITY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    a_report_requiring_acceptance, call, emulator, get, get_report, get_term_for_report, Answer,
};

/// How many ids a fresh store is asked for in the shape case: enough to pin the first id,
/// the advance to the next and a mint taken while the store already holds entries.
const MINTS_PER_STORE: usize = 3;

/// One store this service keys by an id it mints, named by the operation that mints into
/// it.
#[derive(Clone, Copy, Debug)]
enum Store {
    /// The term grants `GetTermForReport` mints.
    TermGrants,
    /// The compliance inquiries `CreateComplianceInquiry` mints.
    Inquiries,
    /// The export records `ExportComplianceInquiry` mints.
    Exports,
}

/// Every store under test, which is one per former copy of the rule.
const STORES: [Store; 3] = [Store::TermGrants, Store::Inquiries, Store::Exports];

/// The stores that RELEASE an entry to make room, as against the one bounded by refusing
/// the write.
const BOUNDED_STORES: [Store; 2] = [Store::TermGrants, Store::Exports];

impl Store {
    /// The prefix the ids of this store carry, spelled out here rather than imported from
    /// the crate so a changed prefix fails this suite instead of being read back out of
    /// the code under test.
    ///
    /// An inquiry id's prefix is the literal part of `InquiryId`'s
    /// `^compliance-inquiry-[a-zA-Z0-9]{16}$`
    /// (`smithy_query(query="shape",shape="InquiryId")`), which the 16 hex digits
    /// complete; a term token's is free within `GetReport$termToken`'s
    /// `^[a-zA-Z0-9_\-\s]*$` within 1..256 bytes
    /// (`smithy_query(query="operation",operation="GetReport")`); an export id is named
    /// only inside the document URL an export announces, which the model constrains
    /// nowhere.
    fn prefix(self) -> &'static str {
        match self {
            Store::TermGrants => "term",
            Store::Inquiries => "compliance-inquiry",
            Store::Exports => "export",
        }
    }

    /// How many entries this store holds before the oldest is released, or `None` for a
    /// store that releases nothing — an inquiry is a resource the caller created and a
    /// later read must still find it, so that store refuses the create when it is full
    /// instead.
    fn capacity(self) -> Option<usize> {
        match self {
            Store::TermGrants => Some(TERM_GRANT_CAPACITY),
            Store::Inquiries => None,
            Store::Exports => Some(INQUIRY_EXPORT_CAPACITY),
        }
    }
}

/// One id a store minted, and what the caller was handed to reach the entry it keys.
struct Minted {
    id: String,
    /// The document URL an export was announced at; `None` for a store whose entry is
    /// reached by naming the id itself.
    url: Option<String>,
}

/// One emulator ready to mint into `store`, holding whatever resource minting there needs.
struct Fixture {
    store: Store,
    router: MultiServiceRouter,
    /// The report an acceptance is taken on: one requiring the explicit acceptance of its
    /// term, so whether a grant is still held is observable through `GetReport`.
    report: &'static str,
    /// The inquiry an export is taken from.
    inquiry_id: String,
}

/// A fixture for `store`, minting nothing into the store under test.
async fn fixture(store: Store) -> Fixture {
    let router = emulator();
    // An export needs an inquiry to export, so that create is made up front rather than
    // between the mints under test. It mints an INQUIRY id, which is another store's
    // counter.
    let inquiry_id = match store {
        Store::Exports => create_inquiry(&router).await,
        _ => String::new(),
    };
    Fixture {
        store,
        router,
        report: a_report_requiring_acceptance().id,
        inquiry_id,
    }
}

impl Fixture {
    /// Mint one id into this fixture's store, through the operation that mints there.
    async fn mint(&self) -> Minted {
        match self.store {
            Store::TermGrants => {
                let answer =
                    get_term_for_report(&self.router, &format!("?reportId={}", self.report)).await;
                assert_eq!(
                    answer.status, 200,
                    "an acceptance must be served, got {} with {:?}",
                    answer.status, answer.body
                );
                Minted {
                    id: string_at(&answer, "termToken"),
                    url: None,
                }
            }
            Store::Inquiries => Minted {
                id: create_inquiry(&self.router).await,
                url: None,
            },
            Store::Exports => {
                let answer = post(
                    &self.router,
                    "/v1/compliance-inquiry/export",
                    &json!({ "complianceInquiryId": self.inquiry_id }),
                )
                .await;
                assert_eq!(
                    answer.status, 200,
                    "an export must be served, got {} with {:?}",
                    answer.status, answer.body
                );
                let url = string_at(&answer, "documentPresignedUrl");
                Minted {
                    id: export_id_in(&url),
                    url: Some(url),
                }
            }
        }
    }

    /// Whether `minted` still reaches the entry it was minted for, asked the way a client
    /// asks: a grant by downloading the report it is the receipt for, an inquiry by
    /// reading its metadata, an export by following the URL it was announced at.
    async fn resolves(&self, minted: &Minted) -> bool {
        let (status, served, released) = match self.store {
            Store::TermGrants => {
                let answer = get_report(
                    &self.router,
                    &format!("?reportId={}&termToken={}", self.report, minted.id),
                )
                .await;
                (answer.status, 200, 400)
            }
            Store::Inquiries => {
                let answer = get(
                    &self.router,
                    &format!(
                        "/v1/compliance-inquiry/getMetadata?complianceInquiryId={}",
                        minted.id
                    ),
                )
                .await;
                (answer.status, 200, 401)
            }
            Store::Exports => {
                let url = minted
                    .url
                    .as_deref()
                    .unwrap_or_else(|| panic!("an export is reached through its announced URL"));
                // A released export's URL is refused as UNAUTHORISED rather than reported
                // absent: the export plane decides authorization before existence, so a
                // caller cannot tell a released export from one it was never handed.
                (follow(&self.router, url).await, 200, 403)
            }
        };
        if status == served {
            return true;
        }
        assert_eq!(
            status, released,
            "{:?}: {:?} must either reach its entry or report it gone, got HTTP {status}",
            self.store, minted.id
        );
        false
    }
}

async fn post(router: &MultiServiceRouter, uri: &str, body: &Value) -> Answer {
    call(router, "POST", uri, Bytes::from(body.to_string())).await
}

/// `CreateComplianceInquiry` of one submitted query, answering the inquiry id it minted.
async fn create_inquiry(router: &MultiServiceRouter) -> String {
    let answer = post(
        router,
        "/v1/compliance-inquiry/create",
        &json!({
            "name": "Data residency",
            "inquiryContent": { "query": "Where is customer data stored?" }
        }),
    )
    .await;
    assert_eq!(
        answer.status, 202,
        "the create must be accepted, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["complianceInquirySummary"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("the create carries no inquiry id: {:?}", answer.body))
        .to_string()
}

/// The string member `name` of an answer.
fn string_at(answer: &Answer, name: &str) -> String {
    answer.body[name]
        .as_str()
        .unwrap_or_else(|| panic!("the answer carries no {name}: {:?}", answer.body))
        .to_string()
}

/// Follow a document URL exactly as a client does — against the host and path it names,
/// with NO credential of any kind — and report the status it answered.
async fn follow(router: &MultiServiceRouter, url: &str) -> u16 {
    let rest = url
        .split_once("://")
        .unwrap_or_else(|| panic!("{url:?} is not an absolute URL"))
        .1;
    let (host, path) = match rest.find('/') {
        Some(mark) => (&rest[..mark], &rest[mark..]),
        None => (rest, "/"),
    };
    let request = ::http::Request::builder()
        .method("GET")
        .uri(path)
        .header("host", host)
        .body(Bytes::new())
        .expect("the test request is well formed");
    router.dispatch(request).await.status().as_u16()
}

/// The export id inside the document URL an export announced: the segment after the
/// `export` one, as the path an export document is published under spells it
/// (`…/<inquiryId>/export/<exportId>/<object>`).
fn export_id_in(url: &str) -> String {
    let segments: Vec<&str> = url.split('/').collect();
    let mark = segments
        .iter()
        .position(|segment| *segment == "export")
        .unwrap_or_else(|| panic!("{url:?} is not an export document URL"));
    (*segments
        .get(mark + 1)
        .unwrap_or_else(|| panic!("{url:?} names no export id")))
    .to_string()
}

/// The id the rule mints for the `nth` mint into a store carrying `prefix`.
fn expected_id(prefix: &str, nth: usize) -> String {
    format!("{prefix}-{nth:016x}")
}

/// Whether `value` is `prefix` followed by 16 lower-case hex digits — the shape the rule
/// mints, judged here rather than through the crate's own id predicate so a wrong
/// predicate cannot make this suite pass.
fn is_minted_id(value: &str, prefix: &str) -> bool {
    let Some(suffix) = value.strip_prefix(&format!("{prefix}-")) else {
        return false;
    };
    suffix.len() == 16
        && suffix
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

#[tokio::test]
async fn every_minted_store_id_carries_the_one_counter_shape() {
    for store in STORES {
        let fixture = fixture(store).await;
        let prefix = store.prefix();
        let mut previous = String::new();

        for nth in 1..=MINTS_PER_STORE {
            let case = format!("{store:?}: mint {nth}");
            let minted = fixture.mint().await;

            assert!(
                is_minted_id(&minted.id, prefix),
                "case {case:?}: expected {prefix:?} and 16 lower-case hex digits, got {:?}",
                minted.id
            );
            assert_eq!(
                minted.id,
                expected_id(prefix, nth),
                "case {case:?}: the counter advances once per mint, so the {nth} id must be \
                 {:?}, got {:?}",
                expected_id(prefix, nth),
                minted.id
            );
            assert!(
                minted.id > previous,
                "case {case:?}: the ids must rise with the counter, so the store's first key \
                 is its oldest entry; got {:?} after {previous:?}",
                minted.id
            );
            assert!(
                fixture.resolves(&minted).await,
                "case {case:?}: the minted id must key the record it was minted for, and \
                 {:?} reaches none",
                minted.id
            );

            previous = minted.id;
        }
    }
}

#[tokio::test]
async fn a_bounded_store_releases_its_oldest_entry_to_make_room() {
    for store in BOUNDED_STORES {
        let capacity = store
            .capacity()
            .unwrap_or_else(|| panic!("{store:?} is a store that releases to make room"));
        let prefix = store.prefix();
        let fixture = fixture(store).await;

        // Fill the store to its bound. Nothing has had to be released yet, so the first
        // id still reaches its entry.
        let oldest = fixture.mint().await;
        for nth in 2..=capacity {
            let case = format!("{store:?}: mint {nth} of {capacity}");
            let minted = fixture.mint().await;
            assert_eq!(
                minted.id,
                expected_id(prefix, nth),
                "case {case:?}: expected {:?}, got {:?}",
                expected_id(prefix, nth),
                minted.id
            );
        }
        assert!(
            fixture.resolves(&oldest).await,
            "case {store:?}: a store filled to its bound has released nothing, yet {:?} \
             reaches no entry",
            oldest.id
        );

        // One mint past the bound: it is SERVED, it continues the one counter, and what it
        // cost is the OLDEST entry.
        let newest = fixture.mint().await;
        let past_the_bound = capacity + 1;
        assert_eq!(
            newest.id,
            expected_id(prefix, past_the_bound),
            "case {store:?}: reaching the bound releases rather than refuses, so mint \
             {past_the_bound} must be {:?}, got {:?}",
            expected_id(prefix, past_the_bound),
            newest.id
        );
        assert!(
            fixture.resolves(&newest).await,
            "case {store:?}: the newest entry must be held, yet {:?} reaches none",
            newest.id
        );
        assert!(
            !fixture.resolves(&oldest).await,
            "case {store:?}: the oldest entry must have been released to make room, yet \
             {:?} still reaches one",
            oldest.id
        );
    }
}
