// SPDX-License-Identifier: CC0-1.0

//! `CreateOpenIDConnectProvider` and its reporting half `GetOpenIDConnectProvider`,
//! end to end over the awsQuery wire.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour (status, awsQuery error code, XML members) of a provider that was
//! PERSISTED, never a handler called directly. The awsQuery harness is the one
//! `tests/all/user_lifecycle.rs` owns and the other suites in this crate share.
//!
//! The rule under test is that a create PERSISTS the provider under an ARN DERIVED from
//! its URL and the read REPORTS what was persisted — the URL in its issuer form (the
//! scheme removed), the client IDs and thumbprints the request carried in the order it
//! carried them, the instant it was created, and its tags sorted by key — while a second
//! create for a URL the account already holds is refused. Each suite is a TABLE whose rows
//! differ along what that rule generalises over (the URL's path depth and case, how many
//! client IDs and thumbprints the request carries, the tag count and the order the tags
//! arrive in), so no single request is the specification.

use std::collections::BTreeMap;

use crate::role_lifecycle::{borrowed, optional, param, reported_tags};
use crate::user_lifecycle::{account_id, call, element, elements, error_code, router};

/// One `CreateOpenIDConnectProvider` request, with the members the row exercises.
struct ProviderCase {
    /// `Url` — "the URL must begin with https:// … path components are allowed but query
    /// parameters are not" (`CreateOpenIDConnectProvider$Url`).
    url: &'static str,
    /// `ClientIDList`, in the order the request carries it.
    client_ids: Vec<String>,
    /// `ThumbprintList`. Each entry is exactly 40 characters, the length
    /// `thumbprintType` declares (`smithy_query(query="shape", shape="thumbprintType")`).
    thumbprints: Vec<String>,
    /// The tags the request carries, in request order.
    tags: Vec<(&'static str, &'static str)>,
}

impl ProviderCase {
    fn params(&self) -> Vec<(String, String)> {
        let mut params = vec![param("Url", self.url)];
        for (index, client_id) in self.client_ids.iter().enumerate() {
            params.push(param(
                &format!("ClientIDList.member.{}", index + 1),
                client_id,
            ));
        }
        for (index, thumbprint) in self.thumbprints.iter().enumerate() {
            params.push(param(
                &format!("ThumbprintList.member.{}", index + 1),
                thumbprint,
            ));
        }
        for (index, (key, value)) in self.tags.iter().enumerate() {
            let position = index + 1;
            params.push(param(&format!("Tags.member.{position}.Key"), key));
            params.push(param(&format!("Tags.member.{position}.Value"), value));
        }
        params
    }

    fn expected_tags(&self) -> BTreeMap<String, String> {
        self.tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }
}

/// A 40-character hex thumbprint — "the hex-encoded SHA-1 hash value of the X.509
/// certificate" is 40 characters, which is also the length `thumbprintType` declares
/// (len 40..40).
fn thumbprint(seed: u32) -> String {
    format!("{seed:040x}")
}

/// `seed` client IDs, distinct so the order the read reports can be checked. "There is no
/// defined format for a client ID" (`CreateOpenIDConnectProvider$ClientIDList`).
fn client_ids(count: usize) -> Vec<String> {
    (0..count).map(|index| format!("audience-{index}")).collect()
}

