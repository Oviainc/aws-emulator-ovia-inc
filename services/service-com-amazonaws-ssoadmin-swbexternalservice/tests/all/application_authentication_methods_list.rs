// SPDX-License-Identifier: CC0-1.0

//! `ListApplicationAuthenticationMethods` end to end over the `awsJson1_1` wire.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! example:
//!
//! 1. the enumeration reports EXACTLY the authentication methods its application carries
//!    — one item per stored `AuthenticationMethodType`, carrying that type and the method
//!    the last put of it stored, verbatim — in one stable order, for every subset of the
//!    method types the model admits and for every method body the union admits; an
//!    application that carries no method is an empty list, not a refusal;
//! 2. the enumeration TRACKS the store: a replacing put changes the item of that type, a
//!    delete removes exactly that item, a later put of the same type stores it afresh, and
//!    a method belongs to one application, so no listing shows another application's
//!    methods;
//! 3. it is a REPORT: repeating it answers identically and changes nothing a later caller
//!    sees (the single-method read still answers from the same stored methods);
//! 4. an `ApplicationArn` this account holds no application for is refused
//!    `ResourceNotFoundException` — never an empty list;
//! 5. the `NextToken` is this service's own cursor: one it did not issue for THIS
//!    enumeration is refused `ValidationException`, and resuming from a cursor it did
//!    issue yields exactly the stored methods that sort after the key it carries.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints,
//! the `awsJson1_1` envelope and the `X-Amz-Target` routing are exercised rather than
//! bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns and the method
//! table is the one `application_authentication_methods.rs` owns.

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::application_authentication_methods::{
    actor_policies, create_application, delete_method, expected_iam_body, get_method,
    put_iam_method, IAM,
};
use crate::ssoadmin_permission_sets::{assert_refused, call, error_code, router, seeded_instance_arn};

/// Every value the model's `AuthenticationMethodType` enum admits, which is the whole
/// class of keys a stored method can carry.
///
/// SOURCE: `smithy_query(query="shape",shape="AuthenticationMethodType")` — the enum's
/// `values` are exactly `["IAM"]`.
const AUTHENTICATION_METHOD_TYPES: &[&str] = &[IAM];

/// Call `ListApplicationAuthenticationMethods` once.
async fn list_methods(
    router: &MultiServiceRouter,
    application_arn: &str,
    next_token: Option<&str>,
) -> (u16, Value) {
    let mut request = json!({"ApplicationArn": application_arn});
    if let Some(token) = next_token {
        request["NextToken"] = json!(token);
    }
    call(router, "ListApplicationAuthenticationMethods", &request).await
}

/// The `(AuthenticationMethodType, AuthenticationMethod)` pairs one answer carries, in
/// the order it listed them.
fn listed_methods(case: &str, body: &Value) -> Vec<(String, Value)> {
    let items = body["AuthenticationMethods"].as_array().unwrap_or_else(|| {
        panic!(
            "case {case:?}: the answer must carry an AuthenticationMethods list, got {body}"
        )
    });
    items
        .iter()
        .map(|item| {
            (
                item["AuthenticationMethodType"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!(
                            "case {case:?}: an item must name the type of the stored method, \
                             got {item}"
                        )
                    })
                    .to_string(),
                item["AuthenticationMethod"].clone(),
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
        let (status, body) = list_methods(router, application_arn, next_token.as_deref()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListApplicationAuthenticationMethods must answer 200, got \
             {status} {body}"
        );
        seen.extend(listed_methods(case, &body));
        pages += 1;
        assert!(
            pages <= AUTHENTICATION_METHOD_TYPES.len() + 1,
            "case {case:?}: the enumeration did not terminate after {pages} pages"
        );
        match body["NextToken"].as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => break,
        }
    }
    seen
}

/// The pair a listing must carry for an IAM method storing `actor_policy`, as the item's
/// `AuthenticationMethod` union body.
fn expected_pair(actor_policy: &Value) -> (String, Value) {
    (
        IAM.to_string(),
        json!({"Iam": {"ActorPolicy": actor_policy}}),
    )
}

