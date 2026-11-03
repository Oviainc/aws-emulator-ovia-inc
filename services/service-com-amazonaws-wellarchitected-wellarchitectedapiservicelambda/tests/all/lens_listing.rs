// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListLenses`: the lens catalog this emulator reports, the
//! filters that select within it and the pages it serves.
//!
//! The rules pinned here, each over a table of inputs rather than one recorded request:
//!
//! 1. The catalog is COMPUTED from state and from the AWS-official lens catalog: every
//!    lens AWS publishes content for here (pinned by `super::official_lens_catalog`), plus
//!    every custom lens the store holds, each
//!    summary's members taken from that lens's own record. Rows differ along the shape of
//!    the imported document — its name, whether it carries a description, how many pillars
//!    and questions it defines — because every lens in the store has to be reported from
//!    the lens itself.
//! 2. `LensType`, `LensName` and `LensStatus` SELECT within that catalog: by the lens's
//!    type, by its full name, and by its publication state — the state each summary
//!    reports as its own `LensStatus`, `DRAFT` until a version has been published from the
//!    content the lens carries and `PUBLISHED` after, with `LensVersion` naming that same
//!    content's version. A request that names no filter asks for no filtering, so a draft
//!    lens is listed too.
//! 3. `MaxResults`/`NextToken` page over the catalog: however small the page, following
//!    the tokens enumerates every lens exactly once, in the same order one page reports,
//!    and the last page carries no token. A token this service did not issue is a 400
//!    `ValidationException` classified `CANNOT_PARSE`.
//! 4. A lens is listed from the moment its import is ACCEPTED, and the content the
//!    listing describes it by is the content the last ACCEPTED import carried. Listing
//!    never changes what the next
//!    caller sees.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client is the shared `super::workload_harness`.

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use serde_json::Value;

use super::custom_lens_import::{import, imported_arn, lens_document, wait_for_the_import_to_land};
use super::custom_lens_versions::create_version;
use super::workload_harness::{encode_path_segment, Answer, Client};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// An account id that is NOT this emulator's, for a lens another account owns. Twelve
/// digits, as `LensSummary$Owner` (`AwsAccountId`, `^[0-9]{12}$`) requires.
const OTHER_ACCOUNT: &str = "210987654321";

/// `GET /lenses`, the wire form of `ListLenses`. Every member of the input is bound to
/// the query string, so `query` is passed as sent — empty for a request that names none.
async fn list(client: &Client, query: &str) -> Answer {
    let uri = if query.is_empty() {
        "/lenses".to_string()
    } else {
        format!("/lenses?{query}")
    };
    client.send("GET", &uri, None).await
}

