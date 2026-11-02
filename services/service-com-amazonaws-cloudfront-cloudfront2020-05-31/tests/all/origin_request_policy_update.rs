// SPDX-License-Identifier: CC0-1.0

//! `UpdateOriginRequestPolicy`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront states for the update of an
//! origin request policy:
//!
//! * a mutation addressed to an id the store holds no record for is refused "does not
//!   exist" (`NoSuchOriginRequestPolicy`, HTTP 404) BEFORE any precondition is evaluated, so
//!   an id that has been deleted is unusable rather than merely unreadable, and the same
//!   fixed sentence the read answers with is the one the write answers with;
//! * an accepted update REPLACES the stored configuration whole — "all the fields are
//!   updated with the values provided in the request. You cannot update some fields
//!   independent of others" — so the read then answers with the submitted configuration down
//!   to the members it left out, under the id the policy was created with, beside the fresh
//!   version token the update handed back;
//! * the version token a mutation hands back is the only one an `If-Match` against that
//!   policy satisfies afterwards: an absent or malformed header is `InvalidIfMatchVersion`
//!   (HTTP 400), a well-formed token that is not the current one is `PreconditionFailed`
//!   (HTTP 412), and the current one — bare, quoted or weak — is accepted; so of two updates
//!   that start from ONE token exactly one lands and the other is refused on the
//!   precondition, whether the two are sent concurrently or one after the other;
//! * every constraint the create path refuses a configuration with holds on an update too (a
//!   list wrapper whose `Quantity` disagrees with its `Items`, a `Name` another policy
//!   holds), and a refused update writes NOTHING: the stored configuration and its version
//!   token are exactly what they were;
//! * a policy Amazon Web Services published is read-only to the account that reads it, so an
//!   update aimed at one is `IllegalUpdate` (HTTP 400) whatever version the request carries.
//!
//! The rows span what those rules generalise over: every configuration of
//! [`crate::origin_request_policy_lifecycle::policy_table`] (which of the three name lists
//! the policy carries at all, each list's length, every value of the three model-derived
//! behaviour catalogs, text that is empty, ASCII, non-ASCII or carries XML metacharacters),
//! every form an id takes that names no policy (a deleted id, a UUID never minted, a case
//! variant, a value that is no id, a policy name), every form an `If-Match` takes (absent,
//! empty, malformed, another resource's live token, a well-formed token never minted, and
//! the current one bare, quoted and weak), and every entry of the published catalogue —
//! including entries no issue cited.
//!
//! The wire harness is the one [`crate::distribution_lifecycle`] builds and the policy
//! create, read and configuration table are [`crate::origin_request_policy_lifecycle`]'s,
//! reused rather than copied.

use std::collections::BTreeSet;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    ILLEGAL_UPDATE_MANAGED_POLICY_MESSAGE, MANAGED_ORIGIN_REQUEST_POLICIES,
    NO_SUCH_ORIGIN_REQUEST_POLICY_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, emulator, inconsistent_quantities_message, is_version_token, Answer, API_VERSION,
};
use crate::origin_request_policy_lifecycle::{
    assert_config, create_policy, get_policy, policy_table, strings, Policy,
};
use crate::response_headers_policy_lifecycle::text;

// ---------------------------------------------------------------------------
// Calling the update
// ---------------------------------------------------------------------------

/// `PUT /2020-05-31/origin-request-policy/{Id}` with the `OriginRequestPolicyConfig`
/// document `body` and the `If-Match` header `if_match` (left off entirely when `None`) —
/// the route, the payload member and the header the model binds this operation to.
async fn update_policy(
    router: &MultiServiceRouter,
    id: &str,
    body: &str,
    if_match: Option<&str>,
) -> Answer {
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/origin-request-policy/{id}"),
        Some(body),
        if_match,
    )
    .await
}

/// `DELETE /2020-05-31/origin-request-policy/{Id}` under the version token `if_match` — the
/// call that makes an id name no policy any more.
async fn delete_policy(router: &MultiServiceRouter, id: &str, if_match: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/{API_VERSION}/origin-request-policy/{id}"),
        None,
        Some(if_match),
    )
    .await
}

