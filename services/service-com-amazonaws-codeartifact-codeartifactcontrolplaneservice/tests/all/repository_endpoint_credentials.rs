// SPDX-License-Identifier: CC0-1.0

//! What a repository ENDPOINT answers a request whose credential it will not accept, over
//! the wire, for every format whose protocol this service serves.
//!
//! A package manager branches on the BODY of such a refusal, not only on its status: the
//! `success` member says whether the call worked and the `error` sentence says why, which is
//! what a CI wrapper reads to decide whether to log in again. So the rule under test is one
//! rule over a table of planes and presentations:
//!
//! 1. A request carrying NO `Authorization` header is `401` with `success: false` and a
//!    sentence naming the absent header.
//! 2. A request carrying a credential the endpoint will not accept — a token this service
//!    never minted, one the clock has expired, one minted for another domain — is `401` with
//!    `success: false` and a DIFFERENT sentence, which names the credential rather than the
//!    missing header.
//! 3. The same URL with a token this service minted for the addressed domain gets PAST
//!    authentication: it is answered `404` with a package-not-found sentence, which is what
//!    proves the two `401`s were about the credential and not about the path.
//!
//! Both planes are driven in the presentation their own clients use: npm's bearer token and
//! pip's HTTP Basic password.

use serde_json::Value;

use crate::domain_repository_lifecycle::create_repository;
use crate::harness::{create_domain, emulator_with_engine, ACCOUNT};
use crate::npm_registry_endpoint::{endpoint_host, token_for};
use crate::pypi_data_plane::{basic_credential, data_plane, Raw};

/// The `success` member of a refusal body, or `None` when the body carries none.
fn success(answer: &Raw) -> Option<bool> {
    answer.json().get("success").and_then(Value::as_bool)
}

/// One plane under test: the format it serves, the path a client fetches a package it does
/// not hold at, how that client presents the token, and the challenge the plane states when
/// it refuses one.
struct Plane {
    case: &'static str,
    /// The path inside the endpoint, which the format's own protocol decides.
    fetched: fn(&str) -> String,
    /// The `Authorization` value a client of this plane presents the token with.
    presented: fn(&str) -> String,
    /// The `WWW-Authenticate` scheme its clients re-present a refreshed token under.
    challenge: &'static str,
}

/// The npm packument path of a package no repository holds.
fn npm_absent(repository: &str) -> String {
    format!("/npm/{repository}/sim2real-absent-package")
}

/// The PyPI index path of a project no repository holds.
fn pypi_absent(repository: &str) -> String {
    format!("/pypi/{repository}/simple/sim2real-absent-project/")
}

/// The bearer presentation npm's clients use.
fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

