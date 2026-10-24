// SPDX-License-Identifier: CC0-1.0

//! `UpdateCachePolicy` and the record `GetCachePolicyConfig` reports, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront states for the update/read
//! pair this operation documents ("Use GetCachePolicyConfig to get the current
//! configuration … Call UpdateCachePolicy by providing the entire cache policy
//! configuration"):
//!
//! * an accepted update REPLACES the stored configuration whole — "all the fields are
//!   updated with the values provided in the request. You cannot update some fields
//!   independent of others" — so both reads then answer with the submitted configuration
//!   down to the members it left out, under the id the policy was created with, and the
//!   `ETag` they report is the fresh version token that update handed back;
//! * the version token a mutation hands back is the only one an `If-Match` against that
//!   policy satisfies afterwards: an absent or malformed header is `InvalidIfMatchVersion`
//!   (HTTP 400), a well-formed token that is not the current one is `PreconditionFailed`
//!   (HTTP 412), and the current one — bare, quoted or weak — is accepted;
//! * every constraint the create path refuses a configuration with holds on an update too
//!   (the TTL ordering, a negative TTL, the documented comment bound, a list wrapper whose
//!   `Quantity` disagrees with its `Items`, a `Name` another policy holds), and a refused
//!   update writes NOTHING: the stored configuration and its version token are exactly what
//!   they were;
//! * an id no stored policy holds is `NoSuchCachePolicy` (HTTP 404), whatever the `If-Match`
//!   header says.
//!
//! The table rows differ along everything these rules generalise over: which optional
//! members a configuration carries at all, each name list's length, the behaviour enum
//! values, the TTL magnitudes, whether the update renames the policy or keeps its name, and
//! the form the `If-Match` header takes.

use std::collections::BTreeSet;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    MAX_CACHE_POLICY_COMMENT, NO_SUCH_CACHE_POLICY_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cache_policy_lifecycle::{
    assert_config, create_policy, get_policy, get_policy_config, policy_table, strings, Parameters,
    Policy,
};
use crate::distribution_lifecycle::{call, emulator, is_version_token, Answer, API_VERSION};
use crate::response_headers_policy_lifecycle::text;

// ---------------------------------------------------------------------------
// Calling the update
// ---------------------------------------------------------------------------

/// `PUT /2020-05-31/cache-policy/{Id}` with the `CachePolicyConfig` document `body` and the
/// `If-Match` header `if_match` (left off entirely when `None`).
async fn update_policy(
    router: &MultiServiceRouter,
    id: &str,
    body: &str,
    if_match: Option<&str>,
) -> Answer {
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/cache-policy/{id}"),
        Some(body),
        if_match,
    )
    .await
}

/// Create `policy` and report the id and version token the create minted.
async fn seed(router: &MultiServiceRouter, policy: &Policy) -> (String, String) {
    let created = create_policy(router, &policy.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201 from the create, got {} with {}",
        policy.case, created.status, created.raw
    );
    let id = text(created.root(), "Id", policy.case, "the create", &created.raw);
    let etag = created.header("etag").to_string();
    (id, etag)
}

