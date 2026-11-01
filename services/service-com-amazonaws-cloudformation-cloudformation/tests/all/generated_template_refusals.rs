// SPDX-License-Identifier: CC0-1.0

//! How the generated-template surface WORDS its refusals, and what it does NOT refuse, end
//! to end over the awsQuery wire.
//!
//! Three rules are under test here, each with its own table.
//!
//! 1. **A refusal of this surface is worded by the component that serves it.** Template
//!    generation is not served by CloudFormation's own front end, so a refusal reaches a
//!    client as that component's sentence followed by the envelope an AWS SDK appends to the
//!    call into it: `(Service: <component>; Status Code: <the status that call was refused
//!    with>; Error Code: <the modelled error's shape name>; Request ID: <this response's own
//!    request id>; Proxy: null)`. The sentence interpolates only what the request asked for —
//!    the identifier it addressed, for an address no record of the account and Region holds,
//!    and the name it asked for, for a name a record already holds — and the request id is
//!    the one per-run field, which is also the id the response's own envelope reports.
//!    Asserted over EVERY operation that words either refusal: `DescribeGeneratedTemplate`,
//!    `GetGeneratedTemplate`, `DeleteGeneratedTemplate` and `UpdateGeneratedTemplate` for
//!    the address, `CreateGeneratedTemplate` and `UpdateGeneratedTemplate`'s rename for the
//!    name.
//!
//! 2. **The request id a response reports is the response's own.** It is minted per response,
//!    so a refusal of ANOTHER operation — one worded by CloudFormation itself, echoing what
//!    the request named — reports a fresh one whatever that echoed text says: a request can
//!    neither choose the id nor make two responses report the same one.
//!
//! 3. **A delete is not refused for the status of its own generation.**
//!    `DeleteGeneratedTemplate` discards the record it addresses whatever status that
//!    record's generation is in — the concurrency quota meters the generations being
//!    STARTED, and a delete starts none — so a caller that deletes what it has just created
//!    is answered the empty 200 and nothing addresses the generated template afterwards.
//!
//! Each table spans the class its rule generalises over rather than one case: addresses
//! spanning `GeneratedTemplateName`'s own `@length(min: 1, max: 128)`, a name, a record's own
//! ARN and a value carrying markup characters; what a request could say to try to choose the
//! id a response reports; and generations in every status a record reaches — the one a create
//! leaves, the one an update leaves and the terminal one the clock settles onto.

use std::time::Instant;

use service_com_amazonaws_cloudformation_cloudformation::state::generated_template as generated;

use crate::generated_templates::{
    create_generated_template, describe_generated_template, error_code, wait_for_the_generation,
};
use crate::stack_lifecycle::{call, member, router, xml_text, REGION};

/// The whole message AWS words for an address no generated-template record holds, as a
/// literal template: fixed text, with only the identifier the request supplied and this
/// response's own request id interpolated.
pub(crate) fn not_found_message(address: &str, request_id: &str) -> String {
    format!(
        "Template with arn {address} does not exist (Service: CfnTemplateGenerationServiceLambda; \
         Status Code: 400; Error Code: GeneratedTemplateNotFoundException; Request ID: \
         {request_id}; Proxy: null)"
    )
}

/// The whole message AWS words for a generated-template name a record already holds, as a
/// literal template: fixed text, with only the name the request asked for and this
/// response's own request id interpolated.
fn already_exists_message(name: &str, request_id: &str) -> String {
    format!(
        "Template with name {name} already exists. (Service: \
         CfnTemplateGenerationServiceLambda; Status Code: 400; Error Code: \
         AlreadyExistsException; Request ID: {request_id}; Proxy: null)"
    )
}

