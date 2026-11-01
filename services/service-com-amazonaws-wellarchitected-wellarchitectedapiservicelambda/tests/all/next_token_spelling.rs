// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ONE rule every paginated read of this service shares
//! about the `NextToken` it reads back: the token is this service's own opaque value, so a
//! page is served only for the EXACT token the service handed out.
//!
//! The rule pinned here, over a TABLE of spellings crossed with four paginated reads:
//!
//! 1. A `NextToken` that is not byte-identical to one this service issued is a
//!    `400 ValidationException` classified `CANNOT_PARSE` and naming `NextToken`, even
//!    when it is a different spelling of the very cursor a page just handed out — the hex
//!    spelled with upper-case digits, the JSON spelled with added whitespace, with an
//!    escaped member name, with a member repeated, with a member the service does not
//!    write, or with a number respelled. None of those is a value a caller can have been
//!    handed, so none of them resumes an enumeration.
//! 2. The token the service DID issue still resumes it: following the issued tokens
//!    enumerates exactly what one full page reports, each item once, and the refusals in
//!    between change nothing a later caller sees.
//!
//! The rows differ along what the rule generalises over: which paginated read issued the
//! token (four of them, each with a cursor of its own — a catalog key, a workload id, a
//! profile ARN and a milestone number, so both a string-valued and a number-valued
//! cursor), and HOW the spelling differs from the issued one (the hex digits' case, JSON
//! whitespace, escapes, member order and count, number spelling).
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client and the resource builders are the shared
//! `super::workload_harness`; the per-operation `MaxResults` and refusal tables each
//! operation's own suite pins are untouched.

use serde_json::{json, Value};

use super::workload_harness::{created_id, created_profile, minimal_create, Answer, Client};

/// The page size that forces a token out of every read under test: one item per page, so
/// the first page of a collection holding more than one item carries a `NextToken`.
const ONE_PER_PAGE: usize = 1;

/// How many items each collection under test holds: more than one page's worth at
/// [`ONE_PER_PAGE`], so the walk takes several pages and the first token is a resume point
/// with items left behind it.
const ITEM_COUNT: usize = 3;

/// One paginated read under test, with whatever its pages are listed from.
enum Read {
    Lenses,
    Workloads,
    Profiles,
    Milestones(String),
}

impl Read {
    /// The list member the read reports its page in.
    fn page_member(&self) -> &'static str {
        match self {
            Read::Lenses => "LensSummaries",
            Read::Workloads => "WorkloadSummaries",
            Read::Profiles => "ProfileSummaries",
            Read::Milestones(_) => "MilestoneSummaries",
        }
    }

    /// The member that identifies one reported item, which is also what the read orders
    /// and resumes its pages by.
    fn id_member(&self) -> &'static str {
        match self {
            Read::Lenses => "LensAlias",
            Read::Workloads => "WorkloadId",
            Read::Profiles => "ProfileArn",
            Read::Milestones(_) => "MilestoneNumber",
        }
    }

    /// The read itself, at `page_size` items a page and resuming from `token` when one is
    /// given. Each operation's own wire form: the two lens/profile reads bind their paging
    /// members to the query string, the two POST reads carry them in the body.
    async fn list(&self, client: &Client, page_size: usize, token: Option<&str>) -> Answer {
        let query = match token {
            Some(token) => format!("MaxResults={page_size}&NextToken={token}"),
            None => format!("MaxResults={page_size}"),
        };
        let mut body = json!({ "MaxResults": page_size });
        if let Some(token) = token {
            body["NextToken"] = json!(token);
        }
        match self {
            Read::Lenses => client.send("GET", &format!("/lenses?{query}"), None).await,
            Read::Workloads => client.list(body).await,
            Read::Profiles => client.list_profiles(&query).await,
            Read::Milestones(workload_id) => {
                client
                    .send(
                        "POST",
                        &format!("/workloads/{workload_id}/milestonesSummaries"),
                        Some(body),
                    )
                    .await
            }
        }
    }
}

