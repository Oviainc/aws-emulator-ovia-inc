// SPDX-License-Identifier: CC0-1.0

//! Service-specific credentials, end to end over the awsQuery wire.
//!
//! `CreateServiceSpecificCredential` generates the credential a service issues to
//! an IAM user, and `ListServiceSpecificCredentials` reads the persisted record
//! back. The awsQuery harness (router, call, element) is the one
//! `tests/all/user_lifecycle.rs` owns, so every suite in this crate builds and reads
//! a request the same way.
//!
//! Each test is a TABLE whose rows differ along what the rule generalises over: the
//! service (and so the FORM of credential it issues), the user name's case and
//! alphabet, the credential age in days, how many credentials a user already holds,
//! the page size, and which filter the listing carries.

use aws_smithy_types::DateTime;
use aws_smithy_types::date_time::Format;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;
use smithy_server_runtime::MultiServiceRouter;

// `router_with_engine` is the shared harness's router whose engine the test also
// holds: this suite rewinds an instant the clock has already made due.
use crate::user_lifecycle::{
    account_id, call, element, elements, error_code, router, router_with_engine,
};

/// The FORM of credential a service issues, which decides which members of
/// `ServiceSpecificCredential` carry it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    /// `ServiceUserName` plus `ServicePassword`.
    UserNameAndPassword,
    /// `ServiceCredentialAlias` plus `ServiceCredentialSecret`.
    LongTermApiKey,
}

/// One create: the IAM user, the `ServiceName` the request carries, the canonical
/// service name the answer must report, and the form that service issues.
#[derive(Debug)]
struct Case {
    user: &'static str,
    requested_service: &'static str,
    canonical_service: &'static str,
    form: Form,
}

/// Rows spanning the whole catalog of services that support service-specific
/// credentials (`state::SERVICES_WITH_SPECIFIC_CREDENTIALS`, whose source is
/// `CreateServiceSpecificCredential`'s documentation), both credential forms, a
/// principal spelled in a case the catalog does not use, and user names drawn from
/// across `userNameType`'s `^[\w+=,.@-]+$` alphabet.
const CASES: &[Case] = &[
    Case {
        user: "keyspaces_user",
        requested_service: "cassandra.amazonaws.com",
        canonical_service: "cassandra.amazonaws.com",
        form: Form::UserNameAndPassword,
    },
    Case {
        user: "Git.User",
        requested_service: "codecommit.amazonaws.com",
        canonical_service: "codecommit.amazonaws.com",
        form: Form::UserNameAndPassword,
    },
    Case {
        user: "MixedCase-Caller",
        requested_service: "CASSANDRA.amazonaws.com",
        canonical_service: "cassandra.amazonaws.com",
        form: Form::UserNameAndPassword,
    },
    Case {
        user: "bedrock+dev",
        requested_service: "bedrock.amazonaws.com",
        canonical_service: "bedrock.amazonaws.com",
        form: Form::LongTermApiKey,
    },
    Case {
        user: "logs=1",
        requested_service: "logs.amazonaws.com",
        canonical_service: "logs.amazonaws.com",
        form: Form::LongTermApiKey,
    },
];

/// Create one IAM user, failing the test if the account would not take it.
async fn create_user(router: &MultiServiceRouter, user: &str) {
    let (status, body) = call(router, "CreateUser", &[("UserName", user)]).await;
    assert_eq!(
        status, 200,
        "case {user:?}: expected the user to be created, got {status}: {body}"
    );
}

/// One `CreateServiceSpecificCredential` call.
async fn create_credential(
    router: &MultiServiceRouter,
    user: &str,
    service: &str,
) -> (u16, String) {
    call(
        router,
        "CreateServiceSpecificCredential",
        &[("UserName", user), ("ServiceName", service)],
    )
    .await
}

/// One `ListServiceSpecificCredentials` call, asserted to succeed.
async fn list_credentials(router: &MultiServiceRouter, params: &[(&str, &str)]) -> String {
    let (status, body) = call(router, "ListServiceSpecificCredentials", params).await;
    assert_eq!(
        status, 200,
        "expected the listing to succeed for {params:?}, got {status}: {body}"
    );
    body
}