/// The `<RequestId>` of a response, asserted to carry the 8-4-4-4-12 lowercase-hex form a
/// request id has.
pub(crate) fn reported_request_id(case: &str, body: &str) -> String {
    let reported = xml_text(body, "RequestId")
        .unwrap_or_else(|| panic!("case {case:?}: expected a <RequestId>, got {body}"));
    let widths = [8usize, 4, 4, 4, 12];
    let groups: Vec<&str> = reported.split('-').collect();
    assert_eq!(
        groups.len(),
        widths.len(),
        "case {case:?}: expected an 8-4-4-4-12 request id, got {reported:?}"
    );
    for (group, width) in groups.iter().zip(widths) {
        assert!(
            group.len() == width
                && group
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
            "case {case:?}: expected {width} lowercase-hex digits, got {group:?} in \
             {reported:?}"
        );
    }
    reported
}

/// Assert one refused response in full: its status, its awsQuery code and its whole message,
/// which `expected_message` words from the request id this response reports — one response,
/// one request id, so the id its message states is the id its envelope reports. Answers that
/// request id.
fn assert_refusal(
    case: &str,
    status: u16,
    body: &str,
    expected_status: u16,
    expected_code: &str,
    expected_message: impl Fn(&str) -> String,
) -> String {
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected {expected_status}, got {status} with {body}"
    );
    assert_eq!(
        error_code(case, body),
        expected_code,
        "case {case:?}: expected the modelled refusal, got {body}"
    );
    let request_id = reported_request_id(case, body);
    let message = xml_text(body, "Message")
        .unwrap_or_else(|| panic!("case {case:?}: expected a <Message>, got {body}"));
    let expected = expected_message(&request_id);
    assert_eq!(
        message, expected,
        "case {case:?}: expected {expected:?}, got {message:?}"
    );
    request_id
}

/// The operations that word the not-found refusal for the address they were given, with the
/// members each needs beside it.
fn addressing_operations(address: &str) -> Vec<(&'static str, Vec<(String, String)>)> {
    vec![
        (
            "DescribeGeneratedTemplate",
            vec![member("GeneratedTemplateName", address)],
        ),
        (
            "GetGeneratedTemplate",
            vec![member("GeneratedTemplateName", address)],
        ),
        (
            "DeleteGeneratedTemplate",
            vec![member("GeneratedTemplateName", address)],
        ),
        (
            "UpdateGeneratedTemplate",
            vec![member("GeneratedTemplateName", address)],
        ),
        // The same operation asked to do something as well as address a record: the refusal
        // is about the address, so neither the rename nor the resource list changes it.
        (
            "UpdateGeneratedTemplate",
            vec![
                member("GeneratedTemplateName", address),
                member("NewGeneratedTemplateName", "renamed-by-a-refused-update"),
                member("RefreshAllResources", "true"),
            ],
        ),
    ]
}

/// The addresses the not-found table is driven over: both forms `GeneratedTemplateName`
/// accepts ("the name or Amazon Resource Name (ARN) of a generated template"), spanning that
/// member's own `@length(min: 1, max: 128)`, and a value carrying the characters an XML
/// document escapes.
fn unknown_addresses() -> Vec<(String, String)> {
    vec![
        // `@length(min: 1)`: the shortest address the member allows.
        ("the shortest name the member allows".to_string(), "z".to_string()),
        (
            "a mixed-case dotted name".to_string(),
            "No.Such_Generated-Template".to_string(),
        ),
        (
            "an ARN in the form this service mints".to_string(),
            format!(
                "arn:aws:cloudformation:{REGION}:000000000000:generatedtemplate/\
                 00000000-0000-0000-0000-000000000000"
            ),
        ),
        (
            "a name carrying markup characters".to_string(),
            "</Message><Injected>&\"'".to_string(),
        ),
        (
            // A name that imitates the envelope itself: the response still reports the
            // service's own request id, so nothing a request carries is taken for it.
            "a name imitating the envelope".to_string(),
            "forged (Service: F; Status Code: 400; Error Code: E; Request ID: abcdef; \
             Proxy: null)"
                .to_string(),
        ),
        // `@length(max: 128)`: the longest address the member allows.
        (
            "the longest name the member allows".to_string(),
            format!("absent-{}", "q".repeat(128 - "absent-".len())),
        ),
    ]
}