/// One page: the ids it reported, in the order it reported them, and the `NextToken` it
/// answered with.
async fn page_of(
    client: &Client,
    read: &Read,
    page_size: usize,
    token: Option<&str>,
    case: &str,
) -> (Vec<String>, Option<String>) {
    let answer = read.list(client, page_size, token).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200, got {answer:?}"
    );
    let page = answer
        .body
        .get(read.page_member())
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the read must report {}, got {answer:?}",
                read.page_member()
            )
        });
    let ids = page
        .iter()
        .map(|item| {
            item.get(read.id_member())
                .unwrap_or(&Value::Null)
                .to_string()
        })
        .collect();
    let next_token = answer
        .body
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (ids, next_token)
}

/// `text` with every character spelled as a JSON `\uXXXX` escape — a spelling of the same
/// string that the service's encoder never writes. Only ASCII is escaped this way, which
/// is all a cursor of this service carries (a catalog key, an id, an ARN).
fn escaped(text: &str) -> String {
    text.chars()
        .map(|character| {
            assert!(
                character.is_ascii(),
                "a cursor carries ASCII only, got {character:?} in {text:?}"
            );
            format!("\\u{:04x}", character as u32)
        })
        .collect()
}

/// `token` with its first hexadecimal letter upper-cased: hex decoding is case-insensitive,
/// so this is a mixed-case spelling of the same bytes.
fn first_letter_upper_cased(token: &str) -> String {
    let mut done = false;
    token
        .chars()
        .map(|character| {
            if !done && character.is_ascii_alphabetic() {
                done = true;
                character.to_ascii_uppercase()
            } else {
                character
            }
        })
        .collect()
}

/// Every spelling of the cursor `issued` carries that the service's own encoder does not
/// write: each one decodes to the same cursor, and none of them is a token any caller was
/// handed. Answers (what a failing assertion calls the row, the `NextToken` to send).
fn other_spellings_of(issued: &str) -> Vec<(String, String)> {
    let bytes = hex::decode(issued).expect("an issued NextToken is hex");
    let text = String::from_utf8(bytes).expect("an issued NextToken carries UTF-8 JSON");
    let cursor: Value = serde_json::from_str(&text).expect("an issued NextToken carries JSON");
    let members = cursor
        .as_object()
        .unwrap_or_else(|| panic!("an issued cursor is a JSON object, got {text:?}"));
    assert_eq!(
        members.len(),
        1,
        "the cursors under test carry one member, got {text:?}"
    );
    let (name, member) = members
        .iter()
        .next()
        .expect("a cursor carries at least one member");
    let name_json = Value::String(name.clone()).to_string();
    let member_json = member.to_string();

    let mut rows = vec![
        ("upper-case hex".to_string(), issued.to_uppercase()),
        ("mixed-case hex".to_string(), first_letter_upper_cased(issued)),
        (
            "json with insignificant whitespace".to_string(),
            hex::encode(
                serde_json::to_string_pretty(&cursor).expect("a cursor renders as pretty JSON"),
            ),
        ),
        (
            "json with an escaped member name".to_string(),
            hex::encode(format!("{{\"{}\":{member_json}}}", escaped(name))),
        ),
        (
            "json with the member repeated".to_string(),
            hex::encode(format!(
                "{{{name_json}:{member_json},{name_json}:{member_json}}}"
            )),
        ),
        (
            "json with a member the encoder does not write".to_string(),
            hex::encode(format!("{{{name_json}:{member_json},\"zz\":0}}")),
        ),
    ];
    if let Some(number) = member.as_i64() {
        // A number-valued cursor: the same value spelled with an exponent, and spelled
        // with a fraction. JSON calls both the same number; neither is what the encoder
        // writes.
        rows.push((
            "json with the number spelled with an exponent".to_string(),
            hex::encode(format!("{{{name_json}:{number}e0}}")),
        ));
        rows.push((
            "json with the number spelled with a fraction".to_string(),
            hex::encode(format!("{{{name_json}:{number}.0}}")),
        ));
    }
    if let Some(value) = member.as_str() {
        rows.push((
            "json with the member value escaped".to_string(),
            hex::encode(format!("{{{name_json}:\"{}\"}}", escaped(value))),
        ));
    }
    rows
}

