// SPDX-License-Identifier: CC0-1.0

//! The Directory Service trust relationships, driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router (`crate::harness`). Nothing here reaches into
//! the state struct, so a response that is not computed from persisted state cannot pass,
//! and every created trust is read back through `DescribeTrusts`.
//!
//! The cases assert RULES over sets of inputs rather than one recorded call: the stored
//! record over a table of create requests spanning the WHOLE modelled `TrustDirection`,
//! `TrustType` and `SelectiveAuth` catalogs, the optional members' defaults, both
//! conditional-forwarder address families and the form and length of the remote domain
//! name (including the published `RemoteDomainName` bound and one past it); the
//! `Creating` -> `Created` transition the clock owes; the one-trust-per-remote-domain rule
//! over a table of spellings of one domain; the refusals over tables of directory ids that
//! name nothing and of directory types a trust is not supported for; the narrowing and
//! paging `DescribeTrusts` performs over a table of filters; the `Verifying` -> `Verified`
//! transition `VerifyTrust` starts, from either state a trust can be verified in; and the
//! teardown `DeleteTrust` starts over a table of what the request says about the
//! conditional forwarder the trust created; and this emulator's two documented guards
//! (the per-directory trust count and the conditional-forwarder slot a trust's remote DNS
//! addresses need) at the bound and one past it.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::conditional_forwarders::{domain_of_length, REMOTE_DOMAIN_NAME_MAX_LEN};
use crate::directory_lifecycle::{
    create_directory, minimal_create, PAST_ONE_SPAN, PAST_ONE_SPAN_ONLY,
};
use crate::harness::{call, ok, refused, service, string};
use crate::microsoft_ad::{create_microsoft_ad, minimal_microsoft_ad};

/// The whole modelled `TrustDirection` catalog
/// (`smithy_query(query="shape",shape="TrustDirection")`), every value of which a trust
/// can be created with.
const TRUST_DIRECTION_CATALOG: [&str; 3] = ["One-Way: Outgoing", "One-Way: Incoming", "Two-Way"];

/// The whole modelled `TrustType` catalog
/// (`smithy_query(query="shape",shape="TrustType")`).
const TRUST_TYPE_CATALOG: [&str; 2] = ["Forest", "External"];

/// The whole modelled `SelectiveAuth` catalog
/// (`smithy_query(query="shape",shape="SelectiveAuth")`).
const SELECTIVE_AUTH_CATALOG: [&str; 2] = ["Enabled", "Disabled"];

/// The `TrustType` a request that names none gets, from
/// `CreateTrustRequest$TrustType`'s own documentation: "The trust relationship type.
/// `Forest` is the default."
const DEFAULT_TRUST_TYPE: &str = "Forest";

/// The `SelectiveAuth` a trust whose create asked for none carries: the modelled enum's
/// not-enabled value, since the member is the "optional parameter to ENABLE selective
/// authentication".
const DEFAULT_SELECTIVE_AUTH: &str = "Disabled";

/// The `TrustState` a trust carries between `CreateTrust` and the transition the clock
/// owes: the first value of the modelled `TrustState` enum.
const TRUST_STATE_CREATING: &str = "Creating";

/// The `TrustState` a trust reaches once that transition has fallen due.
const TRUST_STATE_CREATED: &str = "Created";

/// The `TrustState` a trust carries between `VerifyTrust` and the transition that call
/// owes: the `Verifying` value of the modelled `TrustState` enum.
const TRUST_STATE_VERIFYING: &str = "Verifying";

/// The `TrustState` a verified trust settles on: the enum's `Verified` value.
const TRUST_STATE_VERIFIED: &str = "Verified";

/// The `TrustState` a trust carries between `DeleteTrust` and the teardown falling due:
/// the enum's `Deleting` value.
const TRUST_STATE_DELETING: &str = "Deleting";

/// The `TrustState` a torn-down trust settles on before it leaves the service: the enum's
/// `Deleted` value.
const TRUST_STATE_DELETED: &str = "Deleted";

/// This emulator's documented per-directory trust guard
/// (`state::MAX_TRUSTS_PER_DIRECTORY`).
const TRUST_GUARD: usize = 50;

/// This emulator's documented `TrustIds` filter guard
/// (`state::MAX_TRUST_IDS_PER_REQUEST`).
const TRUST_IDS_GUARD: usize = 100;

/// This emulator's documented per-directory conditional forwarder guard
/// (`state::MAX_CONDITIONAL_FORWARDERS_PER_DIRECTORY`), which a trust's remote DNS server
/// addresses need a slot of.
const FORWARDER_GUARD: usize = 50;

/// A `TrustPassword` the modelled pattern `^(\p{LD}|\p{Punct}| )+$` admits, within its
/// `length 1..128`.
const TRUST_PASSWORD: &str = "Tru5t-P@ssword 1";

/// The modelled `TrustId` pattern, `^t-[0-9a-f]{10}$`, as an assertion.
fn assert_trust_id(id: &str, case: &str) {
    let suffix = id.strip_prefix("t-").unwrap_or_default();
    assert!(
        suffix.len() == 10
            && suffix
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "case {case:?}: expected a TrustId matching ^t-[0-9a-f]{{10}}$, got {id:?}"
    );
}

/// A Managed Microsoft AD directory, the only directory type a trust is supported for.
async fn managed_microsoft_ad(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    create_microsoft_ad(router, minimal_microsoft_ad(name), case).await
}

