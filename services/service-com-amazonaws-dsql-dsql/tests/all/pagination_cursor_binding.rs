// SPDX-License-Identifier: CC0-1.0

//! Which walk a `nextToken` is a cursor OF.
//!
//! RULE under test: a paginated read resumes only from a `nextToken` a previous
//! page of the SAME walk issued — the same operation over the same collection —
//! and answers every other value, however well formed, with the modelled
//! `ValidationException` naming `nextToken` rather than with a page starting after
//! some arbitrary key.
//!
//! Every walk of this service is here (`ListClusters` over the account's clusters,
//! `ListStreams` over the streams of EACH of two clusters), and the rows differ
//! along where the offered token came from: the walk's own page (served), another
//! walk's page, the bare hex of an identifier — one the walked collection holds, one
//! it does not, and one of neither case — and a cursor of this walk with one half
//! rewritten (its integrity check, its identifier, its length). What
//! `tests/all/pagination_token_refusal.rs` pins is the SHAPE of the refusal body;
//! what this suite adds is which values draw it, including the values that are
//! identifier-shaped and so cannot be told from an issued cursor by shape alone.

use serde_json::Value;
use service_com_amazonaws_dsql_dsql::state::{cluster_page_scope, stream_page_scope};

use crate::harness::{absent_identifier, Answer, Dsql};
use crate::pagination_token_refusal::{assert_modeled_validation_exception, create_streams};

/// One paginated walk of this service: how to read a page of it, how to read the
/// identifiers back, and the scope the cursors it issues are bound to.
struct Walk {
    /// The operation, for naming the case in a failure.
    operation: &'static str,
    /// The `@required` list member its output carries.
    items_member: &'static str,
    /// The member each of its summaries names the paged resource with.
    identifier_member: &'static str,
    /// The cluster a stream page is addressed at; empty for the cluster walk,
    /// which is not scoped to one.
    cluster: String,
    /// What this walk is, as the service names it to bind its cursors.
    scope: String,
}

impl Walk {
    /// One page of this walk.
    async fn page(&self, dsql: &Dsql, query: &str) -> Answer {
        match self.operation {
            "ListClusters" => dsql.list(query).await,
            _ => dsql.list_streams(&self.cluster, query).await,
        }
    }

    /// How this walk is named in a failure: the operation, plus the cluster a
    /// stream walk is addressed at.
    fn name(&self) -> String {
        match self.operation {
            "ListClusters" => self.operation.to_string(),
            _ => format!("{} of cluster {}", self.operation, self.cluster),
        }
    }

    /// The identifiers one successful page of this walk reported, in order.
    fn identifiers(&self, case: &str, answer: &Answer) -> Vec<String> {
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: {} must answer 200, got {} {:?}",
            self.name(),
            answer.status,
            answer.body
        );
        answer
            .get(self.items_member)
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|item| {
                item.get(self.identifier_member)
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| {
                        panic!(
                            "case {case:?}: a summary must name its {}, got {item:?}",
                            self.identifier_member
                        )
                    })
                    .to_string()
            })
            .collect()
    }

    /// Every identifier this walk holds, read through the walk itself one page of
    /// one entry at a time on the cursors it issues.
    async fn all_identifiers(&self, dsql: &Dsql) -> Vec<String> {
        let mut seen = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let case = format!("{}: the whole walk", self.name());
            let query = match &token {
                Some(cursor) => format!("?max-results=1&next-token={cursor}"),
                None => "?max-results=1".to_string(),
            };
            let page = self.page(dsql, &query).await;
            seen.extend(self.identifiers(&case, &page));
            token = page.get("nextToken").as_str().map(str::to_string);
            if token.is_none() {
                return seen;
            }
            assert!(
                seen.len() <= 16,
                "case {case:?}: the walk did not terminate, got {seen:?}"
            );
        }
    }

    /// The cursor this walk issues for its first entry: the first page of one
    /// entry leaves the rest behind, so it carries a `nextToken`.
    async fn issued_cursor(&self, dsql: &Dsql) -> String {
        let case = format!("{}: the first page of one entry", self.name());
        let page = self.page(dsql, "?max-results=1").await;
        let identifiers = self.identifiers(&case, &page);
        assert_eq!(
            identifiers.len(),
            1,
            "case {case:?}: a page of one entry holds one item, got {identifiers:?}"
        );
        page.get("nextToken")
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: a walk with entries left behind must issue a cursor, got {:?}",
                    page.body
                )
            })
    }
}