/// RULES 1, 2 and 3: the two `401` bodies of a repository endpoint carry `success: false`
/// and two DIFFERENT sentences, and the same URL with a real token is answered past
/// authentication.
#[tokio::test]
async fn the_endpoint_states_two_distinct_refusals_and_lets_a_real_token_past() {
    let (router, engine) = emulator_with_engine();
    let domain = "ca-credentials";
    let repository = "gated";
    assert_eq!(
        create_domain(&router, domain, None).await.status,
        200,
        "CreateDomain must succeed"
    );
    assert_eq!(
        create_repository(&router, domain, repository, None, &[])
            .await
            .status,
        200,
        "CreateRepository must succeed"
    );
    let token = token_for(&router, domain).await;
    let other = {
        assert_eq!(
            create_domain(&router, "ca-elsewhere", None).await.status,
            200,
            "CreateDomain for the other domain must succeed"
        );
        assert_eq!(
            create_repository(&router, "ca-elsewhere", "elsewhere", None, &[])
                .await
                .status,
            200,
            "CreateRepository in the other domain must succeed"
        );
        token_for(&router, "ca-elsewhere").await
    };
    // A token this service minted and the clock has already passed, seeded into the same
    // store `GetAuthorizationToken` writes.
    let expired = "expired-token-value";
    {
        let state =
            service_com_amazonaws_codeartifact_codeartifactcontrolplaneservice::state::service_state(
                &*engine,
            )
            .expect("the CodeArtifact store");
        let now = std::time::SystemTime::now();
        state.lock().record_authorization_token(
            expired,
            domain,
            ACCOUNT,
            &format!("arn:aws:iam::{ACCOUNT}:user/emulator"),
            now - std::time::Duration::from_secs(1),
            now,
        );
    }

    let planes = [
        Plane {
            case: "the npm endpoint, whose client presents a bearer token",
            fetched: npm_absent,
            presented: bearer,
            challenge: "Bearer",
        },
        Plane {
            case: "the pypi endpoint, whose client presents an HTTP Basic password",
            fetched: pypi_absent,
            presented: basic_credential,
            challenge: "Basic",
        },
    ];
    let host = endpoint_host(domain);

    for plane in planes {
        let uri = (plane.fetched)(repository);

        // RULE 1: no header at all.
        let missing = data_plane(&router, "GET", &host, &uri, &[], Vec::new()).await;
        assert_eq!(
            missing.status, 401,
            "case {:?}, no Authorization header: expected HTTP 401, got {missing:?}",
            plane.case
        );
        assert_eq!(
            success(&missing),
            Some(false),
            "case {:?}, no Authorization header: expected success=false, got {:?}",
            plane.case,
            missing.text()
        );
        assert!(
            missing.error().contains("Authorization header"),
            "case {:?}, no Authorization header: expected a sentence naming the absent header, \
             got {:?}",
            plane.case,
            missing.error()
        );
        assert_eq!(
            missing.www_authenticate.as_deref(),
            Some(plane.challenge),
            "case {:?}, no Authorization header: expected the {} challenge, got {missing:?}",
            plane.case,
            plane.challenge
        );
        // RULE 2: every credential this endpoint will not accept, each answered by the SAME
        // sentence as the others and a different one from RULE 1's.
        let refused: [(&str, String); 3] = [
            ("a token this service never minted", "not-a-minted-token".to_string()),
            ("a token the clock has expired", expired.to_string()),
            ("a token minted for another domain", other.clone()),
        ];
        for (credential, value) in refused {
            let answer = data_plane(
                &router,
                "GET",
                &host,
                &uri,
                &[("authorization", (plane.presented)(&value))],
                Vec::new(),
            )
            .await;
            assert_eq!(
                answer.status, 401,
                "case {:?}, {credential}: expected HTTP 401, got {answer:?}",
                plane.case
            );
            assert_eq!(
                success(&answer),
                Some(false),
                "case {:?}, {credential}: expected success=false, got {:?}",
                plane.case,
                answer.text()
            );
            assert_ne!(
                answer.error(),
                missing.error(),
                "case {:?}, {credential}: expected a sentence other than the missing-header \
                 one",
                plane.case
            );
            assert!(
                !answer.error().is_empty() && !answer.error().contains(&value),
                "case {:?}, {credential}: expected a sentence that states the credential \
                 without quoting it, got {:?}",
                plane.case,
                answer.error()
            );
            assert!(
                answer.error().contains("invalid credentials"),
                "case {:?}, {credential}: expected a sentence naming invalid credentials, got \
                 {:?}",
                plane.case,
                answer.error()
            );
            assert_eq!(
                answer.www_authenticate.as_deref(),
                Some(plane.challenge),
                "case {:?}, {credential}: expected the {} challenge, got {answer:?}",
                plane.case,
                plane.challenge
            );
        }

        // RULE 3: the real token gets past authentication, so the same URL is answered for
        // the PATH it named.
        let allowed = data_plane(
            &router,
            "GET",
            &host,
            &uri,
            &[("authorization", (plane.presented)(&token))],
            Vec::new(),
        )
        .await;
        assert_eq!(
            allowed.status, 404,
            "case {:?}, a token minted for this domain: expected HTTP 404 for the absent \
             package, got {allowed:?}",
            plane.case
        );
        assert!(
            allowed.error().contains("Package not found in repository"),
            "case {:?}, a token minted for this domain: expected the package-not-found \
             sentence, got {:?}",
            plane.case,
            allowed.error()
        );
        assert_eq!(
            success(&allowed),
            None,
            "case {:?}: a package that is absent is not an authentication failure, so its body \
             carries no success member, got {:?}",
            plane.case,
            allowed.text()
        );
    }
}