/// Create `policy` and report the id and version token the create minted.
async fn seed(router: &MultiServiceRouter, policy: &Policy) -> (String, String) {
    let created = create_policy(router, &policy.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201 from CreateOriginRequestPolicy, got {} with {}",
        policy.case, created.status, created.raw
    );
    let id = text(created.root(), "Id", policy.case, "the create", &created.raw);
    let etag = created.header("etag").to_string();
    assert!(
        is_version_token(&etag),
        "case {}: expected a minted version token, got {etag:?}",
        policy.case
    );
    (id, etag)
}

/// Assert `GetOriginRequestPolicy` answers for `id` with `policy`'s configuration beside
/// `etag` — the one stored record, read back through the operation that reports it.
async fn assert_stored(
    router: &MultiServiceRouter,
    id: &str,
    policy: &Policy,
    etag: &str,
    context: &str,
) {
    let case = policy.case;
    let fetched = get_policy(router, id).await;
    assert_eq!(
        fetched.status, 200,
        "case {case} ({context}): expected HTTP 200 from GetOriginRequestPolicy, got {} with {}",
        fetched.status, fetched.raw
    );
    assert_eq!(
        fetched.root().text_at("Id"),
        Some(id),
        "case {case} ({context}): expected the policy to keep the id {id}, got {}",
        fetched.raw
    );
    let config = fetched
        .root()
        .at("OriginRequestPolicyConfig")
        .unwrap_or_else(|| {
            panic!(
                "case {case} ({context}): the get carries no OriginRequestPolicyConfig: {}",
                fetched.raw
            )
        });
    assert_config(config, policy, context, &fetched.raw);
    assert_eq!(
        fetched.header("etag"),
        etag,
        "case {case} ({context}): expected the stored version token {etag}, got {:?}",
        fetched.header("etag")
    );
}

/// The `OriginRequestPolicyConfig` document for a policy named `name` whose `Headers`
/// wrapper is spelled verbatim as `wrapper` — how a `Quantity` that disagrees with its
/// `Items` is put on the wire at all, which the typed table above cannot express.
fn raw_config(name: &str, wrapper: &str) -> String {
    raw_config_with(name, "whitelist", wrapper, "none", "", "none", "")
}