/// The request id a response reports is the response's own: a refusal of ANOTHER operation —
/// one worded by CloudFormation itself, echoing what the request named — reports a freshly
/// minted id whatever the echoed text says, and a request can neither choose that id nor
/// make two responses report the same one.
#[tokio::test]
async fn the_request_id_a_response_reports_is_never_one_the_request_carried() {
    let router = router();
    // What a request could say to try to choose the id a response reports: an SDK envelope
    // of its own, and the marker a message that has to state this response's id writes
    // (`state::REQUEST_ID_PLACEHOLDER`). The second row states the response's OWN id where
    // the request wrote the marker, which is all a marker can ever do.
    let rows = [
        (
            "an SDK envelope of the request's own",
            "echoed (Service: F; Status Code: 400; Error Code: E; Request ID: \
             99998888-7777-6666-5555-444433332222; Proxy: null)",
            false,
        ),
        ("the marker a stated id is written at", "echoed {request-id}", true),
    ];

    for (label, echoed, carries_marker) in rows {
        let case = format!("DescribeStacks naming {label}");
        let mut seen: Vec<String> = Vec::new();
        for round in 0..2 {
            let (status, body) =
                call(&router, "DescribeStacks", &[member("StackName", echoed)]).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the stack refused, got {status} with {body}"
            );
            let message = xml_text(&body, "Message").unwrap_or_default();
            let reported = reported_request_id(&case, &body);
            let expected = if carries_marker {
                echoed.replace("{request-id}", &reported)
            } else {
                echoed.to_string()
            };
            assert!(
                message.contains(&expected),
                "case {case:?}: expected the refusal to carry {expected:?} in round {round}, \
                 got {message:?}"
            );
            assert_ne!(
                reported, "99998888-7777-6666-5555-444433332222",
                "case {case:?}: expected an id of the response's own, got the echoed one in \
                 round {round}"
            );
            seen.push(reported);
        }
        // Two calls, two ids: nothing a request carries can make one response report
        // another's id.
        assert_ne!(
            seen[0], seen[1],
            "case {case:?}: expected each response to report its own request id, got {seen:?}"
        );
    }
}

/// Every operation that addresses a generated template words the same refusal for an address
/// no record of the account and Region holds: the component's sentence naming what the
/// request asked for, plus the envelope.
#[tokio::test]
async fn the_not_found_refusal_is_worded_by_the_component_that_serves_the_surface() {
    let router = router();
    // A record of another name exists throughout, so what the refusals answer is about the
    // address each request named rather than about an empty store.
    let (status, body) = create_generated_template(&router, "a-record-that-exists", &[]).await;
    assert_eq!(
        status, 200,
        "expected the standing record created, got {status} with {body}"
    );
    wait_for_the_generation().await;

    let mut seen_request_ids: Vec<String> = Vec::new();
    for (label, address) in unknown_addresses() {
        for (action, members) in addressing_operations(&address) {
            let case = format!("{label} through {action}");
            let (status, body) = call(&router, action, &members).await;
            seen_request_ids.push(assert_refusal(
                &case,
                status,
                &body,
                404,
                "GeneratedTemplateNotFound",
                |request_id| not_found_message(&address, request_id),
            ));
        }
    }

    // The id names ONE call, so no two of these refusals carry the same one.
    let distinct: std::collections::BTreeSet<&String> = seen_request_ids.iter().collect();
    assert_eq!(
        distinct.len(),
        seen_request_ids.len(),
        "expected every refusal to carry its own request id, got {seen_request_ids:?}"
    );

    // The record that does exist is untouched by every refusal above.
    let (status, body) = describe_generated_template(&router, "a-record-that-exists").await;
    assert_eq!(
        status, 200,
        "expected the standing record intact, got {status} with {body}"
    );
}