/// The table every suite here is written against: a bare hostname, a one-segment path, a
/// nested path, a mixed-case host, and a host carrying a hyphen and digits; client-ID and
/// thumbprint counts from none to several; tag counts from none to several, arriving in
/// an order that is not the sorted one.
fn provider_cases() -> Vec<ProviderCase> {
    vec![
        ProviderCase {
            url: "https://server.example.org",
            client_ids: Vec::new(),
            thumbprints: Vec::new(),
            tags: Vec::new(),
        },
        ProviderCase {
            url: "https://example.com/oidc",
            client_ids: client_ids(1),
            thumbprints: vec![thumbprint(1)],
            tags: vec![("env", "dev")],
        },
        ProviderCase {
            url: "https://oidc.eks.us-east-1.amazonaws.com/id/EXAMPLED539D4633E53DE1B716D3041E",
            client_ids: client_ids(3),
            thumbprints: vec![thumbprint(2), thumbprint(3)],
            tags: vec![("team", "platform"), ("env", "prod")],
        },
        ProviderCase {
            url: "https://Accounts.Example.COM/Realms/Main",
            client_ids: vec!["one.client".to_string(), "another client".to_string()],
            thumbprints: vec![thumbprint(4)],
            tags: vec![("zeta", "last"), ("alpha", "first"), ("mu", "middle")],
        },
        ProviderCase {
            url: "https://idp-7.example-corp.net/auth/realms/dev-2",
            client_ids: client_ids(2),
            thumbprints: (5..10).map(thumbprint).collect(),
            tags: Vec::new(),
        },
    ]
}

/// An OIDC provider ARN is `arn:aws:iam::<account>:oidc-provider/<url without its
/// scheme>` — IAM is global, so the region segment is EMPTY, and the provider is named by
/// its URL.
fn expected_arn(url: &str) -> String {
    format!(
        "arn:aws:iam::{}:oidc-provider/{}",
        account_id(),
        issuer_of(url)
    )
}

/// The ISSUER form of a URL: the URL with its `https://` scheme removed, which is what a
/// provider is stored and reported under (and the resource portion of its ARN).
fn issuer_of(url: &str) -> &str {
    url.strip_prefix("https://")
        .expect("every fixture URL carries the scheme the model requires")
}

/// The `<member>` elements of ONE list member of the response.
///
/// `ClientIDList` and `ThumbprintList` both serialize their entries as `<member>`, so a
/// scan of the whole body cannot tell them apart: this narrows to the named list's
/// content first. A list the response omits (or reports empty) has no entries.
fn list_members(body: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    match body.find(&open) {
        None => Vec::new(),
        Some(start) => {
            let rest = &body[start + open.len()..];
            let end = rest
                .find(&close)
                .unwrap_or_else(|| panic!("<{tag}> is never closed in {body}"));
            elements(&rest[..end], "member")
        }
    }
}

// ---------------------------------------------------------------------------
// Create, then read back
// ---------------------------------------------------------------------------

/// Every row creates a provider that `GetOpenIDConnectProvider` then reports identically:
/// the URL the request chose, the client IDs and thumbprints it carried in order, the
/// instant it was created, and its tags sorted by key. The ARN is derived from the URL,
/// so the create's ARN is the handle the read resolves.
#[tokio::test]
async fn a_created_oidc_provider_round_trips_through_get_open_id_connect_provider() {
    let router = router();

    for case in provider_cases() {
        let params = case.params();
        let (status, created) =
            call(&router, "CreateOpenIDConnectProvider", &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} with body {created}",
            case.url
        );

        let arn = element(&created, "OpenIDConnectProviderArn");
        assert_eq!(
            arn,
            expected_arn(case.url),
            "case {:?}: the ARN must be derived from the URL, got {arn:?}",
            case.url
        );
        // `arnType` is modelled len 20..2048 — the constraint on the value this mints.
        assert!(
            arn.len() >= 20 && arn.len() <= 2048,
            "case {:?}: the ARN must be 20..2048 characters, got {} in {arn:?}",
            case.url,
            arn.len()
        );
        // "The returned list of tags is sorted by tag key"
        // (`CreateOpenIDConnectProvider$Tags`).
        let mut sorted_keys: Vec<String> =
            case.tags.iter().map(|(key, _)| (*key).to_string()).collect();
        sorted_keys.sort();
        assert_eq!(
            elements(&created, "Key"),
            sorted_keys,
            "case {:?}: the create must report its tags sorted by key, got {created}",
            case.url
        );
        assert_eq!(
            reported_tags(&created),
            case.expected_tags(),
            "case {:?}: the create must report the tags it was given, got {created}",
            case.url
        );

        // The read reports the SAME record, resolved by the ARN the create answered with.
        let read_params = vec![param("OpenIDConnectProviderArn", &arn)];
        let (status, read) =
            call(&router, "GetOpenIDConnectProvider", &borrowed(&read_params)).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from GetOpenIDConnectProvider, got {status} with \
             body {read}",
            case.url
        );
        let reported_url = element(&read, "Url");
        assert_eq!(
            reported_url,
            issuer_of(case.url),
            "case {:?}: the read must report the stored URL without its scheme, got \
             {reported_url:?}",
            case.url
        );
        let reported_client_ids = list_members(&read, "ClientIDList");
        assert_eq!(
            reported_client_ids, case.client_ids,
            "case {:?}: the read must report the registered client IDs in order, got \
             {reported_client_ids:?}",
            case.url
        );
        let reported_thumbprints = list_members(&read, "ThumbprintList");
        assert_eq!(
            reported_thumbprints, case.thumbprints,
            "case {:?}: the read must report the stored thumbprints in order, got \
             {reported_thumbprints:?}",
            case.url
        );
        assert_eq!(
            elements(&read, "Key"),
            sorted_keys,
            "case {:?}: the read must report the stored tags sorted by key, got {read}",
            case.url
        );
        assert_eq!(
            reported_tags(&read),
            case.expected_tags(),
            "case {:?}: the read must report the stored tags, got {read}",
            case.url
        );
        // `CreateDate` is minted by the create and reported by the read: a provider the
        // account holds carries the instant it was created.
        let create_date = element(&read, "CreateDate");
        assert!(
            !create_date.is_empty(),
            "case {:?}: the read must report a CreateDate, got {read}",
            case.url
        );
    }
}