/// One row of the create table: what the request asks for. `None` means the member is
/// absent from the request, which is what pins the default the service applies.
struct TrustCase {
    case: &'static str,
    remote_domain_name: String,
    trust_direction: &'static str,
    trust_type: Option<&'static str>,
    selective_auth: Option<&'static str>,
    forwarder_ip_addrs: Option<Vec<&'static str>>,
    forwarder_ipv6_addrs: Option<Vec<&'static str>>,
}

/// The `CreateTrust` body one row of the table sends.
fn create_request(directory_id: &str, row: &TrustCase) -> Value {
    let mut body = json!({
        "DirectoryId": directory_id,
        "RemoteDomainName": &row.remote_domain_name,
        "TrustPassword": TRUST_PASSWORD,
        "TrustDirection": row.trust_direction,
    });
    if let Some(trust_type) = row.trust_type {
        body["TrustType"] = json!(trust_type);
    }
    if let Some(selective_auth) = row.selective_auth {
        body["SelectiveAuth"] = json!(selective_auth);
    }
    if let Some(addrs) = &row.forwarder_ip_addrs {
        body["ConditionalForwarderIpAddrs"] = json!(addrs);
    }
    if let Some(addrs) = &row.forwarder_ipv6_addrs {
        body["ConditionalForwarderIpv6Addrs"] = json!(addrs);
    }
    body
}

/// The create table: rows differ in trust direction (the whole modelled catalog), trust
/// type (the whole catalog and the absent member), selective authentication (the whole
/// catalog and the absent member), which conditional-forwarder address families the
/// request names, and the form, case and length of the remote domain name.
fn trust_table() -> Vec<TrustCase> {
    vec![
        TrustCase {
            case: "a two-way trust with every optional member absent",
            remote_domain_name: "ad1.example.com".to_string(),
            trust_direction: TRUST_DIRECTION_CATALOG[2],
            trust_type: None,
            selective_auth: None,
            forwarder_ip_addrs: None,
            forwarder_ipv6_addrs: None,
        },
        TrustCase {
            case: "a one-way outgoing forest trust with selective auth enabled and IPv4 \
                   forwarder addresses",
            remote_domain_name: "ad2.example.com".to_string(),
            trust_direction: TRUST_DIRECTION_CATALOG[0],
            trust_type: Some(TRUST_TYPE_CATALOG[0]),
            selective_auth: Some(SELECTIVE_AUTH_CATALOG[0]),
            forwarder_ip_addrs: Some(vec!["10.0.0.2", "10.0.0.1"]),
            forwarder_ipv6_addrs: None,
        },
        TrustCase {
            case: "a one-way incoming external trust with selective auth disabled and an \
                   IPv6 forwarder address",
            remote_domain_name: "ad3.example.com".to_string(),
            trust_direction: TRUST_DIRECTION_CATALOG[1],
            trust_type: Some(TRUST_TYPE_CATALOG[1]),
            selective_auth: Some(SELECTIVE_AUTH_CATALOG[1]),
            forwarder_ip_addrs: None,
            forwarder_ipv6_addrs: Some(vec!["2001:db8:0:0:0:0:0:1"]),
        },
        TrustCase {
            case: "a fully qualified remote domain, with the trailing root dot, and both \
                   address families",
            remote_domain_name: "rooted.example.com.".to_string(),
            trust_direction: TRUST_DIRECTION_CATALOG[2],
            trust_type: Some(TRUST_TYPE_CATALOG[1]),
            selective_auth: None,
            forwarder_ip_addrs: Some(vec!["172.16.0.1"]),
            forwarder_ipv6_addrs: Some(vec!["2001:db8:0:0:0:0:0:2", "2001:db8:0:0:0:0:0:3"]),
        },
        TrustCase {
            case: "a mixed-case remote domain",
            remote_domain_name: "AD4.Example.COM".to_string(),
            trust_direction: TRUST_DIRECTION_CATALOG[0],
            trust_type: None,
            selective_auth: Some(SELECTIVE_AUTH_CATALOG[0]),
            forwarder_ip_addrs: None,
            forwarder_ipv6_addrs: None,
        },
        TrustCase {
            case: "the longest remote domain the model admits",
            remote_domain_name: domain_of_length(REMOTE_DOMAIN_NAME_MAX_LEN),
            trust_direction: TRUST_DIRECTION_CATALOG[2],
            trust_type: Some(TRUST_TYPE_CATALOG[0]),
            selective_auth: Some(SELECTIVE_AUTH_CATALOG[1]),
            forwarder_ip_addrs: None,
            forwarder_ipv6_addrs: None,
        },
    ]
}

/// `DescribeTrusts` with `body`, returning its `Trusts`.
async fn describe(router: &MultiServiceRouter, body: Value, case: &str) -> Vec<Value> {
    let answer = ok(router, "DescribeTrusts", body, case).await;
    answer["Trusts"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected Trusts, got {answer}"))
        .clone()
}

/// The one trust `DescribeTrusts` reports for `trust_id`.
async fn describe_one(router: &MultiServiceRouter, trust_id: &str, case: &str) -> Value {
    let found = describe(router, json!({ "TrustIds": [trust_id] }), case).await;
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one trust for {trust_id}, got {found:?}"
    );
    found[0].clone()
}

