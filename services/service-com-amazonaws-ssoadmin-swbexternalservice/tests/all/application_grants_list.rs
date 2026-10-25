// SPDX-License-Identifier: CC0-1.0

//! `ListApplicationGrants` end to end over the `awsJson1_1` wire.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! example:
//!
//! 1. the enumeration reports EXACTLY the grants its application carries — one item per
//!    stored `GrantType`, carrying that type and the grant the last put of it stored,
//!    verbatim — in one stable order, for every subset of the grant types the model
//!    admits and whatever order the puts arrived in; an application that carries no grant
//!    is an empty list, not a refusal;
//! 2. the enumeration TRACKS the store: a replacing put changes the item of that type, a
//!    delete removes exactly that item and leaves the rest, and a grant belongs to one
//!    application, so no listing shows another application's grants;
//! 3. it is a REPORT: repeating it answers identically and changes nothing a later caller
//!    sees (the single-grant read still answers from the same stored grants);
//! 4. an `ApplicationArn` this account holds no application for is refused
//!    `ResourceNotFoundException` — never an empty list;
//! 5. the `NextToken` is this service's own cursor: one it did not issue for THIS
//!    enumeration is refused `ValidationException`, and resuming from a cursor it did
//!    issue yields exactly the stored grants that sort after the key it carries.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints,
//! the `awsJson1_1` envelope and the `X-Amz-Target` routing are exercised rather than
//! bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns and the grant
//! table is the one `application_grants.rs` owns.

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::application_grants::{
    create_application, delete_grant, get_grant, grant_cases, one_grant_per_type, put_grant,
    GrantCase, GRANT_TYPES,
};
use crate::ssoadmin_permission_sets::{assert_refused, call, error_code, router, seeded_instance_arn};

/// Call `ListApplicationGrants` once.
async fn list_grants(
    router: &MultiServiceRouter,
    application_arn: &str,
    next_token: Option<&str>,
) -> (u16, Value) {
    let mut request = json!({"ApplicationArn": application_arn});
    if let Some(token) = next_token {
        request["NextToken"] = json!(token);
    }
    call(router, "ListApplicationGrants", &request).await
}

/// The `(GrantType, Grant)` pairs one answer carries, in the order it listed them.
fn listed_grants(case: &str, body: &Value) -> Vec<(String, Value)> {
    let items = body["Grants"].as_array().unwrap_or_else(|| {
        panic!("case {case:?}: ListApplicationGrantsResponse$Grants is required, got {body}")
    });
    items
        .iter()
        .map(|item| {
            (
                item["GrantType"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: GrantItem$GrantType is required, got {item}")
                    })
                    .to_string(),
                item["Grant"].clone(),
            )
        })
        .collect()
}

/// Every page of the enumeration, walked the way a client walks it.
async fn list_every_page(
    router: &MultiServiceRouter,
    application_arn: &str,
    case: &str,
) -> Vec<(String, Value)> {
    let mut seen: Vec<(String, Value)> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0;
    loop {
        let (status, body) = list_grants(router, application_arn, next_token.as_deref()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListApplicationGrants must answer 200, got {status} {body}"
        );
        seen.extend(listed_grants(case, &body));
        pages += 1;
        assert!(
            pages <= GRANT_TYPES.len() + 1,
            "case {case:?}: the enumeration did not terminate after {pages} pages"
        );
        match body["NextToken"].as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => break,
        }
    }
    seen
}

/// The pairs a listing of these rows must carry, in `GrantType` order.
fn expected_pairs(rows: &[&GrantCase]) -> Vec<(String, Value)> {
    let mut want: Vec<(String, Value)> = rows
        .iter()
        .map(|case| (case.grant_type.to_string(), case.grant.clone()))
        .collect();
    want.sort_by(|left, right| left.0.cmp(&right.0));
    want
}