/// The `OriginRequestPolicyConfig` document for a policy named `name` whose three behaviour
/// configurations carry the behaviours and the verbatim list wrappers stated — so a
/// `Quantity` that disagrees with its `Items` can be put on the wire under ANY of the three
/// wrappers an origin request policy reaches, not only the first.
fn raw_config_with(
    name: &str,
    header_behavior: &str,
    headers: &str,
    cookie_behavior: &str,
    cookies: &str,
    query_string_behavior: &str,
    query_strings: &str,
) -> String {
    format!(
        "<OriginRequestPolicyConfig \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <Name>{name}</Name>\
         <HeadersConfig><HeaderBehavior>{header_behavior}</HeaderBehavior>{headers}\
         </HeadersConfig>\
         <CookiesConfig><CookieBehavior>{cookie_behavior}</CookieBehavior>{cookies}\
         </CookiesConfig>\
         <QueryStringsConfig><QueryStringBehavior>{query_string_behavior}\
         </QueryStringBehavior>{query_strings}</QueryStringsConfig>\
         </OriginRequestPolicyConfig>"
    )
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// A mutation addressed to an id the store holds no policy for is refused
/// `NoSuchOriginRequestPolicy` (HTTP 404) with the fixed sentence of that error class,
/// whatever the `If-Match` header says — the id is what the mutation is about, so it is
/// answered before any precondition. The first row is the lifecycle the gate exists for: a
/// policy created, read, deleted under its live token, then updated under that same token.
/// Nothing is created under a refused id, and the policy that does exist is untouched.
#[tokio::test]
async fn an_id_no_policy_holds_is_refused_before_the_precondition() {
    let router = emulator();

    // The policy that gets deleted, so its id is live, then dead, within one run.
    let doomed = Policy {
        case: "a policy created, read and deleted",
        name: "update-after-delete".to_string(),
        comment: Some("deleted before the update".to_string()),
        header_behavior: "whitelist".to_string(),
        headers: Some(strings(&["CloudFront-Viewer-Country"])),
        ..Policy::default()
    };
    let (dead_id, dead_etag) = seed(&router, &doomed).await;
    assert_stored(&router, &dead_id, &doomed, &dead_etag, "before the delete").await;
    let removed = delete_policy(&router, &dead_id, &dead_etag).await;
    assert_eq!(
        removed.status, 204,
        "case {}: expected HTTP 204 from the delete, got {} with {}",
        doomed.case, removed.status, removed.raw
    );

    // A policy that stays, so each refusal is shown to leave the store alone.
    let present = Policy {
        case: "the one policy that still exists",
        name: "present-under-update".to_string(),
        ..Policy::default()
    };
    let (id, etag) = seed(&router, &present).await;

    let submitted = Policy {
        case: "the configuration the refused updates submit",
        name: "never-stored".to_string(),
        cookie_behavior: "all".to_string(),
        ..Policy::default()
    };
    let body = submitted.xml();

    // Rows: the deleted id under the token that was live for it, a UUID never minted, the
    // live id with one character changed, the live id upper-cased, a value that is no id at
    // all and a policy NAME rather than an id — each under a different `If-Match` form, so
    // the 404 is shown to come before the precondition rather than beside one header.
    let mut flipped: Vec<char> = id.chars().collect();
    let last = flipped.len() - 1;
    flipped[last] = if flipped[last] == 'a' { 'b' } else { 'a' };
    let rows: [(&str, String, Option<String>); 6] = [
        (
            "the deleted id, under the token that was current for it",
            dead_id.clone(),
            Some(dead_etag.clone()),
        ),
        (
            "the deleted id, with no If-Match at all",
            dead_id.clone(),
            None,
        ),
        (
            "a UUID never minted, with the live policy's token",
            "00000000-0000-0000-0000-000000000000".to_string(),
            Some(etag.clone()),
        ),
        (
            "the live id with one character changed, with a malformed If-Match",
            flipped.into_iter().collect(),
            Some("not-a-version-token".to_string()),
        ),
        (
            "the live id upper-cased, with a well-formed token never minted",
            id.to_uppercase(),
            Some("E1234567890123".to_string()),
        ),
        (
            "a policy name rather than an id",
            present.name.clone(),
            Some(etag.clone()),
        ),
    ];

    for (case, candidate, if_match) in rows {
        let answer = update_policy(&router, &candidate, &body, if_match.as_deref()).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404 for {candidate:?}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchOriginRequestPolicy",
            "case {case}: expected NoSuchOriginRequestPolicy for {candidate:?}, got {}",
            answer.raw
        );
        let message = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, NO_SUCH_ORIGIN_REQUEST_POLICY_MESSAGE,
            "case {case}: expected the modelled missing-policy sentence, got {message:?}"
        );

        // The refused update created nothing under the id it named: the read answers with
        // the same code and the same sentence the write just did.
        let fetched = get_policy(&router, &candidate).await;
        assert_eq!(
            fetched.status, 404,
            "case {case}: expected {candidate:?} to still name no policy, got {} with {}",
            fetched.status, fetched.raw
        );
        assert_eq!(
            fetched.root().find_text("Message").unwrap_or_default(),
            NO_SUCH_ORIGIN_REQUEST_POLICY_MESSAGE,
            "case {case}: expected the read to state the same sentence as the write, got {}",
            fetched.raw
        );
    }

    assert_stored(&router, &id, &present, &etag, "after the refused updates").await;
}