/// Assert both readers answer for `id` with `policy`'s configuration beside `etag` — the
/// one stored record, read back through the two operations that report it.
async fn assert_stored(
    router: &MultiServiceRouter,
    id: &str,
    policy: &Policy,
    etag: &str,
    context: &str,
) {
    let case = policy.case;

    let config_answer = get_policy_config(router, id).await;
    assert_eq!(
        config_answer.status, 200,
        "case {case} ({context}): expected HTTP 200 from GetCachePolicyConfig, got {} with {}",
        config_answer.status, config_answer.raw
    );
    assert_config(config_answer.root(), policy, context, &config_answer.raw);
    assert_eq!(
        config_answer.header("etag"),
        etag,
        "case {case} ({context}): expected the stored version token {etag}, got {:?}",
        config_answer.header("etag")
    );

    let fetched = get_policy(router, id).await;
    assert_eq!(
        fetched.status, 200,
        "case {case} ({context}): expected HTTP 200 from GetCachePolicy, got {} with {}",
        fetched.status, fetched.raw
    );
    assert_eq!(
        fetched.root().text_at("Id"),
        Some(id),
        "case {case} ({context}): expected the policy to keep the id {id}, got {}",
        fetched.raw
    );
    let config = fetched.root().at("CachePolicyConfig").unwrap_or_else(|| {
        panic!("case {case} ({context}): the get carries no CachePolicyConfig: {}", fetched.raw)
    });
    assert_config(config, policy, context, &fetched.raw);
    assert_eq!(
        fetched.header("etag"),
        etag,
        "case {case} ({context}): expected the stored version token {etag} from GetCachePolicy, \
         got {:?}",
        fetched.header("etag")
    );
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// An accepted update replaces the stored configuration WHOLE and mints a fresh version
/// token: both reads then report the submitted configuration (down to the members it left
/// out) under the id the create minted, beside that new token — and the token the update
/// matched on no longer satisfies an `If-Match`.
///
/// The rows walk the whole configuration table, each row updated to the NEXT row's
/// configuration so the update differs from what was stored along every member the table
/// varies; alternate rows rename the policy, the others keep the name they already hold.
#[tokio::test]
async fn an_update_replaces_the_whole_configuration_both_reads_then_report() {
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
        let config = updated.root().at("CachePolicyConfig").unwrap_or_else(|| {
            panic!(
                "case {case}: the update carries no CachePolicyConfig: {}",
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

/// Every constraint the create path refuses a configuration with holds on an update, and a
/// refused update writes nothing: the stored configuration, its version token and the name
/// it holds are exactly what they were, so the policy is still readable and still updatable
/// with the token it already had.
///
/// The rows span the refusals: each disordered TTL pair, a negative TTL, the comment bound,
/// a list wrapper's `Quantity` overstated and understated, and a name another policy holds.
#[tokio::test]
async fn a_refused_update_writes_nothing() {
    let router = emulator();

    // The policy every row below tries to update, and the one that holds a name no other
    // policy may take.
    let subject = Policy {
        case: "the policy under update",
        name: "refusal-subject".to_string(),
        comment: Some("the stored configuration".to_string()),
        min_ttl: 10,
        default_ttl: Some(100),
        max_ttl: Some(1_000),
        parameters: Some(Parameters {
            gzip: true,
            brotli: Some(false),
            header_behavior: "whitelist".to_string(),
            headers: Some(strings(&["authorization", "x-tenant-id"])),
            cookie_behavior: "none".to_string(),
            cookies: None,
            query_string_behavior: "all".to_string(),
            query_strings: None,
        }),
    };
    let (id, etag) = seed(&router, &subject).await;
    let occupant = Policy {
        case: "the policy holding a name of its own",
        name: "refusal-occupant".to_string(),
        min_ttl: 0,
        ..Policy::default()
    };
    let (_, _) = seed(&router, &occupant).await;

    let bad_config = |case: &'static str, policy: Policy| (case, policy.xml());
    let raw_wrapper = |wrapper: &str| {
        format!(
            "<CachePolicyConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <Name>refusal-subject</Name><MinTTL>1</MinTTL>\
             <ParametersInCacheKeyAndForwardedToOrigin>\
             <EnableAcceptEncodingGzip>false</EnableAcceptEncodingGzip>\
             <HeadersConfig><HeaderBehavior>whitelist</HeaderBehavior>{wrapper}</HeadersConfig>\
             <CookiesConfig><CookieBehavior>none</CookieBehavior></CookiesConfig>\
             <QueryStringsConfig><QueryStringBehavior>none</QueryStringBehavior>\
             </QueryStringsConfig>\
             </ParametersInCacheKeyAndForwardedToOrigin></CachePolicyConfig>"
        )
    };

    // (case, the submitted document, expected status, expected wire code)
    let rows: Vec<(&'static str, String, u16, &'static str)> = vec![
        {
            let (case, body) = bad_config(
                "MaxTTL below MinTTL",
                Policy {
                    name: "refusal-subject".to_string(),
                    min_ttl: 3_600,
                    max_ttl: Some(60),
                    ..Policy::default()
                },
            );
            (case, body, 400, "InvalidArgument")
        },
        {
            let (case, body) = bad_config(
                "DefaultTTL below MinTTL by one",
                Policy {
                    name: "refusal-subject".to_string(),
                    min_ttl: 100,
                    default_ttl: Some(99),
                    ..Policy::default()
                },
            );
            (case, body, 400, "InvalidArgument")
        },
        {
            let (case, body) = bad_config(
                "MaxTTL below DefaultTTL",
                Policy {
                    name: "refusal-subject".to_string(),
                    min_ttl: 0,
                    default_ttl: Some(86_400),
                    max_ttl: Some(60),
                    ..Policy::default()
                },
            );
            (case, body, 400, "InvalidArgument")
        },
        {
            let (case, body) = bad_config(
                "a negative DefaultTTL",
                Policy {
                    name: "refusal-subject".to_string(),
                    min_ttl: 0,
                    default_ttl: Some(-1),
                    ..Policy::default()
                },
            );
            (case, body, 400, "InvalidArgument")
        },
        {
            let (case, body) = bad_config(
                "a comment one character past the documented bound",
                Policy {
                    name: "refusal-subject".to_string(),
                    comment: Some("c".repeat(MAX_CACHE_POLICY_COMMENT + 1)),
                    min_ttl: 1,
                    ..Policy::default()
                },
            );
            (case, body, 400, "InvalidArgument")
        },
        (
            "a Headers wrapper whose Quantity is overstated",
            raw_wrapper(
                "<Headers><Quantity>2</Quantity><Items><Name>authorization</Name></Items>\
                 </Headers>",
            ),
            400,
            "InconsistentQuantities",
        ),
        (
            "a Headers wrapper whose Quantity is understated",
            raw_wrapper(
                "<Headers><Quantity>1</Quantity><Items><Name>accept</Name>\
                 <Name>authorization</Name></Items></Headers>",
            ),
            400,
            "InconsistentQuantities",
        ),
        {
            let (case, body) = bad_config(
                "a Name another policy already holds",
                Policy {
                    name: occupant.name.clone(),
                    min_ttl: 1,
                    ..Policy::default()
                },
            );
            (case, body, 409, "CachePolicyAlreadyExists")
        },
    ];

    for (case, body, status, code) in rows {
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
        // Nothing was written: the configuration and the version token are untouched, so
        // the token the refused update carried is still the current one.
        assert_stored(&router, &id, &subject, &etag, &format!("after {case}")).await;
    }

    // And the policy is still updatable with that same token.
    let accepted = Policy {
        case: "the update that follows every refusal",
        name: "refusal-subject".to_string(),
        min_ttl: 5,
        default_ttl: Some(5),
        max_ttl: Some(5),
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

/// What the `If-Match` header says about the stored version decides the update: absent or
/// not a version token at all is `InvalidIfMatchVersion` (HTTP 400, "The If-Match version is
/// missing or not valid"); a well-formed token that is not this policy's current one is
/// `PreconditionFailed` (HTTP 412); the current one is accepted however HTTP spells an
/// entity tag — bare, quoted or weak.
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
        min_ttl: 1,
        default_ttl: Some(10),
        max_ttl: Some(100),
        ..Policy::default()
    };
    let (id, mut current) = seed(&router, &stored).await;

    // A second policy, so one of the rows below can carry a version token that is current
    // for SOME policy and still not this one's.
    let other = Policy {
        case: "another policy, holding a live token of its own",
        name: "precondition-other".to_string(),
        min_ttl: 0,
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
        ("the current token, bare", Header::Current("{current}"), 200, ""),
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
            min_ttl: round as i64,
            default_ttl: Some(round as i64 + 10),
            max_ttl: Some(round as i64 + 100),
            parameters: Some(Parameters {
                gzip: round % 2 == 0,
                brotli: Some(round % 3 == 0),
                header_behavior: "whitelist".to_string(),
                headers: Some(strings(&["x-round"])),
                cookie_behavior: "none".to_string(),
                cookies: None,
                query_string_behavior: "none".to_string(),
                query_strings: Some(Vec::new()),
            }),
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

/// An id no stored policy holds is `NoSuchCachePolicy` (HTTP 404) from the update, whatever
/// shape the id takes and whatever the `If-Match` header says — the id is what the update
/// is about, so it is answered before any precondition. Nothing is created under the
/// refused id, and the policy that does exist is untouched.
#[tokio::test]
async fn the_update_refuses_an_id_no_policy_holds() {
    let router = emulator();
    let present = Policy {
        case: "the one policy that exists",
        name: "present-under-update".to_string(),
        min_ttl: 1,
        ..Policy::default()
    };
    let (id, etag) = seed(&router, &present).await;

    let submitted = Policy {
        case: "the configuration the refused updates submit",
        name: "never-stored".to_string(),
        min_ttl: 2,
        ..Policy::default()
    };
    let body = submitted.xml();

    // Rows: a UUID never minted, the existing id in upper case, a token that is no UUID and
    // the existing policy's name — each with a different `If-Match` form, so the 404 is
    // shown to come before the precondition rather than beside one particular header.
    let mut flipped: Vec<char> = id.chars().collect();
    let last = flipped.len() - 1;
    flipped[last] = if flipped[last] == 'a' { 'b' } else { 'a' };
    let rows: [(&str, String, Option<String>); 5] = [
        (
            "a UUID never minted, with the live policy's token",
            "00000000-0000-0000-0000-000000000000".to_string(),
            Some(etag.clone()),
        ),
        (
            "the existing id with one character changed, with no If-Match at all",
            flipped.into_iter().collect(),
            None,
        ),
        (
            "the existing id in upper case, with a malformed If-Match",
            id.to_uppercase(),
            Some("not-a-version-token".to_string()),
        ),
        (
            "a token that is no UUID, with a well-formed token never minted",
            "not-a-policy-id".to_string(),
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
            "NoSuchCachePolicy",
            "case {case}: expected NoSuchCachePolicy for {candidate:?}, got {}",
            answer.raw
        );
        let message = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, NO_SUCH_CACHE_POLICY_MESSAGE,
            "case {case}: expected the modelled message, got {message:?}"
        );

        // The refused update created nothing under the id it named.
        let fetched = get_policy_config(&router, &candidate).await;
        assert_eq!(
            fetched.status, 404,
            "case {case}: expected {candidate:?} to still name no policy, got {} with {}",
            fetched.status, fetched.raw
        );
    }

    assert_stored(&router, &id, &present, &etag, "after the refused updates").await;
}