/// Reading is a READ: two `GetOpenIDConnectProvider` calls report the same record, and
/// the second one is not affected by the first.
#[tokio::test]
async fn reading_an_oidc_provider_changes_nothing_a_later_caller_sees() {
    let router = router();

    for case in provider_cases() {
        let params = case.params();
        let (status, created) =
            call(&router, "CreateOpenIDConnectProvider", &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {:?}: the fixture provider must be created, got {status}: {created}",
            case.url
        );
        let read_params = vec![param(
            "OpenIDConnectProviderArn",
            &element(&created, "OpenIDConnectProviderArn"),
        )];

        let (_, first) = call(&router, "GetOpenIDConnectProvider", &borrowed(&read_params)).await;
        let (_, second) = call(&router, "GetOpenIDConnectProvider", &borrowed(&read_params)).await;
        for member in ["Url", "CreateDate"] {
            assert_eq!(
                element(&first, member),
                element(&second, member),
                "case {:?}: member {member}: two reads must report the same value",
                case.url
            );
        }
        for list in ["ClientIDList", "ThumbprintList"] {
            assert_eq!(
                list_members(&first, list),
                list_members(&second, list),
                "case {:?}: list {list}: two reads must report the same entries",
                case.url
            );
        }
        assert_eq!(
            reported_tags(&first),
            reported_tags(&second),
            "case {:?}: two reads must report the same tags",
            case.url
        );
    }
}

// ---------------------------------------------------------------------------
// A URL is registered at most once
// ---------------------------------------------------------------------------

