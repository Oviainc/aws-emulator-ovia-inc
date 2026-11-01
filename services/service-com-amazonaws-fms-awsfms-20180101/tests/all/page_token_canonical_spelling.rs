// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the SPELLING half of the one `PaginationToken` rule
//! every paginated operation of this service shares: a `NextToken` names a page
//! boundary only when it is a token this service MINTED, and the only spelling
//! of a boundary it mints is the one its encoder emits.
//!
//! Hex decoding admits both cases of the digits `a`-`f` while the encoder emits
//! one of them, so a case-variant of a token this service issued is a string it
//! never issued: it must be refused as invalid input rather than resumed from,
//! exactly like any other value that is no token of this service.
//!
//! The rows below are the class that rule ranges over, driven end-to-end through
//! an operation of EVERY page-token shape this service has — the shared decode
//! step is one helper
//! (`state::decode_key_token`) and each shape adds only its own check on the
//! decoded key, so a spelling one shape accepted and another refused would be a
//! drift between them:
//!
//! * the bare store key — `ListThirdPartyFirewallFirewallPolicies`;
//! * a key stamped with the collection it belongs to — `ListMemberAccounts`
//!   (account-keyed) and `ListPolicies` (keyed by a minted identifier);
//! * a key stamped with the resource a request SELECTED the page by —
//!   `ListResourceSetResources`.
//!
//! Each row's token is the one the SERVICE minted for its own first page, so the
//! only difference between the accepted and the refused spellings is the case of
//! the hex letters, and the variants differ in how many letters they re-case and
//! where (all of them, the first, the last).
//!
//! Every test drives the service the way a client does — an awsJson1_1 `POST /`
//! through the wire harness in [`crate::policy_lifecycle`] — so the assertions
//! are about the WIRE response.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_fms_awsfms_20180101::state;
use smithy_server_runtime::MultiServiceRouter;

// The wire harness, the administrator helper and each collection's own writer
// live in one place for this test target, so no two suites drift in how a
// request is signed or a resource is created.
use crate::administrator_gated_operations::associate_admin;
use crate::compliance_status::{put_policy, put_roster};
use crate::policy_lifecycle::{call, emulator, error_code, ACCOUNT};
use crate::resource_set_resources::{create_set_with_members, instance, seeded_emulator};
use crate::third_party_firewall_firewall_policies::{primary_vendor, seeded};

/// One paginated page class: the operation that serves it, the output member
/// carrying its items, and the modelled error code it refuses a token it did not
/// mint with.
struct PageClass {
    operation: &'static str,
    items: &'static str,
    refusal: &'static str,
}

/// The page classes the spelling rule is asserted over: one per token shape this
/// service mints (see this module's docs).
fn page_classes() -> Vec<PageClass> {
    vec![
        PageClass {
            operation: "ListThirdPartyFirewallFirewallPolicies",
            items: "ThirdPartyFirewallFirewallPolicies",
            refusal: "InvalidInputException",
        },
        PageClass {
            operation: "ListMemberAccounts",
            items: "MemberAccounts",
            refusal: "ValidationException",
        },
        PageClass {
            operation: "ListPolicies",
            items: "PolicyList",
            refusal: "ValidationException",
        },
        PageClass {
            operation: "ListResourceSetResources",
            items: "Items",
            refusal: "InvalidInputException",
        },
    ]
}

/// A prepared page class: an emulator holding enough of its resources for its
/// page to have a boundary, and the request that selects that page with a bound
/// of one — so the first call mints a `NextToken`.
struct Prepared {
    router: MultiServiceRouter,
    request: Value,
    /// Held only to keep the engine the router was built on alive for the test.
    _engine: Option<Arc<engine::InMemoryEngine>>,
}