/// RULES 1 and 3 — the enumeration reports exactly the stored methods, for every subset of
/// the method types and every method body, and repeating it changes nothing.
#[tokio::test]
async fn list_reports_exactly_the_methods_the_application_carries() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;

    // The rows span the class the rule generalises over: every subset of the modelled
    // `AuthenticationMethodType` values (so the empty store as well as every stored type)
    // crossed with every `ActorPolicy` document of the method table, which differ in
    // member count, nesting depth, value kinds and overall size.
    for (case, policy) in actor_policies() {
        for mask in 0..(1u32 << AUTHENTICATION_METHOD_TYPES.len()) {
            let selected: Vec<&str> = AUTHENTICATION_METHOD_TYPES
                .iter()
                .enumerate()
                .filter(|(index, _)| mask & (1 << index) != 0)
                .map(|(_, method_type)| *method_type)
                .collect();
            let label = format!("{case:?} with types {selected:?}");
            let application_arn =
                create_application(&router, &instance_arn, "method-list").await;

            // An application nobody put a method on lists NOTHING, and that is a 200 with
            // an empty list rather than a refusal.
            let empty = list_every_page(&router, &application_arn, &label).await;
            assert!(
                empty.is_empty(),
                "case {label:?}: an application with no method must list none, got {empty:?}"
            );

            for method_type in &selected {
                assert_eq!(
                    *method_type, IAM,
                    "case {label:?}: the only modelled method type is {IAM}, got \
                     {method_type:?}"
                );
                let (status, body) = put_iam_method(&router, &application_arn, &policy).await;
                assert_eq!(
                    status, 200,
                    "case {label:?}: the seed put of {method_type} must answer 200, got \
                     {status} {body}"
                );
            }

            let want: Vec<(String, Value)> = selected
                .iter()
                .map(|_| expected_pair(&policy))
                .collect();
            let listed = list_every_page(&router, &application_arn, &label).await;
            assert_eq!(
                listed, want,
                "case {label:?}: the list must report exactly the stored methods, got \
                 {listed:?}"
            );

            // Every listed item is the method the single-method read reports for that
            // type, and every type the list omits is one the application carries no
            // method for.
            for (method_type, _) in &listed {
                let (status, read) = get_method(&router, &application_arn, method_type).await;
                assert_eq!(
                    status, 200,
                    "case {label:?}: the get of the listed {method_type} must answer 200, got \
                     {status} {read}"
                );
                assert_eq!(
                    read,
                    expected_iam_body(&policy),
                    "case {label:?}: the list and the get of {method_type} must agree, got \
                     {read}"
                );
            }
            for method_type in AUTHENTICATION_METHOD_TYPES {
                if listed.iter().any(|(listed_type, _)| listed_type == method_type) {
                    continue;
                }
                let (status, body) = get_method(&router, &application_arn, method_type).await;
                assert_refused(
                    &label,
                    "a get of a method type the list did not carry",
                    "ResourceNotFoundException",
                    status,
                    &body,
                );
            }

            // RULE 3: the list is a report, so a second and a third call answer
            // identically.
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
}

/// RULE 2 — the enumeration tracks every put and delete of its OWN application.
#[tokio::test]
async fn the_list_tracks_every_put_and_delete_of_its_own_application() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let subject = create_application(&router, &instance_arn, "method-list-subject").await;
    let neighbour = create_application(&router, &instance_arn, "method-list-neighbour").await;

    // The neighbour's own method, distinct in its document, so a leak between the two
    // shows up as the wrong method and not only as the wrong count.
    let neighbour_policy = json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": "Allow", "Action": "sts:AssumeRole", "Resource": "*"}],
    });
    let (status, body) = put_iam_method(&router, &neighbour, &neighbour_policy).await;
    assert_eq!(
        status, 200,
        "the neighbour's seed put must answer 200, got {status} {body}"
    );
    let expected_neighbour = vec![expected_pair(&neighbour_policy)];

    // A REPLACING put changes the item of that type: the rows are the method table's
    // documents, which differ in member count, nesting and size, applied in sequence to
    // the SAME type.
    for (case, policy) in actor_policies() {
        let label = format!("replacing the IAM method with {case:?}");
        let (status, body) = put_iam_method(&router, &subject, &policy).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the replacing put must answer 200, got {status} {body}"
        );
        let listed = list_every_page(&router, &subject, &label).await;
        assert_eq!(
            listed,
            vec![expected_pair(&policy)],
            "case {label:?}: the list must carry the REPLACING method, got {listed:?}"
        );
        let neighbour_listed = list_every_page(&router, &neighbour, &label).await;
        assert_eq!(
            neighbour_listed, expected_neighbour,
            "case {label:?}: another application's list must be untouched, got \
             {neighbour_listed:?}"
        );
    }

    // A delete removes exactly the item its type names, and a later put stores it afresh.
    for method_type in AUTHENTICATION_METHOD_TYPES {
        let label = format!("after deleting {method_type}");
        let (status, body) = delete_method(&router, &subject, method_type).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the delete must answer 200, got {status} {body}"
        );
        let listed = list_every_page(&router, &subject, &label).await;
        assert!(
            listed.is_empty(),
            "case {label:?}: the list must drop exactly the deleted method, got {listed:?}"
        );
        let neighbour_listed = list_every_page(&router, &neighbour, &label).await;
        assert_eq!(
            neighbour_listed, expected_neighbour,
            "case {label:?}: another application's list must be untouched, got \
             {neighbour_listed:?}"
        );

        let restored = json!({"Statement": [{"Effect": "Deny", "Action": "*", "Resource": "*"}]});
        let (status, body) = put_iam_method(&router, &subject, &restored).await;
        assert_eq!(
            status, 200,
            "case {label:?}: a later put of the deleted type must answer 200, got \
             {status} {body}"
        );
        let listed = list_every_page(&router, &subject, &label).await;
        assert_eq!(
            listed,
            vec![expected_pair(&restored)],
            "case {label:?}: the list must carry the re-put method, got {listed:?}"
        );
    }

    // The application itself survives its methods.
    let (status, body) = call(
        &router,
        "DescribeApplication",
        &json!({"ApplicationArn": subject}),
    )
    .await;
    assert_eq!(
        status, 200,
        "the application must survive its methods, got {status} {body}"
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
    // One application with one stored method exists, so the refusal is about the ARN asked
    // for and not about an empty store.
    let stored = create_application(&router, &instance_arn, "stored-method-app").await;
    let policy = json!({"Statement": [{"Effect": "Allow", "Action": "s3:*", "Resource": "*"}]});
    let (status, body) = put_iam_method(&router, &stored, &policy).await;
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
        let (status, body) = list_methods(&router, &arn, None).await;
        assert_refused(
            case,
            "a list under an unminted ARN",
            "ResourceNotFoundException",
            status,
            &body,
        );
    }

    // None of those refusals touched the stored application's methods.
    let listed = list_every_page(&router, &stored, "after the refusals").await;
    assert_eq!(
        listed,
        vec![expected_pair(&policy)],
        "a refused list must change nothing, got {listed:?}"
    );
}