/// "You cannot register the same provider multiple times in a single Amazon Web Services
/// account": a second create for a held URL is `EntityAlreadyExists` (HTTP 409), and the
/// held provider keeps the record the FIRST create wrote.
#[tokio::test]
async fn a_second_create_for_a_held_url_is_refused_and_changes_nothing() {
    let router = router();

    for case in provider_cases() {
        let params = case.params();
        let (status, created) =
            call(&router, "CreateOpenIDConnectProvider", &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {:?}: the first create must succeed, got {status}: {created}",
            case.url
        );
        let arn = element(&created, "OpenIDConnectProviderArn");

        // The repeat carries DIFFERENT client IDs, thumbprints and tags, so a refusal
        // that silently overwrote the held record would be visible in the read below.
        let repeat = ProviderCase {
            url: case.url,
            client_ids: vec!["a-different-audience".to_string()],
            thumbprints: vec![thumbprint(99)],
            tags: vec![("overwritten", "yes")],
        };
        let repeat_params = repeat.params();
        let (status, body) = call(
            &router,
            "CreateOpenIDConnectProvider",
            &borrowed(&repeat_params),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {:?}: expected HTTP 409, got {status} with body {body}",
            case.url
        );
        assert_eq!(
            error_code(&body),
            "EntityAlreadyExists",
            "case {:?}: expected EntityAlreadyExists, got {body}",
            case.url
        );

        let read_params = vec![param("OpenIDConnectProviderArn", &arn)];
        let (status, read) =
            call(&router, "GetOpenIDConnectProvider", &borrowed(&read_params)).await;
        assert_eq!(
            status, 200,
            "case {:?}: the held provider must still read back, got {status}: {read}",
            case.url
        );
        assert_eq!(
            list_members(&read, "ClientIDList"),
            case.client_ids,
            "case {:?}: a refused create must leave the held client IDs alone, got {read}",
            case.url
        );
        assert_eq!(
            list_members(&read, "ThumbprintList"),
            case.thumbprints,
            "case {:?}: a refused create must leave the held thumbprints alone, got {read}",
            case.url
        );
        assert_eq!(
            reported_tags(&read),
            case.expected_tags(),
            "case {:?}: a refused create must leave the held tags alone, got {read}",
            case.url
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A `Url` that does not begin with `https://`, or that carries query parameters, or that
/// names no host, is `InvalidInput` (HTTP 400) — and nothing is persisted for it.
#[tokio::test]
async fn a_url_outside_the_documented_form_is_refused_and_creates_nothing() {
    let router = router();

    // Rows: the other schemes a caller reaches for, a scheme-less host, a query parameter
    // on a bare host and on a path, and a URL naming no host at all.
    let cases = [
        "http://server.example.org",
        "https:/server.example.org",
        "server.example.org",
        "oidc://server.example.org",
        "https://server.example.org?audience=sts",
        "https://server.example.org/oidc?audience=sts",
        "https://",
        "https:///realms/main",
    ];
    for url in cases {
        let params = vec![param("Url", url)];
        let (status, body) = call(&router, "CreateOpenIDConnectProvider", &borrowed(&params)).await;
        assert_eq!(
            status, 400,
            "case {url:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {url:?}: expected InvalidInput, got {body}"
        );
        assert!(
            optional(&body, "OpenIDConnectProviderArn").is_none(),
            "case {url:?}: a refused create mints no ARN, got {body}"
        );
    }

    // Nothing above was registered: a URL that IS well formed still creates the provider,
    // so the refusals are about the value and not about the account.
    let params = vec![param("Url", "https://server.example.org")];
    let (status, body) = call(&router, "CreateOpenIDConnectProvider", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "a well-formed URL must still be accepted, got {status} with body {body}"
    );
}

/// The published per-provider bounds, taken on the bound itself and one past it: "you
/// cannot register more than 100 client IDs with a single IAM OIDC provider" and "IAM lets
/// you have up to five thumbprints for an OIDC provider".
#[tokio::test]
async fn a_request_past_a_published_per_provider_bound_is_refused() {
    let router = router();

    // Rows: one entry, the bound exactly, and one past it — for each of the two lists.
    let cases: Vec<(&str, usize, usize, bool)> = vec![
        ("client-ids-1", 1, 0, true),
        ("client-ids-100", 100, 0, true),
        ("client-ids-101", 101, 0, false),
        ("thumbprints-1", 0, 1, true),
        ("thumbprints-5", 0, 5, true),
        ("thumbprints-6", 0, 6, false),
    ];
    for (name, client_id_count, thumbprint_count, accepted) in cases {
        let case = ProviderCase {
            url: "https://bounds.example.org",
            client_ids: client_ids(client_id_count),
            thumbprints: (0..thumbprint_count as u32).map(thumbprint).collect(),
            tags: Vec::new(),
        };
        // Each row needs its own URL, because a URL is registered at most once.
        let mut params = case.params();
        params[0] = param("Url", &format!("https://{name}.example.org"));
        let (status, body) = call(&router, "CreateOpenIDConnectProvider", &borrowed(&params)).await;

        if accepted {
            assert_eq!(
                status, 200,
                "case {name:?}: a request on the bound must be accepted, got {status}: {body}"
            );
            let arn = element(&body, "OpenIDConnectProviderArn");
            let read_params = vec![param("OpenIDConnectProviderArn", &arn)];
            let (status, read) =
                call(&router, "GetOpenIDConnectProvider", &borrowed(&read_params)).await;
            assert_eq!(
                status, 200,
                "case {name:?}: the created provider must read back, got {status}: {read}"
            );
            assert_eq!(
                list_members(&read, "ClientIDList").len(),
                client_id_count,
                "case {name:?}: the read must report every registered client ID, got {read}"
            );
            assert_eq!(
                list_members(&read, "ThumbprintList").len(),
                thumbprint_count,
                "case {name:?}: the read must report every stored thumbprint, got {read}"
            );
        } else {
            assert_eq!(
                status, 409,
                "case {name:?}: a request past the bound must be refused, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "LimitExceeded",
                "case {name:?}: expected LimitExceeded, got {body}"
            );
            // "The entire request fails and the resource is not created."
            let read_params = vec![param(
                "OpenIDConnectProviderArn",
                &format!(
                    "arn:aws:iam::{}:oidc-provider/{name}.example.org",
                    account_id()
                ),
            )];
            let (status, read) =
                call(&router, "GetOpenIDConnectProvider", &borrowed(&read_params)).await;
            assert_eq!(
                status, 404,
                "case {name:?}: a refused create must persist nothing, got {status}: {read}"
            );
        }
    }
}

/// An account holds a BOUNDED number of OIDC providers (AWS's "IAM and STS quotas":
/// "OIDC identity providers in an account: 100"), so a create past the quota is refused
/// with `LimitExceeded` (HTTP 409) rather than growing the account without end.
#[tokio::test]
async fn a_create_past_the_account_quota_is_refused() {
    let router = router();
    // Fill the account to the quota exactly, each provider under its own URL.
    for index in 0..100 {
        let url = format!("https://quota-{index}.example.org");
        let params = vec![param("Url", &url)];
        let (status, body) = call(&router, "CreateOpenIDConnectProvider", &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {url:?}: a create up to the quota must be accepted, got {status}: {body}"
        );
    }

    // Rows: two further URLs, so the refusal is about the account and not about one URL.
    for url in [
        "https://quota-past.example.org",
        "https://quota-past.example.org/realms/main",
    ] {
        let params = vec![param("Url", url)];
        let (status, body) = call(&router, "CreateOpenIDConnectProvider", &borrowed(&params)).await;
        assert_eq!(
            status, 409,
            "case {url:?}: expected HTTP 409, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "LimitExceeded",
            "case {url:?}: expected LimitExceeded, got {body}"
        );

        let read_params = vec![param("OpenIDConnectProviderArn", &expected_arn(url))];
        let (status, read) =
            call(&router, "GetOpenIDConnectProvider", &borrowed(&read_params)).await;
        assert_eq!(
            status, 404,
            "case {url:?}: a refused create must persist nothing, got {status}: {read}"
        );
    }
}

/// A tag key in AWS's reserved `aws:` namespace is `InvalidInput` (HTTP 400), and — "if
/// any one of the tags is invalid … the entire request fails and the resource is not
/// created" — nothing is persisted.
#[tokio::test]
async fn a_reserved_tag_key_is_refused_and_creates_nothing() {
    let router = router();

    // Rows: the reserved key alone, and beside a valid one, in either order.
    let cases: Vec<(&str, Vec<(&'static str, &'static str)>)> = vec![
        ("reserved-only", vec![("aws:cloudformation:stack", "s")]),
        (
            "reserved-after-valid",
            vec![("env", "dev"), ("aws:created-by", "me")],
        ),
        (
            "reserved-before-valid",
            vec![("aws:created-by", "me"), ("env", "dev")],
        ),
    ];
    for (name, tags) in cases {
        let case = ProviderCase {
            url: "https://refused.example.org",
            client_ids: client_ids(1),
            thumbprints: vec![thumbprint(1)],
            tags,
        };
        let params = case.params();
        let (status, body) = call(&router, "CreateOpenIDConnectProvider", &borrowed(&params)).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {name:?}: expected InvalidInput, got {body}"
        );

        let read_params = vec![param(
            "OpenIDConnectProviderArn",
            &expected_arn(case.url),
        )];
        let (status, read) =
            call(&router, "GetOpenIDConnectProvider", &borrowed(&read_params)).await;
        assert_eq!(
            status, 404,
            "case {name:?}: a refused create must persist nothing, got {status}: {read}"
        );
    }
}

/// A tag key repeated inside one request settles on its LAST value — one tag, not two —
/// the rule every tag call in this service follows.
#[tokio::test]
async fn a_repeated_tag_key_settles_on_its_last_value() {
    let router = router();
    let case = ProviderCase {
        url: "https://repeat-tags.example.org",
        client_ids: Vec::new(),
        thumbprints: Vec::new(),
        tags: vec![("env", "dev"), ("team", "platform"), ("env", "prod")],
    };
    let params = case.params();
    let (status, created) = call(&router, "CreateOpenIDConnectProvider", &borrowed(&params)).await;
    assert_eq!(status, 200, "the create must succeed, got {status}: {created}");

    let expected: BTreeMap<String, String> = [("env", "prod"), ("team", "platform")]
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    let read_params = vec![param(
        "OpenIDConnectProviderArn",
        &element(&created, "OpenIDConnectProviderArn"),
    )];
    let (status, read) = call(&router, "GetOpenIDConnectProvider", &borrowed(&read_params)).await;
    assert_eq!(status, 200, "the read must succeed, got {status}: {read}");
    assert_eq!(
        reported_tags(&read),
        expected,
        "a repeated key must settle on its last value, got {read}"
    );
    assert_eq!(
        elements(&read, "Key"),
        vec!["env".to_string(), "team".to_string()],
        "the read must report one tag per key, sorted by key, got {read}"
    );
}

/// An ARN the account holds no provider for is `NoSuchEntity` (HTTP 404) — never an empty
/// success — while a value that is not an ARN at all is `InvalidInput` (HTTP 400).
#[tokio::test]
async fn reading_what_was_never_created_is_refused() {
    let router = router();
    let seed = vec![param("Url", "https://held.example.org")];
    let (status, body) = call(&router, "CreateOpenIDConnectProvider", &borrowed(&seed)).await;
    assert_eq!(status, 200, "the fixture provider must be created, got {body}");

    // Rows: a well-formed ARN for a URL nothing holds, a near-miss of the held one, an
    // ARN of another IAM resource type, and one in another account.
    let unknown = [
        format!("arn:aws:iam::{}:oidc-provider/never.example.org", account_id()),
        format!("arn:aws:iam::{}:oidc-provider/held.example.org/x", account_id()),
        format!("arn:aws:iam::{}:role/held.example.org", account_id()),
        "arn:aws:iam::111122223333:oidc-provider/held.example.org".to_string(),
    ];
    for arn in unknown {
        let params = vec![param("OpenIDConnectProviderArn", &arn)];
        let (status, body) = call(&router, "GetOpenIDConnectProvider", &borrowed(&params)).await;
        assert_eq!(
            status, 404,
            "case {arn:?}: expected HTTP 404, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {arn:?}: expected NoSuchEntity, got {body}"
        );
        assert!(
            optional(&body, "Url").is_none(),
            "case {arn:?}: a refused read reports no provider, got {body}"
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
        let params = vec![param("OpenIDConnectProviderArn", value)];
        let (status, body) = call(&router, "GetOpenIDConnectProvider", &borrowed(&params)).await;
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
}
