// SPDX-License-Identifier: CC0-1.0

//! `RemoveClientIDFromOpenIDConnectProvider`, end to end over the awsQuery wire.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour (status, awsQuery error code, XML members) of a provider that was
//! PERSISTED by `CreateOpenIDConnectProvider` and is read back through
//! `GetOpenIDConnectProvider`, never a handler called directly. The awsQuery harness is
//! the one `tests/all/user_lifecycle.rs` owns and the other suites in this crate share.
//!
//! The rule under test is that the call takes the NAMED client ID out of the named
//! provider's registered list, leaves every other client ID in the order it was
//! registered, leaves the rest of the provider (its URL, thumbprints, creation instant
//! and tags) and every other provider alone, and — because the operation "is idempotent;
//! it does not fail or return an error if you try to remove a client ID that does not
//! exist" — answers a client ID the provider does not hold with a success that changes
//! nothing. Each suite is a TABLE whose rows differ along what that rule generalises over
//! (how many client IDs are registered, which position the removed one sits at, the form
//! and length of the value, and whether it is registered at all), so no single request is
//! the specification.

use smithy_server_runtime::MultiServiceRouter;

use crate::open_id_connect_providers::list_members;
use crate::role_lifecycle::{borrowed, param, reported_tags};
use crate::user_lifecycle::{account_id, call, element, error_code, router};

/// A 40-character hex thumbprint — the length `thumbprintType` declares (len 40..40) —
/// carried by every fixture provider so a removal that disturbed the rest of the record
/// would be visible.
const THUMBPRINT: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678";

/// One removal: the client IDs the provider is created with, the value the request names,
/// and the client IDs the provider must hold afterwards, in order.
struct RemovalCase {
    /// Names the row in every assertion message, and seeds its provider's URL so each row
    /// gets a provider of its own ("you cannot register the same provider multiple times
    /// in a single Amazon Web Services account").
    name: &'static str,
    /// `ClientIDList` the create carries, in request order.
    registered: Vec<String>,
    /// The `ClientID` the removal names.
    remove: String,
    /// The client IDs `GetOpenIDConnectProvider` must report after the removal, in the
    /// order they were registered.
    expected: Vec<String>,
}

/// `clientIDType` is modelled len 1..255, and "there is no defined format for a client ID"
/// (`CreateOpenIDConnectProvider$ClientIDList`): the longest value a request may carry.
fn longest_client_id() -> String {
    "a".repeat(255)
}