/// The instant an ISO-8601 element carries, in whole seconds since the epoch.
fn instant(body: &str, tag: &str) -> i64 {
    let text = element(body, tag);
    DateTime::from_str(&text, Format::DateTime)
        .unwrap_or_else(|error| panic!("expected an ISO-8601 <{tag}>, got {text:?}: {error}"))
        .secs()
}

/// RULE: a create mints exactly one credential of the form the named service
/// issues, persists every non-secret member of it, and a later read returns the
/// STORED record rather than values recomputed from the reading request; the secret
/// is reported only by the create.
#[tokio::test]
async fn create_issues_the_form_the_service_supports_and_the_list_reads_it_back() {
    let router = router();
    let mut minted_ids: Vec<String> = Vec::new();

    for case in CASES {
        create_user(&router, case.user).await;
        let (status, created) = create_credential(&router, case.user, case.requested_service).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the credential to be created, got {status}: {created}"
        );

        assert_eq!(
            element(&created, "UserName"),
            case.user,
            "case {case:?}: expected the user name the account holds, got {:?}",
            element(&created, "UserName")
        );
        assert_eq!(
            element(&created, "ServiceName"),
            case.canonical_service,
            "case {case:?}: expected the service's own name, got {:?}",
            element(&created, "ServiceName")
        );
        assert_eq!(
            element(&created, "Status"),
            "Active",
            "case {case:?}: expected a new credential to be Active, got {:?}",
            element(&created, "Status")
        );

        // `serviceSpecificCredentialId` is len 20..128, pattern ^[\w]+$.
        let id = element(&created, "ServiceSpecificCredentialId");
        assert!(
            (20..=128).contains(&id.chars().count())
                && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "case {case:?}: expected an id matching ^[\\w]{{20,128}}$, got {id:?}"
        );
        assert!(
            !minted_ids.contains(&id),
            "case {case:?}: expected an id unique across credentials, got the repeat {id:?}"
        );

        // `ServiceSpecificCredential$CreateDate` is a required timestamp.
        let created_at = instant(&created, "CreateDate");

        // Which members carry the credential is the service's form, and the other
        // form's members are absent rather than empty.
        match case.form {
            Form::UserNameAndPassword => {
                assert_eq!(
                    element(&created, "ServiceUserName"),
                    format!("{}-at-{}", case.user, account_id()),
                    "case {case:?}: expected the IAM user name joined to the account id, got {:?}",
                    element(&created, "ServiceUserName")
                );
                assert!(
                    !element(&created, "ServicePassword").is_empty(),
                    "case {case:?}: expected a generated ServicePassword, got an empty one"
                );
                for absent in ["ServiceCredentialAlias", "ServiceCredentialSecret"] {
                    assert!(
                        elements(&created, absent).is_empty(),
                        "case {case:?}: expected no {absent} on a password credential, got {:?}",
                        elements(&created, absent)
                    );
                }
            }
            Form::LongTermApiKey => {
                let alias = element(&created, "ServiceCredentialAlias");
                assert!(
                    alias.starts_with(&format!("{}-", case.user))
                        && alias.len() <= 200
                        && alias
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || "+=,.@-_".contains(c)),
                    "case {case:?}: expected an alias carrying the user name and inside \
                     serviceCredentialAlias's constraints, got {alias:?}"
                );
                assert!(
                    !element(&created, "ServiceCredentialSecret").is_empty(),
                    "case {case:?}: expected a generated ServiceCredentialSecret, got an empty one"
                );
                for absent in ["ServiceUserName", "ServicePassword"] {
                    assert!(
                        elements(&created, absent).is_empty(),
                        "case {case:?}: expected no {absent} on an API key, got {:?}",
                        elements(&created, absent)
                    );
                }
            }
        }

        // No `CredentialAgeDays` was supplied, so the credential does not expire.
        assert!(
            elements(&created, "ExpirationDate").is_empty(),
            "case {case:?}: expected no ExpirationDate without CredentialAgeDays, got {:?}",
            elements(&created, "ExpirationDate")
        );

        // The read returns the stored record.
        let listed = list_credentials(&router, &[("UserName", case.user)]).await;
        assert_eq!(
            elements(&listed, "ServiceSpecificCredentialId"),
            vec![id.clone()],
            "case {case:?}: expected the listing to report exactly the created credential, got {:?}",
            elements(&listed, "ServiceSpecificCredentialId")
        );
        for member in ["UserName", "ServiceName", "Status"] {
            assert_eq!(
                element(&listed, member),
                element(&created, member),
                "case {case:?}: expected the listing to report the stored {member}, got {:?}",
                element(&listed, member)
            );
        }
        assert_eq!(
            instant(&listed, "CreateDate"),
            created_at,
            "case {case:?}: expected the listing to report the stored CreateDate"
        );

        // The password "is only available" in the create response, so a later read
        // reports neither it nor an API key's secret.
        for secret in ["ServicePassword", "ServiceCredentialSecret"] {
            assert!(
                elements(&listed, secret).is_empty(),
                "case {case:?}: expected the listing to carry no {secret}, got {:?}",
                elements(&listed, secret)
            );
        }

        // Reading again cannot change what the next caller sees. (The envelope's
        // `RequestId` is per call, so the comparison is over the credential itself.)
        let again = list_credentials(&router, &[("UserName", case.user)]).await;
        for member in [
            "UserName",
            "ServiceName",
            "Status",
            "CreateDate",
            "ServiceSpecificCredentialId",
        ] {
            assert_eq!(
                elements(&again, member),
                elements(&listed, member),
                "case {case:?}: a second listing changed {member}, so the record was not stored"
            );
        }

        minted_ids.push(id);
    }
}