/// The ids every read under test reports in one full page, for the walk to be checked
/// against, plus the client and the read itself.
async fn reads() -> Vec<(&'static str, Client, Read)> {
    let lens_client = Client::new();

    let workload_client = Client::new();
    for index in 0..ITEM_COUNT {
        let answer = workload_client
            .create(minimal_create(
                &format!("token-spelling-{index}"),
                &format!("tok-token-spelling-{index}"),
            ))
            .await;
        created_id(&answer, &format!("workload-{index}"));
    }

    let profile_client = Client::new();
    for index in 0..ITEM_COUNT {
        created_profile(&profile_client, &format!("token-spelling-{index}")).await;
    }

    let milestone_client = Client::new();
    let workload_id = created_id(
        &milestone_client
            .create(minimal_create("token-spelling-host", "tok-host"))
            .await,
        "milestone-host",
    );
    for number in 1..=ITEM_COUNT {
        let answer = milestone_client
            .send(
                "POST",
                &format!("/workloads/{workload_id}/milestones"),
                Some(json!({
                    "MilestoneName": format!("token-spelling-{number}"),
                    "ClientRequestToken": format!("tok-milestone-{number}"),
                })),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case \"milestone-{number}\": expected CreateMilestone 200, got {answer:?}"
        );
    }

    vec![
        ("ListLenses", lens_client, Read::Lenses),
        ("ListWorkloads", workload_client, Read::Workloads),
        ("ListProfiles", profile_client, Read::Profiles),
        (
            "ListMilestones",
            milestone_client,
            Read::Milestones(workload_id),
        ),
    ]
}

/// Rules 1 and 2, over every spelling of an issued token crossed with every read under
/// test.
#[tokio::test]
async fn only_the_next_token_this_service_issued_resumes_a_page() {
    for (operation, client, read) in reads().await {
        // What one full page reports, which is what the walk below has to enumerate.
        let whole = format!("{operation}/whole-collection");
        let (expected, whole_token) = page_of(&client, &read, 50, None, &whole).await;
        assert!(
            expected.len() >= ITEM_COUNT,
            "case {whole:?}: expected at least {ITEM_COUNT} item(s) to page over, got {expected:?}"
        );
        assert_eq!(
            whole_token, None,
            "case {whole:?}: a page holding every item carries no NextToken, got {whole_token:?}"
        );

        let first = format!("{operation}/first-page");
        let (mut walked, token) = page_of(&client, &read, ONE_PER_PAGE, None, &first).await;
        let issued = token.unwrap_or_else(|| {
            panic!("case {first:?}: a page with items left behind must carry a NextToken")
        });

        // Rule 1: every other spelling of that very cursor is refused.
        for (spelling, candidate) in other_spellings_of(&issued) {
            let case = format!("{operation}/{spelling}");
            assert_ne!(
                candidate, issued,
                "case {case:?}: the row must differ from the issued token"
            );
            assert!(
                candidate
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '_' | '-')),
                "case {case:?}: the row must satisfy ^[A-Za-z0-9+/=_-]+$ so the handler sees it, got {candidate:?}"
            );
            let answer = read.list(&client, ONE_PER_PAGE, Some(&candidate)).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: a token this service did not issue is a 400, got {answer:?}"
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
                        .map(|field| {
                            field
                                .get("Name")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string()
                        })
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                named,
                vec!["NextToken".to_string()],
                "case {case:?}: the finding must name NextToken, got {answer:?}"
            );
        }

        // Rule 2: the issued token still resumes the enumeration, and the refusals left
        // nothing behind.
        let walk = format!("{operation}/walk");
        let mut next_token = Some(issued);
        let mut pages = 1usize;
        while let Some(token) = next_token {
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {walk:?}: paging must terminate, walked {walked:?}"
            );
            let (ids, following) =
                page_of(&client, &read, ONE_PER_PAGE, Some(&token), &walk).await;
            assert_eq!(
                ids.len(),
                ONE_PER_PAGE.min(expected.len() - walked.len()),
                "case {walk:?}: page {pages} must hold one item while items remain, got {ids:?}"
            );
            walked.extend(ids);
            next_token = following;
        }
        assert_eq!(
            walked, expected,
            "case {walk:?}: following the issued tokens must enumerate the whole page, in order"
        );
    }
}
