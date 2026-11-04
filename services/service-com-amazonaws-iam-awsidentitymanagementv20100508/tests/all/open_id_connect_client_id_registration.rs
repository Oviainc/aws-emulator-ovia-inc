// SPDX-License-Identifier: CC0-1.0

//! `AddClientIDToOpenIDConnectProvider` and `DeleteOpenIDConnectProvider`, end to end over
//! the awsQuery wire.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour (status, awsQuery error code, XML members) of a provider that was
//! PERSISTED by `CreateOpenIDConnectProvider` and is read back through
//! `GetOpenIDConnectProvider`, never a handler called directly. The awsQuery harness is
//! the one `tests/all/user_lifecycle.rs` owns and the other suites in this crate share.
//!
//! The rule under test is that the call registers the NAMED client ID on the named
//! provider AFTER the client IDs already registered, leaves the rest of that provider
//! (its URL, thumbprints, creation instant and tags) and every other provider alone, and
//! — because the operation "is idempotent; it does not fail or return an error if you add
//! an existing client ID to the provider" — answers a client ID the provider already
//! holds with a success that changes nothing, so the registered list never holds the same
//! value twice. Past the per-provider client-ID quota a registration that would grow the
//! list is refused with `LimitExceeded`. Each suite is a TABLE whose rows differ along
//! what that rule generalises over (how many client IDs are already registered, whether
//! the value is one of them and at which position, and the form and length of the value —
//! including a value carrying the characters XML gives meaning to, which must come back
//! out of the read as the caller wrote it in a body that still parses), so no single
//! request is the specification.
//!
//! The DELETE half is the second rule: it FORGETS the named provider with everything held
//! on it, refuses an ARN the account holds no provider under with `404` `NoSuchEntity`
//! (against its doc's claim of idempotency — the measured answer to the second delete is
//! that refusal), and updates nothing that references the provider — so a later read of that
//! ARN is a 404, the URL it was registered under is free again, and every other provider is
//! untouched.

use smithy_server_runtime::MultiServiceRouter;

use crate::open_id_connect_providers::{
    absence_message, client_ids, list_members, tagging_absence_message,
};
use crate::role_lifecycle::{borrowed, param, reported_tags};
use crate::user_lifecycle::{account_id, call, element, error_code, router};

/// A 40-character hex thumbprint — the length `thumbprintType` declares (len 40..40) —
/// carried by every fixture provider so a registration that disturbed the rest of the
/// record would be visible.
const THUMBPRINT: &str = "b1c2d3e4f5061728394a5b6c7d8e9f0123456789";

/// How many client IDs one IAM OIDC provider may carry.
/// `CreateOpenIDConnectProvider$ClientIDList`'s documentation: "You cannot register more
/// than 100 client IDs with a single IAM OIDC provider."
const MAX_CLIENT_IDS: usize = 100;

/// `clientIDType` is modelled len 1..255: the longest value a request may carry.
fn longest_client_id() -> String {
    "z".repeat(255)
}

/// One registration: the client IDs the provider is created with, the value the request
/// names, and the client IDs the provider must hold afterwards, in order.
struct AdditionCase {
    /// Names the row in every assertion message, and seeds its provider's URL so each row
    /// gets a provider of its own ("you cannot register the same provider multiple times
    /// in a single Amazon Web Services account").
    name: &'static str,
    /// `ClientIDList` the create carries, in request order.
    registered: Vec<String>,
    /// The `ClientID` the registration names.
    add: String,
    /// The client IDs `GetOpenIDConnectProvider` must report after the registration, in
    /// the order they were registered.
    expected: Vec<String>,
}