/// RULE: a user holds a bounded number of credential sets PER service, and a create
/// past that bound is refused with `LimitExceeded` (HTTP 409) without minting
/// anything; the bound is counted per (user, service), so another service or
/// another user is unaffected.
#[tokio::test]
async fn creating_past_the_per_service_quota_is_refused() {
    // The quota the handler enforces, read from the service's own constant so the
    // test cannot drift from it — and pinned to the number AWS documents.
    const QUOTA: usize = state::MAX_SERVICE_SPECIFIC_CREDENTIALS_PER_SERVICE;
    assert_eq!(
        QUOTA, 2,
        "CreateServiceSpecificCredential documents \"a maximum of two sets of \
         service-specific credentials for each supported service per user\", got {QUOTA}"
    );

    let router = router();
    // Rows: two users, so the quota is exercised on more than one holder.
    for user in ["quota.user", "Quota_Second"] {
        create_user(&router, user).await;
        // Rows: one service of each form, so the per-service counting is exercised
        // beyond a single service.
        for service in ["cassandra.amazonaws.com", "bedrock.amazonaws.com"] {
            let case = format!("{user} on {service}");
            let mut held = Vec::new();
            for attempt in 1..=QUOTA {
                let (status, body) = create_credential(&router, user, service).await;
                assert_eq!(
                    status, 200,
                    "case {case:?} attempt {attempt}: expected 200, got {status}: {body}"
                );
                held.push(element(&body, "ServiceSpecificCredentialId"));
            }

            let (status, body) = create_credential(&router, user, service).await;
            assert_eq!(
                status, 409,
                "case {case:?}: expected the quota to refuse the next create, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "LimitExceeded",
                "case {case:?}: expected LimitExceeded, got {:?}",
                error_code(&body)
            );

            // The refusal minted nothing: the user still holds exactly the quota.
            let listed =
                list_credentials(&router, &[("UserName", user), ("ServiceName", service)]).await;
            let mut listed_ids = elements(&listed, "ServiceSpecificCredentialId");
            listed_ids.sort();
            held.sort();
            assert_eq!(
                listed_ids, held,
                "case {case:?}: expected exactly the {QUOTA} credentials the quota allows, got {listed_ids:?}"
            );
        }
    }
}