/// The `LensSummaries` of a successful listing.
fn summaries(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListLenses 200, got {answer:?}"
    );
    answer
        .body
        .get("LensSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `LensAlias` of every summary a listing reported, in the order it reported them.
fn aliases(answer: &Answer, case: &str) -> Vec<String> {
    summaries(answer, case)
        .iter()
        .map(|summary| {
            summary
                .get("LensAlias")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// One summary of a listing, by the `LensAlias` that names it.
fn summary_of(answer: &Answer, alias: &str, case: &str) -> Value {
    summaries(answer, case)
        .into_iter()
        .find(|summary| summary.get("LensAlias").and_then(Value::as_str) == Some(alias))
        .unwrap_or_else(|| panic!("case {case:?}: no summary for {alias:?} in {answer:?}"))
}

fn member(summary: &Value, name: &str) -> String {
    summary
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Percent-encode a filter value so it survives the query string: `LensName` is a full
/// lens name, which carries spaces. The crate's test encoder escapes everything RFC 3986
/// leaves reserved, which is what a query value needs too.
fn query_value(value: &str) -> String {
    encode_path_segment(value)
}

/// Record a custom lens OWNED BY ANOTHER ACCOUNT straight through the state the handlers
/// share, and answer its lens ARN.
///
/// A lens another account shared with the caller has no HTTP path in this emulator:
/// `CreateLensShare` is still a stub with its own issue, and this emulator attributes every
/// request to its one account, so the account that would own the lens cannot sign the
/// import either. The seed records exactly what `ImportLens` records — a lens id, its ARN,
/// its owner and the parsed content — and the READ under test still crosses the HTTP
/// boundary. Every other lens this suite lists is imported, and published, over HTTP.
fn record_shared_lens(client: &Client, owner: &str, document: &Value) -> String {
    let json = document.to_string();
    let definition = state::parse_lens_json(&json).expect("the seeded document is a lens");
    let state_handle = client.store();
    let mut store = state_handle.lock();
    let lens_id = store.mint_lens_id();
    let lens_arn = format!("arn:aws:wellarchitected:us-west-2:{owner}:lens/{lens_id}");
    store.lenses.insert(
        lens_id.clone(),
        state::LensRecord {
            lens_id,
            lens_arn: lens_arn.clone(),
            owner: owner.to_string(),
            created_at: SystemTime::now(),
            tags: BTreeMap::new(),
            draft: Some(state::LensContent {
                json,
                definition,
                imported_at: SystemTime::now(),
            }),
            import_completes_at: None,
            versions: BTreeMap::new(),
        },
    );
    lens_arn
}

/// The `LensArn` the AWS Well-Architected Framework lens carries in the Region these
/// tests sign for: an official lens ARN names no owner account (`LensAlias` doc).
fn official_lens_arn() -> String {
    format!(
        "arn:aws:wellarchitected:us-west-2::lens/{}",
        state::FRAMEWORK_LENS_ALIAS
    )
}

/// The publication instant of the shipped official lens, in epoch seconds — how a
/// restJson1 `Timestamp` travels.
fn framework_published_epoch_seconds() -> f64 {
    state::framework_lens_published_at()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("the shipped lens was published after the Unix epoch")
        .as_secs_f64()
}

fn timestamp(summary: &Value, name: &str) -> Option<f64> {
    summary.get(name).and_then(Value::as_f64)
}

/// The aliases of every AWS-official lens this emulator publishes, in the order the
/// catalog enumerates them (by alias).
///
/// WHAT that catalog publishes — each lens's alias, name, version, description and
/// question set — is pinned by `super::official_lens_catalog`; the suites here are about
/// the FILTERING and PAGING over the lenses available to the caller, so they take the
/// official half of that from the one catalog every reader of a lens goes through.
fn official_aliases() -> Vec<String> {
    let mut aliases: Vec<String> = state::OFFICIAL_LENS_CATALOG
        .iter()
        .map(|official| official.alias.to_string())
        .collect();
    aliases.sort();
    aliases
}

/// Every AWS-official lens's alias, plus the custom lenses a row names: the official half
/// of the catalog is in the selection whenever a row's filters admit it.
fn with_officials<'a>(officials: &'a [String], extra: &[&'a String]) -> Vec<&'a String> {
    officials.iter().chain(extra.iter().copied()).collect()
}

/// Rule 1. Every imported lens is reported from its OWN record, whatever the shape of the
/// document it carries, and every AWS-official lens is reported from its own catalog entry.
#[tokio::test]
async fn the_catalog_reports_every_stored_lens_from_its_own_record() {
    let cases: Vec<(&str, Value)> = vec![
        (
            "one-pillar-one-question",
            lens_document("Minimal listed lens", None, &[("governance", 1, 2)]),
        ),
        (
            "described",
            lens_document(
                "Described listed lens",
                Some("the questions company policy adds to a review"),
                &[("security", 2, 3)],
            ),
        ),
        (
            "many-pillars",
            lens_document(
                "Wide listed lens",
                Some("several pillars of questions"),
                &[("security", 1, 2), ("costOptimization", 3, 2), ("reliability", 2, 4)],
            ),
        ),
    ];

    let client = Client::new();
    let mut imported: Vec<(&str, Value, String)> = Vec::new();
    for (case, document) in &cases {
        let answer = import(
            &client,
            &document.to_string(),
            None,
            &format!("catalog-{case}"),
            None,
        )
        .await;
        imported.push((case, document.clone(), imported_arn(&answer, case)));
    }
    wait_for_the_import_to_land();

    let listed = list(&client, "LensStatus=ALL&MaxResults=50").await;
    let reported = aliases(&listed, "catalog");
    let official = official_aliases();
    assert_eq!(
        reported.len(),
        cases.len() + official.len(),
        "case \"catalog\": every AWS-official lens and every imported lens must be \
         reported, got {reported:?}"
    );
    assert_eq!(
        reported.get(..official.len()).map(<[String]>::to_vec),
        Some(official.clone()),
        "case \"catalog\": the AWS-official lenses are enumerated first, got {reported:?}"
    );

    // The shipped lens: its summary is the catalog content beside its question set, with
    // no `Owner`, because AWS publishes the lens rather than an account owning it.
    let official = summary_of(&listed, state::FRAMEWORK_LENS_ALIAS, "official");
    assert_eq!(
        member(&official, "LensArn"),
        official_lens_arn(),
        "case \"official\": expected the account-less official ARN, got {official:?}"
    );
    assert_eq!(
        member(&official, "LensName"),
        state::FRAMEWORK_LENS_NAME,
        "case \"official\": expected the shipped lens's full name, got {official:?}"
    );
    assert_eq!(
        member(&official, "LensType"),
        state::LENS_TYPE_AWS_OFFICIAL,
        "case \"official\": an AWS lens is AWS_OFFICIAL, got {official:?}"
    );
    assert_eq!(
        member(&official, "LensVersion"),
        state::FRAMEWORK_LENS_VERSION,
        "case \"official\": expected the shipped lens version, got {official:?}"
    );
    assert_eq!(
        member(&official, "LensStatus"),
        state::LENS_STATUS_PUBLISHED,
        "case \"official\": AWS's own lens is published content, got {official:?}"
    );
    assert_eq!(
        member(&official, "Description"),
        state::FRAMEWORK_LENS_DESCRIPTION,
        "case \"official\": expected the shipped lens's description, got {official:?}"
    );
    assert!(
        official.get("Owner").is_none(),
        "case \"official\": an official lens ARN names no owner account, got {official:?}"
    );
    assert_eq!(
        timestamp(&official, "CreatedAt"),
        Some(framework_published_epoch_seconds()),
        "case \"official\": CreatedAt is the shipped version's publication instant, got {official:?}"
    );
    assert_eq!(
        timestamp(&official, "UpdatedAt"),
        Some(framework_published_epoch_seconds()),
        "case \"official\": UpdatedAt is the shipped version's publication instant, got {official:?}"
    );

    for (case, document, lens_arn) in &imported {
        let summary = summary_of(&listed, lens_arn, case);
        assert_eq!(
            member(&summary, "LensArn"),
            *lens_arn,
            "case {case:?}: expected the ARN the import minted, got {summary:?}"
        );
        assert_eq!(
            member(&summary, "LensName"),
            document["name"].as_str().unwrap_or_default(),
            "case {case:?}: the name comes from the imported document, got {summary:?}"
        );
        assert_eq!(
            summary.get("Description").and_then(Value::as_str),
            document.get("description").and_then(Value::as_str),
            "case {case:?}: the description comes from the imported document, got {summary:?}"
        );
        assert_eq!(
            member(&summary, "LensType"),
            state::LENS_TYPE_CUSTOM_SELF,
            "case {case:?}: a lens this account imported is CUSTOM_SELF, got {summary:?}"
        );
        assert_eq!(
            member(&summary, "Owner"),
            engine::emulator_account_id(),
            "case {case:?}: the owner is the importing account, got {summary:?}"
        );
        assert_eq!(
            member(&summary, "LensStatus"),
            state::LENS_STATUS_DRAFT,
            "case {case:?}: a lens no version was published from is DRAFT, got {summary:?}"
        );
        assert_eq!(
            member(&summary, "LensVersion"),
            state::LENS_VERSION_DRAFT,
            "case {case:?}: a lens no version was published from reports its draft, got \
             {summary:?}"
        );
        let created = timestamp(&summary, "CreatedAt");
        let updated = timestamp(&summary, "UpdatedAt");
        assert!(
            created.is_some() && updated.is_some(),
            "case {case:?}: an imported lens carries both instants, got {summary:?}"
        );
        assert!(
            created <= updated,
            "case {case:?}: CreatedAt must not be after UpdatedAt, got {summary:?}"
        );
    }

    // Rule 4's other half: a listing is a REPORT, so a second one answers the same thing.
    let again = list(&client, "LensStatus=ALL&MaxResults=50").await;
    assert_eq!(
        summaries(&again, "re-read"),
        summaries(&listed, "catalog"),
        "case \"re-read\": listing must not change what a later caller sees, got {again:?}"
    );
}

/// Rule 1's two hand-written catalog facts about the shipped lens cannot drift apart: the
/// publication instant its summary reports IS midnight of the day its `LensVersion` names,
/// so bumping the version without the instant (or the other way round) fails here.
#[tokio::test]
async fn the_shipped_lens_version_and_its_publication_instant_name_the_same_day() {
    let published = ::aws_smithy_types::DateTime::from(state::framework_lens_published_at());
    let formatted = published
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .expect("the shipped lens's publication instant formats as a date-time");
    let expected_prefix = format!("{}T00:00:00", state::FRAMEWORK_LENS_VERSION);
    assert!(
        formatted.starts_with(&expected_prefix),
        "case \"shipped-lens-date\": the publication instant must be midnight of the day \
         {:?} names, got {formatted:?}",
        state::FRAMEWORK_LENS_VERSION
    );

    // And the listing reports both of them, from those same two values.
    let client = Client::new();
    let listed = list(&client, "LensType=AWS_OFFICIAL").await;
    let official = summary_of(&listed, state::FRAMEWORK_LENS_ALIAS, "shipped-lens-date");
    assert_eq!(
        member(&official, "LensVersion"),
        state::FRAMEWORK_LENS_VERSION,
        "case \"shipped-lens-date\": expected the shipped lens version, got {official:?}"
    );
    assert_eq!(
        timestamp(&official, "CreatedAt"),
        Some(framework_published_epoch_seconds()),
        "case \"shipped-lens-date\": expected that version's publication instant, got {official:?}"
    );
}

/// Rule 2. The filters select within the catalog: by type, by full name and by
/// publication state, in every combination the table spells out.
#[tokio::test]
async fn the_filters_select_within_the_catalog() {
    let client = Client::new();

    // Three lenses imported over HTTP, each taken to a different publication state below.
    let draft_document = lens_document("Draft policy lens", None, &[("governance", 1, 2)]);
    let published_document = lens_document(
        "Published platform lens",
        Some("in force"),
        &[("security", 1, 2)],
    );
    let newer_document = lens_document("Republished platform lens", None, &[("security", 2, 2)]);
    let draft_arn = imported_arn(
        &import(
            &client,
            &draft_document.to_string(),
            None,
            "filters-draft",
            None,
        )
        .await,
        "filters-draft",
    );
    let published_arn = imported_arn(
        &import(
            &client,
            &published_document.to_string(),
            None,
            "filters-published",
            None,
        )
        .await,
        "filters-published",
    );
    let republished_arn = imported_arn(
        &import(
            &client,
            &published_document.to_string(),
            None,
            "filters-republished",
            None,
        )
        .await,
        "filters-republished",
    );
    wait_for_the_import_to_land();

    // `draft_arn` keeps its draft; the other two get a version published from it, over
    // HTTP, so the PUBLISHED rows below are a real publish rather than a seeded one.
    for (case, lens_arn) in [
        ("filters-published", &published_arn),
        ("filters-republished", &republished_arn),
    ] {
        let published = create_version(&client, lens_arn, "1.0", Some(true), case).await;
        assert_eq!(
            published.status, 200,
            "case {case:?}: expected CreateLensVersion 200, got {published:?}"
        );
    }
    // And one of them is then imported over: a lens with a published version AND newer
    // content, which "will have a status of DRAFT" again (`ImportLens` doc).
    let reimported = import(
        &client,
        &newer_document.to_string(),
        Some(&republished_arn),
        "filters-reimport",
        None,
    )
    .await;
    assert_eq!(
        reimported.status, 200,
        "case \"filters-reimport\": expected ImportLens 200, got {reimported:?}"
    );
    wait_for_the_import_to_land();

    let shared_document = lens_document("Shared partner lens", None, &[("reliability", 1, 2)]);
    let shared_arn = record_shared_lens(&client, OTHER_ACCOUNT, &shared_document);

    let official = state::FRAMEWORK_LENS_ALIAS.to_string();
    let officials = official_aliases();
    let cases: Vec<(&str, String, Vec<&String>)> = vec![
        // No filter: no filtering by status either, so every lens the account has.
        (
            "no-filter",
            String::new(),
            with_officials(
                &officials,
                &[&draft_arn, &published_arn, &republished_arn, &shared_arn],
            ),
        ),
        (
            "status-all",
            "LensStatus=ALL".to_string(),
            with_officials(
                &officials,
                &[&draft_arn, &published_arn, &republished_arn, &shared_arn],
            ),
        ),
        (
            "status-published",
            "LensStatus=PUBLISHED".to_string(),
            with_officials(&officials, &[&published_arn]),
        ),
        (
            "status-draft",
            "LensStatus=DRAFT".to_string(),
            vec![&draft_arn, &republished_arn, &shared_arn],
        ),
        (
            "type-aws-official",
            "LensStatus=ALL&LensType=AWS_OFFICIAL".to_string(),
            with_officials(&officials, &[]),
        ),
        (
            "type-custom-self",
            "LensStatus=ALL&LensType=CUSTOM_SELF".to_string(),
            vec![&draft_arn, &published_arn, &republished_arn],
        ),
        (
            "type-custom-shared",
            "LensStatus=ALL&LensType=CUSTOM_SHARED".to_string(),
            vec![&shared_arn],
        ),
        (
            "type-and-status",
            "LensStatus=PUBLISHED&LensType=CUSTOM_SELF".to_string(),
            vec![&published_arn],
        ),
        (
            "name-of-a-custom-lens",
            format!("LensStatus=ALL&LensName={}", query_value("Draft policy lens")),
            vec![&draft_arn],
        ),
        (
            "name-of-the-official-lens",
            format!(
                "LensStatus=ALL&LensName={}",
                query_value(state::FRAMEWORK_LENS_NAME)
            ),
            vec![&official],
        ),
        // `LensName` is the FULL name of the lens, so part of one names nothing.
        (
            "a-prefix-is-not-a-full-name",
            format!("LensStatus=ALL&LensName={}", query_value("Draft policy")),
            Vec::new(),
        ),
        (
            "filters-that-disagree",
            format!(
                "LensStatus=ALL&LensType=AWS_OFFICIAL&LensName={}",
                query_value("Draft policy lens")
            ),
            Vec::new(),
        ),
        (
            "a-draft-under-the-published-status",
            format!(
                "LensStatus=PUBLISHED&LensName={}",
                query_value("Draft policy lens")
            ),
            Vec::new(),
        ),
    ];

    for (case, query, expected) in cases {
        let listed = list(&client, &query).await;
        let reported = aliases(&listed, case);
        let reported_set: BTreeSet<&str> = reported.iter().map(String::as_str).collect();
        let expected_set: BTreeSet<&str> = expected.iter().map(|alias| alias.as_str()).collect();
        assert_eq!(
            reported_set, expected_set,
            "case {case:?}: wrong selection for {query:?}, got {reported:?}"
        );
        assert_eq!(
            reported.len(),
            reported_set.len(),
            "case {case:?}: a lens must be reported once, got {reported:?}"
        );
        assert!(
            listed.body.get("NextToken").is_none(),
            "case {case:?}: a page that holds the whole selection carries no NextToken, \
             got {listed:?}"
        );
    }

    // Both lenses a version was published from report the version their CURRENT content is
    // published as, with the status that goes with it: "The new or updated lens will have a
    // status of DRAFT" (`ImportLens` doc), so the one imported over since reports its
    // draft again.
    for (case, lens_arn, query, version, status) in [
        (
            "published",
            &published_arn,
            "LensStatus=PUBLISHED",
            "1.0",
            state::LENS_STATUS_PUBLISHED,
        ),
        (
            "republished",
            &republished_arn,
            "LensStatus=DRAFT",
            state::LENS_VERSION_DRAFT,
            state::LENS_STATUS_DRAFT,
        ),
    ] {
        let listed = list(&client, query).await;
        let summary = summary_of(&listed, lens_arn, case);
        assert_eq!(
            member(&summary, "LensVersion"),
            version,
            "case {case:?}: expected the version of the content it carries, got {summary:?}"
        );
        assert_eq!(
            member(&summary, "LensStatus"),
            status,
            "case {case:?}: expected the lens's publication state, got {summary:?}"
        );
    }
}

/// Rule 2's publication state over the whole import/publish lifecycle of ONE lens: DRAFT
/// when imported, PUBLISHED once a version has been published from the content it carries,
/// and DRAFT again after an import over it — even when the document that import carries is
/// byte-identical to the published one, because "The new or updated lens will have a status
/// of DRAFT" (`ImportLens` doc) is about the update, not about the bytes.
#[tokio::test]
async fn the_publication_state_follows_the_import_and_publish_lifecycle() {
    let client = Client::new();
    let document = lens_document(
        "Lifecycle lens",
        Some("the lens whose publication state is walked"),
        &[("security", 1, 2)],
    );
    let lens_arn = imported_arn(
        &import(&client, &document.to_string(), None, "lifecycle", None).await,
        "lifecycle",
    );
    wait_for_the_import_to_land();

    // Each step, and the `(LensStatus, LensVersion)` the lens reports after it.
    let reported = |case: &'static str| {
        let lens_arn = lens_arn.clone();
        let client = &client;
        async move {
            let listed = list(client, "LensStatus=ALL").await;
            let summary = summary_of(&listed, &lens_arn, case);
            (
                member(&summary, "LensStatus"),
                member(&summary, "LensVersion"),
            )
        }
    };

    assert_eq!(
        reported("imported").await,
        (
            state::LENS_STATUS_DRAFT.to_string(),
            state::LENS_VERSION_DRAFT.to_string()
        ),
        "case \"imported\": a lens nothing was published from is a draft"
    );

    let published = create_version(&client, &lens_arn, "1.0", None, "lifecycle-publish").await;
    assert_eq!(
        published.status, 200,
        "case \"published\": expected CreateLensVersion 200, got {published:?}"
    );
    assert_eq!(
        reported("published").await,
        (state::LENS_STATUS_PUBLISHED.to_string(), "1.0".to_string()),
        "case \"published\": a published version is what the lens reports"
    );

    // The SAME document imported again: an update, so a draft again.
    let reimported = import(
        &client,
        &document.to_string(),
        Some(&lens_arn),
        "lifecycle-reimport",
        None,
    )
    .await;
    assert_eq!(
        reimported.status, 200,
        "case \"reimported-identical\": expected ImportLens 200, got {reimported:?}"
    );
    wait_for_the_import_to_land();
    assert_eq!(
        reported("reimported-identical").await,
        (
            state::LENS_STATUS_DRAFT.to_string(),
            state::LENS_VERSION_DRAFT.to_string()
        ),
        "case \"reimported-identical\": an updated lens is a draft again, whatever it carries"
    );

    let republished = create_version(&client, &lens_arn, "2.0", Some(true), "lifecycle-republish")
        .await;
    assert_eq!(
        republished.status, 200,
        "case \"republished\": expected CreateLensVersion 200, got {republished:?}"
    );
    assert_eq!(
        reported("republished").await,
        (state::LENS_STATUS_PUBLISHED.to_string(), "2.0".to_string()),
        "case \"republished\": the version published from the lens's current content"
    );
}

/// Rule 3. However small the page, following `NextToken` enumerates the catalog exactly
/// once, in the order one page reports it, and the last page carries no token.
#[tokio::test]
async fn paging_enumerates_every_lens_exactly_once() {
    let client = Client::new();
    // Four custom lenses imported over HTTP beside the shipped official one.
    for index in 0..4 {
        let document = lens_document(&format!("Paged lens {index}"), None, &[("governance", 1, 2)]);
        import(
            &client,
            &document.to_string(),
            None,
            &format!("paging-{index}"),
            None,
        )
        .await;
    }
    wait_for_the_import_to_land();

    // The whole catalog on one page: every AWS-official lens and the four custom ones.
    let whole = list(&client, "LensStatus=ALL&MaxResults=50").await;
    let order = aliases(&whole, "one-page");
    assert_eq!(
        order.len(),
        official_aliases().len() + 4,
        "case \"one-page\": expected every official lens and four custom ones, got {order:?}"
    );

    // The published `@range(1, 50)` minimum, sizes either side of the catalog, the exact
    // count and the modelled maximum.
    for page_size in [1usize, 2, 3, 4, 5, order.len(), 50] {
        let case = format!("page-size-{page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut query = format!("LensStatus=ALL&MaxResults={page_size}");
            if let Some(token) = &token {
                query.push_str(&format!("&NextToken={token}"));
            }
            let page = list(&client, &query).await;
            let reported = aliases(&page, &case);
            assert!(
                reported.len() <= page_size,
                "case {case:?}: a page must not exceed MaxResults, got {reported:?}"
            );
            seen.extend(reported);
            token = page
                .body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            match &token {
                Some(token) => assert!(
                    token
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '_' | '-')),
                    "case {case:?}: NextToken must match ^[A-Za-z0-9+/=_-]+$, got {token:?}"
                ),
                None => break,
            }
            assert!(
                seen.len() <= order.len(),
                "case {case:?}: paging must terminate, got {seen:?}"
            );
        }
        assert_eq!(
            seen, order,
            "case {case:?}: paging must enumerate the catalog exactly once, in order"
        );
    }
}

/// Rule 3's refusal: a `NextToken` this service did not issue is a 400 classified
/// `CANNOT_PARSE` and naming the member, not an empty page. The rows are the ways a token
/// can fail to be one of ours.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let client = Client::new();
    let cases: Vec<(&str, String)> = vec![
        ("not-this-services-encoding", "not-a-token".to_string()),
        // This service's encoding carrying ANOTHER operation's cursor: decodable, but not
        // a lens cursor.
        (
            "another-operations-cursor",
            state::encode_milestone_next_token(1),
        ),
        (
            "another-list-cursor",
            state::encode_next_token("0123456789abcdef0123456789abcdef"),
        ),
    ];

    for (case, token) in cases {
        let answer = list(&client, &format!("NextToken={token}")).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: an unissued NextToken is a 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("Reason").and_then(Value::as_str),
            Some("CANNOT_PARSE"),
            "case {case:?}: an undecodable value is CANNOT_PARSE, got {answer:?}"
        );
        let named: Vec<String> = answer
            .body
            .get("Fields")
            .and_then(Value::as_array)
            .map(|fields| {
                fields
                    .iter()
                    .map(|field| member(field, "Name"))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            named,
            vec!["NextToken".to_string()],
            "case {case:?}: the finding must name NextToken, got {answer:?}"
        );
    }

    // A token this service DID issue resumes the enumeration.
    let document = lens_document("Resumed lens", None, &[("governance", 1, 2)]);
    import(&client, &document.to_string(), None, "resume", None).await;
    wait_for_the_import_to_land();
    let total = official_aliases().len() + 1;
    let first = list(&client, "LensStatus=ALL&MaxResults=1").await;
    let issued = first
        .body
        .get("NextToken")
        .and_then(Value::as_str)
        .expect("a catalog of more than one lens and a page of one leave a NextToken")
        .to_string();
    let resumed = list(&client, &format!("LensStatus=ALL&NextToken={issued}")).await;
    assert_eq!(
        aliases(&resumed, "resumed").len(),
        total - 1,
        "case \"resumed\": the rest of the catalog is every lens but the first, got {resumed:?}"
    );
}

/// Rule 4. A lens is listed from the moment `ImportLens` accepts it — that call answers
/// its ARN and records the document it carried, so the lens exists and the listing
/// describes it by that document at once, while the import's own job still reports
/// `IN_PROGRESS`. An UPDATE moves the content the listing describes the lens by, and moves
/// `UpdatedAt` with it while leaving `CreatedAt` where it was. The handler writes nothing
/// to make the status move; it compares the stored instant with the clock on every read.
#[tokio::test]
async fn a_lens_is_listed_from_acceptance_and_described_by_its_imported_content() {
    let client = Client::new();
    let document = lens_document("Landing lens", None, &[("governance", 1, 2)]);
    let answer = import(&client, &document.to_string(), None, "landing", None).await;
    let lens_arn = imported_arn(&answer, "landing");
    assert_eq!(
        answer.body.get("Status").and_then(Value::as_str),
        Some("IN_PROGRESS"),
        "case \"landing\": an accepted import is IN_PROGRESS, got {answer:?}"
    );

    // While that first import's job is still in progress the lens is already a lens the
    // account has, and nothing has been published from it, so it is a DRAFT one.
    for (case, query, expected) in [
        ("in-flight-all", "LensStatus=ALL", true),
        ("in-flight-draft", "LensStatus=DRAFT", true),
        ("in-flight-custom-self", "LensType=CUSTOM_SELF", true),
        ("in-flight-default", "", true),
        // Nothing has been published from it, so it is not an available lens yet.
        ("in-flight-published", "LensStatus=PUBLISHED", false),
        ("in-flight-custom-shared", "LensType=CUSTOM_SHARED", false),
    ] {
        let listed = list(&client, query).await;
        let reported = aliases(&listed, case);
        assert_eq!(
            reported.contains(&lens_arn),
            expected,
            "case {case:?}: expected the accepted import to be reported={expected}, got \
             {reported:?}"
        );
    }
    // The listing reports the shipped lens all the while, whatever the store holds.
    let listed = list(&client, "LensStatus=ALL").await;
    assert!(
        aliases(&listed, "in-flight-official").contains(&state::FRAMEWORK_LENS_ALIAS.to_string()),
        "case \"in-flight-official\": the shipped lens is listed regardless, got {listed:?}"
    );

    let accepted = summary_of(
        &list(&client, "LensStatus=DRAFT").await,
        &lens_arn,
        "accepted",
    );
    assert_eq!(
        member(&accepted, "LensName"),
        document["name"].as_str().unwrap_or_default(),
        "case \"accepted\": the name comes from the imported document, got {accepted:?}"
    );
    assert_eq!(
        member(&accepted, "LensType"),
        state::LENS_TYPE_CUSTOM_SELF,
        "case \"accepted\": a lens this account imported is CUSTOM_SELF, got {accepted:?}"
    );
    assert_eq!(
        member(&accepted, "LensStatus"),
        state::LENS_STATUS_DRAFT,
        "case \"accepted\": a lens only imported is DRAFT, got {accepted:?}"
    );
    assert_eq!(
        member(&accepted, "LensVersion"),
        state::LENS_VERSION_DRAFT,
        "case \"accepted\": a lens only imported reports its draft as its version, got \
         {accepted:?}"
    );

    // The import's job finishing is a status of the IMPORT, so it moves nothing the
    // listing reports about the lens.
    wait_for_the_import_to_land();
    let complete = summary_of(
        &list(&client, "LensStatus=DRAFT").await,
        &lens_arn,
        "import-complete",
    );
    assert_eq!(
        complete, accepted,
        "case \"import-complete\": the import's own status must not move the listing, got \
         {complete:?}"
    );

    // An import OVER that lens: the listing describes the lens by the document it carried.
    let newer = lens_document(
        "Relanded lens",
        Some("the document the second import carries"),
        &[("security", 2, 2)],
    );
    import(
        &client,
        &newer.to_string(),
        Some(&lens_arn),
        "relanding",
        None,
    )
    .await;
    let updated = summary_of(
        &list(&client, "LensStatus=ALL").await,
        &lens_arn,
        "updated",
    );
    assert_eq!(
        member(&updated, "LensName"),
        newer["name"].as_str().unwrap_or_default(),
        "case \"updated\": the accepted import is what the lens is described by, got \
         {updated:?}"
    );
    assert_eq!(
        updated.get("Description").and_then(Value::as_str),
        newer.get("description").and_then(Value::as_str),
        "case \"updated\": the description comes from the imported document, got {updated:?}"
    );
    assert!(
        timestamp(&updated, "UpdatedAt") > timestamp(&accepted, "UpdatedAt"),
        "case \"updated\": UpdatedAt moves with the content, got {updated:?} after \
         {accepted:?}"
    );
    assert_eq!(
        timestamp(&updated, "CreatedAt"),
        timestamp(&accepted, "CreatedAt"),
        "case \"updated\": CreatedAt is when the lens was created, got {updated:?} after \
         {accepted:?}"
    );
}