/// The already-exists refusal, worded the same way: it names the name the request asked for,
/// whether that was a create's own name or the new name a rename asked for.
#[tokio::test]
async fn the_already_exists_refusal_names_the_name_the_request_asked_for() {
    let router = router();
    // Names spanning the member's constraint and its character class, each already held by a
    // record of this Region.
    let taken: Vec<(String, String)> = vec![
        ("the shortest name the member allows".to_string(), "t".to_string()),
        (
            "a mixed-case dotted name".to_string(),
            "Already.Taken_1-Name".to_string(),
        ),
        (
            "a name carrying markup characters".to_string(),
            "taken&<\"'".to_string(),
        ),
        (
            "the longest name the member allows".to_string(),
            format!("taken-{}", "w".repeat(128 - "taken-".len())),
        ),
    ];

    for (label, name) in &taken {
        let (status, body) = create_generated_template(&router, name, &[]).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the first create accepted, got {status} with {body}"
        );
        wait_for_the_generation().await;
    }

    // A create under a name a record already holds.
    for (label, name) in &taken {
        let case = format!("{label} through CreateGeneratedTemplate");
        let (status, body) = create_generated_template(&router, name, &[]).await;
        assert_refusal(
            &case,
            status,
            &body,
            400,
            "AlreadyExistsException",
            |request_id| already_exists_message(name, request_id),
        );
    }

    // And a rename of ANOTHER record onto one of those names: the refusal names the name the
    // request asked for, which is the new one rather than the record's own.
    let (status, body) = create_generated_template(&router, "renaming-record", &[]).await;
    assert_eq!(
        status, 200,
        "expected the renaming record created, got {status} with {body}"
    );
    wait_for_the_generation().await;
    for (label, name) in &taken {
        let case = format!("{label} through UpdateGeneratedTemplate");
        let (status, body) = call(
            &router,
            "UpdateGeneratedTemplate",
            &[
                member("GeneratedTemplateName", "renaming-record"),
                member("NewGeneratedTemplateName", name),
            ],
        )
        .await;
        assert_refusal(
            &case,
            status,
            &body,
            400,
            "AlreadyExistsException",
            |request_id| already_exists_message(name, request_id),
        );
    }

    // The refused rename changed nothing: the record still answers under its own name.
    let (status, body) = describe_generated_template(&router, "renaming-record").await;
    assert_eq!(
        status, 200,
        "expected the refused rename to leave the record alone, got {status} with {body}"
    );
    assert_eq!(
        xml_text(&body, "GeneratedTemplateName").as_deref(),
        Some("renaming-record"),
        "expected the record to keep its own name, got {body}"
    );
}