/// RULE: only a service that supports service-specific credentials has any to
/// generate, so a create naming any other service is refused with
/// `NotSupportedService` (HTTP 404) and persists nothing — and so is a listing
/// filtered by such a service.
#[tokio::test]
async fn a_service_that_issues_no_credentials_is_not_supported() {
    let router = router();
    create_user(&router, "unsupported.caller").await;

    // Rows: a service that has no service-specific credentials at all, a bare label
    // that is not a service principal, a name that merely extends a supported one,
    // one that merely prefixes it, and the empty name (`serviceName` carries no
    // length constraint).
    for service in [
        "s3.amazonaws.com",
        "cassandra",
        "cassandra.amazonaws.com.example.net",
        "notcassandra.amazonaws.com",
        "",
    ] {
        let (status, body) = create_credential(&router, "unsupported.caller", service).await;
        assert_eq!(
            status, 404,
            "case {service:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotSupportedService",
            "case {service:?}: expected NotSupportedService, got {:?}",
            error_code(&body)
        );

        let (status, body) = call(
            &router,
            "ListServiceSpecificCredentials",
            &[("UserName", "unsupported.caller"), ("ServiceName", service)],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {service:?}: expected the filtered listing to be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotSupportedService",
            "case {service:?}: expected NotSupportedService, got {:?}",
            error_code(&body)
        );
    }

    // Nothing was persisted by any of the refusals.
    let listed = list_credentials(&router, &[("UserName", "unsupported.caller")]).await;
    assert!(
        elements(&listed, "ServiceSpecificCredentialId").is_empty(),
        "expected a refused create to persist nothing, got {:?}",
        elements(&listed, "ServiceSpecificCredentialId")
    );
    assert_eq!(
        element(&listed, "IsTruncated"),
        "false",
        "expected an empty listing not to be truncated, got {listed}"
    );
}

/// RULE: a credential belongs to an IAM user the account holds, so a request naming
/// any other user answers `NoSuchEntity` (HTTP 404) — on the create and on the
/// listing alike.
#[tokio::test]
async fn naming_a_user_the_account_does_not_hold_is_not_found() {
    let router = router();
    create_user(&router, "present.user").await;

    // Rows: a name never created, two near-misses of the one that was, and a name at
    // the longest length `userNameType` admits (64).
    let longest = "u".repeat(64);
    for absent in ["nobody", "present.use", "present.userr", longest.as_str()] {
        let (status, body) = create_credential(&router, absent, "cassandra.amazonaws.com").await;
        assert_eq!(
            status, 404,
            "case {absent:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {absent:?}: expected NoSuchEntity, got {:?}",
            error_code(&body)
        );

        let (status, body) = call(
            &router,
            "ListServiceSpecificCredentials",
            &[("UserName", absent)],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {absent:?}: expected the listing to be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {absent:?}: expected NoSuchEntity, got {:?}",
            error_code(&body)
        );
    }
}