/// The table: list sizes from one to five; the removed value at the only, first, middle
/// and last position; values whose form varies (plain, dotted, containing a space,
/// upper-case, an STS-style audience URL, the shortest and the longest `clientIDType`
/// allows); and two rows whose value the provider does not hold at all — one never
/// registered, one a prefix of a registered value.
fn removal_cases() -> Vec<RemovalCase> {
    let owned = |values: &[&str]| -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    };
    vec![
        RemovalCase {
            name: "only-entry",
            registered: owned(&["sts.amazonaws.com"]),
            remove: "sts.amazonaws.com".to_string(),
            expected: Vec::new(),
        },
        RemovalCase {
            name: "first-of-three",
            registered: owned(&["audience-a", "audience-b", "audience-c"]),
            remove: "audience-a".to_string(),
            expected: owned(&["audience-b", "audience-c"]),
        },
        RemovalCase {
            name: "middle-of-three",
            registered: owned(&["audience-a", "audience-b", "audience-c"]),
            remove: "audience-b".to_string(),
            expected: owned(&["audience-a", "audience-c"]),
        },
        RemovalCase {
            name: "last-of-three",
            registered: owned(&["audience-a", "audience-b", "audience-c"]),
            remove: "audience-c".to_string(),
            expected: owned(&["audience-a", "audience-b"]),
        },
        RemovalCase {
            name: "dotted-value",
            registered: owned(&["one.client", "two.client"]),
            remove: "one.client".to_string(),
            expected: owned(&["two.client"]),
        },
        RemovalCase {
            name: "value-with-a-space",
            registered: owned(&["another client", "plain"]),
            remove: "another client".to_string(),
            expected: owned(&["plain"]),
        },
        RemovalCase {
            name: "upper-case-value",
            registered: owned(&["MixedCaseAudience", "lowercase"]),
            remove: "MixedCaseAudience".to_string(),
            expected: owned(&["lowercase"]),
        },
        RemovalCase {
            name: "audience-url",
            registered: owned(&[
                "https://signin.aws.amazon.com/oauth",
                "sts.amazonaws.com",
                "my-app",
            ]),
            remove: "https://signin.aws.amazon.com/oauth".to_string(),
            expected: owned(&["sts.amazonaws.com", "my-app"]),
        },
        RemovalCase {
            // The shortest value `clientIDType` allows (len 1..255).
            name: "shortest-value",
            registered: owned(&["x", "xx"]),
            remove: "x".to_string(),
            expected: owned(&["xx"]),
        },
        RemovalCase {
            // The longest value `clientIDType` allows (len 1..255).
            name: "longest-value",
            registered: vec![longest_client_id(), "short".to_string()],
            remove: longest_client_id(),
            expected: owned(&["short"]),
        },
        RemovalCase {
            name: "five-registered-remove-fourth",
            registered: owned(&["a-1", "a-2", "a-3", "a-4", "a-5"]),
            remove: "a-4".to_string(),
            expected: owned(&["a-1", "a-2", "a-3", "a-5"]),
        },
        RemovalCase {
            // "It does not fail or return an error if you try to remove a client ID that
            // does not exist": the list is unchanged and the call still succeeds.
            name: "never-registered",
            registered: owned(&["audience-a", "audience-b"]),
            remove: "audience-z".to_string(),
            expected: owned(&["audience-a", "audience-b"]),
        },
        RemovalCase {
            // A PREFIX of a registered value is a different value, so it is not associated
            // either — the removal names one client ID, not a family of them.
            name: "prefix-of-a-registered-value",
            registered: owned(&["audience-a", "audience-b"]),
            remove: "audience".to_string(),
            expected: owned(&["audience-a", "audience-b"]),
        },
        RemovalCase {
            name: "none-registered",
            registered: Vec::new(),
            remove: "audience-a".to_string(),
            expected: Vec::new(),
        },
    ]
}

/// Create the row's provider and answer the ARN the create minted.
async fn seed(router: &MultiServiceRouter, case: &RemovalCase) -> String {
    let mut params = vec![
        param("Url", &format!("https://{}.example.org", case.name)),
        param("ThumbprintList.member.1", THUMBPRINT),
        param("Tags.member.1.Key", "env"),
        param("Tags.member.1.Value", "dev"),
    ];
    for (index, client_id) in case.registered.iter().enumerate() {
        params.push(param(
            &format!("ClientIDList.member.{}", index + 1),
            client_id,
        ));
    }
    let (status, created) = call(router, "CreateOpenIDConnectProvider", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {:?}: the fixture provider must be created, got {status}: {created}",
        case.name
    );
    element(&created, "OpenIDConnectProviderArn")
}

/// Read one provider back through `GetOpenIDConnectProvider`.
async fn read(router: &MultiServiceRouter, arn: &str) -> (u16, String) {
    let params = vec![param("OpenIDConnectProviderArn", arn)];
    call(router, "GetOpenIDConnectProvider", &borrowed(&params)).await
}

/// One removal request.
async fn remove(router: &MultiServiceRouter, arn: &str, client_id: &str) -> (u16, String) {
    let params = vec![
        param("OpenIDConnectProviderArn", arn),
        param("ClientID", client_id),
    ];
    call(
        router,
        "RemoveClientIDFromOpenIDConnectProvider",
        &borrowed(&params),
    )
    .await
}