/// A store holding every walk of this service with more than one page in it: two
/// clusters, each carrying three streams.
async fn walks(dsql: &Dsql) -> Vec<Walk> {
    let clusters = dsql.create_settled_many(2).await;
    let mut walks = vec![Walk {
        operation: "ListClusters",
        items_member: "clusters",
        identifier_member: "identifier",
        cluster: String::new(),
        scope: cluster_page_scope(),
    }];
    for cluster in &clusters {
        create_streams(dsql, cluster, 3).await;
        walks.push(Walk {
            operation: "ListStreams",
            items_member: "streams",
            identifier_member: "streamIdentifier",
            cluster: cluster.clone(),
            scope: stream_page_scope(cluster),
        });
    }
    walks
}

/// The hex width of the identifier half of a cursor: a cursor carries its
/// integrity check followed by the identifier the page stopped on, and an
/// identifier is `ClusterId`'s and `StreamId`'s 26 characters
/// (`smithy_query(query="shape", shape="ClusterId,StreamId")`: `^[a-z0-9]{26}$`).
const IDENTIFIER_HEX_LEN: usize = 52;

/// The integrity-check half of a cursor this service issued.
fn integrity_half(token: &str) -> &str {
    assert!(
        token.len() > IDENTIFIER_HEX_LEN,
        "an issued cursor carries an integrity check beside its identifier, got {token:?}"
    );
    &token[..token.len() - IDENTIFIER_HEX_LEN]
}

/// An issued cursor with its identifier half replaced by `identifier`, the
/// integrity check it was issued with left as it is.
fn with_identifier(token: &str, identifier: &str) -> String {
    format!("{}{}", integrity_half(token), ::hex::encode(identifier))
}

/// An issued cursor with one character of its integrity check rewritten, its
/// identifier half left as it is.
fn with_rewritten_integrity(token: &str) -> String {
    let mut characters: Vec<char> = token.chars().collect();
    characters[0] = if characters[0] == '0' { '1' } else { '0' };
    characters.into_iter().collect()
}

/// RULE: a page is served only for a `nextToken` a previous page of the SAME walk
/// issued — the cursor of any other walk of this service names no page of this one.
#[tokio::test]
async fn a_cursor_resolves_only_against_the_walk_that_issued_it() {
    let dsql = Dsql::new();
    let walks = walks(&dsql).await;

    let mut issued = Vec::new();
    for walk in &walks {
        issued.push(walk.issued_cursor(&dsql).await);
    }
    let scopes: Vec<&str> = walks.iter().map(|walk| walk.scope.as_str()).collect();
    assert_eq!(
        scopes.len(),
        scopes.iter().collect::<std::collections::BTreeSet<_>>().len(),
        "the walks under test must be distinct walks, got the scopes {scopes:?}"
    );

    for (reader_index, reader) in walks.iter().enumerate() {
        let entries = reader.all_identifiers(&dsql).await;
        assert!(
            entries.len() > 1,
            "case {:?}: a walk under test must hold more than one page, got {entries:?}",
            reader.name()
        );

        for (issuer_index, cursor) in issued.iter().enumerate() {
            // The same cursor is offered with and without `max-results`, so the
            // verdict is the cursor's and not the page size's.
            for max_results in ["", "&max-results=1"] {
                let query = format!("?next-token={cursor}{max_results}");
                let answer = reader.page(&dsql, &query).await;
                if issuer_index == reader_index {
                    let case = format!(
                        "{}: its own cursor, query {query:?}",
                        reader.name()
                    );
                    let resumed = reader.identifiers(&case, &answer);
                    assert_eq!(
                        resumed.first().map(String::as_str),
                        entries.get(1).map(String::as_str),
                        "case {case:?}: a walk resumes after the entry its own cursor \
                         named, got {resumed:?} over {entries:?}"
                    );
                } else {
                    let case = format!(
                        "{}: the cursor {} issued, query {query:?}",
                        reader.name(),
                        walks[issuer_index].name()
                    );
                    assert_modeled_validation_exception(&case, &answer);
                }
            }
        }

        // Nothing a refused cursor touched changed what this walk reports.
        let after = reader.all_identifiers(&dsql).await;
        assert_eq!(
            after,
            entries,
            "case {:?}: reading pages, refused or served, must leave the walk \
             reporting the same entries, got {after:?} after {entries:?}",
            reader.name()
        );
    }
}