/// RULE: `CredentialAgeDays` expires a long-term API key that many days after its
/// creation; a service that issues no long-term API keys cannot take the parameter
/// at all, and a value outside the published range never reaches the store.
#[tokio::test]
async fn a_credential_age_expires_a_long_term_api_key() {
    /// Seconds in a day: the unit `CredentialAgeDays` counts.
    const DAY: i64 = 86_400;

    let router = router();

    // Rows: each published bound of `credentialAgeDays` (range 1..36600) and a value
    // between them. Each row takes its own user, so the per-service quota (two sets
    // per user) is not what the row runs into.
    for days in [1, 90, 36_600] {
        let user = format!("aged.key.{days}");
        create_user(&router, &user).await;
        let (status, body) = call(
            &router,
            "CreateServiceSpecificCredential",
            &[
                ("UserName", user.as_str()),
                ("ServiceName", "bedrock.amazonaws.com"),
                ("CredentialAgeDays", &days.to_string()),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {days} days: expected 200, got {status}: {body}"
        );
        let created_at = instant(&body, "CreateDate");
        let expires_at = instant(&body, "ExpirationDate");
        assert_eq!(
            expires_at - created_at,
            days * DAY,
            "case {days} days: expected the expiry {} seconds after creation, got {}",
            days * DAY,
            expires_at - created_at
        );

        // The expiry is persisted, not recomputed per read.
        let listed = list_credentials(
            &router,
            &[
                ("UserName", user.as_str()),
                ("ServiceName", "bedrock.amazonaws.com"),
            ],
        )
        .await;
        assert!(
            elements(&listed, "ExpirationDate")
                .contains(&element(&body, "ExpirationDate")),
            "case {days} days: expected the stored ExpirationDate in the listing, got {:?}",
            elements(&listed, "ExpirationDate")
        );
    }

    // One past each published bound, refused before the operation runs.
    create_user(&router, "aged.range").await;
    for out_of_range in ["0", "36601", "-1"] {
        let (status, body) = call(
            &router,
            "CreateServiceSpecificCredential",
            &[
                ("UserName", "aged.range"),
                ("ServiceName", "bedrock.amazonaws.com"),
                ("CredentialAgeDays", out_of_range),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {out_of_range:?}: expected the range to be enforced, got {status}: {body}"
        );
    }

    // A service that issues user names and passwords supports no expiry at all.
    create_user(&router, "no.expiry").await;
    for service in ["cassandra.amazonaws.com", "codecommit.amazonaws.com"] {
        let (status, body) = call(
            &router,
            "CreateServiceSpecificCredential",
            &[
                ("UserName", "no.expiry"),
                ("ServiceName", service),
                ("CredentialAgeDays", "30"),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {service:?}: expected CredentialAgeDays to be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {service:?}: expected the client-input refusal code, got {:?}",
            error_code(&body)
        );
    }
    let listed = list_credentials(&router, &[("UserName", "no.expiry")]).await;
    assert!(
        elements(&listed, "ServiceSpecificCredentialId").is_empty(),
        "expected the refused creates to persist nothing, got {:?}",
        elements(&listed, "ServiceSpecificCredentialId")
    );
}

/// RULE: a credential whose expiration date has passed is `Expired`, whether or not
/// anybody called — the status a read reports is the one that instant already
/// carried, and it stays that way for every later reader.
#[tokio::test]
async fn an_expiration_date_in_the_past_reports_expired() {
    let (router, engine) = router_with_engine();
    create_user(&router, "clock.user").await;
    let (status, created) = call(
        &router,
        "CreateServiceSpecificCredential",
        &[
            ("UserName", "clock.user"),
            ("ServiceName", "logs.amazonaws.com"),
            ("CredentialAgeDays", "1"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the API key to be created, got {status}: {created}"
    );
    let id = element(&created, "ServiceSpecificCredentialId");

    // Rows: an expiry a second ago and one a day ago. Rewinding the stored instant
    // is how a day-granular expiry is reached without waiting for it; the handler
    // reads the clock, never the test.
    for ago in [1, 86_400] {
        let store = state::service_state(&*engine).expect("the IAM store");
        {
            let mut store = store.lock();
            let credential = store
                .service_specific_credentials
                .get_mut(&id)
                .unwrap_or_else(|| panic!("case {ago}s ago: the created credential is not stored"));
            let now = DateTime::from(std::time::SystemTime::now()).secs();
            credential.expiration_date = Some(DateTime::from_secs(now - ago));
            credential.status = state::CREDENTIAL_STATUS_ACTIVE.to_string();
        }

        let listed = list_credentials(&router, &[("UserName", "clock.user")]).await;
        assert_eq!(
            element(&listed, "Status"),
            "Expired",
            "case {ago}s ago: expected the passed expiry to report Expired, got {:?}",
            element(&listed, "Status")
        );

        // The materialised status is what the next reader sees too.
        let again = list_credentials(&router, &[("UserName", "clock.user")]).await;
        assert_eq!(
            element(&again, "Status"),
            "Expired",
            "case {ago}s ago: expected the expiry to stay materialised, got {:?}",
            element(&again, "Status")
        );
    }

    // An expiry still in the future leaves the credential valid for API calls.
    let store = state::service_state(&*engine).expect("the IAM store");
    {
        let mut store = store.lock();
        let credential = store
            .service_specific_credentials
            .get_mut(&id)
            .expect("the created credential");
        let now = DateTime::from(std::time::SystemTime::now()).secs();
        credential.expiration_date = Some(DateTime::from_secs(now + 86_400));
        credential.status = state::CREDENTIAL_STATUS_ACTIVE.to_string();
    }
    let listed = list_credentials(&router, &[("UserName", "clock.user")]).await;
    assert_eq!(
        element(&listed, "Status"),
        "Active",
        "expected an expiry in the future to leave the credential Active, got {:?}",
        element(&listed, "Status")
    );
}

/// RULE: the listing enumerates the persisted credentials in one total order,
/// selects exactly those its filters name, honours `MaxItems` as a real page size,
/// and hands out a `Marker` a follow-up call resumes from without repeating or
/// skipping a credential.
#[tokio::test]
async fn the_listing_filters_and_pages_through_every_credential_exactly_once() {
    let router = router();
    // Two users × two services, with two credentials for one pairing, so a filter
    // has something to exclude on every axis.
    let fixtures: &[(&str, &str)] = &[
        ("page.one", "cassandra.amazonaws.com"),
        ("page.one", "cassandra.amazonaws.com"),
        ("page.one", "codecommit.amazonaws.com"),
        ("page.two", "cassandra.amazonaws.com"),
        ("page.two", "bedrock.amazonaws.com"),
    ];
    for user in ["page.one", "page.two"] {
        create_user(&router, user).await;
    }
    let mut all_ids = Vec::new();
    for (user, service) in fixtures {
        let (status, body) = create_credential(&router, user, service).await;
        assert_eq!(
            status, 200,
            "case {user} on {service}: expected 200, got {status}: {body}"
        );
        all_ids.push(element(&body, "ServiceSpecificCredentialId"));
    }
    all_ids.sort();

    // Rows span the page size: one, sizes that do and do not divide the set, the set
    // itself, and a size past it.
    for page_size in [1, 2, 3, all_ids.len(), all_ids.len() + 1] {
        let mut seen: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        loop {
            let size = page_size.to_string();
            let mut params = vec![("AllUsers", "true"), ("MaxItems", size.as_str())];
            if let Some(token) = &marker {
                params.push(("Marker", token.as_str()));
            }
            let body = list_credentials(&router, &params).await;
            let page = elements(&body, "ServiceSpecificCredentialId");
            assert!(
                page.len() <= page_size,
                "case MaxItems={page_size}: expected at most {page_size} per page, got {}",
                page.len()
            );
            seen.extend(page);
            marker = match element(&body, "IsTruncated").as_str() {
                "true" => Some(element(&body, "Marker")),
                _ => None,
            };
            if marker.is_none() {
                break;
            }
        }
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted, all_ids,
            "case MaxItems={page_size}: expected every credential exactly once, got {seen:?}"
        );
    }

    // Rows: each filter axis, with the number of credentials it selects.
    let filters: &[(&[(&str, &str)], usize)] = &[
        (&[("UserName", "page.one")], 3),
        (&[("UserName", "page.two")], 2),
        (
            &[
                ("UserName", "page.one"),
                ("ServiceName", "cassandra.amazonaws.com"),
            ],
            2,
        ),
        (
            &[
                ("UserName", "page.one"),
                ("ServiceName", "bedrock.amazonaws.com"),
            ],
            0,
        ),
        (
            &[("AllUsers", "true"), ("ServiceName", "cassandra.amazonaws.com")],
            3,
        ),
        (&[("AllUsers", "true")], all_ids.len()),
    ];
    for (params, expected) in filters {
        let body = list_credentials(&router, params).await;
        let found = elements(&body, "ServiceSpecificCredentialId");
        assert_eq!(
            found.len(),
            *expected,
            "case {params:?}: expected {expected} credentials, got {found:?}"
        );
        assert!(
            found.iter().all(|id| all_ids.contains(id)),
            "case {params:?}: expected only credentials this account minted, got {found:?}"
        );
    }

    // `AllUsers` "cannot be specified together with UserName".
    let (status, body) = call(
        &router,
        "ListServiceSpecificCredentials",
        &[("AllUsers", "true"), ("UserName", "page.one")],
    )
    .await;
    assert_eq!(
        status, 400,
        "expected AllUsers with UserName to be refused, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationError",
        "expected the client-input refusal code, got {:?}",
        error_code(&body)
    );

    // A `Marker` this service did not issue names no position in the store.
    for bogus in ["not-a-marker", "zz", "0g"] {
        let (status, body) = call(
            &router,
            "ListServiceSpecificCredentials",
            &[("AllUsers", "true"), ("Marker", bogus)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {bogus:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {bogus:?}: expected the marker to be refused as invalid, got {:?}",
            error_code(&body)
        );
    }

    // A user holding none gets an empty list, not a refusal.
    create_user(&router, "page.empty").await;
    let body = list_credentials(&router, &[("UserName", "page.empty")]).await;
    assert!(
        elements(&body, "ServiceSpecificCredentialId").is_empty(),
        "expected an empty list for a user holding none, got {:?}",
        elements(&body, "ServiceSpecificCredentialId")
    );
    assert_eq!(
        element(&body, "IsTruncated"),
        "false",
        "expected an empty listing not to be truncated, got {body}"
    );
}