// ---------------------------------------------------------------------------
// The removal, read back
// ---------------------------------------------------------------------------

/// Every row's removal succeeds and the read reports exactly the client IDs that remain,
/// in the order they were registered — including the rows whose value the provider never
/// held, which the idempotent contract answers with an unchanged list.
#[tokio::test]
async fn removing_a_client_id_leaves_the_other_registered_client_ids_in_order() {
    let router = router();

    for case in removal_cases() {
        let arn = seed(&router, &case).await;
        let (_, before) = read(&router, &arn).await;
        let create_date = element(&before, "CreateDate");

        let (status, body) = remove(&router, &arn, &case.remove).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} with body {body}",
            case.name
        );

        let (status, after) = read(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: the provider must still read back, got {status}: {after}",
            case.name
        );
        let reported = list_members(&after, "ClientIDList");
        assert_eq!(
            reported, case.expected,
            "case {:?}: expected the client IDs {:?} in registered order, got {reported:?}",
            case.name, case.expected
        );
        // The rest of the record is untouched: a removal takes a client ID out, it does
        // not re-write the provider.
        let reported_url = element(&after, "Url");
        assert_eq!(
            reported_url,
            format!("{}.example.org", case.name),
            "case {:?}: the URL must be unchanged, got {reported_url:?}",
            case.name
        );
        assert_eq!(
            list_members(&after, "ThumbprintList"),
            vec![THUMBPRINT.to_string()],
            "case {:?}: the thumbprints must be unchanged, got {after}",
            case.name
        );
        assert_eq!(
            reported_tags(&after),
            reported_tags(&before),
            "case {:?}: the tags must be unchanged, got {after}",
            case.name
        );
        assert_eq!(
            element(&after, "CreateDate"),
            create_date,
            "case {:?}: the creation instant must be unchanged, got {after}",
            case.name
        );
    }
}

/// The operation "is idempotent": the SAME removal repeated answers 200 every time and the
/// provider holds the same client IDs after the second and third call as after the first.
#[tokio::test]
async fn repeating_a_removal_is_a_success_that_changes_nothing() {
    let router = router();

    for case in removal_cases() {
        let arn = seed(&router, &case).await;

        for attempt in 1..=3 {
            let (status, body) = remove(&router, &arn, &case.remove).await;
            assert_eq!(
                status, 200,
                "case {:?}: attempt {attempt}: expected HTTP 200, got {status} with body \
                 {body}",
                case.name
            );
            let (_, after) = read(&router, &arn).await;
            let reported = list_members(&after, "ClientIDList");
            assert_eq!(
                reported, case.expected,
                "case {:?}: attempt {attempt}: expected the client IDs {:?}, got {reported:?}",
                case.name, case.expected
            );
        }
    }
}

