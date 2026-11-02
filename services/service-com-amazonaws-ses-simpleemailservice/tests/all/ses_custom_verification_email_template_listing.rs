// SPDX-License-Identifier: CC0-1.0

//! `ListCustomVerificationEmailTemplates`, end to end over the wire.
//!
//! Every call here goes through `MultiServiceRouter::dispatch`, so each one is a real
//! awsQuery request: form-urlencoded body in, XML out, one shared engine behind it.
//! What is asserted is the RULE the operation follows over a SET of inputs rather than
//! one recorded request:
//!
//! * the listing reports the custom verification email templates this account's store
//!   holds, each one as the modelled `CustomVerificationEmailTemplate` — the name, the
//!   sender, the subject and the two redirection URLs, and NOT the template content,
//!   which that shape does not carry (`smithy_query(query="shape",
//!   shape="CustomVerificationEmailTemplate")`) and only
//!   `GetCustomVerificationEmailTemplate` reports;
//! * it reports them in one stable order, and paging with `MaxResults`/`NextToken`
//!   visits every template exactly once for EVERY page size;
//! * `MaxResults` follows the bounds the member publishes (`@range` 1..50): a value
//!   inside them is the page size, a value outside them is refused before the handler
//!   runs, and an absent value returns up to 50 results;
//! * a `NextToken` this listing did not issue is refused with HTTP 400
//!   `InvalidParameterValue`, whatever shape the token has, and the store is left
//!   alone;
//! * a template created over the wire is in the listing that follows, which is the
//!   round trip through `CreateCustomVerificationEmailTemplate`.
//!
//! The paging and projection suites seed the store through the same `state` API the
//! handlers use (`CreateCustomVerificationEmailTemplate` over the wire is
//! `tests/all/ses_custom_verification_email_templates.rs`' subject), so a page size can
//! be exercised without the sender-verification gate in the way.

use crate::harness::{
    call, element, elements, emulator_and_engine, emulator_over, field, settle_verifications,
    xml_text, Field,
};
use service_com_amazonaws_ses_simpleemailservice::state::{
    encode_page_token, service_state, CustomVerificationEmailTemplate,
    LISTING_CUSTOM_VERIFICATION_EMAIL_TEMPLATES, MAX_CUSTOM_VERIFICATION_EMAIL_TEMPLATES,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The largest page the operation serves, and the page size an absent `MaxResults`
/// takes: "This value must be at least 1 and less than or equal to 50. If you do
/// not specify a value, or if you specify a value less than 1 or greater than 50,
/// the operation returns up to 50 results"
/// (`ListCustomVerificationEmailTemplatesRequest$MaxResults`).
const MAX_RESULTS: usize = 50;

/// The one error code every refusal the request boundary makes carries, as
/// `tests/all/ses_validation_refusal_wording.rs` pins for this query API.
const VALIDATION_ERROR: &str = "ValidationError";

/// The five members of the modelled `CustomVerificationEmailTemplate`, in the order
/// the shape declares them (`smithy_query(query="shape",
/// shape="CustomVerificationEmailTemplate")`).
const REPORTED_MEMBERS: [&str; 5] = [
    "TemplateName",
    "FromEmailAddress",
    "TemplateSubject",
    "SuccessRedirectionURL",
    "FailureRedirectionURL",
];

/// One stored template, as the listing is expected to report it: the five members
/// above, in that order.
type Reported = [String; 5];

/// A stored record whose members are derived from `name`, so the listing's
/// projection of each one is checked against a value no other template carries.
fn seeded(name: &str) -> CustomVerificationEmailTemplate {
    CustomVerificationEmailTemplate {
        template_name: name.to_string(),
        from_email_address: format!("sender-{name}@example.com"),
        template_subject: format!("Subject of {name}"),
        template_content: format!("<html><body>Content of {name}</body></html>"),
        success_redirection_url: format!("https://example.com/ok/{name}"),
        failure_redirection_url: format!("https://example.com/no/{name}"),
    }
}

/// How the listing must report `template`.
fn expected(template: &CustomVerificationEmailTemplate) -> Reported {
    [
        template.template_name.clone(),
        template.from_email_address.clone(),
        template.template_subject.clone(),
        template.success_redirection_url.clone(),
        template.failure_redirection_url.clone(),
    ]
}

/// A router over an engine this suite keeps, holding each of `templates` in the
/// store, inserted in the order given — which is NOT the lexicographic order the
/// listing reports.
///
/// Returns the router and, in name order, how each template must be reported.
fn emulator_holding(
    templates: &[CustomVerificationEmailTemplate],
) -> (MultiServiceRouter, Vec<Reported>) {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let store = service_state(&*engine).expect("this service has a store on the shared engine");
    for template in templates {
        let recorded = store
            .lock()
            .put_custom_verification_email_template(template.clone())
            .is_some();
        assert!(
            recorded,
            "case {:?}: a template inside the account's limit must be recorded",
            template.template_name
        );
    }
    let mut rows: Vec<Reported> = templates.iter().map(expected).collect();
    rows.sort();
    (emulator_over(engine), rows)
}

/// `verify-<index>` for each of `count` templates, in an order whose lexicographic
/// sort differs from the insertion order.
fn template_names(count: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("verify-{:04}", count - index))
        .collect()
}