/// RULES 1 and 3 — the enumeration reports exactly the stored grants, for every SUBSET of
/// the grant types and whatever order the puts arrived in, and repeating it changes
/// nothing.
#[tokio::test]
async fn list_reports_exactly_the_grants_the_application_carries() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let all = one_grant_per_type();

    // The rows span the whole class the rule generalises over: every subset of the four
    // `GrantType` values (so every count from none to all, and every combination of
    // types), each seeded in a ROTATED put order so the answer's order is the service's
    // and not the order the caller happened to write.
    for mask in 0..(1u32 << all.len()) {
        let selected: Vec<&GrantCase> = all
            .iter()
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .map(|(_, case)| case)
            .collect();
        let label = format!(
            "grants {:?}",
            selected
                .iter()
                .map(|case| case.grant_type)
                .collect::<Vec<&str>>()
        );
        let application_arn = create_application(&router, &instance_arn, "grant-list").await;

        // An application nobody put a grant on lists NOTHING, and that is a 200 with an
        // empty list rather than a refusal.
        let empty = list_every_page(&router, &application_arn, &label).await;
        assert!(
            empty.is_empty(),
            "case {label:?}: an application with no grant must list none, got {empty:?}"
        );

        let mut order: Vec<&GrantCase> = selected.clone();
        let rotation = mask as usize % order.len().max(1);
        order.rotate_right(rotation);
        for case in &order {
            let (status, body) =
                put_grant(&router, &application_arn, case.grant_type, &case.grant).await;
            assert_eq!(
                status, 200,
                "case {label:?}: the seed put of {} must answer 200, got {status} {body}",
                case.grant_type
            );
        }

        let listed = list_every_page(&router, &application_arn, &label).await;
        assert_eq!(
            listed,
            expected_pairs(&selected),
            "case {label:?}: the list must report exactly the stored grants, got {listed:?}"
        );

        // Every listed item is the grant the single-grant read reports for that type, and
        // every type the list omits is a type the application really carries no grant for.
        for (grant_type, grant) in &listed {
            let (status, read) = get_grant(&router, &application_arn, grant_type).await;
            assert_eq!(
                status, 200,
                "case {label:?}: the get of the listed {grant_type} must answer 200, got \
                 {status} {read}"
            );
            assert_eq!(
                read,
                json!({"Grant": grant}),
                "case {label:?}: the list and the get of {grant_type} must agree, got {read}"
            );
        }
        for (grant_type, _) in GRANT_TYPES {
            if listed.iter().any(|(listed_type, _)| listed_type == grant_type) {
                continue;
            }
            let (status, body) = get_grant(&router, &application_arn, grant_type).await;
            assert_refused(
                &label,
                "a get of a grant type the list did not carry",
                "ResourceNotFoundException",
                status,
                &body,
            );
        }

        // RULE 3: the list is a report, so a second and a third call answer identically.
        let again = list_every_page(&router, &application_arn, &label).await;
        assert_eq!(
            again, listed,
            "case {label:?}: repeating the list must answer identically, got {again:?}"
        );
        let third = list_every_page(&router, &application_arn, &label).await;
        assert_eq!(
            third, listed,
            "case {label:?}: a list must change nothing a later caller sees, got {third:?}"
        );
    }
}

