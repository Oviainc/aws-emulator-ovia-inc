// SPDX-License-Identifier: CC0-1.0

//! A grant scoped to a NETWORK, over a real signed IAM call.
//!
//! RULE under test: IAM decides a statement's `Condition` against the request context it
//! fills itself, and the address the request was made from is one of the values it fills
//! (`aws:SourceIp`, "the requester's IP address"). So an `Allow` guarded by an address
//! clause permits the call when the request's own address satisfies the clause and withholds
//! it when it does not — the same decision the simulator reports for the same clause handed
//! the same address (`tests/all/simulated_address_conditions.rs` owns the simulator side).
//! A clause over a key IAM can answer for is DECIDED; it is not left undecided, which would
//! make every network-scoped grant inert.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which
//! address operator guards the grant (`IpAddress`, `NotIpAddress`), what it states (one
//! block, a single address with no prefix length, a list of blocks, the whole address
//! space), and where the request was made from relative to it — inside, outside, and at the
//! bounds of the block. The refusal rows show the grant is really the only thing serving
//! the call.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour of a signed call against PERSISTED policies. The awsQuery harness,
//! including the header a proxy states a forwarded request's address in, is the one
//! `tests/all/user_lifecycle.rs` owns.

use crate::user_lifecycle::{call, call_signed_from, error_code, router, HARNESS_REGION};

/// The action every row's grant is about, and the call every row makes: a read of the
/// caller's own access keys, which needs nothing but the grant.
const ACTION: &str = "iam:ListAccessKeys";
const OPERATION: &str = "ListAccessKeys";

/// The address this emulator sees for a request nothing forwarded: it serves its API over
/// the loopback interface, so a connection that reached it came from the local host.
const LOOPBACK: &str = "127.0.0.1";

/// Documentation addresses (RFC 5737) and the blocks they live in, so no row names an
/// address that belongs to anyone.
const INSIDE: &str = "203.0.113.7";
const FIRST_IN_BLOCK: &str = "203.0.113.0";
const LAST_IN_BLOCK: &str = "203.0.113.255";
const OUTSIDE: &str = "198.51.100.7";
const BLOCK: &str = "203.0.113.0/24";

/// A grant for [`ACTION`] on every resource, guarded by `clauses`.
fn grant(clauses: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"{ACTION}","Resource":"*","Condition":{clauses}}}]}}"#
    )
}

/// One row: the condition the grant carries, the address the request is made from, and
/// whether the call is then served.
struct Case {
    case: &'static str,
    /// The `Condition` element, as JSON.
    condition: String,
    /// The address a proxy states it forwarded the request for, or `None` for a request
    /// nothing forwarded — which is made from [`LOOPBACK`].
    from: Option<&'static str>,
    /// Whether the grant permits the call from that address.
    served: bool,
}