/// Stand up one class's resources, each created the way a client creates it (or,
/// for the vendor inventory no API creates, seeded through the store by the
/// suite that owns it).
async fn prepare(class: &PageClass) -> Prepared {
    match class.operation {
        "ListThirdPartyFirewallFirewallPolicies" => {
            let (router, engine) = seeded().await;
            Prepared {
                router,
                request: json!({ "ThirdPartyFirewall": primary_vendor(), "MaxResults": 1 }),
                _engine: Some(engine),
            }
        }
        "ListMemberAccounts" => {
            let router = emulator();
            associate_admin(&router).await;
            // The organisation roster a policy's `IncludeMap` contributes, which
            // is what gives this listing more than one account to page.
            put_roster(&router).await;
            Prepared {
                router,
                request: json!({ "MaxResults": 1 }),
                _engine: None,
            }
        }
        "ListPolicies" => {
            let router = emulator();
            associate_admin(&router).await;
            for name in ["first.policy", "second.policy"] {
                put_policy(&router, name, None, None).await;
            }
            Prepared {
                router,
                request: json!({ "MaxResults": 1 }),
                _engine: None,
            }
        }
        "ListResourceSetResources" => {
            let (router, engine) = seeded_emulator().await;
            let members = vec![instance(ACCOUNT, 1), instance(ACCOUNT, 2)];
            let (id, _arn) = create_set_with_members(
                &router,
                "page-token-spelling",
                &[state::RESOURCE_TYPE_EC2_INSTANCE],
                &members,
            )
            .await;
            Prepared {
                router,
                request: json!({ "Identifier": id, "MaxResults": 1 }),
                _engine: Some(engine),
            }
        }
        other => panic!("case {other}: this suite has no setup for that page class"),
    }
}

/// One call of a class's operation, as `(http status, error code or "", the
/// number of items served, NextToken)`.
async fn page(
    prepared: &Prepared,
    class: &PageClass,
    token: Option<&str>,
) -> (u16, String, usize, Option<String>) {
    let mut request = prepared.request.clone();
    if let Some(token) = token {
        request["NextToken"] = json!(token);
    }
    let (status, body) = call(&prepared.router, class.operation, &request).await;
    let code = if status == 200 {
        String::new()
    } else {
        error_code(&body)
    };
    let served = body[class.items].as_array().map_or(0, Vec::len);
    let next_token = body
        .get("NextToken")
        .filter(|token| !token.is_null())
        .and_then(Value::as_str)
        .map(str::to_string);
    (status, code, served, next_token)
}

/// The spellings of `token` that differ from it only in the case of its hex
/// letters: all of them re-cased, its first one, and its last one. Empty when
/// the token carries no letter at all, which the caller asserts against.
fn case_variants(token: &str) -> Vec<(String, String)> {
    let letters: Vec<usize> = token
        .char_indices()
        .filter(|(_, letter)| letter.is_ascii_alphabetic())
        .map(|(index, _)| index)
        .collect();
    let recased_at = |index: usize| {
        let mut spelling = token.to_string();
        spelling.replace_range(index..index + 1, &token[index..index + 1].to_uppercase());
        spelling
    };
    let mut variants: Vec<(String, String)> = Vec::new();
    if letters.is_empty() {
        return variants;
    }
    variants.push((
        "every hex letter re-cased".to_string(),
        token.to_uppercase(),
    ));
    if let Some(&first) = letters.first() {
        variants.push((
            format!("the hex letter at {first} re-cased"),
            recased_at(first),
        ));
    }
    if let Some(&last) = letters.last() {
        variants.push((
            format!("the hex letter at {last} re-cased"),
            recased_at(last),
        ));
    }
    variants
}