/// One `ListCustomVerificationEmailTemplates` call: `(status, body)`.
async fn list_raw(
    router: &MultiServiceRouter,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> (u16, String) {
    let mut params: Vec<Field> = Vec::new();
    if let Some(max_results) = max_results {
        params.push(field("MaxResults", max_results.to_string()));
    }
    if let Some(next_token) = next_token {
        params.push(field("NextToken", next_token));
    }
    call(router, "ListCustomVerificationEmailTemplates", &params).await
}

/// `ListCustomVerificationEmailTemplates`, as `(the entries on this page, the next
/// token)`. Asserts the call succeeded, that the list member is present, and that
/// every entry carries each reported member and NO `TemplateContent`.
async fn list(
    router: &MultiServiceRouter,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> (Vec<Reported>, Option<String>) {
    let (status, body) = list_raw(router, max_results, next_token).await;
    let case = format!("List(max={max_results:?}, token={next_token:?})");
    assert_eq!(
        status, 200,
        "case {case}: expected 200, got {status} with {body}"
    );
    let list = element(&body, "CustomVerificationEmailTemplates").unwrap_or_else(|| {
        panic!("case {case}: the response must carry the list member, got {body}")
    });
    let entries = elements(&list, "member")
        .iter()
        .map(|member| {
            assert!(
                element(member, "TemplateContent").is_none(),
                "case {case}: the listed shape carries no TemplateContent, got {member}"
            );
            let mut reported: Reported = Default::default();
            for (slot, name) in REPORTED_MEMBERS.iter().enumerate() {
                reported[slot] = element(member, name)
                    .map(|raw| xml_text(&raw))
                    .unwrap_or_else(|| {
                        panic!("case {case}: an entry must carry {name}, got {member}")
                    });
            }
            reported
        })
        .collect();
    (entries, element(&body, "NextToken"))
}

/// Every page of the listing at `max_results`, concatenated, following each
/// `NextToken` until none comes back. Also asserts no page is larger than the page
/// size the request asks for, and that every page but the last is full.
async fn every_page(
    router: &MultiServiceRouter,
    max_results: Option<i32>,
    page_size: usize,
) -> Vec<Reported> {
    let mut seen = Vec::new();
    let mut token: Option<String> = None;
    // Bounded above every page count this suite can ask for, so a listing that
    // never terminates fails here instead of looping forever.
    for round in 0..1_000 {
        let (page, next) = list(router, max_results, token.as_deref()).await;
        assert!(
            page.len() <= page_size,
            "case max={max_results:?} page {round}: expected at most {page_size} entries, got {}",
            page.len()
        );
        assert!(
            next.is_none() || page.len() == page_size,
            "case max={max_results:?} page {round}: a page that is not the last must be full, \
             got {} of {page_size}",
            page.len()
        );
        seen.extend(page);
        match next {
            None => return seen,
            Some(next) => token = Some(next),
        }
    }
    panic!("case max={max_results:?}: the listing never ran out of pages");
}

#[tokio::test]
async fn an_account_with_no_templates_lists_nothing() {
    let (router, _) = emulator_holding(&[]);
    for max_results in [None, Some(1), Some(MAX_RESULTS as i32)] {
        let (page, token) = list(&router, max_results, None).await;
        assert!(
            page.is_empty(),
            "case max={max_results:?}: expected no templates, got {page:?}"
        );
        assert_eq!(
            token, None,
            "case max={max_results:?}: expected no NextToken, got {token:?}"
        );
    }
}

#[tokio::test]
async fn every_stored_member_is_reported_as_the_store_holds_it() {
    // The rows differ along what the rule generalises over: the form of the name,
    // the shape of the sender, subjects carrying markup, an ampersand and non-ASCII
    // text, and URLs with a path, a query and a non-default scheme. The content is
    // varied too, to pin that it is NOT part of what a listing reports.
    let stored = vec![
        seeded("verify-plain"),
        CustomVerificationEmailTemplate {
            from_email_address: "no-reply@mail.example.net".to_string(),
            template_subject: "Confirm & continue <now>".to_string(),
            ..seeded("verify.with.dots")
        },
        CustomVerificationEmailTemplate {
            template_subject: "Bitte bestätigen Sie Ihre Adresse".to_string(),
            template_content: "<html><body>Adresse bestätigen — danke!</body></html>".to_string(),
            ..seeded("verify-non-ascii")
        },
        CustomVerificationEmailTemplate {
            success_redirection_url: "http://example.com/ses/ok?token=1&next=/home".to_string(),
            failure_redirection_url: "http://example.com/ses/no?token=1&next=/help".to_string(),
            ..seeded("VERIFY-UPPER")
        },
        CustomVerificationEmailTemplate {
            template_subject: "x".to_string(),
            template_content: "y".to_string(),
            ..seeded("v")
        },
    ];
    let (router, rows) = emulator_holding(&stored);

    let (page, token) = list(&router, None, None).await;
    assert_eq!(
        token, None,
        "case every stored template on one page: {} templates fit the default page, so no \
         NextToken must come back, got {token:?}",
        stored.len()
    );
    assert_eq!(
        page, rows,
        "case every stored template on one page: expected each stored template's members, in \
         name order, got {page:?}"
    );

    // The content every record holds is reported by the single-template read and by
    // nothing else, so the member the listing omits is not a member the store lost.
    for template in &stored {
        let (status, body) = call(
            &router,
            "GetCustomVerificationEmailTemplate",
            &[field("TemplateName", template.template_name.clone())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case read {:?} back: expected 200, got {status} with {body}",
            template.template_name
        );
        let reported = element(&body, "TemplateContent")
            .map(|raw| xml_text(&raw))
            .unwrap_or_default();
        assert_eq!(
            reported, template.template_content,
            "case read {:?} back: expected the stored content, got {reported:?}",
            template.template_name
        );
    }
}

#[tokio::test]
async fn paging_visits_every_stored_template_exactly_once_at_every_page_size() {
    // The account's ceiling on these templates, so the store is as large as a page
    // size can ever have to walk.
    const STORED: usize = MAX_CUSTOM_VERIFICATION_EMAIL_TEMPLATES;
    let stored: Vec<CustomVerificationEmailTemplate> =
        template_names(STORED).iter().map(|n| seeded(n)).collect();
    let (router, rows) = emulator_holding(&stored);

    // EVERY page size the member admits, plus the absent value, so the rule is
    // asserted over the whole class rather than at one size.
    let sizes: Vec<Option<i32>> = std::iter::once(None)
        .chain((1..=MAX_RESULTS).map(|size| Some(size as i32)))
        .collect();
    for max_results in sizes {
        let page_size = max_results.map_or(MAX_RESULTS, |size| size as usize);
        let seen = every_page(&router, max_results, page_size).await;
        assert_eq!(
            seen, rows,
            "case max={max_results:?}: expected every stored template once, in name order, with \
             every stored member, got {seen:?}"
        );
    }

    // An absent `MaxResults` returns up to 50 results, which is the whole store
    // here — so it comes back on one page with no cursor.
    let (page, token) = list(&router, None, None).await;
    assert_eq!(
        page.len(),
        STORED,
        "case absent MaxResults: expected up to {MAX_RESULTS} results, got {}",
        page.len()
    );
    assert_eq!(
        token, None,
        "case absent MaxResults: the whole store fits one page, got {token:?}"
    );
}

#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    const STORED: usize = 12;
    let stored: Vec<CustomVerificationEmailTemplate> =
        template_names(STORED).iter().map(|n| seeded(n)).collect();
    let (router, rows) = emulator_holding(&stored);

    // Each published bound is INSIDE the range, so it reaches the handler and is the
    // page size.
    for (max_results, expected_len) in [(1, 1), (MAX_RESULTS as i32, STORED)] {
        let (page, _) = list(&router, Some(max_results), None).await;
        assert_eq!(
            page.len(),
            expected_len,
            "case MaxResults={max_results}: expected a page of {expected_len}, got {}",
            page.len()
        );
    }

    // One past each bound, and the extremes of the member's type: outside the range
    // the model publishes, so the request is refused before the handler runs.
    for max_results in [0, -1, MAX_RESULTS as i32 + 1, 1_000, i32::MAX, i32::MIN] {
        let (status, body) = list_raw(&router, Some(max_results), None).await;
        assert_eq!(
            status, 400,
            "case MaxResults={max_results}: expected HTTP 400, got {status} with {body}"
        );
        let code = element(&body, "Code").unwrap_or_default();
        assert_eq!(
            code, VALIDATION_ERROR,
            "case MaxResults={max_results}: expected {VALIDATION_ERROR}, got {code:?} in {body}"
        );
        let bound = if max_results < 1 { 1 } else { MAX_RESULTS as i32 };
        let requirement = if max_results < 1 {
            format!("Member must have value greater than or equal to {bound}")
        } else {
            format!("Member must have value less than or equal to {bound}")
        };
        let stated = element(&body, "Message")
            .map(|raw| xml_text(&raw))
            .unwrap_or_default();
        assert_eq!(
            stated,
            format!(
                "1 validation error detected: Value '{max_results}' at 'maxResults' failed to \
                 satisfy constraint: {requirement}"
            ),
            "case MaxResults={max_results}: expected the published bound to be stated, got \
             {stated:?}"
        );
    }

    // The refusals changed nothing: the listing still reports every stored template.
    let seen = every_page(&router, None, MAX_RESULTS).await;
    assert_eq!(
        seen, rows,
        "case after the refusals: the store must be untouched, got {seen:?}"
    );
}

#[tokio::test]
async fn a_page_token_this_listing_did_not_issue_is_refused() {
    const STORED: usize = 12;
    const PAGE: usize = 5;
    let stored: Vec<CustomVerificationEmailTemplate> =
        template_names(STORED).iter().map(|n| seeded(n)).collect();
    let (router, rows) = emulator_holding(&stored);

    // The cursor the listing itself hands out is accepted, and resumes the page
    // after the one it was cut from.
    let (first_page, issued) = list(&router, Some(PAGE as i32), None).await;
    let issued = issued.expect("a store larger than the page size yields a NextToken");
    let (second_page, _) = list(&router, Some(PAGE as i32), Some(&issued)).await;
    assert_eq!(
        second_page,
        rows[first_page.len()..first_page.len() + PAGE].to_vec(),
        "case the issued cursor: expected the page after the first, got {second_page:?}"
    );

    // Each row is a token shaped unlike anything this listing issues, or one it
    // issued for a different listing or a tampered position.
    let real = encode_page_token(
        LISTING_CUSTOM_VERIFICATION_EMAIL_TEMPLATES,
        &rows[3][0].clone(),
    );
    let tampered = {
        let mut bytes = ::hex::decode(&real).expect("an issued token is hex");
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        ::hex::encode(bytes)
    };
    let tokens: Vec<(&str, String)> = vec![
        (
            "a token the caller made up",
            "not-a-real-pagination-token".to_string(),
        ),
        ("an empty token", String::new()),
        ("a template name sent bare", rows[0][0].clone()),
        ("hex that carries no key", "0011223344556677".to_string()),
        ("hex too short to carry a tag", "00112233".to_string()),
        ("odd-length hex", "abc".to_string()),
        ("a tag that does not match the key", tampered),
        (
            "a token truncated by a byte",
            real[..real.len() - 2].to_string(),
        ),
        ("a token with a byte appended", format!("{real}ff")),
        (
            "a cursor of another listing of this service",
            encode_page_token("ListTemplates", &rows[3][0]),
        ),
        // A rejected value is quoted back into the error envelope, so one carrying
        // XML metacharacters must travel escaped rather than breaking the envelope
        // it is rendered into.
        (
            "a token carrying XML metacharacters",
            "<Message>&\"'".to_string(),
        ),
    ];
    for (case, token) in &tokens {
        let (status, body) = list_raw(&router, None, Some(token)).await;
        assert_eq!(
            status, 400,
            "case {case} ({token:?}): expected HTTP 400, got {status} with {body}"
        );
        let code = element(&body, "Code").unwrap_or_default();
        assert_eq!(
            code, "InvalidParameterValue",
            "case {case} ({token:?}): expected InvalidParameterValue, got {code:?} in {body}"
        );
        let stated = element(&body, "Message")
            .map(|raw| xml_text(&raw))
            .unwrap_or_default();
        assert_eq!(
            stated,
            format!("Invalid PageToken <{token}>."),
            "case {case} ({token:?}): the message must name the rejected token, got {stated:?}"
        );
        assert!(
            !body.contains(token.as_str()) || !token.contains(['<', '>', '&']),
            "case {case} ({token:?}): an XML metacharacter must be escaped in the envelope, \
             got {body}"
        );
    }

    // A refused token changes nothing: the listing still reports every stored
    // template, and the cursor issued before the refusals still resumes.
    let seen = every_page(&router, None, MAX_RESULTS).await;
    assert_eq!(
        seen, rows,
        "case after {} refusals: the store must be untouched, got {seen:?}",
        tokens.len()
    );
    let (resumed, _) = list(&router, Some(PAGE as i32), Some(&issued)).await;
    assert_eq!(
        resumed, second_page,
        "case after {} refusals: the issued cursor must still resume the same page, got \
         {resumed:?}",
        tokens.len()
    );

    // A cursor IS accepted over a position the store no longer holds: the token
    // names where the next page starts, so the listing resumes at the first template
    // at or after it rather than refusing a caller whose page boundary was deleted
    // between calls.
    let over_a_gap = encode_page_token(
        LISTING_CUSTOM_VERIFICATION_EMAIL_TEMPLATES,
        &format!("{}-gone", rows[7][0]),
    );
    let (after_gap, _) = list(&router, None, Some(&over_a_gap)).await;
    assert_eq!(
        after_gap,
        rows[8..].to_vec(),
        "case a cursor over a position the store does not hold: expected the templates after \
         it, got {after_gap:?}"
    );
}

#[tokio::test]
async fn a_template_created_over_the_wire_is_in_the_listing_that_follows() {
    let (router, engine) = emulator_and_engine();
    let sender = "verification@example.com";
    let (status, body) = call(&router, "VerifyEmailIdentity", &[field("EmailAddress", sender)]).await;
    assert_eq!(
        status, 200,
        "case verify {sender:?}: expected 200, got {status} with {body}"
    );
    settle_verifications(&engine);

    // The rows differ in the values the create carries, and each one is expected in
    // the listing as soon as it is accepted — so the listing is computed from the
    // store rather than from the request that happens to be in flight.
    let creates: Vec<(&str, Reported, String)> = vec![
        (
            "the first template of the account",
            [
                "wire-created".to_string(),
                sender.to_string(),
                "Please verify your address".to_string(),
                "https://example.com/verified".to_string(),
                "https://example.com/failed".to_string(),
            ],
            "<html><body>Verify.</body></html>".to_string(),
        ),
        (
            "a second template, sorting BEFORE the first",
            [
                "a-wire-created".to_string(),
                sender.to_string(),
                "Confirm & continue".to_string(),
                "http://example.com/ok?x=1".to_string(),
                "http://example.com/no?x=1".to_string(),
            ],
            "<html><body>Confirm &amp; continue.</body></html>".to_string(),
        ),
    ];
    let mut expected_rows: Vec<Reported> = Vec::new();
    for (case, row, content) in &creates {
        let (status, body) = call(
            &router,
            "CreateCustomVerificationEmailTemplate",
            &[
                field("TemplateName", row[0].clone()),
                field("FromEmailAddress", row[1].clone()),
                field("TemplateSubject", row[2].clone()),
                field("TemplateContent", content.clone()),
                field("SuccessRedirectionURL", row[3].clone()),
                field("FailureRedirectionURL", row[4].clone()),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        expected_rows.push(row.clone());
        expected_rows.sort();
        let (page, token) = list(&router, None, None).await;
        assert_eq!(
            page, expected_rows,
            "case {case:?}: expected the listing to report every created template, got {page:?}"
        );
        assert_eq!(
            token, None,
            "case {case:?}: {} templates fit the default page, got {token:?}",
            expected_rows.len()
        );
    }
}