/// RULE 5 — the `NextToken` is this service's own cursor: a token it did not issue for
/// this enumeration is refused, and its own cursor resumes after the key it carries.
#[tokio::test]
async fn list_refuses_a_next_token_it_did_not_issue_and_resumes_from_its_own() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let subject = create_application(&router, &instance_arn, "method-list-cursor").await;
    let other = create_application(&router, &instance_arn, "method-list-other").await;

    let policy = json!({"Statement": [{"Effect": "Allow", "Action": "sso:*", "Resource": "*"}]});
    for application_arn in [&subject, &other] {
        let (status, body) = put_iam_method(&router, application_arn, &policy).await;
        assert_eq!(
            status, 200,
            "the seed put must answer 200, got {status} {body}"
        );
    }
    let every = vec![expected_pair(&policy)];

    // `Token` is modelled `^[-a-zA-Z0-9+=/_]*$`, so every row below reaches the handler.
    // The rows differ along what a token this enumeration never issued can look like: not
    // hex at all, hex of a plain word, hex of a stored key with no cursor tag, hex of a
    // cursor of another kind, a cursor of ANOTHER enumeration over the same application,
    // and a cursor of the SAME enumeration over another application.
    let hex_of_a_word = hex::encode("aa");
    let hex_of_a_key = hex::encode(IAM);
    let hex_of_another_kind = hex::encode("ssoadmin:v1:bananas:IAM");
    let other_enumeration =
        state::encode_page_token(&state::cursor_application_grants(&subject), IAM);
    let foreign_cursor = state::encode_page_token(
        &state::cursor_application_authentication_methods(&other),
        IAM,
    );
    for token in [
        "notatoken",
        "zz",
        "0",
        hex_of_a_word.as_str(),
        hex_of_a_key.as_str(),
        hex_of_another_kind.as_str(),
        other_enumeration.as_str(),
        foreign_cursor.as_str(),
    ] {
        let (status, body) = list_methods(&router, &subject, Some(token)).await;
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

    // Its OWN cursor resumes the enumeration AFTER the key it carries: the rows are keys
    // that sort before, at and after every stored method type, so each page is exactly the
    // stored methods whose type sorts strictly after the cursor's key.
    let scope = state::cursor_application_authentication_methods(&subject);
    let mut keys: Vec<String> = vec!["AAA".to_string(), "ZZZ".to_string()];
    keys.extend(AUTHENTICATION_METHOD_TYPES.iter().map(|key| key.to_string()));
    for key in &keys {
        let token = state::encode_page_token(&scope, key);
        let (status, body) = list_methods(&router, &subject, Some(&token)).await;
        assert_eq!(
            status, 200,
            "case {key:?}: resuming from this enumeration's own cursor must answer 200, got \
             {status} {body}"
        );
        let listed = listed_methods(key, &body);
        let want: Vec<(String, Value)> = every
            .iter()
            .filter(|(method_type, _)| method_type > key)
            .cloned()
            .collect();
        assert_eq!(
            listed, want,
            "case {key:?}: a resumed page must carry exactly the methods after the cursor, \
             got {listed:?}"
        );
        // The neighbouring application is unaffected by the resumed read.
        let neighbour_listed = list_every_page(&router, &other, key).await;
        assert_eq!(
            neighbour_listed, every,
            "case {key:?}: another application's methods must be untouched, got \
             {neighbour_listed:?}"
        );
    }
}