/// RULE 2 — the enumeration tracks every put and delete of its OWN application.
#[tokio::test]
async fn the_list_tracks_every_put_and_delete_of_its_own_application() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let subject = create_application(&router, &instance_arn, "grant-list-subject").await;
    let neighbour = create_application(&router, &instance_arn, "grant-list-neighbour").await;

    // The neighbour's own grant, distinct in its body, so a leak between the two shows up
    // as the wrong grant and not only as the wrong count.
    let neighbour_grant = json!({"AuthorizationCode": {"RedirectUris": ["https://neighbour.example.com/cb"]}});
    let (status, body) = put_grant(
        &router,
        &neighbour,
        "authorization_code",
        &neighbour_grant,
    )
    .await;
    assert_eq!(
        status, 200,
        "the neighbour's seed put must answer 200, got {status} {body}"
    );
    let expected_neighbour: Vec<(String, Value)> =
        vec![("authorization_code".to_string(), neighbour_grant.clone())];

    let all = one_grant_per_type();
    for case in &all {
        let (status, body) = put_grant(&router, &subject, case.grant_type, &case.grant).await;
        assert_eq!(
            status, 200,
            "case {:?}: the seed put must answer 200, got {status} {body}",
            case.grant_type
        );
    }
    let mut remaining: Vec<&GrantCase> = all.iter().collect();
    let listed = list_every_page(&router, &subject, "every grant type stored").await;
    assert_eq!(
        listed,
        expected_pairs(&remaining),
        "case \"every grant type stored\": the list must carry every stored grant, got {listed:?}"
    );

    // A REPLACING put of one type changes that item and leaves every other one: the rows
    // span the `authorization_code` bodies of the grant table, which differ in list size
    // and in entries.
    let replacements: Vec<GrantCase> = grant_cases()
        .into_iter()
        .filter(|case| case.grant_type == "authorization_code")
        .collect();
    for replacement in &replacements {
        let label = format!("replacing authorization_code with {:?}", replacement.label);
        let (status, body) = put_grant(
            &router,
            &subject,
            replacement.grant_type,
            &replacement.grant,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: the replacing put must answer 200, got {status} {body}"
        );
        let listed = list_every_page(&router, &subject, &label).await;
        let mut want: Vec<&GrantCase> = remaining
            .iter()
            .copied()
            .filter(|case| case.grant_type != "authorization_code")
            .collect();
        want.push(replacement);
        assert_eq!(
            listed,
            expected_pairs(&want),
            "case {label:?}: the list must carry the REPLACING grant, got {listed:?}"
        );
        let neighbour_listed = list_every_page(&router, &neighbour, &label).await;
        assert_eq!(
            neighbour_listed, expected_neighbour,
            "case {label:?}: another application's list must be untouched, got \
             {neighbour_listed:?}"
        );
    }
    // The last replacement is what `authorization_code` now carries.
    let last_replacement = replacements
        .last()
        .expect("the grant table carries an authorization_code row")
        .clone();

    // A delete removes exactly the item its type names, in several deletion ORDERS' worth
    // of positions: from the middle out.
    for index in [1usize, 2, 0, 3] {
        let doomed = all[index].grant_type;
        let label = format!("after deleting {doomed}");
        let (status, body) = delete_grant(&router, &subject, doomed).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the delete must answer 200, got {status} {body}"
        );
        remaining.retain(|case| case.grant_type != doomed);
        let mut want: Vec<&GrantCase> = remaining
            .iter()
            .copied()
            .filter(|case| case.grant_type != "authorization_code")
            .collect();
        if remaining
            .iter()
            .any(|case| case.grant_type == "authorization_code")
        {
            want.push(&last_replacement);
        }
        let listed = list_every_page(&router, &subject, &label).await;
        assert_eq!(
            listed,
            expected_pairs(&want),
            "case {label:?}: the list must drop exactly the deleted grant, got {listed:?}"
        );
        let neighbour_listed = list_every_page(&router, &neighbour, &label).await;
        assert_eq!(
            neighbour_listed, expected_neighbour,
            "case {label:?}: another application's list must be untouched, got \
             {neighbour_listed:?}"
        );
    }

    // Every grant deleted: the list is empty and the application itself survives.
    let listed = list_every_page(&router, &subject, "after deleting every grant").await;
    assert!(
        listed.is_empty(),
        "case \"after deleting every grant\": the list must be empty, got {listed:?}"
    );
    let (status, body) = call(
        &router,
        "DescribeApplication",
        &json!({"ApplicationArn": subject}),
    )
    .await;
    assert_eq!(
        status, 200,
        "the application must survive its grants, got {status} {body}"
    );
}

/// RULE 4 — an `ApplicationArn` no create minted is refused `ResourceNotFoundException`.
#[tokio::test]
async fn list_of_an_application_arn_no_create_minted_is_refused_not_found() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let instance_id = instance_arn
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();
    // One application with one stored grant exists, so the refusal is about the ARN asked
    // for and not about an empty store.
    let stored = create_application(&router, &instance_arn, "stored-app").await;
    let grant = json!({"AuthorizationCode": {"RedirectUris": ["https://stored.example.com/cb"]}});
    let (status, body) = put_grant(&router, &stored, "authorization_code", &grant).await;
    assert_eq!(status, 200, "the seed put must answer 200, got {status} {body}");

    let account = stored.split(':').nth(4).unwrap_or_default().to_string();
    let suffix = stored.rsplit("/apl-").next().unwrap_or_default().to_string();
    let application_arn = |partition: &str, account: &str, instance: &str, suffix: &str| {
        format!("arn:{partition}:sso::{account}:application/{instance}/apl-{suffix}")
    };

    // The rows span the parts of the ARN that identify the application: the `apl-`
    // suffix, the instance segment, the account segment and the partition.
    let cases: Vec<(&str, String)> = vec![
        (
            "a pattern-valid apl- suffix that was never minted",
            application_arn("aws", &account, &instance_id, "0000000000000000"),
        ),
        (
            "the stored suffix under another instance",
            application_arn("aws", &account, "ssoins-0000000000000000", &suffix),
        ),
        (
            "the stored application under another account",
            application_arn("aws", "210987654321", &instance_id, &suffix),
        ),
        (
            "the stored application in another partition",
            application_arn("aws-us-gov", &account, &instance_id, &suffix),
        ),
    ];
    for (case, arn) in cases {
        assert_ne!(
            arn, stored,
            "case {case:?}: the row must name a DIFFERENT ARN than the stored one"
        );
        let (status, body) = list_grants(&router, &arn, None).await;
        assert_refused(
            case,
            "a list under an unminted ARN",
            "ResourceNotFoundException",
            status,
            &body,
        );
    }

    // None of those refusals touched the stored application's grants.
    let listed = list_every_page(&router, &stored, "after the refusals").await;
    assert_eq!(
        listed,
        vec![("authorization_code".to_string(), grant)],
        "a refused list must change nothing, got {listed:?}"
    );
}