/// RULE: a value is a cursor only by having been issued, never by its shape — the
/// bare hex of an identifier, and a cursor of this walk with either half
/// rewritten, name no page of it.
#[tokio::test]
async fn a_value_that_is_only_identifier_shaped_is_not_a_cursor() {
    let dsql = Dsql::new();
    let walks = walks(&dsql).await;

    for walk in &walks {
        let entries = walk.all_identifiers(&dsql).await;
        let held = entries.first().cloned().expect("a walk under test has entries");
        let other_held = entries.get(1).cloned().expect("a walk under test has two entries");
        let issued = walk.issued_cursor(&dsql).await;

        let fabricated = vec![
            // The form the walk's own cursor is built on top of: the identifier
            // of an entry the walked collection really holds, carrying no
            // integrity check at all.
            ("the bare hex of the entry a page stopped on", ::hex::encode(&held)),
            (
                "the bare hex of an identifier the collection does not hold",
                ::hex::encode(absent_identifier()),
            ),
            (
                "the bare hex of an all-digit identifier",
                ::hex::encode("00000000000000000000000000"),
            ),
            (
                "an issued cursor with its integrity check rewritten",
                with_rewritten_integrity(&issued),
            ),
            (
                "an issued cursor carrying another held identifier",
                with_identifier(&issued, &other_held),
            ),
            (
                "an issued cursor carrying an identifier the collection does not hold",
                with_identifier(&issued, &absent_identifier()),
            ),
            (
                "an issued cursor with its last byte cut off",
                issued[..issued.len() - 2].to_string(),
            ),
            (
                "an issued cursor with a byte appended",
                format!("{issued}00"),
            ),
            ("an issued cursor's integrity check alone", integrity_half(&issued).to_string()),
        ];

        for (form, token) in fabricated {
            for max_results in ["", "&max-results=1"] {
                let query = format!("?next-token={token}{max_results}");
                let case = format!("{}: {form}, query {query:?}", walk.name());
                let answer = walk.page(&dsql, &query).await;
                assert_modeled_validation_exception(&case, &answer);
            }
        }

        // The complement: the walk still serves the cursor it issued itself, and
        // the whole walk still reports every entry exactly once.
        let served = walk.page(&dsql, &format!("?next-token={issued}")).await;
        let case = format!("{}: its own cursor after the refusals", walk.name());
        let resumed = walk.identifiers(&case, &served);
        assert_eq!(
            resumed.first().map(String::as_str),
            entries.get(1).map(String::as_str),
            "case {case:?}: the walk resumes after the entry its cursor named, got \
             {resumed:?} over {entries:?}"
        );
        let after = walk.all_identifiers(&dsql).await;
        assert_eq!(
            after, entries,
            "case {case:?}: the refusals must leave the walk reporting the same \
             entries, got {after:?} after {entries:?}"
        );
    }
}