/// The table: a provider holding none, one, three and five client IDs; a value that is new
/// and a value the provider already holds at the only, first, middle and last position;
/// values whose form varies (plain, dotted, containing a space, upper-case, an STS-style
/// audience URL, the shortest and the longest `clientIDType` allows); and a value that is
/// a prefix of, or a case variant of, a registered one — a different client ID, so it is
/// registered beside it.
fn addition_cases() -> Vec<AdditionCase> {
    let owned =
        |values: &[&str]| -> Vec<String> { values.iter().map(|value| (*value).to_string()).collect() };
    vec![
        AdditionCase {
            name: "first-client-id",
            registered: Vec::new(),
            add: "sts.amazonaws.com".to_string(),
            expected: owned(&["sts.amazonaws.com"]),
        },
        AdditionCase {
            name: "appended-after-one",
            registered: owned(&["audience-a"]),
            add: "audience-b".to_string(),
            expected: owned(&["audience-a", "audience-b"]),
        },
        AdditionCase {
            name: "appended-after-three",
            registered: owned(&["audience-a", "audience-b", "audience-c"]),
            add: "audience-d".to_string(),
            expected: owned(&["audience-a", "audience-b", "audience-c", "audience-d"]),
        },
        AdditionCase {
            name: "appended-after-five",
            registered: owned(&["a-1", "a-2", "a-3", "a-4", "a-5"]),
            add: "a-6".to_string(),
            expected: owned(&["a-1", "a-2", "a-3", "a-4", "a-5", "a-6"]),
        },
        AdditionCase {
            // "It does not fail or return an error if you add an existing client ID to the
            // provider": the only registered value stays single.
            name: "already-the-only-entry",
            registered: owned(&["sts.amazonaws.com"]),
            add: "sts.amazonaws.com".to_string(),
            expected: owned(&["sts.amazonaws.com"]),
        },
        AdditionCase {
            name: "already-registered-first",
            registered: owned(&["audience-a", "audience-b", "audience-c"]),
            add: "audience-a".to_string(),
            expected: owned(&["audience-a", "audience-b", "audience-c"]),
        },
        AdditionCase {
            name: "already-registered-middle",
            registered: owned(&["audience-a", "audience-b", "audience-c"]),
            add: "audience-b".to_string(),
            expected: owned(&["audience-a", "audience-b", "audience-c"]),
        },
        AdditionCase {
            name: "already-registered-last",
            registered: owned(&["audience-a", "audience-b", "audience-c"]),
            add: "audience-c".to_string(),
            expected: owned(&["audience-a", "audience-b", "audience-c"]),
        },
        AdditionCase {
            name: "dotted-value",
            registered: owned(&["one.client"]),
            add: "two.client".to_string(),
            expected: owned(&["one.client", "two.client"]),
        },
        AdditionCase {
            name: "value-with-a-space",
            registered: owned(&["plain"]),
            add: "another client".to_string(),
            expected: owned(&["plain", "another client"]),
        },
        AdditionCase {
            // A client ID is matched exactly, so a case variant of a registered value is a
            // different client ID and joins the list rather than deduplicating against it.
            name: "case-variant-of-a-registered-value",
            registered: owned(&["mixedcaseaudience"]),
            add: "MixedCaseAudience".to_string(),
            expected: owned(&["mixedcaseaudience", "MixedCaseAudience"]),
        },
        AdditionCase {
            name: "audience-url",
            registered: owned(&["sts.amazonaws.com", "my-app"]),
            add: "https://signin.aws.amazon.com/oauth".to_string(),
            expected: owned(&[
                "sts.amazonaws.com",
                "my-app",
                "https://signin.aws.amazon.com/oauth",
            ]),
        },
        AdditionCase {
            // The shortest value `clientIDType` allows (len 1..255).
            name: "shortest-value",
            registered: owned(&["xx"]),
            add: "x".to_string(),
            expected: owned(&["xx", "x"]),
        },
        AdditionCase {
            // The longest value `clientIDType` allows (len 1..255).
            name: "longest-value",
            registered: owned(&["short"]),
            add: longest_client_id(),
            expected: vec!["short".to_string(), longest_client_id()],
        },
        AdditionCase {
            // The longest value, already registered: the dedup is on the whole value, so
            // the bound of the type is no special case.
            name: "longest-value-already-registered",
            registered: vec![longest_client_id(), "short".to_string()],
            add: longest_client_id(),
            expected: vec![longest_client_id(), "short".to_string()],
        },
        AdditionCase {
            // A PREFIX of a registered value is a different client ID, so it is registered
            // beside it: the request names one client ID, not a family of them.
            name: "prefix-of-a-registered-value",
            registered: owned(&["audience-a", "audience-b"]),
            add: "audience".to_string(),
            expected: owned(&["audience-a", "audience-b", "audience"]),
        },
        AdditionCase {
            // `clientIDType` bounds the value by length alone, so a value carrying the
            // characters XML gives meaning to is a legal client ID: it must come back out
            // of `GetOpenIDConnectProvider` as the caller wrote it, in a body that still
            // parses, rather than breaking the response it is reported in.
            name: "xml-metacharacters",
            registered: owned(&["plain"]),
            add: "a&b<c>d\"e'f".to_string(),
            expected: owned(&["plain", "a&b<c>d\"e'f"]),
        },
    ]
}