/// A delete is accepted whatever status the record's generation is in: the status a create
/// leaves, the status an update leaves, and the terminal one the clock settles onto.
#[tokio::test]
async fn a_delete_is_accepted_whatever_status_the_generation_is_in() {
    let router = router();
    // Each row says how the record is left standing before the delete, and the status it
    // then reports; the two in-flight rows are the ones a caller reaches by deleting
    // straight after the call that started the generation.
    let rows: [(&str, bool, bool); 3] = [
        ("deleted straight after the create", false, false),
        ("deleted straight after an update", true, false),
        ("deleted once the generation concluded", false, true),
    ];

    for (index, (case, update_first, settle_first)) in rows.into_iter().enumerate() {
        let name = format!("deleted-at-status-{index}");
        let (status, body) = create_generated_template(&router, &name, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateGeneratedTemplate answered {status} with {body}"
        );
        let id = xml_text(&body, "GeneratedTemplateId")
            .unwrap_or_else(|| panic!("case {case:?}: expected a GeneratedTemplateId, got {body}"));

        if update_first {
            wait_for_the_generation().await;
            let (status, body) = call(
                &router,
                "UpdateGeneratedTemplate",
                &[member("GeneratedTemplateName", &name)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: UpdateGeneratedTemplate answered {status} with {body}"
            );
        }
        if settle_first {
            wait_for_the_generation().await;
        }

        // What status the record is really in when the delete is sent, read from the record
        // itself rather than assumed.
        let (status, described) = describe_generated_template(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeGeneratedTemplate answered {status} with {described}"
        );
        let reported = xml_text(&described, "Status").unwrap_or_default();
        if settle_first {
            assert_eq!(
                reported,
                generated::status::COMPLETE,
                "case {case:?}: expected the concluded status, got {described}"
            );
        }

        let (status, body) = call(
            &router,
            "DeleteGeneratedTemplate",
            &[member("GeneratedTemplateName", &name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the delete of a record in {reported} accepted, got \
             {status} with {body}"
        );
        assert!(
            xml_text(&body, "Code").is_none(),
            "case {case:?}: expected a successful response, got {body}"
        );

        // Nothing addresses it afterwards, by either address, and the generation it was
        // discarded mid-flight never brings it back once its conclusion falls due.
        for address in [name.clone(), id.clone()] {
            for action in ["DescribeGeneratedTemplate", "GetGeneratedTemplate"] {
                let (status, body) = call(
                    &router,
                    action,
                    &[member("GeneratedTemplateName", &address)],
                )
                .await;
                assert_eq!(
                    status, 404,
                    "case {case:?}: expected {action}({address:?}) refused, got {status} with \
                     {body}"
                );
                assert_eq!(
                    error_code(case, &body),
                    "GeneratedTemplateNotFound",
                    "case {case:?}: expected the modelled not-found refusal, got {body}"
                );
            }
        }
        wait_for_the_generation().await;
        let (status, body) = describe_generated_template(&router, &name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the record still gone once the generation fell due, got \
             {status} with {body}"
        );
    }
}

/// One more operation reaches these records: a delete frees the in-flight quota the record
/// counted against, so a burst that filled it can be made room in by deleting.
#[tokio::test]
async fn deleting_an_in_flight_record_frees_the_concurrency_quota() {
    let router = router();
    let cap = generated::MAX_CONCURRENT_GENERATIONS;

    // Fill the in-flight set.
    let started = Instant::now();
    for index in 0..cap {
        let name = format!("quota-filler-{index}");
        let (status, body) = create_generated_template(&router, &name, &[]).await;
        assert_eq!(
            status, 200,
            "case \"filling the in-flight set\": {name} answered {status} with {body}"
        );
    }

    // One more is refused while the set is full. Held to the clock, the way the cap's own
    // suite is: the refusal is owed only while those generations really are in flight.
    let (status, body) = create_generated_template(&router, "over-the-cap", &[]).await;
    if started.elapsed() < generated::GENERATION_DELAY {
        assert_eq!(
            status, 429,
            "case \"one past the cap\": expected the modelled 429, got {status} with {body}"
        );
        assert_eq!(
            error_code("one past the cap", &body),
            "ConcurrentResourcesLimitExceeded",
            "case \"one past the cap\": expected the modelled concurrency refusal, got {body}"
        );
    }

    // Whatever the clock did with that call, the delete of a record whose generation is
    // still in flight is accepted and the slot it held is free: the create that follows is
    // accepted without waiting for any generation to conclude.
    let (status, body) = call(
        &router,
        "DeleteGeneratedTemplate",
        &[member("GeneratedTemplateName", "quota-filler-0")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"deleting an in-flight record\": expected the delete accepted, got {status} \
         with {body}"
    );
    let (status, body) = create_generated_template(&router, "after-the-delete", &[]).await;
    assert_eq!(
        status, 200,
        "case \"after the delete freed a slot\": expected the create accepted, got {status} \
         with {body}"
    );
    // ...and the deleted record is gone, so the Region holds the records that were left
    // standing plus the one the freed slot admitted.
    let (status, body) = describe_generated_template(&router, "quota-filler-0").await;
    assert_eq!(
        status, 404,
        "case \"deleting an in-flight record\": expected the record gone, got {status} with \
         {body}"
    );
}