/// An accepted update replaces the stored configuration WHOLE and mints a fresh version
/// token: the read then reports the submitted configuration (down to the members it left
/// out) under the id the create minted, beside that new token — and the token the update
/// matched on no longer satisfies an `If-Match`.
///
/// The rows walk the whole configuration table, each row updated to the NEXT row's
/// configuration so the update differs from what was stored along every member the table
/// varies; alternate rows rename the policy, the others keep the name they already hold.
#[tokio::test]
async fn an_update_replaces_the_whole_configuration_and_the_read_reports_it() {
    let router = emulator();
    let table = policy_table();
    let mut etags = BTreeSet::new();

    for (index, policy) in table.iter().enumerate() {
        let case = policy.case;
        let created = Policy {
            name: format!("update-from-{index}"),
            ..policy.clone()
        };
        let (id, create_etag) = seed(&router, &created).await;
        assert!(
            etags.insert(create_etag.clone()),
            "case {case}: the version token {create_etag} repeated"
        );

        // The next row's configuration, under a name this policy may keep or move to.
        let next = &table[(index + 1) % table.len()];
        let renamed = index % 2 == 1;
        let target = Policy {
            case,
            name: if renamed {
                format!("update-to-{index}")
            } else {
                created.name.clone()
            },
            ..next.clone()
        };

        let updated = update_policy(&router, &id, &target.xml(), Some(&create_etag)).await;
        assert_eq!(
            updated.status, 200,
            "case {case} (renamed {renamed}): expected HTTP 200 from the update, got {} with {}",
            updated.status, updated.raw
        );
        assert_eq!(
            updated.root().text_at("Id"),
            Some(id.as_str()),
            "case {case}: expected the update to report the policy's own id {id}, got {}",
            updated.raw
        );
        let last_modified = text(
            updated.root(),
            "LastModifiedTime",
            case,
            "the update",
            &updated.raw,
        );
        assert!(
            !last_modified.is_empty(),
            "case {case}: expected a LastModifiedTime from the update, got {last_modified:?}"
        );
        let config = updated
            .root()
            .at("OriginRequestPolicyConfig")
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: the update carries no OriginRequestPolicyConfig: {}",
                    updated.raw
                )
            });
        assert_config(config, &target, "the update", &updated.raw);

        let etag = updated.header("etag").to_string();
        assert!(
            is_version_token(&etag),
            "case {case}: expected a version token in the update's ETag header, got {etag:?}"
        );
        assert_ne!(
            etag, create_etag,
            "case {case}: expected the update to mint a fresh version token, got {etag} again"
        );
        assert!(
            etags.insert(etag.clone()),
            "case {case}: the version token {etag} repeated"
        );

        assert_stored(&router, &id, &target, &etag, "after the update").await;

        // The token the update matched on is superseded: it is a well-formed version that
        // is no longer current, so a second update carrying it fails the precondition.
        let stale = update_policy(&router, &id, &created.xml(), Some(&create_etag)).await;
        assert_eq!(
            stale.status, 412,
            "case {case}: expected HTTP 412 for the superseded token {create_etag}, got {} with {}",
            stale.status, stale.raw
        );
        assert_eq!(
            stale.error_code(),
            "PreconditionFailed",
            "case {case}: expected PreconditionFailed, got {}",
            stale.raw
        );
        assert_stored(&router, &id, &target, &etag, "after the stale update").await;
    }
}

/// Two updates that start from ONE version token resolve to exactly one winner: the first
/// to land mints a new token, so the other no longer matches the stored version and is
/// refused on the precondition. Asserted both concurrently (the two requests dispatched
/// together) and sequentially, over the whole table, and the stored record afterwards is
/// the winner's configuration — never a blend of the two.
#[tokio::test]
async fn two_updates_from_one_token_resolve_to_one_winner() {
    let router = emulator();
    let table = policy_table();

    for (index, policy) in table.iter().enumerate() {
        let case = policy.case;
        let created = Policy {
            name: format!("orp-race-{index}"),
            ..policy.clone()
        };
        let (id, etag) = seed(&router, &created).await;

        // Two candidate configurations, each differing from the stored one and from each
        // other, under the name the policy already holds.
        let first = Policy {
            case,
            name: created.name.clone(),
            comment: Some(format!("racer a for {index}")),
            ..table[(index + 1) % table.len()].clone()
        };
        let second = Policy {
            case,
            name: created.name.clone(),
            comment: Some(format!("racer b for {index}")),
            ..table[(index + 2) % table.len()].clone()
        };

        let first_body = first.xml();
        let second_body = second.xml();
        let (a, b) = tokio::join!(
            update_policy(&router, &id, &first_body, Some(&etag)),
            update_policy(&router, &id, &second_body, Some(&etag)),
        );

        let winners: Vec<&Answer> = [&a, &b]
            .into_iter()
            .filter(|answer| answer.status == 200)
            .collect();
        assert_eq!(
            winners.len(),
            1,
            "case {case}: expected exactly one winner, got statuses {} and {} with {} / {}",
            a.status,
            b.status,
            a.raw,
            b.raw
        );
        let loser = if a.status == 200 { &b } else { &a };
        assert_eq!(
            loser.status, 412,
            "case {case}: expected the loser to be refused on the precondition, got {} with {}",
            loser.status, loser.raw
        );
        assert_eq!(
            loser.error_code(),
            "PreconditionFailed",
            "case {case}: expected PreconditionFailed from the loser, got {}",
            loser.raw
        );

        // The stored record is the winner's configuration, at the token the winner minted.
        let winner_etag = winners[0].header("etag").to_string();
        let landed = if a.status == 200 { &first } else { &second };
        assert_stored(&router, &id, landed, &winner_etag, "after the race").await;

        // Sequentially, from the token that is now stale: the same refusal, and nothing
        // changes.
        let late = update_policy(&router, &id, &second_body, Some(&etag)).await;
        assert_eq!(
            late.status, 412,
            "case {case}: expected HTTP 412 from the superseded token, got {} with {}",
            late.status, late.raw
        );
        assert_stored(&router, &id, landed, &winner_etag, "after the late update").await;
    }
}

