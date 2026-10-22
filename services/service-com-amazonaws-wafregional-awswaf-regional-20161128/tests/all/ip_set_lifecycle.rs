// SPDX-License-Identifier: CC0-1.0

//! The AWS WAF Classic (regional) IPSet lifecycle, end to end over the wire:
//! `GetChangeToken`, `GetChangeTokenStatus`, `CreateIPSet`, `UpdateIPSet`, `GetIPSet`,
//! `ListIPSets`, `DeleteIPSet`.
//!
//! Every call goes through the real router (see `tests/all/wire.rs`), so what these
//! tests assert is what an SDK client sees: each response is built from the persisted
//! record, not from the request.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_wafregional_awswaf_regional_20161128::state::{
    MAX_CHANGE_TOKENS, MAX_IP_SETS, MAX_IP_SET_DESCRIPTORS, MAX_UPDATES_PER_REQUEST,
};

// The wire harness is shared with the other suites in this crate.
use crate::wire::{
    call, change_token, error_code, expect_error, is_resource_id, ok_call, router, token_status,
};

/// Create an IPSet named `name` and return its `IPSetId`.
async fn create_ip_set(router: &MultiServiceRouter, name: &str) -> String {
    let token = change_token(router).await;
    let body = ok_call(
        router,
        "CreateIPSet",
        &json!({"Name": name, "ChangeToken": token}),
    )
    .await;
    body["IPSet"]["IPSetId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateIPSet returned no IPSetId: {body}"))
        .to_string()
}

/// One `IPSetUpdate` on the wire.
fn update(action: &str, kind: &str, value: &str) -> Value {
    json!({
        "Action": action,
        "IPSetDescriptor": {"Type": kind, "Value": value}
    })
}

/// Apply `updates` to `ip_set_id` with a fresh change token.
async fn apply_updates(
    router: &MultiServiceRouter,
    ip_set_id: &str,
    updates: Vec<Value>,
) -> (u16, Value) {
    let token = change_token(router).await;
    call(
        router,
        "UpdateIPSet",
        &json!({"IPSetId": ip_set_id, "ChangeToken": token, "Updates": updates}),
    )
    .await
}

