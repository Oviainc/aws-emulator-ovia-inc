// SPDX-License-Identifier: CC0-1.0

//! The SPELLING a `NextToken` of this service has to be in, end to end over the
//! `awsJson1_1` wire.
//!
//! THE RULE: a cursor is self-identifying, so a paginated read resumes ONLY from a token
//! byte for byte as this service minted it — the decoder's acceptance set is the encoder's
//! output set and nothing wider — and every other value, including a respelling of an
//! issued token, is refused the modelled `ValidationException` (HTTP 400) rather than
//! answered with a page. The encoding is hex, whose digits this service writes in lower
//! case, so a respelling is an upper- or mixed-case one; a page positioned from a value
//! this service never handed out would let a client fabricate a cursor, and an empty page
//! for it would end a caller's enumeration early.
//!
//! Every paginated operation of this crate resolves its token through the one gate
//! (`state::cursor_after` -> `state::decode_page_token`), so the rows span the
//! ENUMERATIONS that gate serves — an unscoped catalog, an unscoped account listing and a
//! listing scoped to one instance — crossed with the respellings `hex` reads but never
//! writes: every digit upper-cased, and a single digit of each end upper-cased. Each row
//! also asserts the token as MINTED is still served, so the rule is the encoder's output
//! set and not an empty one.
//!
//! The harness is the one `ssoadmin_permission_sets.rs` owns.

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{call, error_code, organization_router};

/// One enumeration of this service, as the rows drive it: the operation, the request that
/// pages it (`{instance}` standing in for the account's instance ARN) and the output
/// member its items arrive in.
struct Enumeration {
    operation: &'static str,
    request: fn(&str) -> Value,
    items: &'static str,
}

/// The enumerations the rows walk: `ListApplicationProviders` (a catalog scoped to
/// nothing), `ListInstances` (the account's own listing) and `ListPermissionSets` (scoped
/// to one instance, so its cursor carries that scope).
const ENUMERATIONS: &[Enumeration] = &[
    Enumeration {
        operation: "ListApplicationProviders",
        request: |_| json!({"MaxResults": 1}),
        items: "ApplicationProviders",
    },
    Enumeration {
        operation: "ListInstances",
        request: |_| json!({}),
        items: "Instances",
    },
    Enumeration {
        operation: "ListPermissionSets",
        request: |instance| json!({"InstanceArn": instance, "MaxResults": 1}),
        items: "PermissionSets",
    },
];

/// The same token with the hex digit at `which` of its LETTER digits (`a`-`f`, the only
/// digits that have a second spelling) upper-cased; `None` when it holds none.
fn with_letter_uppercased(token: &str, which: Which) -> Option<String> {
    let letters: Vec<usize> = token
        .char_indices()
        .filter(|(_, c)| c.is_ascii_alphabetic())
        .map(|(at, _)| at)
        .collect();
    let at = match which {
        Which::First => *letters.first()?,
        Which::Last => *letters.last()?,
    };
    let mut respelled = token.to_string();
    respelled.replace_range(at..at + 1, &token[at..at + 1].to_ascii_uppercase());
    Some(respelled)
}

/// Which letter digit of a token a row respells.
#[derive(Debug, Clone, Copy)]
enum Which {
    First,
    Last,
}

/// Every respelling of `token` the rows drive, labelled.
///
/// `pub(crate)` so another suite asserts the same rule over the enumerations it owns
/// (`account_assignment_creation_status_token_spelling.rs`) rather than a second copy of
/// the spellings.
pub(crate) fn respellings(token: &str) -> Vec<(String, String)> {
    let mut rows = vec![(
        "every hex digit upper-cased".to_string(),
        token.to_ascii_uppercase(),
    )];
    for (label, which) in [
        ("the first letter digit upper-cased", Which::First),
        ("the last letter digit upper-cased", Which::Last),
    ] {
        if let Some(respelled) = with_letter_uppercased(token, which) {
            rows.push((label.to_string(), respelled));
        }
    }
    rows.retain(|(_, respelled)| respelled != token);
    rows
}

/// A router whose account holds an instance with two permission sets, so every
/// enumeration below has more than one row and really mints a cursor.
async fn router_with_two_permission_sets() -> (MultiServiceRouter, String) {
    let (router, instance_arn) = organization_router();
    for name in ["TokenSpellingOne", "TokenSpellingTwo"] {
        let (status, created) = call(
            &router,
            "CreatePermissionSet",
            &json!({"InstanceArn": instance_arn, "Name": name}),
        )
        .await;
        assert_eq!(
            status, 200,
            "CreatePermissionSet {name} expected 200, got {status} {created}"
        );
    }
    (router, instance_arn)
}

/// The cursor an enumeration's first page hands back, with that page.
async fn first_page_cursor(
    router: &MultiServiceRouter,
    instance_arn: &str,
    enumeration: &Enumeration,
) -> String {
    let (status, body) = call(
        router,
        enumeration.operation,
        &(enumeration.request)(instance_arn),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: the first page expected 200, got {status} {body}",
        enumeration.operation
    );
    body["NextToken"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: this enumeration must mint a cursor: {body}",
                enumeration.operation
            )
        })
        .to_string()
}

/// THE RULE: a respelling of an issued cursor is a token this service never handed out, so
/// every paginated read refuses it with the modelled `ValidationException` instead of
/// positioning a page from it.
#[tokio::test]
async fn a_respelled_cursor_is_refused_by_every_enumeration() {
    let (router, instance_arn) = router_with_two_permission_sets().await;

    for enumeration in ENUMERATIONS {
        let issued = first_page_cursor(&router, &instance_arn, enumeration).await;
        // The encoder's output set: lower-case hex digits only, which is what makes any
        // other spelling a value this service never issued.
        assert!(
            !issued.is_empty()
                && issued
                    .chars()
                    .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "case {:?}: a minted cursor is lower-case hex, got {issued:?}",
            enumeration.operation
        );

        let rows = respellings(&issued);
        assert!(
            rows.len() == 3,
            "case {:?}: every respelling row must differ from the minted cursor {issued:?}, \
             got {rows:?}",
            enumeration.operation
        );
        for (label, respelled) in rows {
            let case = format!("{} / {label}", enumeration.operation);
            let mut request = (enumeration.request)(&instance_arn);
            request["NextToken"] = json!(respelled);
            let (status, body) = call(&router, enumeration.operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for {respelled:?}, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException for {respelled:?}, got {body}"
            );
            assert_eq!(
                body["Message"].as_str(),
                Some(state::BAD_CURSOR_MESSAGE),
                "case {case:?}: expected the unissued-cursor refusal, got {body}"
            );
        }
    }
}

/// The other half of the same rule: the cursor as MINTED is served, so the acceptance set
/// is the encoder's output set rather than nothing — the respelled rows above are refused
/// because they are respellings, not because the enumeration stopped paging.
#[tokio::test]
async fn the_cursor_as_minted_still_resumes_its_enumeration() {
    let (router, instance_arn) = router_with_two_permission_sets().await;

    for enumeration in ENUMERATIONS {
        let issued = first_page_cursor(&router, &instance_arn, enumeration).await;
        let mut request = (enumeration.request)(&instance_arn);
        request["NextToken"] = json!(issued);
        let (status, body) = call(&router, enumeration.operation, &request).await;
        assert_eq!(
            status, 200,
            "case {:?}: the minted cursor expected 200, got {status} {body}",
            enumeration.operation
        );
        assert!(
            body[enumeration.items].is_array(),
            "case {:?}: the resumed page reports its items in {:?}, got {body}",
            enumeration.operation,
            enumeration.items
        );
    }
}