/// A removal names ONE provider: another provider that registered the same client ID keeps
/// it, so the write is scoped to the ARN the request carries.
#[tokio::test]
async fn removing_a_client_id_touches_only_the_named_provider() {
    let router = router();

    // Rows: the shared client ID at a different position in each provider's list, so the
    // untouched provider is checked on more than one shape of list.
    let shared = "sts.amazonaws.com";
    let providers: Vec<(&'static str, Vec<&'static str>)> = vec![
        ("scoped-a", vec![shared]),
        ("scoped-b", vec![shared, "other-1"]),
        ("scoped-c", vec!["other-2", shared, "other-3"]),
    ];

    let mut arns = Vec::new();
    for (name, client_ids) in &providers {
        let case = RemovalCase {
            name,
            registered: client_ids.iter().map(|id| (*id).to_string()).collect(),
            remove: shared.to_string(),
            expected: Vec::new(),
        };
        arns.push(seed(&router, &case).await);
    }

    // Remove the shared client ID from the FIRST provider only.
    let (status, body) = remove(&router, &arns[0], shared).await;
    assert_eq!(
        status, 200,
        "the scoped removal must succeed, got {status} with body {body}"
    );

    let (_, first) = read(&router, &arns[0]).await;
    assert!(
        !list_members(&first, "ClientIDList").contains(&shared.to_string()),
        "case {:?}: the named provider must no longer hold the client ID, got {first}",
        providers[0].0
    );
    for (index, (name, client_ids)) in providers.iter().enumerate().skip(1) {
        let (_, other) = read(&router, &arns[index]).await;
        let reported = list_members(&other, "ClientIDList");
        assert_eq!(
            reported,
            client_ids
                .iter()
                .map(|id| (*id).to_string())
                .collect::<Vec<String>>(),
            "case {name:?}: another provider's client IDs must be untouched, got {reported:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// An ARN the account holds no provider for is `NoSuchEntity` (HTTP 404) — never an empty
/// success — while a value that is not an ARN at all is `InvalidInput` (HTTP 400); neither
/// disturbs the provider the account does hold.
#[tokio::test]
async fn removing_from_a_provider_the_account_does_not_hold_is_refused() {
    let router = router();
    let case = RemovalCase {
        name: "held",
        registered: vec!["audience-a".to_string(), "audience-b".to_string()],
        remove: "audience-a".to_string(),
        expected: Vec::new(),
    };
    let held_arn = seed(&router, &case).await;

    // Rows: a well-formed ARN for a URL nothing holds, a near-miss of the held one, an ARN
    // of another IAM resource type, and one in another account.
    let unknown = [
        format!("arn:aws:iam::{}:oidc-provider/never.example.org", account_id()),
        format!("arn:aws:iam::{}:oidc-provider/held.example.org/x", account_id()),
        format!("arn:aws:iam::{}:role/held.example.org", account_id()),
        "arn:aws:iam::111122223333:oidc-provider/held.example.org".to_string(),
    ];
    for arn in &unknown {
        let (status, body) = remove(&router, arn, "audience-a").await;
        assert_eq!(
            status, 404,
            "case {arn:?}: expected HTTP 404, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {arn:?}: expected NoSuchEntity, got {body}"
        );
        // "The error message describes the resource" (`NoSuchEntityException`): the
        // resource a removal names is the provider ARN the request carried.
        let message = element(&body, "Message");
        assert_eq!(
            message,
            format!("OpenIDConnect Provider not found for arn {arn}"),
            "case {arn:?}: the message must describe the ARN the request named, got \
             {message:?}"
        );
        // The same missing provider, looked up through the reporting half, is described
        // the same way: the description belongs to the resource, not to the call.
        let (status, read_body) = read(&router, arn).await;
        assert_eq!(
            status, 404,
            "case {arn:?}: expected HTTP 404 from GetOpenIDConnectProvider, got {status} \
             with body {read_body}"
        );
        let read_message = element(&read_body, "Message");
        assert_eq!(
            read_message,
            format!("OpenIDConnect Provider not found for arn {arn}"),
            "case {arn:?}: the read must describe the same missing provider, got \
             {read_message:?}"
        );
    }

    // Rows: values long enough to pass `arnType`'s 20-character floor but that are not
    // ARNs — a URL, a bare name, and a resource portion with the `arn:` prefix missing.
    let not_an_arn = [
        "https://held.example.org",
        "held.example.org-provider",
        "aws:iam::000000000000:oidc-provider/held.example.org",
    ];
    for value in not_an_arn {
        let (status, body) = remove(&router, value, "audience-a").await;
        assert_eq!(
            status, 400,
            "case {value:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {value:?}: expected InvalidInput, got {body}"
        );
    }

    // Every refusal above left the held provider exactly as it was created.
    let (status, after) = read(&router, &held_arn).await;
    assert_eq!(
        status, 200,
        "the held provider must still read back, got {status}: {after}"
    );
    assert_eq!(
        list_members(&after, "ClientIDList"),
        case.registered,
        "a refused removal must leave the held client IDs alone, got {after}"
    );
}