/// The `(Type, Value)` pairs `GetIPSet` reports for `ip_set_id`, in stored order.
async fn descriptors(router: &MultiServiceRouter, ip_set_id: &str) -> Vec<(String, String)> {
    let body = ok_call(router, "GetIPSet", &json!({"IPSetId": ip_set_id})).await;
    body["IPSet"]["IPSetDescriptors"]
        .as_array()
        .unwrap_or_else(|| panic!("GetIPSet returned no IPSetDescriptors: {body}"))
        .iter()
        .map(|descriptor| {
            (
                descriptor["Type"].as_str().unwrap_or_default().to_string(),
                descriptor["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The change-token protocol every mutating AWS WAF Classic call follows.
///
/// The rules under test are `GetChangeToken`'s own: a second `GetChangeToken` before
/// any create/update/delete "returns the same value as the first"; using a token in a
/// mutating request moves it to `PENDING` "which indicates that AWS WAF is propagating
/// the change"; propagation completing makes it `INSYNC`; and "each create, update, or
/// delete request must use a unique change token", so a used one is stale data.
#[tokio::test]
async fn change_tokens_are_minted_persisted_and_consumed_exactly_once() {
    let router = router();

    let first = change_token(&router).await;
    assert!(
        is_resource_id(&first),
        "GetChangeToken must mint a ChangeToken value, got {first:?}"
    );
    let second = change_token(&router).await;
    assert_eq!(
        second, first,
        "a second GetChangeToken before any mutating call must return the first token, \
         got {second:?} after {first:?}"
    );
    assert_eq!(
        token_status(&router, &first).await,
        "PROVISIONED",
        "case freshly minted: a token no mutating call has used is PROVISIONED"
    );

    let created = ok_call(
        &router,
        "CreateIPSet",
        &json!({"Name": "token-probe", "ChangeToken": first}),
    )
    .await;
    assert_eq!(
        created["ChangeToken"].as_str(),
        Some(first.as_str()),
        "CreateIPSet must report the change token it consumed: {created}"
    );
    assert_eq!(
        token_status(&router, &first).await,
        "PENDING",
        "case just consumed: propagation has not finished yet, so the token is PENDING"
    );

    let third = change_token(&router).await;
    assert_ne!(
        third, first,
        "once a token is consumed GetChangeToken must mint a new one, got {third:?}"
    );
    assert_eq!(
        token_status(&router, &third).await,
        "PROVISIONED",
        "case newly minted after a consumption: the new token is PROVISIONED"
    );

    // A token a mutating call already used, and a token this service never minted,
    // are both unusable: "each create, update, or delete request must use a unique
    // change token".
    for (label, token) in [
        ("already consumed", first.clone()),
        ("never minted", "11111111-2222-4333-8444-555555555555".to_string()),
    ] {
        expect_error(
            &router,
            "CreateIPSet",
            &json!({"Name": "stale-token-probe", "ChangeToken": token}),
            "WAFStaleDataException",
            label,
        )
        .await;
    }

    expect_error(
        &router,
        "GetChangeTokenStatus",
        &json!({"ChangeToken": "11111111-2222-4333-8444-555555555555"}),
        "WAFNonexistentItemException",
        "status of a token that was never minted",
    )
    .await;

    // The PENDING -> INSYNC step is owed by the CLOCK, so it lands without any further
    // call being made. Nothing in the handler sleeps for it.
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert_eq!(
        token_status(&router, &first).await,
        "INSYNC",
        "case propagation elapsed: the token must settle to INSYNC"
    );
}

/// `CreateIPSet` stores a new, empty IPSet under a minted id, and `GetIPSet` reads
/// that record back — across a table of names differing in length, case and the
/// characters they use. `ResourceName` carries no uniqueness constraint, so the last
/// two rows share a name and must still be two distinct resources.
#[tokio::test]
async fn create_then_get_reads_back_the_stored_ip_set() {
    let router = router();
    let max_length_name = "n".repeat(128);
    let cases: &[(&str, &str)] = &[
        // (label, Name) — `ResourceName` is len 1..128, pattern \S.
        ("the issue's own name", "bootstrap-ipset"),
        ("one character (the minimum length)", "x"),
        ("128 characters (the maximum length)", max_length_name.as_str()),
        ("mixed case, digits, separators", "Prod_IPs-2"),
        ("an interior space", "corp office ranges"),
        ("a name shared with the next row", "shared-name"),
        ("the same name again", "shared-name"),
    ];

    let mut seen_ids: BTreeSet<String> = BTreeSet::new();
    for (label, name) in cases {
        let token = change_token(&router).await;
        let created = ok_call(
            &router,
            "CreateIPSet",
            &json!({"Name": name, "ChangeToken": token}),
        )
        .await;

        let ip_set_id = created["IPSet"]["IPSetId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label}: no IPSetId was returned: {created}"))
            .to_string();
        assert!(
            is_resource_id(&ip_set_id),
            "case {label}: expected a ResourceId, got {ip_set_id:?}"
        );
        assert!(
            seen_ids.insert(ip_set_id.clone()),
            "case {label}: CreateIPSet reused the id {ip_set_id}"
        );
        assert_eq!(
            created["IPSet"]["Name"].as_str(),
            Some(*name),
            "case {label}: CreateIPSet must report the stored name: {created}"
        );
        assert_eq!(
            created["IPSet"]["IPSetDescriptors"].as_array().map(Vec::len),
            Some(0),
            "case {label}: a new IPSet holds no descriptors: {created}"
        );

        let fetched = ok_call(&router, "GetIPSet", &json!({"IPSetId": ip_set_id})).await;
        assert_eq!(
            fetched["IPSet"]["IPSetId"].as_str(),
            Some(ip_set_id.as_str()),
            "case {label}: GetIPSet must answer for the id it was given: {fetched}"
        );
        assert_eq!(
            fetched["IPSet"]["Name"].as_str(),
            Some(*name),
            "case {label}: GetIPSet must read the stored name back: {fetched}"
        );
        assert_eq!(
            fetched["IPSet"]["IPSetDescriptors"].as_array().map(Vec::len),
            Some(0),
            "case {label}: GetIPSet must read back an empty descriptor list: {fetched}"
        );
    }
}

/// `UpdateIPSet` applies its updates to the stored IPSet — across a table of
/// descriptors spanning both address families and every prefix length AWS WAF
/// documents, inserted and then deleted again.
#[tokio::test]
async fn update_inserts_and_deletes_every_documented_prefix_length() {
    let router = router();
    // Source: `UpdateIPSet`'s documentation — "AWS WAF supports IPv4 address ranges:
    // /8 and any range between /16 through /32. AWS WAF supports IPv6 address ranges:
    // /24, /32, /48, /56, /64, and /128."
    let cases: &[(&str, &str, &str)] = &[
        ("IPv4 host route", "IPV4", "192.0.2.44/32"),
        ("IPv4 /24 (the issue's own row)", "IPV4", "198.51.100.0/24"),
        ("IPv4 at the lower end of 16..32", "IPV4", "203.0.113.0/16"),
        ("IPv4 /8, the one range below /16", "IPV4", "10.0.0.0/8"),
        ("IPv6 host route", "IPV6", "2001:db8::1/128"),
        ("IPv6 /64", "IPV6", "2001:db8:1:2::/64"),
        ("IPv6 /56", "IPV6", "2001:db8:3::/56"),
        ("IPv6 /48", "IPV6", "2001:db8:4::/48"),
        ("IPv6 /32", "IPV6", "2001:db9::/32"),
        ("IPv6 /24, the shortest supported", "IPV6", "2001:d00::/24"),
    ];

    let ip_set_id = create_ip_set(&router, "every-prefix-length").await;
    let mut expected: Vec<(String, String)> = Vec::new();
    for (label, kind, value) in cases {
        let (status, body) = apply_updates(&router, &ip_set_id, vec![update("INSERT", kind, value)]).await;
        assert_eq!(
            status, 200,
            "case {label}: inserting {value} must be accepted, got {status}: {body}"
        );
        expected.push((kind.to_string(), value.to_string()));
        assert_eq!(
            descriptors(&router, &ip_set_id).await,
            expected,
            "case {label}: the stored descriptors must be the inserted ones, in order"
        );
    }

    // Deleting them again empties the set, one row at a time.
    for (label, kind, value) in cases {
        let (status, body) = apply_updates(&router, &ip_set_id, vec![update("DELETE", kind, value)]).await;
        assert_eq!(
            status, 200,
            "case {label}: deleting {value} must be accepted, got {status}: {body}"
        );
        expected.remove(0);
        assert_eq!(
            descriptors(&router, &ip_set_id).await,
            expected,
            "case {label}: deleting {value} must remove exactly that descriptor"
        );
    }
    assert!(
        descriptors(&router, &ip_set_id).await.is_empty(),
        "every inserted descriptor was deleted again, so the set must be empty"
    );
}

/// A descriptor's identity is the address and prefix length it denotes, not the text
/// it was written as: `UpdateIPSet` documents four spellings of one IPv6 address. An
/// update with nothing to do — an insert of a descriptor already held, a delete of one
/// not held — is `WAFInvalidOperationException`, and applies nothing.
#[tokio::test]
async fn one_address_is_one_descriptor_however_it_is_spelled() {
    let router = router();
    // Source: `UpdateIPSet`'s documentation lists these as formats of the same
    // address: 1111:0000:0000:0000:0000:0000:0000:0111/128, 1111:0:0:0:0:0:0:0111/128,
    // 1111::0111/128, 1111::111/128.
    let spellings = [
        "1111:0000:0000:0000:0000:0000:0000:0111/128",
        "1111:0:0:0:0:0:0:0111/128",
        "1111::0111/128",
        "1111::111/128",
    ];

    let ip_set_id = create_ip_set(&router, "one-address-many-spellings").await;
    let (status, body) = apply_updates(
        &router,
        &ip_set_id,
        vec![update("INSERT", "IPV6", spellings[0])],
    )
    .await;
    assert_eq!(
        status, 200,
        "inserting {:?} must be accepted, got {status}: {body}",
        spellings[0]
    );
    // Stored as written: the response is the record, not a re-rendering of it.
    assert_eq!(
        descriptors(&router, &ip_set_id).await,
        vec![("IPV6".to_string(), spellings[0].to_string())],
        "the descriptor must be stored exactly as it was written"
    );

    for spelling in &spellings {
        let token = change_token(&router).await;
        expect_error(
            &router,
            "UpdateIPSet",
            &json!({
                "IPSetId": ip_set_id,
                "ChangeToken": token,
                "Updates": [update("INSERT", "IPV6", spelling)]
            }),
            "WAFInvalidOperationException",
            &format!("re-inserting the same address as {spelling}"),
        )
        .await;
        assert_eq!(
            token_status(&router, &token).await,
            "PROVISIONED",
            "case {spelling}: a refused update must not consume the change token"
        );
    }

    // Any spelling deletes it, and only the first delete has anything to do.
    let (status, body) = apply_updates(
        &router,
        &ip_set_id,
        vec![update("DELETE", "IPV6", spellings[3])],
    )
    .await;
    assert_eq!(
        status, 200,
        "deleting the address as {:?} must be accepted, got {status}: {body}",
        spellings[3]
    );
    assert!(
        descriptors(&router, &ip_set_id).await.is_empty(),
        "deleting the one descriptor must empty the set"
    );
    for spelling in &spellings {
        let token = change_token(&router).await;
        expect_error(
            &router,
            "UpdateIPSet",
            &json!({
                "IPSetId": ip_set_id,
                "ChangeToken": token,
                "Updates": [update("DELETE", "IPV6", spelling)]
            }),
            "WAFInvalidOperationException",
            &format!("deleting an address the set does not hold, as {spelling}"),
        )
        .await;
    }
}

/// One request carrying several updates applies them in order — and is applied or
/// refused as a unit, so a request whose last update has nothing to do leaves the
/// stored IPSet and the change token untouched.
#[tokio::test]
async fn a_multi_update_request_is_applied_as_a_unit() {
    let router = router();
    let ip_set_id = create_ip_set(&router, "batched-updates").await;

    let (status, body) = apply_updates(
        &router,
        &ip_set_id,
        vec![
            update("INSERT", "IPV4", "192.0.2.0/24"),
            update("INSERT", "IPV6", "2001:db8::/32"),
            update("INSERT", "IPV4", "192.0.2.44/32"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case three inserts: expected 200, got {status}: {body}"
    );
    assert_eq!(
        descriptors(&router, &ip_set_id).await,
        vec![
            ("IPV4".to_string(), "192.0.2.0/24".to_string()),
            ("IPV6".to_string(), "2001:db8::/32".to_string()),
            ("IPV4".to_string(), "192.0.2.44/32".to_string()),
        ],
        "case three inserts: the descriptors must be stored in insertion order"
    );

    // A delete and an insert in one request: changing an address is "delete the
    // existing IP address and add the new one".
    let (status, body) = apply_updates(
        &router,
        &ip_set_id,
        vec![
            update("DELETE", "IPV6", "2001:db8::/32"),
            update("INSERT", "IPV6", "2001:db8:1::/48"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case delete-then-insert: expected 200, got {status}: {body}"
    );
    assert_eq!(
        descriptors(&router, &ip_set_id).await,
        vec![
            ("IPV4".to_string(), "192.0.2.0/24".to_string()),
            ("IPV4".to_string(), "192.0.2.44/32".to_string()),
            ("IPV6".to_string(), "2001:db8:1::/48".to_string()),
        ],
        "case delete-then-insert: both updates must be applied, in order"
    );

    // The same descriptor inserted twice in ONE request: the second has nothing to do,
    // so the whole request — including its first, valid update — applies nothing.
    let before = descriptors(&router, &ip_set_id).await;
    let token = change_token(&router).await;
    expect_error(
        &router,
        "UpdateIPSet",
        &json!({
            "IPSetId": ip_set_id,
            "ChangeToken": token,
            "Updates": [
                update("INSERT", "IPV4", "203.0.113.0/24"),
                update("INSERT", "IPV4", "203.0.113.0/24"),
            ]
        }),
        "WAFInvalidOperationException",
        "one request inserting the same descriptor twice",
    )
    .await;
    assert_eq!(
        descriptors(&router, &ip_set_id).await,
        before,
        "a refused request must apply none of its updates"
    );
    assert_eq!(
        token_status(&router, &token).await,
        "PROVISIONED",
        "a refused request must not consume the change token"
    );
}

/// An address AWS WAF cannot hold — a prefix length outside the documented ranges, a
/// value that is not a CIDR, or a family that disagrees with the address — is
/// `WAFInvalidParameterException`, and nothing is stored.
#[tokio::test]
async fn update_refuses_values_outside_the_documented_cidr_rule() {
    let router = router();
    let ip_set_id = create_ip_set(&router, "rejected-values").await;
    // Every row is one step past a published bound, or not a CIDR at all. Source: the
    // supported ranges in `UpdateIPSet`'s documentation (IPv4 /8 and /16../32; IPv6
    // /24, /32, /48, /56, /64, /128).
    let cases: &[(&str, &str, &str)] = &[
        ("IPv4 /0, below every supported range", "IPV4", "0.0.0.0/0"),
        ("IPv4 /7, one below /8", "IPV4", "10.0.0.0/7"),
        ("IPv4 /9, one above /8 and below /16", "IPV4", "10.0.0.0/9"),
        ("IPv4 /15, one below /16", "IPV4", "10.0.0.0/15"),
        ("IPv4 /33, one above /32", "IPV4", "192.0.2.44/33"),
        ("IPv4 prefix length out of byte range", "IPV4", "192.0.2.44/300"),
        ("IPv6 /0", "IPV6", "::/0"),
        ("IPv6 /23, one below /24", "IPV6", "2001:d00::/23"),
        ("IPv6 /25, one above /24", "IPV6", "2001:d00::/25"),
        ("IPv6 /127, one below /128", "IPV6", "2001:db8::1/127"),
        ("IPv6 /129, one above /128", "IPV6", "2001:db8::1/129"),
        ("an IPv6 address declared IPV4", "IPV4", "2001:db8::1/128"),
        ("an IPv4 address declared IPV6", "IPV6", "192.0.2.0/24"),
        ("no prefix length at all", "IPV4", "192.0.2.0"),
        ("an empty prefix length", "IPV4", "192.0.2.0/"),
        ("a negative prefix length", "IPV4", "192.0.2.0/-1"),
        ("not an address", "IPV4", "not-an-address/24"),
        ("an IPv4 octet out of range", "IPV4", "300.0.2.0/24"),
    ];

    for (label, kind, value) in cases {
        let token = change_token(&router).await;
        let body = expect_error(
            &router,
            "UpdateIPSet",
            &json!({
                "IPSetId": ip_set_id,
                "ChangeToken": token,
                "Updates": [update("INSERT", kind, value)]
            }),
            "WAFInvalidParameterException",
            label,
        )
        .await;
        assert_eq!(
            body["field"].as_str(),
            Some("IPSET_TYPE"),
            "case {label}: the refusal must name the member it is about: {body}"
        );
        assert_eq!(
            body["parameter"].as_str(),
            Some(*value),
            "case {label}: the refusal must name the value it refused: {body}"
        );
        assert!(
            descriptors(&router, &ip_set_id).await.is_empty(),
            "case {label}: a refused update must store nothing"
        );
        assert_eq!(
            token_status(&router, &token).await,
            "PROVISIONED",
            "case {label}: a refused update must not consume the change token"
        );
    }

    // An action other than INSERT or DELETE is refused the same way, naming the
    // `CHANGE_ACTION` field.
    let token = change_token(&router).await;
    let body = expect_error(
        &router,
        "UpdateIPSet",
        &json!({
            "IPSetId": ip_set_id,
            "ChangeToken": token,
            "Updates": [update("REPLACE", "IPV4", "192.0.2.0/24")]
        }),
        "Exception",
        "an action other than INSERT or DELETE",
    )
    .await;
    assert!(
        error_code(&body).contains("WAFInvalidParameterException")
            || error_code(&body).contains("ValidationException"),
        "case an action other than INSERT or DELETE: expected the request to be refused \
         as invalid input, got {}",
        error_code(&body)
    );
}

/// "You can insert a maximum of 1000 addresses in a single request": a request at that
/// bound is accepted and one past it is `WAFLimitsExceededException`.
#[tokio::test]
async fn update_refuses_more_updates_than_one_request_may_carry() {
    let router = router();
    let ip_set_id = create_ip_set(&router, "bulk-insert").await;

    // 10.0.0.0/32 .. counting upwards: distinct host routes, so every insert has
    // something to do.
    let addresses = |count: usize| -> Vec<Value> {
        (0..count)
            .map(|n| {
                let value = format!("10.{}.{}.{}/32", (n >> 16) & 0xff, (n >> 8) & 0xff, n & 0xff);
                update("INSERT", "IPV4", &value)
            })
            .collect()
    };

    let (status, body) = apply_updates(&router, &ip_set_id, addresses(MAX_UPDATES_PER_REQUEST + 1)).await;
    assert_eq!(
        status, 400,
        "case one past the bound: {} updates must be refused, got {status}: {body}",
        MAX_UPDATES_PER_REQUEST + 1
    );
    assert!(
        error_code(&body).contains("WAFLimitsExceededException"),
        "case one past the bound: expected WAFLimitsExceededException, got {}",
        error_code(&body)
    );
    assert!(
        descriptors(&router, &ip_set_id).await.is_empty(),
        "case one past the bound: the refused request must store nothing"
    );

    let (status, body) = apply_updates(&router, &ip_set_id, addresses(MAX_UPDATES_PER_REQUEST)).await;
    assert_eq!(
        status, 200,
        "case at the bound: {MAX_UPDATES_PER_REQUEST} updates must be accepted, got {status}: {body}"
    );
    assert_eq!(
        descriptors(&router, &ip_set_id).await.len(),
        MAX_UPDATES_PER_REQUEST,
        "case at the bound: every inserted descriptor must be stored"
    );
}

/// `ListIPSets` pages the stored IPSets: `Limit` bounds the page, `NextMarker` resumes
/// it, and walking the pages yields every stored IPSet exactly once.
#[tokio::test]
async fn list_ip_sets_pages_by_limit_and_marker() {
    let router = router();
    let mut created: Vec<(String, String)> = Vec::new();
    for n in 0..7 {
        let name = format!("page-probe-{n}");
        let id = create_ip_set(&router, &name).await;
        created.push((id, name));
    }
    created.sort();

    // `Limit` 0 is the model's declared default and asks for the service's own page
    // size; 100 is its `@range` maximum. Each row walks every page.
    for limit in [0_usize, 1, 2, 3, 7, 100] {
        let mut seen: Vec<(String, String)> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"Limit": limit});
            if let Some(marker) = &marker {
                request["NextMarker"] = json!(marker);
            }
            let page = ok_call(&router, "ListIPSets", &request).await;
            let ip_sets = page["IPSets"]
                .as_array()
                .unwrap_or_else(|| panic!("case Limit {limit}: no IPSets array: {page}"))
                .clone();
            if limit > 0 {
                assert!(
                    ip_sets.len() <= limit,
                    "case Limit {limit}: a page must hold at most {limit} summaries, got {}",
                    ip_sets.len()
                );
            }
            for summary in &ip_sets {
                let id = summary["IPSetId"].as_str().unwrap_or_default().to_string();
                let name = summary["Name"].as_str().unwrap_or_default().to_string();
                assert!(
                    is_resource_id(&id),
                    "case Limit {limit}: {id:?} is not a ResourceId"
                );
                seen.push((id, name));
            }
            marker = page["NextMarker"].as_str().map(str::to_string);
            pages += 1;
            // The marker is present exactly when an IPSet remains past this page.
            assert_eq!(
                marker.is_some(),
                seen.len() < created.len(),
                "case Limit {limit}: NextMarker must be present only while IPSets remain: {page}"
            );
            if marker.is_none() {
                break;
            }
            assert!(
                pages <= created.len() + 1,
                "case Limit {limit}: paging did not terminate"
            );
        }
        assert_eq!(
            seen, created,
            "case Limit {limit}: paging must yield every stored IPSet exactly once, in id order"
        );
    }

    // `PaginationLimit`'s modelled `@range` is 0..100, so one past either bound is
    // refused before the handler runs.
    for limit in [-1_i64, 101] {
        let (status, body) = call(&router, "ListIPSets", &json!({"Limit": limit})).await;
        assert_eq!(
            status, 400,
            "case Limit {limit}: a value outside 0..100 must be refused, got {status}: {body}"
        );
    }
}

/// `DeleteIPSet` permanently removes the record — but only once it holds no addresses,
/// and only for an IPSet that exists.
#[tokio::test]
async fn delete_requires_an_empty_ip_set_and_then_removes_it() {
    let router = router();
    let doomed = create_ip_set(&router, "doomed").await;
    let survivor = create_ip_set(&router, "survivor").await;

    let (status, body) = apply_updates(&router, &doomed, vec![update("INSERT", "IPV4", "192.0.2.0/24")]).await;
    assert_eq!(status, 200, "inserting a descriptor: got {status}: {body}");

    // "You can't delete an IPSet … if it still includes any IP addresses."
    let token = change_token(&router).await;
    expect_error(
        &router,
        "DeleteIPSet",
        &json!({"IPSetId": doomed, "ChangeToken": token}),
        "WAFNonEmptyEntityException",
        "a set that still holds an address",
    )
    .await;
    assert_eq!(
        descriptors(&router, &doomed).await,
        vec![("IPV4".to_string(), "192.0.2.0/24".to_string())],
        "a refused delete must leave the record alone"
    );
    assert_eq!(
        token_status(&router, &token).await,
        "PROVISIONED",
        "a refused delete must not consume the change token"
    );

    let (status, body) = apply_updates(&router, &doomed, vec![update("DELETE", "IPV4", "192.0.2.0/24")]).await;
    assert_eq!(status, 200, "emptying the set: got {status}: {body}");

    let token = change_token(&router).await;
    let deleted = ok_call(
        &router,
        "DeleteIPSet",
        &json!({"IPSetId": doomed, "ChangeToken": token}),
    )
    .await;
    assert_eq!(
        deleted["ChangeToken"].as_str(),
        Some(token.as_str()),
        "DeleteIPSet must report the change token it consumed: {deleted}"
    );

    expect_error(
        &router,
        "GetIPSet",
        &json!({"IPSetId": doomed}),
        "WAFNonexistentItemException",
        "reading a deleted IPSet",
    )
    .await;
    let listed = ok_call(&router, "ListIPSets", &json!({"Limit": 100})).await;
    let remaining: Vec<&str> = listed["IPSets"]
        .as_array()
        .expect("an IPSets array")
        .iter()
        .map(|summary| summary["IPSetId"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        remaining,
        vec![survivor.as_str()],
        "a deleted IPSet must not be listed any more: {listed}"
    );

    // Deleting it again, and updating it, now fail: the record is gone.
    let token = change_token(&router).await;
    expect_error(
        &router,
        "DeleteIPSet",
        &json!({"IPSetId": doomed, "ChangeToken": token}),
        "WAFNonexistentItemException",
        "deleting an IPSet twice",
    )
    .await;
    expect_error(
        &router,
        "UpdateIPSet",
        &json!({
            "IPSetId": doomed,
            "ChangeToken": token,
            "Updates": [update("INSERT", "IPV4", "192.0.2.0/24")]
        }),
        "WAFNonexistentContainerException",
        "updating a deleted IPSet",
    )
    .await;
}

/// An id this service holds no IPSet for is answered with the modelled not-found
/// error for each operation — never a 501 and never a hollow 200.
#[tokio::test]
async fn an_unknown_ip_set_id_is_a_modelled_not_found() {
    let router = router();
    // Well-formed `ResourceId` values (len 1..128, pattern \S) that were never created.
    let never_created: &[(&str, &str)] = &[
        ("UUID-shaped", "deadbeef-0000-4000-8000-000000000000"),
        ("one character", "x"),
        ("a plain string, not a UUID", "i-am-not-an-ip-set"),
    ];

    for (label, ip_set_id) in never_created {
        expect_error(
            &router,
            "GetIPSet",
            &json!({"IPSetId": ip_set_id}),
            "WAFNonexistentItemException",
            label,
        )
        .await;

        let token = change_token(&router).await;
        // The IPSet is the CONTAINER an update goes into: "You tried to add an IP
        // address to or delete an IP address from an IPSet that doesn't exist."
        expect_error(
            &router,
            "UpdateIPSet",
            &json!({
                "IPSetId": ip_set_id,
                "ChangeToken": token,
                "Updates": [update("INSERT", "IPV4", "192.0.2.0/24")]
            }),
            "WAFNonexistentContainerException",
            label,
        )
        .await;
        expect_error(
            &router,
            "DeleteIPSet",
            &json!({"IPSetId": ip_set_id, "ChangeToken": token}),
            "WAFNonexistentItemException",
            label,
        )
        .await;
        assert_eq!(
            token_status(&router, &token).await,
            "PROVISIONED",
            "case {label}: a refused call must not consume the change token"
        );
    }
}

/// The store a caller can grow is bounded: at the account's IPSet quota `CreateIPSet`
/// answers its modelled `WAFLimitsExceededException`, and deleting one makes room
/// again.
#[tokio::test]
async fn create_refuses_past_the_ip_set_quota() {
    let router = router();
    let mut first: Option<String> = None;
    for n in 0..MAX_IP_SETS {
        let id = create_ip_set(&router, &format!("quota-probe-{n}")).await;
        first = first.or(Some(id));
    }
    let first = first.expect("the first IPSet's id");

    let token = change_token(&router).await;
    expect_error(
        &router,
        "CreateIPSet",
        &json!({"Name": "one-too-many", "ChangeToken": token}),
        "WAFLimitsExceededException",
        "at the quota",
    )
    .await;
    assert_eq!(
        token_status(&router, &token).await,
        "PROVISIONED",
        "case at the quota: a refused create must not consume the change token"
    );

    let token = change_token(&router).await;
    ok_call(
        &router,
        "DeleteIPSet",
        &json!({"IPSetId": first, "ChangeToken": token}),
    )
    .await;
    let token = change_token(&router).await;
    ok_call(
        &router,
        "CreateIPSet",
        &json!({"Name": "room-again", "ChangeToken": token}),
    )
    .await;
}

/// One IPSet holds at most the account's per-IPSet address quota: filling it is
/// accepted, and the next distinct address is `WAFLimitsExceededException`, so the
/// descriptor state a caller can accumulate over many requests is bounded too.
#[tokio::test]
async fn update_refuses_past_the_per_ip_set_address_quota() {
    let router = router();
    let ip_set_id = create_ip_set(&router, "quota-of-addresses").await;

    // Fill the set to its bound, one maximum-size request at a time: 10.0.x.y/32 host
    // routes are distinct, so every insert has something to do.
    let mut inserted = 0;
    while inserted < MAX_IP_SET_DESCRIPTORS {
        let batch = MAX_UPDATES_PER_REQUEST.min(MAX_IP_SET_DESCRIPTORS - inserted);
        let updates: Vec<Value> = (inserted..inserted + batch)
            .map(|n| {
                let value = format!("10.{}.{}.{}/32", (n >> 16) & 0xff, (n >> 8) & 0xff, n & 0xff);
                update("INSERT", "IPV4", &value)
            })
            .collect();
        let (status, body) = apply_updates(&router, &ip_set_id, updates).await;
        assert_eq!(
            status, 200,
            "case filling to {}: expected 200, got {status}: {body}",
            inserted + batch
        );
        inserted += batch;
    }
    assert_eq!(
        descriptors(&router, &ip_set_id).await.len(),
        MAX_IP_SET_DESCRIPTORS,
        "case at the bound: every inserted address must be stored"
    );

    let token = change_token(&router).await;
    expect_error(
        &router,
        "UpdateIPSet",
        &json!({
            "IPSetId": ip_set_id,
            "ChangeToken": token,
            "Updates": [update("INSERT", "IPV4", "11.0.0.1/32")]
        }),
        "WAFLimitsExceededException",
        "one address past the per-IPSet bound",
    )
    .await;
    assert_eq!(
        descriptors(&router, &ip_set_id).await.len(),
        MAX_IP_SET_DESCRIPTORS,
        "case one past the bound: the refused request must store nothing"
    );
    assert_eq!(
        token_status(&router, &token).await,
        "PROVISIONED",
        "case one past the bound: a refused update must not consume the change token"
    );

    // Removing one address makes room for one more.
    let (status, body) = apply_updates(
        &router,
        &ip_set_id,
        vec![update("DELETE", "IPV4", "10.0.0.0/32")],
    )
    .await;
    assert_eq!(status, 200, "freeing one slot: got {status}: {body}");
    let (status, body) = apply_updates(
        &router,
        &ip_set_id,
        vec![update("INSERT", "IPV4", "11.0.0.1/32")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case back under the bound: expected 200, got {status}: {body}"
    );
}

/// A change token is a short-lived record on the real service too, so the token store
/// is bounded as well: once more tokens than it keeps have been consumed, the oldest
/// records are gone and their status is a `WAFNonexistentItemException`, while a
/// recently consumed token is still reported.
#[tokio::test]
async fn the_change_token_store_is_bounded() {
    let router = router();
    let ip_set_id = create_ip_set(&router, "token-churn").await;

    let oldest = change_token(&router).await;
    let (status, body) = call(
        &router,
        "UpdateIPSet",
        &json!({
            "IPSetId": ip_set_id,
            "ChangeToken": oldest,
            "Updates": [update("INSERT", "IPV4", "192.0.2.0/24")]
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the first update must be accepted, got {status}: {body}"
    );
    assert_eq!(
        token_status(&router, &oldest).await,
        "PENDING",
        "the first token must be consumed by that update"
    );

    // Each churn update toggles the one descriptor, so every one of them has something
    // to do and consumes its own token.
    let mut newest = oldest.clone();
    for n in 0..MAX_CHANGE_TOKENS {
        let action = if n % 2 == 0 { "DELETE" } else { "INSERT" };
        newest = change_token(&router).await;
        let (status, body) = call(
            &router,
            "UpdateIPSet",
            &json!({
                "IPSetId": ip_set_id,
                "ChangeToken": newest,
                "Updates": [update(action, "IPV4", "192.0.2.0/24")]
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case churn {n} ({action}): expected 200, got {status}: {body}"
        );
    }

    expect_error(
        &router,
        "GetChangeTokenStatus",
        &json!({"ChangeToken": oldest}),
        "WAFNonexistentItemException",
        "a token older than the store's bound",
    )
    .await;
    let status = token_status(&router, &newest).await;
    assert!(
        status == "PENDING" || status == "INSYNC",
        "case the most recently consumed token: expected PENDING or INSYNC, got {status:?}"
    );
}