/// The trust ids an answer reports, in the order it reported them.
fn reported_ids(trusts: &[Value]) -> Vec<String> {
    trusts
        .iter()
        .map(|trust| trust["TrustId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The remote domains the conditional forwarders of `directory_id` point at.
async fn forwarder_domains(
    router: &MultiServiceRouter,
    directory_id: &str,
    case: &str,
) -> BTreeSet<String> {
    let answer = ok(
        router,
        "DescribeConditionalForwarders",
        json!({ "DirectoryId": directory_id }),
        case,
    )
    .await;
    answer["ConditionalForwarders"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected ConditionalForwarders, got {answer}"))
        .iter()
        .map(|forwarder| {
            forwarder["RemoteDomainName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// `CreateTrust` with `body`, asserting it was accepted and returning the minted id.
async fn create_trust(router: &MultiServiceRouter, body: Value, case: &str) -> String {
    let answer = ok(router, "CreateTrust", body, case).await;
    let id = string(&answer, "TrustId", case);
    assert_trust_id(&id, case);
    id
}

#[tokio::test]
async fn created_trusts_are_reported_from_persisted_state() {
    let router = service();
    let directory_id = managed_microsoft_ad(&router, "corp.example.com", "round trip").await;

    // A directory with no trust reports an empty list, not a 501 and not a trust nobody
    // created.
    let before = describe(
        &router,
        json!({ "DirectoryId": &directory_id }),
        "before any create",
    )
    .await;
    assert!(
        before.is_empty(),
        "case \"before any create\": expected no trusts, got {before:?}"
    );

    let table = trust_table();
    let mut minted: Vec<String> = Vec::new();
    for row in &table {
        let id = create_trust(&router, create_request(&directory_id, row), row.case).await;
        minted.push(id);
    }

    // Every id is distinct: a trust is a resource of its own, so no two creates can mint
    // one handle.
    let distinct: BTreeSet<&String> = minted.iter().collect();
    assert_eq!(
        distinct.len(),
        minted.len(),
        "case \"minted ids\": expected every CreateTrust to mint its own TrustId, got {minted:?}"
    );

    // Past the transition the create owes, so every row's state is the settled one and
    // the table's assertions do not race the clock.
    tokio::time::sleep(PAST_ONE_SPAN).await;

    for (row, trust_id) in table.iter().zip(&minted) {
        let case = row.case;
        let reported = describe_one(&router, trust_id, case).await;

        for (member, expected) in [
            ("TrustId", json!(trust_id)),
            ("DirectoryId", json!(&directory_id)),
            // The remote domain comes back exactly as the request spelled it, case and
            // trailing root dot included, even though the duplicate rule folds both.
            ("RemoteDomainName", json!(&row.remote_domain_name)),
            (
                "TrustType",
                json!(row.trust_type.unwrap_or(DEFAULT_TRUST_TYPE)),
            ),
            ("TrustDirection", json!(row.trust_direction)),
            (
                "SelectiveAuth",
                json!(row.selective_auth.unwrap_or(DEFAULT_SELECTIVE_AUTH)),
            ),
            ("TrustState", json!(TRUST_STATE_CREATED)),
        ] {
            assert_eq!(
                reported[member], expected,
                "case {case:?}: expected {member} to be {expected}, got {reported}"
            );
        }

        // The three modelled timestamps are reported, and the state's own is not before
        // the creation: the state moved after the trust was created.
        let created = reported["CreatedDateTime"].as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: expected a CreatedDateTime, got {reported}")
        });
        let state_updated = reported["StateLastUpdatedDateTime"]
            .as_f64()
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a StateLastUpdatedDateTime, got {reported}")
            });
        let last_updated = reported["LastUpdatedDateTime"].as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: expected a LastUpdatedDateTime, got {reported}")
        });
        assert!(
            state_updated >= created && last_updated >= created,
            "case {case:?}: expected the update timestamps not to precede CreatedDateTime, got \
             {reported}"
        );
        assert_eq!(
            reported["TrustStateReason"],
            Value::Null,
            "case {case:?}: expected no TrustStateReason on a created trust, got {reported}"
        );
    }

    // The conditional forwarder a create's address lists ask for is stored on the
    // directory, in the order the request listed the addresses — and a create that named
    // no address stored no forwarder.
    let forwarders = ok(
        &router,
        "DescribeConditionalForwarders",
        json!({ "DirectoryId": &directory_id }),
        "the forwarders the creates asked for",
    )
    .await;
    let stored = forwarders["ConditionalForwarders"]
        .as_array()
        .unwrap_or_else(|| panic!("expected ConditionalForwarders, got {forwarders}"))
        .clone();
    for row in &table {
        let case = row.case;
        let wanted = row.forwarder_ip_addrs.is_some() || row.forwarder_ipv6_addrs.is_some();
        let found = stored.iter().find(|forwarder| {
            forwarder["RemoteDomainName"].as_str() == Some(row.remote_domain_name.as_str())
        });
        match (wanted, found) {
            (false, Some(unexpected)) => panic!(
                "case {case:?}: expected no conditional forwarder for a create that named no \
                 address, got {unexpected}"
            ),
            (false, None) => {}
            (true, None) => panic!(
                "case {case:?}: expected a conditional forwarder for {domain:?}, got {stored:?}",
                domain = row.remote_domain_name
            ),
            (true, Some(forwarder)) => {
                let expected_v4 = match &row.forwarder_ip_addrs {
                    Some(addrs) => json!(addrs),
                    None => Value::Null,
                };
                let expected_v6 = match &row.forwarder_ipv6_addrs {
                    Some(addrs) => json!(addrs),
                    None => Value::Null,
                };
                assert_eq!(
                    forwarder["DnsIpAddrs"], expected_v4,
                    "case {case:?}: expected DnsIpAddrs {expected_v4}, got {forwarder}"
                );
                assert_eq!(
                    forwarder["DnsIpv6Addrs"], expected_v6,
                    "case {case:?}: expected DnsIpv6Addrs {expected_v6}, got {forwarder}"
                );
            }
        }
    }

    // A read does not change what the next caller sees.
    let all = describe(&router, json!({}), "repeated read").await;
    let again = describe(&router, json!({}), "repeated read").await;
    assert_eq!(
        all, again,
        "case \"repeated read\": expected a repeated describe to report the same trusts, got \
         {again:?}"
    );

    // One past the upper `RemoteDomainName` length bound is refused by the modelled
    // constraint, before any handler runs, and stores nothing.
    let too_long = domain_of_length(REMOTE_DOMAIN_NAME_MAX_LEN + 1);
    let (status, parsed) = call(
        &router,
        "CreateTrust",
        json!({
            "DirectoryId": &directory_id,
            "RemoteDomainName": &too_long,
            "TrustPassword": TRUST_PASSWORD,
            "TrustDirection": TRUST_DIRECTION_CATALOG[2],
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"one past the RemoteDomainName length bound\": expected 400, got {status} with \
         {parsed}"
    );
    let after_refusal = describe(&router, json!({}), "after the refused create").await;
    assert_eq!(
        after_refusal, all,
        "case \"one past the RemoteDomainName length bound\": expected the refused create to \
         store nothing, got {after_refusal:?}"
    );
}

#[tokio::test]
async fn a_created_trust_leaves_the_creating_state_on_the_clock() {
    let router = service();
    let directory_id = managed_microsoft_ad(&router, "corp.example.com", "the clock").await;
    let trust_id = create_trust(
        &router,
        json!({
            "DirectoryId": &directory_id,
            "RemoteDomainName": "remote.example.com",
            "TrustPassword": TRUST_PASSWORD,
            "TrustDirection": TRUST_DIRECTION_CATALOG[2],
        }),
        "the clock",
    )
    .await;

    // A caller acting straight after the create still sees the transient state the create
    // answers in: the create "initiates" the relationship rather than completing it.
    let transient = describe_one(&router, &trust_id, "straight after the create").await;
    assert_eq!(
        transient["TrustState"],
        json!(TRUST_STATE_CREATING),
        "case \"straight after the create\": expected {TRUST_STATE_CREATING}, got {transient}"
    );

    // Past the span, the transition the clock owed has been made — by the clock, not by
    // the read: it was due at the same instant whether or not anybody called.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    let settled = describe_one(&router, &trust_id, "past one span").await;
    assert_eq!(
        settled["TrustState"],
        json!(TRUST_STATE_CREATED),
        "case \"past one span\": expected {TRUST_STATE_CREATED}, got {settled}"
    );
    assert_eq!(
        settled["CreatedDateTime"], transient["CreatedDateTime"],
        "case \"past one span\": expected the creation time not to move, got {settled}"
    );
    assert!(
        settled["StateLastUpdatedDateTime"].as_f64()
            > transient["StateLastUpdatedDateTime"].as_f64(),
        "case \"past one span\": expected StateLastUpdatedDateTime to move with the state, got \
         {settled}"
    );

    let again = describe_one(&router, &trust_id, "a repeated read past one span").await;
    assert_eq!(
        again, settled,
        "case \"a repeated read past one span\": expected the same trust, got {again}"
    );
}

#[tokio::test]
async fn a_second_trust_with_the_same_remote_domain_is_refused_and_changes_nothing() {
    let router = service();
    let first = managed_microsoft_ad(&router, "corp.example.com", "duplicates").await;
    let second = managed_microsoft_ad(&router, "other.example.com", "duplicates").await;

    let domain = "remote.example.com";
    let trust_id = create_trust(
        &router,
        json!({
            "DirectoryId": &first,
            "RemoteDomainName": domain,
            "TrustPassword": TRUST_PASSWORD,
            "TrustDirection": TRUST_DIRECTION_CATALOG[2],
        }),
        "duplicates: the first create",
    )
    .await;

    // A domain name is compared case-insensitively, and its fully qualified form names the
    // same domain, so every row below names the domain this directory already trusts —
    // whatever direction or type it asks for.
    for spelling in [
        "remote.example.com",
        "REMOTE.EXAMPLE.COM",
        "Remote.Example.Com",
        "remote.example.com.",
        "REMOTE.example.COM.",
    ] {
        let case = format!("a second create spelled {spelling:?}");
        refused(
            &router,
            "CreateTrust",
            json!({
                "DirectoryId": &first,
                "RemoteDomainName": spelling,
                "TrustPassword": TRUST_PASSWORD,
                "TrustDirection": TRUST_DIRECTION_CATALOG[0],
                "TrustType": TRUST_TYPE_CATALOG[1],
            }),
            "EntityAlreadyExistsException",
            &case,
        )
        .await;
    }

    // The refused creates changed nothing: the one stored trust is still the first one.
    let held = describe(&router, json!({ "DirectoryId": &first }), "after the refusals").await;
    assert_eq!(
        reported_ids(&held),
        vec![trust_id.clone()],
        "case \"after the refusals\": expected only the first trust, got {held:?}"
    );

    // The rule is per directory: a trust is a relationship between ONE directory and the
    // external domain, so another directory can trust the same domain.
    let other = create_trust(
        &router,
        json!({
            "DirectoryId": &second,
            "RemoteDomainName": domain,
            "TrustPassword": TRUST_PASSWORD,
            "TrustDirection": TRUST_DIRECTION_CATALOG[1],
        }),
        "the same domain on another directory",
    )
    .await;
    assert_ne!(
        other, trust_id,
        "case \"the same domain on another directory\": expected a distinct TrustId"
    );

    // And a different domain on the first directory is a second relationship, not a
    // duplicate.
    create_trust(
        &router,
        json!({
            "DirectoryId": &first,
            "RemoteDomainName": "another.example.com",
            "TrustPassword": TRUST_PASSWORD,
            "TrustDirection": TRUST_DIRECTION_CATALOG[2],
        }),
        "a different domain on the same directory",
    )
    .await;
    let held = describe(
        &router,
        json!({ "DirectoryId": &first }),
        "after a second domain",
    )
    .await;
    assert_eq!(
        held.len(),
        2,
        "case \"after a second domain\": expected two trusts, got {held:?}"
    );
}

#[tokio::test]
async fn a_trust_needs_a_stored_managed_microsoft_ad_directory() {
    let router = service();

    // A well-formed id that names no stored directory, and one that names a directory of a
    // type a trust is not supported for: each is refused with its own modelled error and
    // stores nothing.
    let simple_ad = create_directory(
        &router,
        minimal_create("simple.example.com"),
        "the unsupported type",
    )
    .await;
    let cases: [(&str, String, &str); 2] = [
        (
            "a DirectoryId that names nothing",
            "d-0123456789".to_string(),
            "EntityDoesNotExistException",
        ),
        (
            "a Simple AD directory, a type a trust is not supported for",
            simple_ad,
            "UnsupportedOperationException",
        ),
    ];
    for (case, directory_id, expected) in &cases {
        refused(
            &router,
            "CreateTrust",
            json!({
                "DirectoryId": directory_id,
                "RemoteDomainName": "remote.example.com",
                "TrustPassword": TRUST_PASSWORD,
                "TrustDirection": TRUST_DIRECTION_CATALOG[2],
            }),
            expected,
            case,
        )
        .await;
    }

    let stored = describe(&router, json!({}), "after the refusals").await;
    assert!(
        stored.is_empty(),
        "case \"after the refusals\": expected no trust to have been stored, got {stored:?}"
    );
}

#[tokio::test]
async fn describe_trusts_narrows_and_pages_over_what_is_stored() {
    let router = service();
    let first = managed_microsoft_ad(&router, "corp.example.com", "listing").await;
    let second = managed_microsoft_ad(&router, "other.example.com", "listing").await;

    let mut ids: Vec<String> = Vec::new();
    for (directory_id, domain) in [
        (&first, "one.example.com"),
        (&first, "two.example.com"),
        (&second, "three.example.com"),
    ] {
        ids.push(
            create_trust(
                &router,
                json!({
                    "DirectoryId": directory_id,
                    "RemoteDomainName": domain,
                    "TrustPassword": TRUST_PASSWORD,
                    "TrustDirection": TRUST_DIRECTION_CATALOG[2],
                }),
                "listing",
            )
            .await,
        );
    }

    // The filter table: each row is a request and the trusts, in creation order, it must
    // narrow the store to. The two filters are a conjunction.
    let filters: Vec<(&str, Value, Vec<String>)> = vec![
        ("no filter at all", json!({}), ids.clone()),
        (
            "one directory",
            json!({ "DirectoryId": &first }),
            vec![ids[0].clone(), ids[1].clone()],
        ),
        (
            "the other directory",
            json!({ "DirectoryId": &second }),
            vec![ids[2].clone()],
        ),
        (
            "one trust id",
            json!({ "TrustIds": [&ids[1]] }),
            vec![ids[1].clone()],
        ),
        (
            "two trust ids, in the other order",
            json!({ "TrustIds": [&ids[2], &ids[0]] }),
            vec![ids[0].clone(), ids[2].clone()],
        ),
        (
            "a directory and a trust it holds",
            json!({ "DirectoryId": &first, "TrustIds": [&ids[0]] }),
            vec![ids[0].clone()],
        ),
        (
            "a directory and a trust another directory holds",
            json!({ "DirectoryId": &first, "TrustIds": [&ids[2]] }),
            Vec::new(),
        ),
    ];
    for (case, request, expected) in &filters {
        let reported = describe(&router, request.clone(), case).await;
        assert_eq!(
            &reported_ids(&reported),
            expected,
            "case {case:?}: expected {expected:?} in creation order, got {reported:?}"
        );
    }

    // A `Limit` pages the listing, and the token the page hands back resumes it: every
    // trust is reported exactly once, in creation order, and the last page carries no
    // token.
    let mut paged: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..ids.len() + 1 {
        let mut request = json!({ "Limit": 1 });
        if let Some(next) = &token {
            request["NextToken"] = json!(next);
        }
        let answer = ok(&router, "DescribeTrusts", request, "paging by one").await;
        let page = answer["Trusts"]
            .as_array()
            .unwrap_or_else(|| panic!("case \"paging by one\": expected Trusts, got {answer}"))
            .clone();
        paged.extend(reported_ids(&page));
        token = answer["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    assert!(
        token.is_none(),
        "case \"paging by one\": expected the last page to carry no NextToken, got {token:?}"
    );
    assert_eq!(
        paged, ids,
        "case \"paging by one\": expected every trust once in creation order, got {paged:?}"
    );

    // The refusal table: a token this service did not issue, a token issued under another
    // filter, an empty `TrustIds` ("an empty list results in an InvalidParameterException
    // being thrown"), an id naming no stored trust and one naming no stored directory.
    let issued = {
        let answer = ok(
            &router,
            "DescribeTrusts",
            json!({ "Limit": 1 }),
            "a token issued unfiltered",
        )
        .await;
        string(&answer, "NextToken", "a token issued unfiltered")
    };
    let refusals: Vec<(&str, Value, &str)> = vec![
        (
            "a NextToken this service did not issue",
            json!({ "NextToken": "not-a-token" }),
            "InvalidNextTokenException",
        ),
        (
            "a NextToken issued under another filter",
            json!({ "DirectoryId": &first, "NextToken": issued }),
            "InvalidNextTokenException",
        ),
        (
            "an empty TrustIds",
            json!({ "TrustIds": [] }),
            "InvalidParameterException",
        ),
        (
            "a TrustId naming no stored trust",
            json!({ "TrustIds": ["t-0123456789"] }),
            "EntityDoesNotExistException",
        ),
        (
            "a DirectoryId naming no stored directory",
            json!({ "DirectoryId": "d-0123456789" }),
            "EntityDoesNotExistException",
        ),
        (
            "one past the TrustIds filter guard",
            json!({
                "TrustIds": (0..=TRUST_IDS_GUARD)
                    .map(|index| format!("t-{index:010x}"))
                    .collect::<Vec<String>>()
            }),
            "InvalidParameterException",
        ),
    ];
    for (case, request, expected) in &refusals {
        refused(&router, "DescribeTrusts", request.clone(), expected, case).await;
    }

    // At the guard the filter is resolved rather than refused: the ids name no stored
    // trust, so the listing reports the absent entity instead of the oversized filter.
    refused(
        &router,
        "DescribeTrusts",
        json!({
            "TrustIds": (0..TRUST_IDS_GUARD)
                .map(|index| format!("t-{index:010x}"))
                .collect::<Vec<String>>()
        }),
        "EntityDoesNotExistException",
        "at the TrustIds filter guard",
    )
    .await;
}

#[tokio::test]
async fn verifying_a_trust_moves_it_through_the_verifying_state() {
    let router = service();
    let directory_id = managed_microsoft_ad(&router, "corp.example.com", "verification").await;

    // The rule holds whatever state the relationship is in when the verification starts,
    // so the rows differ in that: one trust is verified while its create is still under
    // way, the other once the create has settled.
    let mut rows: Vec<(&str, String)> = Vec::new();
    for (case, domain) in [
        ("verified while the create is still under way", "fresh.example.com"),
        ("verified once the create has settled", "settled.example.com"),
    ] {
        let trust_id = create_trust(
            &router,
            json!({
                "DirectoryId": &directory_id,
                "RemoteDomainName": domain,
                "TrustPassword": TRUST_PASSWORD,
                "TrustDirection": TRUST_DIRECTION_CATALOG[2],
            }),
            case,
        )
        .await;
        rows.push((case, trust_id));
    }
    tokio::time::sleep(PAST_ONE_SPAN).await;
    // The second row's create has settled; the first row gets a fresh trust so its
    // verification starts from the transient create state.
    let fresh = create_trust(
        &router,
        json!({
            "DirectoryId": &directory_id,
            "RemoteDomainName": "newest.example.com",
            "TrustPassword": TRUST_PASSWORD,
            "TrustDirection": TRUST_DIRECTION_CATALOG[0],
        }),
        rows[0].0,
    )
    .await;
    rows[0].1 = fresh;

    for (case, trust_id) in &rows {
        let answer = ok(
            &router,
            "VerifyTrust",
            json!({ "TrustId": trust_id }),
            case,
        )
        .await;
        assert_eq!(
            answer["TrustId"],
            json!(trust_id),
            "case {case:?}: expected VerifyTrust to report the trust it was called for, got \
             {answer}"
        );
        let started = describe_one(&router, trust_id, case).await;
        assert_eq!(
            started["TrustState"],
            json!(TRUST_STATE_VERIFYING),
            "case {case:?}: expected the transient Verifying state, got {started}"
        );
    }

    // The settled state is owed by the clock, not by the read that observes it.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    for (case, trust_id) in &rows {
        let settled = describe_one(&router, trust_id, case).await;
        assert_eq!(
            settled["TrustState"],
            json!(TRUST_STATE_VERIFIED),
            "case {case:?}: expected the settled Verified state, got {settled}"
        );
    }

    refused(
        &router,
        "VerifyTrust",
        json!({ "TrustId": "t-0123456789" }),
        "EntityDoesNotExistException",
        "a TrustId naming no stored trust",
    )
    .await;
}

#[tokio::test]
async fn deleting_a_trust_tears_it_down_and_takes_the_asked_for_forwarder() {
    let router = service();
    let directory_id = managed_microsoft_ad(&router, "corp.example.com", "deletion").await;

    // The rows differ in what the request says about the conditional forwarder the trust
    // created: asked to delete it, asked not to, and not saying at all — the member's
    // documented default, `false`.
    let rows: [(&str, &str, Option<bool>, bool); 3] = [
        (
            "the associated forwarder is asked for",
            "withforwarder.example.com",
            Some(true),
            false,
        ),
        (
            "the associated forwarder is explicitly kept",
            "keepforwarder.example.com",
            Some(false),
            true,
        ),
        (
            "the member is absent, so it defaults to false",
            "defaultforwarder.example.com",
            None,
            true,
        ),
    ];

    let mut deleted: Vec<(&str, String, &str)> = Vec::new();
    for (case, domain, delete_forwarder, forwarder_survives) in rows {
        let trust_id = create_trust(
            &router,
            json!({
                "DirectoryId": &directory_id,
                "RemoteDomainName": domain,
                "TrustPassword": TRUST_PASSWORD,
                "TrustDirection": TRUST_DIRECTION_CATALOG[2],
                "ConditionalForwarderIpAddrs": ["10.0.0.1"],
            }),
            case,
        )
        .await;
        assert!(
            forwarder_domains(&router, &directory_id, case)
                .await
                .contains(domain),
            "case {case:?}: expected the create to have stored a forwarder for {domain:?}"
        );

        let mut request = json!({ "TrustId": &trust_id });
        if let Some(flag) = delete_forwarder {
            request["DeleteAssociatedConditionalForwarder"] = json!(flag);
        }
        let answer = ok(&router, "DeleteTrust", request, case).await;
        assert_eq!(
            answer["TrustId"],
            json!(&trust_id),
            "case {case:?}: expected DeleteTrust to report the trust it was called for, got \
             {answer}"
        );

        // The teardown is answered while it is still under way, so a caller reading
        // straight afterwards still sees the relationship in the transient state.
        let tearing_down = describe_one(&router, &trust_id, case).await;
        assert_eq!(
            tearing_down["TrustState"],
            json!(TRUST_STATE_DELETING),
            "case {case:?}: expected the transient Deleting state, got {tearing_down}"
        );

        let forwarders = forwarder_domains(&router, &directory_id, case).await;
        assert_eq!(
            forwarders.contains(domain),
            forwarder_survives,
            "case {case:?}: expected the forwarder for {domain:?} to survive: \
             {forwarder_survives}, got {forwarders:?}"
        );
        deleted.push((case, trust_id, domain));
    }

    // Once the teardown has fallen due, the relationship carries the settled `Deleted`
    // state of the modelled enum — still reported, so a caller polling the teardown sees
    // how it ended — and one span later it is gone from this service altogether: in no
    // listing, and resolving it by id reports that it does not exist. One span at a time,
    // so the settled state is read before the transition that takes it away.
    tokio::time::sleep(PAST_ONE_SPAN_ONLY).await;
    let torn_down = describe(&router, json!({}), "after the teardowns fell due").await;
    for (case, trust_id, _) in &deleted {
        let reported = torn_down
            .iter()
            .find(|trust| trust["TrustId"] == json!(trust_id))
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected the torn-down trust to still be reported, got \
                     {torn_down:?}"
                )
            });
        assert_eq!(
            reported["TrustState"],
            json!(TRUST_STATE_DELETED),
            "case {case:?}: expected the settled Deleted state once the teardown fell due, \
             got {reported}"
        );
    }

    tokio::time::sleep(PAST_ONE_SPAN).await;
    let left = describe(&router, json!({}), "after the teardowns fell due").await;
    assert!(
        left.is_empty(),
        "case \"after the teardowns fell due\": expected no trust to be left, got {left:?}"
    );
    for (case, trust_id, domain) in &deleted {
        refused(
            &router,
            "DescribeTrusts",
            json!({ "TrustIds": [trust_id] }),
            "EntityDoesNotExistException",
            case,
        )
        .await;
        // And the directory can trust that external domain again: the relationship the
        // duplicate rule refused a second create for no longer exists.
        create_trust(
            &router,
            json!({
                "DirectoryId": &directory_id,
                "RemoteDomainName": domain,
                "TrustPassword": TRUST_PASSWORD,
                "TrustDirection": TRUST_DIRECTION_CATALOG[2],
            }),
            case,
        )
        .await;
    }

    refused(
        &router,
        "DeleteTrust",
        json!({ "TrustId": "t-0123456789" }),
        "EntityDoesNotExistException",
        "a TrustId naming no stored trust",
    )
    .await;
}

#[tokio::test]
async fn a_directory_holds_at_most_the_documented_number_of_trusts() {
    let router = service();
    let directory_id = managed_microsoft_ad(&router, "corp.example.com", "the trust guard").await;
    for index in 0..TRUST_GUARD {
        create_trust(
            &router,
            json!({
                "DirectoryId": &directory_id,
                "RemoteDomainName": format!("guard{index}.example.com"),
                "TrustPassword": TRUST_PASSWORD,
                "TrustDirection": TRUST_DIRECTION_CATALOG[2],
            }),
            "at the trust guard",
        )
        .await;
    }
    let held = describe(
        &router,
        json!({ "DirectoryId": &directory_id, "Limit": TRUST_GUARD }),
        "at the trust guard",
    )
    .await;
    assert_eq!(
        held.len(),
        TRUST_GUARD,
        "case \"at the trust guard\": expected {TRUST_GUARD} trusts, got {}",
        held.len()
    );

    // One past the guard is refused, and stores nothing.
    refused(
        &router,
        "CreateTrust",
        json!({
            "DirectoryId": &directory_id,
            "RemoteDomainName": "onepast.example.com",
            "TrustPassword": TRUST_PASSWORD,
            "TrustDirection": TRUST_DIRECTION_CATALOG[2],
        }),
        "ClientException",
        "one past the trust guard",
    )
    .await;
    let after = describe(
        &router,
        json!({ "DirectoryId": &directory_id, "Limit": TRUST_GUARD + 1 }),
        "one past the trust guard",
    )
    .await;
    assert_eq!(
        after.len(),
        TRUST_GUARD,
        "case \"one past the trust guard\": expected the refused create to store nothing, got {}",
        after.len()
    );

    // The guard is per directory, so another directory has its own allowance.
    let other = managed_microsoft_ad(&router, "other.example.com", "the trust guard").await;
    create_trust(
        &router,
        json!({
            "DirectoryId": &other,
            "RemoteDomainName": "onepast.example.com",
            "TrustPassword": TRUST_PASSWORD,
            "TrustDirection": TRUST_DIRECTION_CATALOG[2],
        }),
        "another directory's own allowance",
    )
    .await;
}

#[tokio::test]
async fn a_trust_whose_forwarder_has_no_slot_left_is_refused_rather_than_served_without_it() {
    let router = service();
    let directory_id = managed_microsoft_ad(&router, "corp.example.com", "the forwarder slot").await;
    // A conditional forwarder is replicated to the directory's own domain controllers, so
    // it needs a directory in service: the create's stage transition has to have fallen
    // due before the index can be filled.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    for index in 0..FORWARDER_GUARD {
        ok(
            &router,
            "CreateConditionalForwarder",
            json!({
                "DirectoryId": &directory_id,
                "RemoteDomainName": format!("filler{index}.example.com"),
                "DnsIpAddrs": ["10.1.0.1"],
            }),
            "filling the forwarder index",
        )
        .await;
    }

    // The addresses are part of what the request asked for, so a trust that would have to
    // create a forwarder there is no slot for is refused — not answered with a trust whose
    // DNS forwarding was silently dropped.
    refused(
        &router,
        "CreateTrust",
        json!({
            "DirectoryId": &directory_id,
            "RemoteDomainName": "needsaslot.example.com",
            "TrustPassword": TRUST_PASSWORD,
            "TrustDirection": TRUST_DIRECTION_CATALOG[2],
            "ConditionalForwarderIpAddrs": ["10.2.0.1"],
        }),
        "ClientException",
        "a full forwarder index and addresses that need a slot",
    )
    .await;
    let stored = describe(&router, json!({}), "after the refusal").await;
    assert!(
        stored.is_empty(),
        "case \"a full forwarder index and addresses that need a slot\": expected the refused \
         create to store no trust, got {stored:?}"
    );

    // The guard bites only what the request asked for: a trust that needs no slot is
    // accepted on the same directory — whether because it names no address at all, or
    // because the directory already forwards the domain it names.
    for (case, body) in [
        (
            "no address, so no slot is needed",
            json!({
                "DirectoryId": &directory_id,
                "RemoteDomainName": "noaddress.example.com",
                "TrustPassword": TRUST_PASSWORD,
                "TrustDirection": TRUST_DIRECTION_CATALOG[2],
            }),
        ),
        (
            "a domain the directory already forwards, so no slot is needed",
            json!({
                "DirectoryId": &directory_id,
                "RemoteDomainName": "filler0.example.com",
                "TrustPassword": TRUST_PASSWORD,
                "TrustDirection": TRUST_DIRECTION_CATALOG[2],
                "ConditionalForwarderIpAddrs": ["10.2.0.2"],
            }),
        ),
    ] {
        create_trust(&router, body, case).await;
    }
    let held = describe(&router, json!({}), "after the accepted creates").await;
    assert_eq!(
        held.len(),
        2,
        "case \"after the accepted creates\": expected the two trusts that needed no forwarder \
         slot, got {held:?}"
    );
    // The forwarder the second one named was left exactly as it was, addresses included:
    // changing a forwarder already in place is `UpdateConditionalForwarder`'s job.
    let forwarders = ok(
        &router,
        "DescribeConditionalForwarders",
        json!({ "DirectoryId": &directory_id, "RemoteDomainNames": ["filler0.example.com"] }),
        "the forwarder already in place",
    )
    .await;
    assert_eq!(
        forwarders["ConditionalForwarders"][0]["DnsIpAddrs"],
        json!(["10.1.0.1"]),
        "case \"a domain the directory already forwards\": expected the forwarder to keep its \
         addresses, got {forwarders}"
    );
}