/// Create the row's provider and answer the ARN the create minted.
async fn seed(router: &MultiServiceRouter, name: &str, registered: &[String]) -> String {
    let mut params = vec![
        param("Url", &format!("https://{name}.example.org")),
        param("ThumbprintList.member.1", THUMBPRINT),
        param("Tags.member.1.Key", "env"),
        param("Tags.member.1.Value", "dev"),
    ];
    for (index, client_id) in registered.iter().enumerate() {
        params.push(param(
            &format!("ClientIDList.member.{}", index + 1),
            client_id,
        ));
    }
    let (status, created) = call(router, "CreateOpenIDConnectProvider", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {name:?}: the fixture provider must be created, got {status}: {created}"
    );
    element(&created, "OpenIDConnectProviderArn")
}

/// Read one provider back through `GetOpenIDConnectProvider`.
async fn read(router: &MultiServiceRouter, arn: &str) -> (u16, String) {
    let params = vec![param("OpenIDConnectProviderArn", arn)];
    call(router, "GetOpenIDConnectProvider", &borrowed(&params)).await
}

/// One registration request.
async fn add(router: &MultiServiceRouter, arn: &str, client_id: &str) -> (u16, String) {
    let params = vec![
        param("OpenIDConnectProviderArn", arn),
        param("ClientID", client_id),
    ];
    call(
        router,
        "AddClientIDToOpenIDConnectProvider",
        &borrowed(&params),
    )
    .await
}

/// One delete request.
async fn delete(router: &MultiServiceRouter, arn: &str) -> (u16, String) {
    let params = vec![param("OpenIDConnectProviderArn", arn)];
    call(router, "DeleteOpenIDConnectProvider", &borrowed(&params)).await
}

// ---------------------------------------------------------------------------
// The registration, read back
// ---------------------------------------------------------------------------

/// Every row's registration succeeds and the read reports exactly the client IDs the
/// provider now holds, in registration order — including the rows whose value the provider
/// already held, which the idempotent contract answers with an unchanged list.
#[tokio::test]
async fn adding_a_client_id_registers_it_after_the_client_ids_already_registered() {
    let router = router();

    for case in addition_cases() {
        let arn = seed(&router, case.name, &case.registered).await;
        let (_, before) = read(&router, &arn).await;
        let create_date = element(&before, "CreateDate");

        let (status, body) = add(&router, &arn, &case.add).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} with body {body}",
            case.name
        );

        let (status, after) = read(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: the provider must read back, got {status}: {after}",
            case.name
        );
        let reported = list_members(&after, "ClientIDList");
        assert_eq!(
            reported, case.expected,
            "case {:?}: expected the client IDs {:?} in registration order, got {reported:?}",
            case.name, case.expected
        );
        // The rest of the record is untouched: a registration adds a client ID, it does
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

/// The operation "is idempotent": the SAME registration repeated answers 200 every time
/// and the provider holds the value exactly once, whatever it already held.
#[tokio::test]
async fn repeating_a_registration_is_a_success_that_changes_nothing() {
    let router = router();

    for case in addition_cases() {
        let arn = seed(&router, case.name, &case.registered).await;

        for attempt in 1..=3 {
            let (status, body) = add(&router, &arn, &case.add).await;
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
            let held = reported
                .iter()
                .filter(|value| *value == &case.add)
                .count();
            assert_eq!(
                held, 1,
                "case {:?}: attempt {attempt}: the client ID must be registered exactly \
                 once, got {held} copies in {reported:?}",
                case.name
            );
        }
    }
}