/// RULE 5 — the `NextToken` is this service's own cursor: a token it did not issue for
/// this enumeration is refused, and its own cursor resumes after the key it carries.
#[tokio::test]
async fn list_refuses_a_next_token_it_did_not_issue_and_resumes_from_its_own() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let subject = create_application(&router, &instance_arn, "grant-list-cursor").await;
    let other = create_application(&router, &instance_arn, "grant-list-other").await;

    let all = one_grant_per_type();
    for application_arn in [&subject, &other] {
        for case in &all {
            let (status, body) =
                put_grant(&router, application_arn, case.grant_type, &case.grant).await;
            assert_eq!(
                status, 200,
                "case {:?}: the seed put must answer 200, got {status} {body}",
                case.grant_type
            );
        }
    }

    // `Token` is modelled `^[-a-zA-Z0-9+=/_]*$`, so every row below reaches the handler.
    // The rows differ along what a token this enumeration never issued can look like: not
    // hex at all, hex of a plain word, hex of a stored key with no cursor tag, hex of a
    // cursor of another kind, and a cursor of the SAME enumeration over another
    // application.
    let hex_of_a_word = hex::encode("aa");
    let hex_of_a_key = hex::encode("authorization_code");
    let hex_of_another_kind = hex::encode("ssoadmin:v1:bananas:authorization_code");
    let foreign_cursor = state::encode_page_token(
        &state::cursor_application_grants(&other),
        "authorization_code",
    );
    for token in [
        "notatoken",
        "zz",
        "0",
        hex_of_a_word.as_str(),
        hex_of_a_key.as_str(),
        hex_of_another_kind.as_str(),
        foreign_cursor.as_str(),
    ] {
        let (status, body) = list_grants(&router, &subject, Some(token)).await;
        assert_eq!(
            status, 400,
            "case {token:?}: a token this enumeration did not issue must be refused 400, got \
             {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {token:?}: expected ValidationException, got {body}"
        );
    }

    // Its OWN cursor resumes the enumeration: for every stored grant type, a cursor at
    // that key answers exactly the stored grants whose type sorts after it — the
    // invariant that makes a walk of the pages see each grant once.
    let scope = state::cursor_application_grants(&subject);
    let every = expected_pairs(&all.iter().collect::<Vec<&GrantCase>>());
    for (key, _) in &every {
        let token = state::encode_page_token(&scope, key);
        let (status, body) = list_grants(&router, &subject, Some(&token)).await;
        assert_eq!(
            status, 200,
            "case {key:?}: resuming from this enumeration's own cursor must answer 200, got \
             {status} {body}"
        );
        let listed = listed_grants(key, &body);
        let want: Vec<(String, Value)> = every
            .iter()
            .filter(|(grant_type, _)| grant_type > key)
            .cloned()
            .collect();
        assert_eq!(
            listed, want,
            "case {key:?}: a resumed page must carry exactly the grants after the cursor, got \
             {listed:?}"
        );
        // The neighbouring application is unaffected by the resumed read.
        let neighbour_listed = list_every_page(&router, &other, key).await;
        assert_eq!(
            neighbour_listed, every,
            "case {key:?}: another application's grants must be untouched, got \
             {neighbour_listed:?}"
        );
    }
}