#[tokio::test]
async fn a_deleted_directory_takes_its_trusts_with_it() {
    let router = service();
    let kept = managed_microsoft_ad(&router, "corp.example.com", "deletion").await;
    let doomed = managed_microsoft_ad(&router, "other.example.com", "deletion").await;

    let kept_trust = create_trust(
        &router,
        json!({
            "DirectoryId": &kept,
            "RemoteDomainName": "remote.example.com",
            "TrustPassword": TRUST_PASSWORD,
            "TrustDirection": TRUST_DIRECTION_CATALOG[2],
        }),
        "deletion: the trust that stays",
    )
    .await;
    let doomed_trust = create_trust(
        &router,
        json!({
            "DirectoryId": &doomed,
            "RemoteDomainName": "remote.example.com",
            "TrustPassword": TRUST_PASSWORD,
            "TrustDirection": TRUST_DIRECTION_CATALOG[2],
        }),
        "deletion: the trust that goes",
    )
    .await;

    ok(
        &router,
        "DeleteDirectory",
        json!({ "DirectoryId": &doomed }),
        "deletion",
    )
    .await;

    let left = describe(&router, json!({}), "after the directory was deleted").await;
    assert_eq!(
        reported_ids(&left),
        vec![kept_trust],
        "case \"after the directory was deleted\": expected only the other directory's trust, \
         got {left:?}"
    );
    refused(
        &router,
        "DescribeTrusts",
        json!({ "TrustIds": [doomed_trust] }),
        "EntityDoesNotExistException",
        "the deleted directory's trust",
    )
    .await;
}