/// Rows spanning the two address operators, the forms an address value takes, and where
/// the request sits relative to the block — inside, outside, and at both bounds.
fn cases() -> Vec<Case> {
    let block = format!(r#"{{"IpAddress":{{"aws:SourceIp":"{BLOCK}"}}}}"#);
    let not_block = format!(r#"{{"NotIpAddress":{{"aws:SourceIp":"{BLOCK}"}}}}"#);
    vec![
        Case {
            case: "inside the block the grant names",
            condition: block.clone(),
            from: Some(INSIDE),
            served: true,
        },
        Case {
            case: "the first address of that block",
            condition: block.clone(),
            from: Some(FIRST_IN_BLOCK),
            served: true,
        },
        Case {
            case: "the last address of that block",
            condition: block.clone(),
            from: Some(LAST_IN_BLOCK),
            served: true,
        },
        Case {
            case: "one address past that block",
            condition: block.clone(),
            from: Some(OUTSIDE),
            served: false,
        },
        Case {
            case: "a request nothing forwarded, outside the block",
            condition: block,
            from: None,
            served: false,
        },
        Case {
            case: "a single address with no prefix length, matched exactly",
            condition: format!(r#"{{"IpAddress":{{"aws:SourceIp":"{INSIDE}"}}}}"#),
            from: Some(INSIDE),
            served: true,
        },
        Case {
            case: "a single address with no prefix length, from another address",
            condition: format!(r#"{{"IpAddress":{{"aws:SourceIp":"{INSIDE}"}}}}"#),
            from: Some(OUTSIDE),
            served: false,
        },
        Case {
            case: "a list of blocks, from an address in the second",
            condition: format!(
                r#"{{"IpAddress":{{"aws:SourceIp":["198.51.100.0/24","{BLOCK}"]}}}}"#
            ),
            from: Some(INSIDE),
            served: true,
        },
        Case {
            case: "the whole IPv4 space admits the local host",
            condition: r#"{"IpAddress":{"aws:SourceIp":"0.0.0.0/0"}}"#.to_string(),
            from: None,
            served: true,
        },
        Case {
            case: "the whole IPv4 space admits a forwarded address",
            condition: r#"{"IpAddress":{"aws:SourceIp":"0.0.0.0/0"}}"#.to_string(),
            from: Some(OUTSIDE),
            served: true,
        },
        Case {
            case: "a negated clause admits an address outside the block",
            condition: not_block.clone(),
            from: Some(OUTSIDE),
            served: true,
        },
        Case {
            case: "a negated clause withholds inside the block",
            condition: not_block.clone(),
            from: Some(INSIDE),
            served: false,
        },
        Case {
            case: "a negated clause admits the local host",
            condition: not_block,
            from: None,
            served: true,
        },
    ]
}

/// RULE over every row: the grant is decided against the address the request was made
/// from, so the call is served exactly where the clause holds for that address.
#[tokio::test]
async fn an_address_scoped_grant_decides_a_real_signed_call() {
    let router = router();

    for (row, case) in cases().into_iter().enumerate() {
        let user_name = format!("networked{row}");
        let access_key_id = principal(&router, case.case, &user_name, &case.condition).await;

        let (status, body) = call_signed_from(
            &router,
            Some(&access_key_id),
            HARNESS_REGION,
            case.from,
            OPERATION,
            &[("UserName", &user_name)],
        )
        .await;

        if case.served {
            assert_eq!(
                status, 200,
                "case {:?}: expected the grant to serve the call from {:?}, got {status}: \
                 {body}",
                case.case, case.from
            );
        } else {
            assert_eq!(
                (status, error_code(&body)),
                (403, "AccessDenied".to_string()),
                "case {:?}: expected the call from {:?} to be withheld, got {status}: {body}",
                case.case, case.from
            );
        }
    }
}

/// RULE: the address is read from the CONNECTION and from nothing the request says, so a
/// request that NAMES an in-block address among its own parameters is still decided against
/// the address it was made from — the two are different facts, and only the first is
/// `aws:SourceIp`.
#[tokio::test]
async fn the_address_is_read_from_the_connection_and_not_from_the_request() {
    let router = router();
    // A `userNameType` is `^[\w+=,.@-]+$`, which an IPv4 address satisfies, so the request
    // can carry the in-block address as one of its own parameters.
    let user_name = INSIDE;
    let condition = format!(r#"{{"IpAddress":{{"aws:SourceIp":"{BLOCK}"}}}}"#);
    let access_key_id = principal(&router, "the connection decides", user_name, &condition).await;

    let (status, body) = call_signed_from(
        &router,
        Some(&access_key_id),
        HARNESS_REGION,
        None,
        OPERATION,
        &[("UserName", user_name)],
    )
    .await;
    assert_eq!(
        (status, error_code(&body)),
        (403, "AccessDenied".to_string()),
        "expected a call made from {LOOPBACK} to be withheld however its parameters are \
         spelled, got {status}: {body}"
    );

    // The same call, made from that address: the connection is what the clause is decided
    // against, so now it holds.
    let (status, served) = call_signed_from(
        &router,
        Some(&access_key_id),
        HARNESS_REGION,
        Some(INSIDE),
        OPERATION,
        &[("UserName", user_name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the same call made from {INSIDE} to be served, got {status}: {served}"
    );
}

/// Create the user `user_name`, put the row's address-guarded grant on it, and answer the
/// access key id a request signs with to speak for it.
///
/// The grant is the ONLY document in force, so what serves a call is the clause under test
/// and nothing else.
async fn principal(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    user_name: &str,
    condition: &str,
) -> String {
    let (status, created) = call(router, "CreateUser", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateUser must succeed, got {status}: {created}"
    );
    let document = grant(condition);
    let (status, put) = call(
        router,
        "PutUserPolicy",
        &[
            ("UserName", user_name),
            ("PolicyName", "network-scoped"),
            ("PolicyDocument", &document),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture PutUserPolicy must succeed, got {status}: {put}"
    );
    let (status, key) = call(router, "CreateAccessKey", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateAccessKey must succeed, got {status}: {key}"
    );
    crate::user_lifecycle::element(&key, "AccessKeyId")
}