/// A token this service minted carries one spelling, and only that spelling
/// names a boundary: the minted token resumes the page, every case-variant of it
/// is refused with the operation's modelled error and reports nothing, and the
/// minted token still resumes the page afterwards.
#[tokio::test]
async fn only_the_spelling_a_page_class_mints_resumes_its_page() {
    for class in page_classes() {
        let operation = class.operation;
        let prepared = prepare(&class).await;

        let (status, code, served, minted) = page(&prepared, &class, None).await;
        assert_eq!(
            (status, code.as_str(), served),
            (200, "", 1),
            "case {operation}: the first page of one must be served, got {status} / {code:?} with \
             {served} items"
        );
        let minted = minted.unwrap_or_else(|| {
            panic!("case {operation}: a first page of one with more to come must carry a NextToken")
        });

        // The minted spelling: the encoder's own alphabet, hex digits in ONE
        // case.
        assert!(
            minted
                .chars()
                .all(|digit| digit.is_ascii_digit() || ('a'..='f').contains(&digit)),
            "case {operation}: a minted token is hex in the encoder's own case, got {minted:?}"
        );

        let variants = case_variants(&minted);
        assert!(
            !variants.is_empty(),
            "case {operation}: this class's boundary must spell out hex letters for the rule to \
             range over, got {minted:?}"
        );

        // The control row: the spelling the service minted resumes the page.
        let (status, code, served, _) = page(&prepared, &class, Some(&minted)).await;
        assert_eq!(
            (status, code.as_str(), served),
            (200, "", 1),
            "case {operation}: the minted token must resume the page, got {status} / {code:?} with \
             {served} items"
        );

        for (variant, token) in variants {
            let (status, code, served, next) = page(&prepared, &class, Some(&token)).await;
            assert_eq!(
                (status, code.as_str()),
                (400, class.refusal),
                "case {operation} / {variant:?}: a spelling this service never minted must be \
                 refused, got {status} / {code:?} for {token:?}"
            );
            assert_eq!(
                (served, next.as_deref()),
                (0, None),
                "case {operation} / {variant:?}: a refused call must report nothing, got {served} \
                 items and {next:?}"
            );

            // The refusal changed nothing: the minted spelling still resumes.
            let (status, code, served, _) = page(&prepared, &class, Some(&minted)).await;
            assert_eq!(
                (status, code.as_str(), served),
                (200, "", 1),
                "case {operation} / {variant:?}: a refused spelling must leave the page as it \
                 was, got {status} / {code:?} with {served} items"
            );
        }
    }
}

/// The spelling rule is the shared decoder's, not one handler's: the decode step
/// answers for the spelling it mints and for no other, over keys spanning the
/// byte classes this service's page keys are built from.
#[tokio::test]
async fn the_shared_decoder_answers_only_for_the_spelling_it_mints() {
    // Keys of the classes this service pages by: a vendor-stamped inventory key,
    // an ARN boundary, a collection-stamped account id, a minted identifier and
    // a namespace flag — each built through this service's own key helpers where
    // it has one.
    let keys: Vec<String> = vec![
        state::third_party_firewall_policy_key(primary_vendor(), "global-1"),
        instance(ACCOUNT, 3),
        format!("{}#{ACCOUNT}", state::MEMBER_ACCOUNTS_PAGE),
        format!(
            "{}#0f13c2a4-1111-4a26-9d0e-6b4f8d2a7c31",
            state::POLICIES_PAGE
        ),
        format!("{}#false#{}", state::APPS_LISTS_PAGE, "list"),
    ];

    for key in keys {
        let minted = state::encode_key_token(&key);
        assert_eq!(
            state::decode_key_token(&minted).as_deref(),
            Some(key.as_str()),
            "case {key:?}: the spelling this service mints must decode back to its key"
        );

        let variants = case_variants(&minted);
        assert!(
            !variants.is_empty(),
            "case {key:?}: this key's token must spell out hex letters for the rule to range \
             over, got {minted:?}"
        );
        for (variant, token) in variants {
            assert_eq!(
                state::decode_key_token(&token),
                None,
                "case {key:?} / {variant:?}: a spelling this service never mints names no \
                 boundary, got {:?} for {token:?}",
                state::decode_key_token(&token)
            );
        }
    }
}