/// What the `If-Match` header says about the stored version decides the update: absent or
/// not a version token at all is `InvalidIfMatchVersion` (HTTP 400); a well-formed token
/// that is not this policy's current one is `PreconditionFailed` (HTTP 412); the current one
/// is accepted however HTTP spells an entity tag — bare, quoted or weak.
///
/// Each refused row leaves the stored configuration and its version token untouched; each
/// accepted row replaces them, and the row that follows is checked against the token the
/// accepted one minted.
#[tokio::test]
async fn the_if_match_precondition_decides_the_update() {
    let router = emulator();
    let mut stored = Policy {
        case: "the policy under update",
        name: "precondition-subject".to_string(),
        comment: Some("the stored configuration".to_string()),
        ..Policy::default()
    };
    let (id, mut current) = seed(&router, &stored).await;

    // A second policy, so one of the rows below can carry a version token that is current
    // for SOME policy and still not this one's.
    let other = Policy {
        case: "another policy, holding a live token of its own",
        name: "precondition-other".to_string(),
        ..Policy::default()
    };
    let (_, other_etag) = seed(&router, &other).await;

    /// How the `If-Match` header is spelled for one row.
    #[derive(Debug, Clone, Copy)]
    enum Header {
        /// No `If-Match` header at all.
        Absent,
        /// A literal value, carried verbatim.
        Literal(&'static str),
        /// Another policy's current token.
        Other,
        /// This policy's current token, rendered by the row's own format string.
        Current(&'static str),
    }

    // (case, the header, expected status, expected wire code — 200 and "" when accepted)
    let rows: [(&str, Header, u16, &str); 9] = [
        (
            "no If-Match header at all",
            Header::Absent,
            400,
            "InvalidIfMatchVersion",
        ),
        (
            "an empty If-Match header",
            Header::Literal(""),
            400,
            "InvalidIfMatchVersion",
        ),
        (
            "an If-Match that is no version token",
            Header::Literal("not-a-version-token"),
            400,
            "InvalidIfMatchVersion",
        ),
        (
            "an If-Match in the token shape but lower case",
            Header::Literal("e1234567890123"),
            400,
            "InvalidIfMatchVersion",
        ),
        (
            "a well-formed token never minted",
            Header::Literal("E1234567890123"),
            412,
            "PreconditionFailed",
        ),
        (
            "another policy's current token",
            Header::Other,
            412,
            "PreconditionFailed",
        ),
        (
            "the current token, bare",
            Header::Current("{current}"),
            200,
            "",
        ),
        (
            "the current token, quoted as an entity tag",
            Header::Current("\"{current}\""),
            200,
            "",
        ),
        (
            "the current token, as a weak entity tag",
            Header::Current("W/\"{current}\""),
            200,
            "",
        ),
    ];

    for (round, (case, header, status, code)) in rows.iter().enumerate() {
        // A configuration that differs from the stored one in every round, so an update
        // that landed when it should not have is visible in the read that follows.
        let submitted = Policy {
            case: "the submitted configuration",
            name: "precondition-subject".to_string(),
            comment: Some(format!("round {round}")),
            header_behavior: "whitelist".to_string(),
            headers: Some(strings(&["x-round", &format!("x-round-{round}")])),
            cookie_behavior: if round % 2 == 0 { "all" } else { "none" }.to_string(),
            cookies: None,
            query_string_behavior: "whitelist".to_string(),
            query_strings: Some(strings(&[&format!("round{round}")])),
        };

        let value = match header {
            Header::Absent => None,
            Header::Literal(literal) => Some((*literal).to_string()),
            Header::Other => Some(other_etag.clone()),
            Header::Current(format) => Some(format.replace("{current}", &current)),
        };
        let answer = update_policy(&router, &id, &submitted.xml(), value.as_deref()).await;
        assert_eq!(
            answer.status, *status,
            "case {case}: expected HTTP {status} for If-Match {value:?}, got {} with {}",
            answer.status, answer.raw
        );

        if *status != 200 {
            assert_eq!(
                answer.error_code(),
                *code,
                "case {case}: expected {code} for If-Match {value:?}, got {}",
                answer.raw
            );
            assert_stored(&router, &id, &stored, &current, &format!("after {case}")).await;
            continue;
        }

        let fresh = answer.header("etag").to_string();
        assert!(
            is_version_token(&fresh),
            "case {case}: expected a version token in the ETag header, got {fresh:?}"
        );
        assert_ne!(
            fresh, current,
            "case {case}: expected a fresh version token, got {fresh} again"
        );
        stored = Policy {
            case: stored.case,
            ..submitted
        };
        current = fresh;
        assert_stored(&router, &id, &stored, &current, &format!("after {case}")).await;
    }
}

/// Every constraint the create path refuses a configuration with holds on an update, and a
/// refused update writes nothing: the stored configuration, its version token and the name
/// it holds are exactly what they were, so the policy is still readable and still updatable
/// with the token it already had.
///
/// The rows span the refusals: a list wrapper's `Quantity` overstated and understated, and a
/// name another policy holds.
#[tokio::test]
async fn a_refused_update_writes_nothing() {
    let router = emulator();

    // The policy every row below tries to update.
    let subject = Policy {
        case: "the policy under update",
        name: "refusal-subject".to_string(),
        comment: Some("the stored configuration".to_string()),
        header_behavior: "whitelist".to_string(),
        headers: Some(strings(&["authorization", "x-tenant-id"])),
        cookie_behavior: "none".to_string(),
        cookies: None,
        query_string_behavior: "all".to_string(),
        query_strings: None,
    };
    let (id, etag) = seed(&router, &subject).await;

    // The policy that holds a name no other policy may take.
    let occupant = Policy {
        case: "the policy holding a name of its own",
        name: "refusal-occupant".to_string(),
        ..Policy::default()
    };
    let (_, _) = seed(&router, &occupant).await;

    // (case, the submitted document, expected status, expected wire code, expected message)
    //
    // The `InconsistentQuantities` rows span all three wrappers an origin request policy
    // reaches and both directions the `Quantity` can disagree in, and each asserts the
    // SENTENCE: it names the wrapper member whose quantity disagreed and the two counts
    // taken from the request.
    let rows: Vec<(&'static str, String, u16, &'static str, Option<String>)> = vec![
        (
            "a Headers wrapper whose Quantity is overstated",
            raw_config(
                "refusal-subject",
                "<Headers><Quantity>2</Quantity><Items><Name>authorization</Name></Items>\
                 </Headers>",
            ),
            400,
            "InconsistentQuantities",
            Some(inconsistent_quantities_message("Headers", 2, 1)),
        ),
        (
            "a Headers wrapper whose Quantity is understated",
            raw_config(
                "refusal-subject",
                "<Headers><Quantity>1</Quantity><Items><Name>accept</Name>\
                 <Name>authorization</Name></Items></Headers>",
            ),
            400,
            "InconsistentQuantities",
            Some(inconsistent_quantities_message("Headers", 1, 2)),
        ),
        (
            "a Cookies wrapper whose Quantity is overstated",
            raw_config_with(
                "refusal-subject",
                "none",
                "",
                "whitelist",
                "<Cookies><Quantity>3</Quantity><Items><Name>session</Name></Items></Cookies>",
                "none",
                "",
            ),
            400,
            "InconsistentQuantities",
            Some(inconsistent_quantities_message("Cookies", 3, 1)),
        ),
        (
            "a QueryStrings wrapper whose Quantity is overstated",
            raw_config_with(
                "refusal-subject",
                "none",
                "",
                "none",
                "",
                "whitelist",
                "<QueryStrings><Quantity>2</Quantity><Items><Name>page</Name></Items>\
                 </QueryStrings>",
            ),
            400,
            "InconsistentQuantities",
            Some(inconsistent_quantities_message("QueryStrings", 2, 1)),
        ),
        (
            "a Name another policy already holds",
            Policy {
                name: occupant.name.clone(),
                ..Policy::default()
            }
            .xml(),
            409,
            "OriginRequestPolicyAlreadyExists",
            None,
        ),
    ];

    for (case, body, status, code, message) in rows {
        let answer = update_policy(&router, &id, &body, Some(&etag)).await;
        assert_eq!(
            answer.status, status,
            "case {case}: expected HTTP {status} from the update, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            code,
            "case {case}: expected {code}, got {}",
            answer.raw
        );
        if let Some(expected) = &message {
            let actual = answer.root().find_text("Message").unwrap_or_default();
            assert_eq!(
                actual, *expected,
                "case {case}: expected {expected:?}, got {actual:?} in {}",
                answer.raw
            );
        }
        // Nothing was written: the configuration and the version token are untouched, so
        // the token the refused update carried is still the current one.
        assert_stored(&router, &id, &subject, &etag, &format!("after {case}")).await;
    }

    // And the policy is still updatable with that same token.
    let accepted = Policy {
        case: "the update that follows every refusal",
        name: "refusal-subject".to_string(),
        header_behavior: "allViewer".to_string(),
        ..Policy::default()
    };
    let answer = update_policy(&router, &id, &accepted.xml(), Some(&etag)).await;
    assert_eq!(
        answer.status, 200,
        "case {}: expected HTTP 200, got {} with {}",
        accepted.case, answer.status, answer.raw
    );
    let fresh = answer.header("etag").to_string();
    assert_stored(&router, &id, &accepted, &fresh, "after the accepted update").await;
}

/// A policy Amazon Web Services published cannot be written over by the account that reads
/// it: every entry of the published catalogue is refused `IllegalUpdate` whatever version
/// the request carries, and still reports the configuration AWS gave it afterwards.
#[tokio::test]
async fn a_published_policy_cannot_be_updated() {
    let router = emulator();

    let submitted = Policy {
        case: "the configuration the refused updates submit",
        name: "over-a-published-policy".to_string(),
        header_behavior: "allViewer".to_string(),
        ..Policy::default()
    };
    let body = submitted.xml();

    for managed in MANAGED_ORIGIN_REQUEST_POLICIES {
        let fetched = get_policy(&router, managed.id).await;
        assert_eq!(
            fetched.status, 200,
            "case {}: expected HTTP 200 from GetOriginRequestPolicy for the published policy, \
             got {} with {}",
            managed.name, fetched.status, fetched.raw
        );
        let etag = fetched.header("etag").to_string();
        let before = fetched.raw.clone();

        // The version the request carries cannot make the update legal, so every form of
        // `If-Match` is refused the same way.
        for (description, if_match) in [
            ("the policy's own version token", Some(etag.as_str())),
            ("no If-Match at all", None),
            ("an If-Match that is no version token", Some("not-a-version")),
        ] {
            let refused = update_policy(&router, managed.id, &body, if_match).await;
            assert_eq!(
                refused.status, 400,
                "case {} with {description}: expected HTTP 400, got {} with {}",
                managed.name, refused.status, refused.raw
            );
            assert_eq!(
                refused.error_code(),
                "IllegalUpdate",
                "case {} with {description}: expected IllegalUpdate, got {}",
                managed.name,
                refused.raw
            );
            assert_eq!(
                refused.root().find_text("Message").unwrap_or_default(),
                ILLEGAL_UPDATE_MANAGED_POLICY_MESSAGE,
                "case {} with {description}: expected the modelled illegal-update sentence, got {}",
                managed.name,
                refused.raw
            );
        }

        let after = get_policy(&router, managed.id).await;
        assert_eq!(
            after.status, 200,
            "case {}: expected the published policy to stay readable, got {} with {}",
            managed.name, after.status, after.raw
        );
        assert_eq!(
            after.raw, before,
            "case {}: expected the refused updates to leave the published policy alone, got {}",
            managed.name, after.raw
        );
        assert_eq!(
            after.header("etag"),
            etag,
            "case {}: expected the refused updates to leave the version token alone, got {:?}",
            managed.name,
            after.header("etag")
        );
    }
}