/// A registration names ONE provider: another provider keeps the client IDs it held, so
/// the write is scoped to the ARN the request carries.
#[tokio::test]
async fn adding_a_client_id_touches_only_the_named_provider() {
    let router = router();

    // Rows: a provider holding nothing, one client ID, and three — so the untouched
    // providers are checked on more than one shape of list.
    let shared = "sts.amazonaws.com";
    let providers: Vec<(&'static str, Vec<&'static str>)> = vec![
        ("add-scoped-a", Vec::new()),
        ("add-scoped-b", vec!["other-1"]),
        ("add-scoped-c", vec!["other-2", "other-3", "other-4"]),
    ];

    let mut arns = Vec::new();
    for (name, client_ids) in &providers {
        let registered: Vec<String> = client_ids.iter().map(|id| (*id).to_string()).collect();
        arns.push(seed(&router, name, &registered).await);
    }

    // Register the shared client ID on the FIRST provider only.
    let (status, body) = add(&router, &arns[0], shared).await;
    assert_eq!(
        status, 200,
        "the scoped registration must succeed, got {status} with body {body}"
    );

    let (_, first) = read(&router, &arns[0]).await;
    assert_eq!(
        list_members(&first, "ClientIDList"),
        vec![shared.to_string()],
        "case {:?}: the named provider must hold the client ID, got {first}",
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
// The quota
// ---------------------------------------------------------------------------

/// The published bound: a provider may hold 100 client IDs, so the registration that fills
/// it succeeds and the one past it is refused with `LimitExceeded` (HTTP 409) without
/// changing the list. A value the full provider ALREADY holds grows nothing, so it is
/// still the idempotent success the contract promises.
#[tokio::test]
async fn registering_past_the_client_id_quota_is_refused() {
    let router = router();

    let registered: Vec<String> = (1..MAX_CLIENT_IDS)
        .map(|index| format!("audience-{index}"))
        .collect();
    let arn = seed(&router, "quota", &registered).await;

    // The registration that reaches the bound exactly.
    let at_bound = format!("audience-{MAX_CLIENT_IDS}");
    let (status, body) = add(&router, &arn, &at_bound).await;
    assert_eq!(
        status, 200,
        "case {at_bound:?}: the registration filling the quota must succeed, got {status} \
         with body {body}"
    );
    let (_, after) = read(&router, &arn).await;
    let reported = list_members(&after, "ClientIDList");
    assert_eq!(
        reported.len(),
        MAX_CLIENT_IDS,
        "case {at_bound:?}: expected {MAX_CLIENT_IDS} registered client IDs, got {}",
        reported.len()
    );

    // One past the bound: refused, and the list is exactly what it was.
    let past_bound = "audience-one-too-many";
    let (status, body) = add(&router, &arn, past_bound).await;
    assert_eq!(
        status, 409,
        "case {past_bound:?}: expected HTTP 409, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "LimitExceeded",
        "case {past_bound:?}: expected LimitExceeded, got {body}"
    );
    // "The error message describes the limit exceeded" (`LimitExceededException`): the
    // limit a registration can exceed is the provider's client-ID quota, named with the
    // bound itself, the wording this service reports for a quota refusal.
    let message = element(&body, "Message");
    assert_eq!(
        message,
        format!("Cannot exceed quota for ClientIdsPerOpenIdConnectProvider: {MAX_CLIENT_IDS}"),
        "case {past_bound:?}: the message must describe the quota exceeded, got {message:?}"
    );
    let (_, unchanged) = read(&router, &arn).await;
    assert_eq!(
        list_members(&unchanged, "ClientIDList"),
        reported,
        "case {past_bound:?}: a refused registration must leave the list alone, got \
         {unchanged}"
    );

    // A client ID the FULL provider already holds registers nothing, so the quota does not
    // turn the idempotent success into a refusal.
    for already in [registered[0].clone(), at_bound.clone()] {
        let (status, body) = add(&router, &arn, &already).await;
        assert_eq!(
            status, 200,
            "case {already:?}: an already-registered client ID must still succeed at the \
             quota, got {status} with body {body}"
        );
        let (_, same) = read(&router, &arn).await;
        assert_eq!(
            list_members(&same, "ClientIDList"),
            reported,
            "case {already:?}: the list must be unchanged, got {same}"
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
async fn adding_to_a_provider_the_account_does_not_hold_is_refused() {
    let router = router();
    let registered = vec!["audience-a".to_string(), "audience-b".to_string()];
    let held_arn = seed(&router, "add-held", &registered).await;

    // Rows: a well-formed ARN for a URL nothing holds, a near-miss of the held one, an ARN
    // of another IAM resource type, and one in another account.
    let unknown = [
        format!("arn:aws:iam::{}:oidc-provider/never.example.org", account_id()),
        format!(
            "arn:aws:iam::{}:oidc-provider/add-held.example.org/x",
            account_id()
        ),
        format!("arn:aws:iam::{}:role/add-held.example.org", account_id()),
        "arn:aws:iam::111122223333:oidc-provider/add-held.example.org".to_string(),
        // An ARN-shaped value carrying the characters XML gives meaning to: the refusal
        // describes the resource the request named, so the value the caller wrote comes
        // back in a body that still parses.
        format!(
            "arn:aws:iam::{}:oidc-provider/a&b<c>d\"e'f.example.org",
            account_id()
        ),
    ];
    for arn in &unknown {
        let (status, body) = add(&router, arn, "audience-z").await;
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
        // resource a registration names is the provider ARN the request carried.
        let message = element(&body, "Message");
        assert_eq!(
            message,
            absence_message(arn),
            "case {arn:?}: the message must describe the ARN the request named, got \
             {message:?}"
        );
        // The same missing provider, looked up through the reporting half, is described the
        // same way: the description belongs to the resource, not to the call.
        let (status, read_body) = read(&router, arn).await;
        assert_eq!(
            status, 404,
            "case {arn:?}: expected HTTP 404 from GetOpenIDConnectProvider, got {status} \
             with body {read_body}"
        );
        let read_message = element(&read_body, "Message");
        assert_eq!(
            read_message,
            absence_message(arn),
            "case {arn:?}: the read must describe the same missing provider, got \
             {read_message:?}"
        );
    }

    // Rows: values long enough to pass `arnType`'s 20-character floor but that are not
    // ARNs — a URL, a bare name, and a resource portion with the `arn:` prefix missing.
    let not_an_arn = [
        "https://add-held.example.org",
        "add-held.example.org-provider",
        "aws:iam::000000000000:oidc-provider/add-held.example.org",
    ];
    for value in not_an_arn {
        let (status, body) = add(&router, value, "audience-z").await;
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
        registered,
        "a refused registration must leave the held client IDs alone, got {after}"
    );
}

// ---------------------------------------------------------------------------
// The delete
// ---------------------------------------------------------------------------

/// A delete forgets the named provider and nothing else: its ARN stops resolving, every
/// other provider the account holds is untouched, repeating the delete is refused as the
/// missing entity the provider now is, and the URL it was registered under is free again —
/// while a value that is not an ARN is refused `InvalidInput`.
#[tokio::test]
async fn deleting_a_provider_forgets_only_that_provider() {
    let router = router();
    // Rows: providers whose records differ in what is held on them, so the delete is
    // exercised on more than one shape of record.
    let providers: Vec<(&str, Vec<String>)> = vec![
        ("deleted-none", Vec::new()),
        ("deleted-one", client_ids(1)),
        ("kept-three", client_ids(3)),
    ];
    let mut arns = Vec::new();
    for (name, registered) in &providers {
        arns.push(seed(&router, name, registered).await);
    }

    for index in 0..2 {
        let (name, registered) = &providers[index];
        let arn = &arns[index];
        for attempt in 1..=3 {
            let (status, body) = delete(&router, arn).await;
            // The FIRST delete forgets the provider; every later one addresses an ARN the
            // account now holds nothing under, which is the missing entity every other call
            // on that ARN reports.
            let expected = if attempt == 1 { 200 } else { 404 };
            assert_eq!(
                status, expected,
                "case {name:?}: attempt {attempt}: expected HTTP {expected}, got {status} with \
                 body {body}"
            );
            if attempt > 1 {
                assert_eq!(
                    error_code(&body),
                    "NoSuchEntity",
                    "case {name:?}: attempt {attempt}: expected NoSuchEntity, got {body}"
                );
            }
            let (status, after) = read(&router, arn).await;
            assert_eq!(
                status, 404,
                "case {name:?}: attempt {attempt}: a deleted provider must stop resolving, got \
                 {status} with {after}"
            );
            assert_eq!(
                error_code(&after),
                "NoSuchEntity",
                "case {name:?}: attempt {attempt}: expected NoSuchEntity, got {after}"
            );
        }
        // Nothing may be registered with a provider that is gone.
        let (status, body) = add(&router, arn, "audience-z").await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "NoSuchEntity"),
            "case {name:?}: an add to a deleted provider expected (404, NoSuchEntity), got \
             ({status}, {body})"
        );
        // The URL is free again, and a provider registered for it again carries only what
        // the new create asked for.
        let recreated = seed(&router, name, registered).await;
        assert_eq!(
            &recreated, arn,
            "case {name:?}: the ARN is derived from the URL, so re-registering it answers the \
             same ARN, got {recreated}"
        );
        let (status, after) = read(&router, arn).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the re-registered provider must read back, got {status}: {after}"
        );
        assert_eq!(
            &list_members(&after, "ClientIDList"),
            registered,
            "case {name:?}: the re-registered provider holds what its create asked for, got \
             {after}"
        );
    }

    // The provider no delete named is exactly as it was created.
    let (status, kept) = read(&router, &arns[2]).await;
    assert_eq!(
        status, 200,
        "the provider no delete named must still read back, got {status}: {kept}"
    );
    assert_eq!(
        &list_members(&kept, "ClientIDList"),
        &providers[2].1,
        "another provider's client IDs must be untouched, got {kept}"
    );

    for value in [
        "https://kept-three.example.net",
        "kept-three.example.net-provider",
        "aws:iam::000000000000:oidc-provider/kept-three.example.net",
    ] {
        let (status, body) = delete(&router, value).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidInput"),
            "case {value:?}: expected (400, InvalidInput), got ({status}, {body})"
        );
    }

    // A well-formed ARN naming no provider is refused as the missing entity it is, in the
    // sentence this family words that absence with — not answered as the deletion it already
    // is, which is what this operation's own doc claims.
    for arn in [
        format!("arn:aws:iam::{}:oidc-provider/never.example.net", account_id()),
        "arn:aws:iam::111122223333:oidc-provider/kept-three.example.net".to_string(),
    ] {
        let (status, body) = delete(&router, &arn).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "NoSuchEntity"),
            "case {arn:?}: deleting a provider the account does not hold expected (404, \
             NoSuchEntity), got ({status}, {body})"
        );
        assert_eq!(
            element(&body, "Message"),
            tagging_absence_message(&arn),
            "case {arn:?}: expected the family's absence wording, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Against the removal half
// ---------------------------------------------------------------------------

/// A client ID registered here is the one `RemoveClientIDFromOpenIDConnectProvider` takes
/// out again, and re-registering it after a removal puts it back at the END of the list:
/// the two halves share one registered list, and position follows registration order.
#[tokio::test]
async fn a_registered_client_id_round_trips_through_the_removal() {
    let router = router();

    let registered = vec!["audience-a".to_string(), "audience-b".to_string()];
    let arn = seed(&router, "round-trip", &registered).await;

    let (status, body) = add(&router, &arn, "audience-c").await;
    assert_eq!(
        status, 200,
        "the registration must succeed, got {status} with body {body}"
    );

    // Remove the FIRST registered value, then register it again: it comes back last.
    let remove_params = vec![
        param("OpenIDConnectProviderArn", &arn),
        param("ClientID", "audience-a"),
    ];
    let (status, body) = call(
        &router,
        "RemoveClientIDFromOpenIDConnectProvider",
        &borrowed(&remove_params),
    )
    .await;
    assert_eq!(
        status, 200,
        "the removal must succeed, got {status} with body {body}"
    );
    let (_, after_removal) = read(&router, &arn).await;
    let reported = list_members(&after_removal, "ClientIDList");
    assert_eq!(
        reported,
        vec!["audience-b".to_string(), "audience-c".to_string()],
        "the removal must drop only the named client ID, got {reported:?}"
    );

    let (status, body) = add(&router, &arn, "audience-a").await;
    assert_eq!(
        status, 200,
        "the re-registration must succeed, got {status} with body {body}"
    );
    let (_, after) = read(&router, &arn).await;
    let reported = list_members(&after, "ClientIDList");
    assert_eq!(
        reported,
        vec![
            "audience-b".to_string(),
            "audience-c".to_string(),
            "audience-a".to_string()
        ],
        "a re-registered client ID must come back last, got {reported:?}"
    );
}
