// SPDX-License-Identifier: CC0-1.0

//! The membership of an address list, end to end over the wire:
//! `RegisterMemberToAddressList`, `DeregisterMemberFromAddressList`,
//! `GetMemberOfAddressList` and `ListMembersOfAddressList`.
//!
//! Every assertion here goes through the real request path — `MultiServiceRouter::dispatch`
//! with `awsJson1_0` bodies, `POST /` and `X-Amz-Target: MailManagerSvc.<Operation>` — so
//! what it pins is the emulator's HTTP behaviour, not an internal helper.
//!
//! The tests are tables: each row is one input drawn from the class the rule covers —
//! addresses whose domain and local part differ in letter case, bare domains, both ends of
//! `Address`'s modelled 3..320 length, several filter prefixes, every page size around a
//! known membership, and the per-list membership quota.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_mailmanager_mailmanagersvc::state::MAX_ADDRESS_LIST_MEMBERS;
use service_com_amazonaws_mailmanager_mailmanagersvc::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for.
const REGION: &str = "eu-west-1";

/// One emulator holding one MailManager service on one shared engine.
struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The signature itself is not under test here, and the fixed test credential
        // below is not this process's signing key.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Call one operation and return `(http status, parsed body)`.
    async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("MailManagerSvc.{operation}"))
            // A signed SDK request routes by its SigV4 credential scope, whose 4th field
            // is this service's signing name (`ses`).
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261021/{REGION}/ses/aws4_request, \
                     SignedHeaders=host;x-amz-target, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a valid HTTP request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            json!({})
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Create an address list and return its `AddressListId`, failing the test on any
    /// refusal.
    async fn create_list(&self, name: &str) -> String {
        let (status, out) = self
            .call("CreateAddressList", json!({ "AddressListName": name }))
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected CreateAddressList to answer 200, got {status} {out:?}"
        );
        out["AddressListId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: no AddressListId in {out:?}"))
            .to_string()
    }

    /// Deregister one address, failing the test on any refusal, and assert the answer
    /// carries no members (`DeregisterMemberFromAddressListResponse` models none).
    async fn deregister(&self, address_list_id: &str, address: &str) {
        let (status, out) = self
            .call(
                "DeregisterMemberFromAddressList",
                json!({ "AddressListId": address_list_id, "Address": address }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {address:?}: expected DeregisterMemberFromAddressList to answer 200, got \
             {status} {out:?}"
        );
        assert_eq!(
            out,
            json!({}),
            "case {address:?}: the response models no members, got {out:?}"
        );
    }

    /// Register one address, failing the test on any refusal.
    async fn register(&self, address_list_id: &str, address: &str) {
        let (status, out) = self
            .call(
                "RegisterMemberToAddressList",
                json!({ "AddressListId": address_list_id, "Address": address }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {address:?}: expected RegisterMemberToAddressList to answer 200, got {status} \
             {out:?}"
        );
    }

    /// One page of `ListMembersOfAddressList`: the addresses it reports, and its
    /// `NextToken`.
    async fn list_page(&self, body: Value) -> (Vec<String>, Option<String>) {
        let (status, out) = self.call("ListMembersOfAddressList", body.clone()).await;
        assert_eq!(
            status, 200,
            "case {body:?}: expected ListMembersOfAddressList to answer 200, got {status} {out:?}"
        );
        let addresses = out["Addresses"]
            .as_array()
            .unwrap_or_else(|| panic!("case {body:?}: no Addresses list in {out:?}"))
            .iter()
            .map(|entry| {
                assert!(
                    entry["CreatedTimestamp"].as_f64().is_some_and(|t| t > 0.0),
                    "case {body:?}: every member must carry a real CreatedTimestamp, got {entry:?}"
                );
                entry["Address"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {body:?}: a member with no Address: {entry:?}"))
                    .to_string()
            })
            .collect();
        let next = out["NextToken"].as_str().map(str::to_string);
        (addresses, next)
    }

    /// Every member of an address list, walked with `PageSize` pages until the walk ends.
    async fn list_all(&self, address_list_id: &str, page_size: usize) -> Vec<String> {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut body = json!({ "AddressListId": address_list_id, "PageSize": page_size });
            if let Some(next) = &token {
                body["NextToken"] = json!(next);
            }
            let (page, next) = self.list_page(body).await;
            walked.extend(page);
            match next {
                Some(next) => token = Some(next),
                None => return walked,
            }
        }
    }
}

/// The wire error code of a failure body: the `__type` member, without its namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .split('#')
        .next_back()
        .unwrap_or_default()
        .to_string()
}

/// The longest address `Address`'s modelled 3..320 length admits, spelled with an
/// upper-case domain; `.1` is how it must be stored.
fn longest_address() -> (String, String) {
    let domain = "EXAMPLE.COM";
    let local = "u".repeat(320 - 1 - domain.len());
    (
        format!("{local}@{domain}"),
        format!("{local}@{}", domain.to_lowercase()),
    )
}

/// RULE: an address is stored, reported and looked up with its DOMAIN folded to lower case
/// and its LOCAL PART exactly as it was spelled; a bare domain has no local part, so it
/// folds whole.
///
/// Rows span the class: an already-lower address, an upper-case domain, an upper-case local
/// part, mixed case in both, a multi-label domain, bare domains in two casings, the
/// shortest value the modelled 3..320 length admits and the longest.
#[tokio::test]
async fn register_then_get_folds_the_domain_and_keeps_the_local_part() {
    let harness = Harness::new();
    let list = harness.create_list("folding").await;

    let (longest, longest_stored) = longest_address();
    // (case, as registered, as stored)
    let rows: [(&str, String, String); 9] = [
        (
            "already lower case",
            "user@example.com".to_string(),
            "user@example.com".to_string(),
        ),
        (
            "upper-case domain",
            "user@EXAMPLE.COM".to_string(),
            "user@example.com".to_string(),
        ),
        (
            "upper-case local part",
            "USER2@EXAMPLE.COM".to_string(),
            "USER2@example.com".to_string(),
        ),
        (
            "mixed case in both halves",
            "First.Last+Tag@Sub.Example.Co.UK".to_string(),
            "First.Last+Tag@sub.example.co.uk".to_string(),
        ),
        (
            "atom punctuation in the local part",
            "a_b-c!d#e$f%g&h'i*j+k@Example.ORG".to_string(),
            "a_b-c!d#e$f%g&h'i*j+k@example.org".to_string(),
        ),
        (
            "a bare domain",
            "EXAMPLE.NET".to_string(),
            "example.net".to_string(),
        ),
        (
            "a bare multi-label domain",
            "Mail.Example.Net".to_string(),
            "mail.example.net".to_string(),
        ),
        (
            "the shortest value the modelled length admits",
            "A.b".to_string(),
            "a.b".to_string(),
        ),
        ("the longest value the modelled length admits", longest, longest_stored),
    ];

    for (case, registered, stored) in &rows {
        harness.register(&list, registered).await;

        // The address is readable back under the spelling it was registered with and
        // under the spelling it is stored as, and both report the STORED spelling.
        for (lookup_case, lookup) in [("as registered", registered), ("as stored", stored)] {
            let (status, out) = harness
                .call(
                    "GetMemberOfAddressList",
                    json!({ "AddressListId": list, "Address": lookup }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?} ({lookup_case}): expected GetMemberOfAddressList to answer 200, \
                 got {status} {out:?}"
            );
            assert_eq!(
                out["Address"].as_str(),
                Some(stored.as_str()),
                "case {case:?} ({lookup_case}): expected the stored address {stored:?}, got \
                 {out:?}"
            );
            assert!(
                out["CreatedTimestamp"].as_f64().is_some_and(|t| t > 0.0),
                "case {case:?} ({lookup_case}): CreatedTimestamp must be a real instant, got \
                 {out:?}"
            );

            // A read changes nothing a later caller sees.
            let (_, again) = harness
                .call(
                    "GetMemberOfAddressList",
                    json!({ "AddressListId": list, "Address": lookup }),
                )
                .await;
            assert_eq!(
                again, out,
                "case {case:?} ({lookup_case}): a member read must be stable, got {again:?}"
            );
        }
    }

    // Every registered address is a member, under its stored spelling and no other.
    let expected: BTreeSet<String> = rows.iter().map(|(_, _, stored)| stored.clone()).collect();
    let listed: BTreeSet<String> = harness.list_all(&list, 100).await.into_iter().collect();
    assert_eq!(
        listed, expected,
        "case whole-membership: ListMembersOfAddressList must report every stored address"
    );
}

/// RULE: the local part of an address is case-SENSITIVE, so a lookup whose local part
/// differs from the stored one names a different member and finds nothing; only the domain
/// half folds.
#[tokio::test]
async fn a_member_lookup_requires_the_stored_local_part_casing() {
    let harness = Harness::new();
    let list = harness.create_list("casing").await;

    // (case, registered, lookups that must HIT, lookups that must MISS)
    let rows: [(&str, &str, [&str; 2], [&str; 3]); 3] = [
        (
            "a capitalised local part",
            "User@example.com",
            ["User@EXAMPLE.COM", "User@Example.Com"],
            ["user@example.com", "USER@example.com", "uSer@example.com"],
        ),
        (
            "an all-upper local part",
            "POSTMASTER@Example.ORG",
            ["POSTMASTER@example.org", "POSTMASTER@EXAMPLE.ORG"],
            [
                "postmaster@example.org",
                "Postmaster@example.org",
                "postMaster@example.org",
            ],
        ),
        (
            "a dotted mixed-case local part",
            "Jane.Doe@mail.Example.NET",
            ["Jane.Doe@MAIL.EXAMPLE.NET", "Jane.Doe@mail.example.net"],
            [
                "jane.doe@mail.example.net",
                "JANE.DOE@mail.example.net",
                "Jane.doe@mail.example.net",
            ],
        ),
    ];

    for (case, registered, hits, misses) in rows {
        harness.register(&list, registered).await;
        let stored = {
            let (_, out) = harness
                .call(
                    "GetMemberOfAddressList",
                    json!({ "AddressListId": list, "Address": registered }),
                )
                .await;
            out["Address"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no Address in {out:?}"))
                .to_string()
        };

        for lookup in hits {
            let (status, out) = harness
                .call(
                    "GetMemberOfAddressList",
                    json!({ "AddressListId": list, "Address": lookup }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: {lookup:?} differs from {stored:?} only in its domain, so it must \
                 name the same member, got {status} {out:?}"
            );
            assert_eq!(
                out["Address"].as_str(),
                Some(stored.as_str()),
                "case {case:?}: {lookup:?} must report the stored address, got {out:?}"
            );
        }

        for lookup in misses {
            let (status, out) = harness
                .call(
                    "GetMemberOfAddressList",
                    json!({ "AddressListId": list, "Address": lookup }),
                )
                .await;
            assert_eq!(
                status, 404,
                "case {case:?}: {lookup:?} differs from {stored:?} in its local part, so it must \
                 name no member, got {status} {out:?}"
            );
            assert_eq!(
                error_code(&out),
                "ResourceNotFoundException",
                "case {case:?}: unexpected error code for {lookup:?}, got {out:?}"
            );
            assert!(
                !out["message"]
                    .as_str()
                    .unwrap_or_default()
                    .contains(lookup),
                "case {case:?}: the refusal must not disclose the address it was handed, got \
                 {out:?}"
            );
        }
    }
}

/// RULE: registering an address that is already a member is the state the caller asked for,
/// so it succeeds and leaves the membership exactly as it stands — one member, with the
/// instant it was FIRST added.
#[tokio::test]
async fn registering_an_existing_member_changes_nothing() {
    let harness = Harness::new();
    let list = harness.create_list("idempotent").await;

    // Rows: the repeat differs from the first register in nothing, in its domain's case,
    // and (for a bare domain) in the whole value's case — every one of which names the
    // same member.
    let rows: [(&str, &str, &str); 3] = [
        ("identical repeat", "user@example.com", "user@example.com"),
        (
            "repeat with another domain casing",
            "Owner@Example.com",
            "Owner@EXAMPLE.COM",
        ),
        ("a bare domain in another casing", "example.net", "Example.NET"),
    ];

    for (case, first, repeat) in rows {
        harness.register(&list, first).await;
        let (_, before) = harness
            .call(
                "GetMemberOfAddressList",
                json!({ "AddressListId": list, "Address": first }),
            )
            .await;

        // Let the clock advance, so a second register that (wrongly) re-stamped the
        // member would be visible.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        harness.register(&list, repeat).await;

        let (status, after) = harness
            .call(
                "GetMemberOfAddressList",
                json!({ "AddressListId": list, "Address": repeat }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a repeat register to leave the member readable, got {status} \
             {after:?}"
        );
        assert_eq!(
            after, before,
            "case {case:?}: a repeat register must not move the member's CreatedTimestamp, got \
             {after:?}"
        );
    }

    // One member per address, however often it was registered.
    let listed = harness.list_all(&list, 10).await;
    assert_eq!(
        listed,
        vec![
            "Owner@example.com".to_string(),
            "example.net".to_string(),
            "user@example.com".to_string(),
        ],
        "case membership-count: each address must be ONE member, got {listed:?}"
    );
}

/// RULE: `Filter.AddressPrefix` limits a listing to the members whose STORED address starts
/// with it; an absent filter limits nothing.
#[tokio::test]
async fn the_address_prefix_filter_scopes_the_listing() {
    let harness = Harness::new();
    let list = harness.create_list("prefixes").await;

    let registered = [
        "alice@example.com",
        "alice@example.org",
        "alicia@example.com",
        "bob@EXAMPLE.com",
        "Carol@example.com",
        "example.com",
        "mail.example.com",
    ];
    for address in registered {
        harness.register(&list, address).await;
    }

    // (case, prefix, the stored addresses it scopes to)
    let rows: [(&str, Option<&str>, &[&str]); 8] = [
        (
            "no filter at all",
            None,
            &[
                "Carol@example.com",
                "alice@example.com",
                "alice@example.org",
                "alicia@example.com",
                "bob@example.com",
                "example.com",
                "mail.example.com",
            ],
        ),
        (
            "a local-part prefix shared by several",
            Some("alic"),
            &["alice@example.com", "alice@example.org", "alicia@example.com"],
        ),
        (
            "a longer local-part prefix",
            Some("alice@example."),
            &["alice@example.com", "alice@example.org"],
        ),
        (
            "a whole stored address",
            Some("alice@example.org"),
            &["alice@example.org"],
        ),
        (
            "a prefix of the folded domain of a bare-domain member",
            Some("example."),
            &["example.com"],
        ),
        (
            "a prefix matching one member only",
            Some("mail"),
            &["mail.example.com"],
        ),
        (
            "an upper-case prefix, which only a stored upper-case local part carries",
            Some("Carol"),
            &["Carol@example.com"],
        ),
        ("a prefix no stored address carries", Some("zzz"), &[]),
    ];

    for (case, prefix, expected) in rows {
        let mut body = json!({ "AddressListId": list });
        if let Some(prefix) = prefix {
            body["Filter"] = json!({ "AddressPrefix": prefix });
        }
        let (addresses, next) = harness.list_page(body.clone()).await;
        assert_eq!(
            addresses,
            expected.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
            "case {case:?}: unexpected scoped listing for prefix {prefix:?}"
        );
        assert!(
            next.is_none(),
            "case {case:?}: the whole scoped listing fits one page, so it must carry no \
             NextToken, got {next:?}"
        );
    }

    // A filtered walk is still complete and unique when it spans several pages.
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({
            "AddressListId": list,
            "PageSize": 1,
            "Filter": { "AddressPrefix": "alic" },
        });
        if let Some(next) = &token {
            body["NextToken"] = json!(next);
        }
        let (page, next) = harness.list_page(body).await;
        walked.extend(page);
        match next {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    assert_eq!(
        walked,
        vec![
            "alice@example.com".to_string(),
            "alice@example.org".to_string(),
            "alicia@example.com".to_string(),
        ],
        "case filtered-pagination: a filtered walk must report each scoped member once"
    );
}

/// RULE: a paginated walk reports every member exactly once, in one stable order, whatever
/// page size it is walked with; only the last page carries no `NextToken`.
#[tokio::test]
async fn pagination_is_complete_and_unique_at_every_page_size() {
    let harness = Harness::new();
    let list = harness.create_list("pagination").await;

    let registered = [
        "a@example.com",
        "b@example.com",
        "c@example.com",
        "d@example.com",
        "e@example.com",
        "f@example.com",
        "g@example.com",
    ];
    for address in registered {
        harness.register(&list, address).await;
    }
    let expected: Vec<String> = {
        let mut sorted: Vec<String> = registered.iter().map(|a| a.to_string()).collect();
        sorted.sort();
        sorted
    };

    // Rows: one member per page, a size that divides the membership, one that does not,
    // exactly the membership, one past it, and the modelled maximum.
    for page_size in [1_usize, 2, 3, 7, 8, 1000] {
        let mut walked: Vec<String> = Vec::new();
        let mut pages = 0_usize;
        let mut token: Option<String> = None;
        loop {
            let mut body = json!({ "AddressListId": list, "PageSize": page_size });
            if let Some(next) = &token {
                body["NextToken"] = json!(next);
            }
            let (page, next) = harness.list_page(body).await;
            pages += 1;
            assert!(
                page.len() <= page_size,
                "case page size {page_size}: a page must hold at most PageSize members, got \
                 {page:?}"
            );
            if next.is_some() {
                assert_eq!(
                    page.len(),
                    page_size,
                    "case page size {page_size}: a page carrying a NextToken must be full, got \
                     {page:?}"
                );
            }
            walked.extend(page);
            match next {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        assert_eq!(
            walked, expected,
            "case page size {page_size}: the walk must report every member once, in order"
        );
        assert_eq!(
            pages,
            expected.len().div_ceil(page_size).max(1),
            "case page size {page_size}: unexpected number of pages for {} members",
            expected.len()
        );
    }

    // An empty membership is one empty page and no token.
    let empty = harness.create_list("pagination-empty").await;
    let (addresses, next) = harness.list_page(json!({ "AddressListId": empty })).await;
    assert!(
        addresses.is_empty() && next.is_none(),
        "case empty-membership: expected one empty page and no NextToken, got {addresses:?} \
         {next:?}"
    );
}

/// RULE: only a `NextToken` this store ISSUED, for the walk it is presented to, names a
/// position in that walk; any other value is refused with the modelled
/// `ValidationException` rather than answered with a page the caller chose.
#[tokio::test]
async fn a_next_token_this_store_did_not_mint_is_refused() {
    let harness = Harness::new();
    let list = harness.create_list("tokens").await;
    // Three members, two of them sharing the prefix the filtered walk below pages under,
    // so that walk really has a second page and the token it issues is not `None`.
    for address in ["a@example.com", "ab@example.com", "b@example.com"] {
        harness.register(&list, address).await;
    }

    // Another address list of the SAME store, holding the same addresses: a token issued
    // for its walk is one this store minted, but not for the walk below.
    let other_list = harness.create_list("tokens-other").await;
    for address in ["a@example.com", "b@example.com"] {
        harness.register(&other_list, address).await;
    }
    let (_, other_walk) = harness
        .list_page(json!({ "AddressListId": other_list, "PageSize": 1 }))
        .await;
    let other_walk = other_walk.expect("a two-member listing of one-member pages carries a token");

    // A token issued for the same list under a FILTER: a different walk again. Two members
    // carry the prefix, so a one-member page leaves a second one and a token for it.
    let (_, filtered_walk) = harness
        .list_page(json!({
            "AddressListId": list,
            "PageSize": 1,
            "Filter": { "AddressPrefix": "a" },
        }))
        .await;
    let filtered_walk =
        filtered_walk.expect("a two-member filtered listing of one-member pages carries a token");

    // A token another store minted for the same walk: its position is authenticated under
    // that store's own secret, so it names nothing here.
    let other = Harness::new();
    let other_store_list = other.create_list("tokens").await;
    for address in ["a@example.com", "b@example.com"] {
        other.register(&other_store_list, address).await;
    }
    let (_, foreign) = other
        .list_page(json!({ "AddressListId": other_store_list, "PageSize": 1 }))
        .await;
    let foreign = foreign.expect("a two-member listing of one-member pages carries a NextToken");

    // Rows: a value that is not this token's encoding at all, a plain address, an
    // authenticator a caller made up over a real position, a truncated real token, a real
    // token with one character changed, a token from another store, and a token this store
    // issued for another walk.
    let (_, mine) = harness
        .list_page(json!({ "AddressListId": list, "PageSize": 1 }))
        .await;
    let mine = mine.expect("a two-member listing of one-member pages carries a NextToken");
    let tampered = {
        let mut bytes = mine.clone().into_bytes();
        bytes[0] = if bytes[0] == b'0' { b'1' } else { b'0' };
        String::from_utf8(bytes).expect("hex stays UTF-8")
    };
    let rows: [(&str, String); 8] = [
        ("not hex at all", "not-a-token".to_string()),
        ("a bare address", "b@example.com".to_string()),
        (
            "a made-up authenticator over a real position",
            format!("{}{}", "00".repeat(32), hex::encode("b@example.com")),
        ),
        ("a truncated token", mine[..mine.len() - 2].to_string()),
        ("a token with one character changed", tampered),
        ("a token another store minted", foreign),
        ("a token issued for another address list", other_walk),
        ("a token issued under another filter", filtered_walk),
    ];

    for (case, token) in rows {
        let (status, out) = harness
            .call(
                "ListMembersOfAddressList",
                json!({ "AddressListId": list, "NextToken": token }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a token this store did not mint for this walk to answer \
             400, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ValidationException",
            "case {case:?}: unexpected error code, got {out:?}"
        );
    }

    // The token this store did mint for THIS walk still works, so the refusals above are
    // the authenticator and the walk scope and not a blanket rejection.
    let (page, _) = harness
        .list_page(json!({ "AddressListId": list, "PageSize": 1, "NextToken": mine }))
        .await;
    assert_eq!(
        page,
        vec!["ab@example.com".to_string()],
        "case own-token: this store's own token must resume the walk"
    );
}

/// RULE: an `Address` that is neither an email address nor a domain name names nothing an
/// address list can hold, so every member operation refuses it with a 400
/// `ValidationException` — and the refusal discloses no address.
#[tokio::test]
async fn an_address_that_is_no_address_is_refused() {
    let harness = Harness::new();
    let list = harness.create_list("malformed").await;

    // Rows walk the ways a value of the modelled 3..320 length is not an address: no
    // domain, an empty local part, an empty label, a single-label domain, a second `@`, a
    // space, and an all-numeric top-level label.
    let rows: [(&str, &str); 8] = [
        ("no @ and no dot", "nobody"),
        ("an empty local part", "@example.com"),
        ("an empty domain", "user@"),
        ("a doubled dot in the local part", "us..er@example.com"),
        ("a single-label domain", "user@localhost"),
        ("a second @", "user@host@example.com"),
        ("a space", "user name@example.com"),
        ("an all-numeric top-level label", "user@example.123"),
    ];

    for (case, address) in rows {
        for operation in [
            "RegisterMemberToAddressList",
            "DeregisterMemberFromAddressList",
            "GetMemberOfAddressList",
        ] {
            let (status, out) = harness
                .call(
                    operation,
                    json!({ "AddressListId": list, "Address": address }),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected {operation} to refuse {address:?} with 400, got {status} \
                 {out:?}"
            );
            assert_eq!(
                error_code(&out),
                "ValidationException",
                "case {case:?}: unexpected error code from {operation}, got {out:?}"
            );
            assert!(
                !out["message"]
                    .as_str()
                    .unwrap_or_default()
                    .contains(address),
                "case {case:?}: the refusal must not disclose the address it was handed, got \
                 {out:?}"
            );
        }
    }

    // Nothing was stored by any of those refusals.
    let (addresses, _) = harness.list_page(json!({ "AddressListId": list })).await;
    assert!(
        addresses.is_empty(),
        "case refusals-store-nothing: expected no members, got {addresses:?}"
    );
}

/// RULE: a member belongs to an address list, so an `AddressListId` nothing created is a
/// `ResourceNotFoundException` (404) from every member operation.
#[tokio::test]
async fn member_operations_on_an_unknown_address_list_are_not_found() {
    let harness = Harness::new();
    let live = harness.create_list("present").await;
    harness.register(&live, "user@example.com").await;

    // Rows: ids that satisfy `AddressListId`'s pattern but name nothing — the shortest,
    // a hyphenated one, the longest, and the live id with one character changed.
    let unknown: [String; 4] = [
        "x".to_string(),
        "no-such-address-list".to_string(),
        "9".repeat(255),
        format!("{}0", &live[..live.len() - 1]),
    ];
    for id in &unknown {
        if id == &live {
            continue;
        }
        for (operation, body) in [
            (
                "RegisterMemberToAddressList",
                json!({ "AddressListId": id, "Address": "user@example.com" }),
            ),
            (
                "DeregisterMemberFromAddressList",
                json!({ "AddressListId": id, "Address": "user@example.com" }),
            ),
            (
                "GetMemberOfAddressList",
                json!({ "AddressListId": id, "Address": "user@example.com" }),
            ),
            ("ListMembersOfAddressList", json!({ "AddressListId": id })),
        ] {
            let (status, out) = harness.call(operation, body).await;
            assert_eq!(
                status, 404,
                "case {id:?}: expected {operation} to answer 404, got {status} {out:?}"
            );
            assert_eq!(
                error_code(&out),
                "ResourceNotFoundException",
                "case {id:?}: unexpected error code from {operation}, got {out:?}"
            );
        }
    }

    // The live list's membership survived every refusal.
    let (addresses, _) = harness.list_page(json!({ "AddressListId": live })).await;
    assert_eq!(
        addresses,
        vec!["user@example.com".to_string()],
        "case live-after-misses: the stored membership must be untouched, got {addresses:?}"
    );
}

/// RULE: a list's membership is BOUNDED — a register past the per-list quota is refused
/// with the modelled `ServiceQuotaExceededException`, while a register of an address
/// already in the list still succeeds, because it consumes nothing.
#[tokio::test]
async fn registers_past_the_membership_quota_are_refused() {
    let harness = Harness::new();
    let list = harness.create_list("quota").await;

    for index in 0..MAX_ADDRESS_LIST_MEMBERS {
        harness
            .register(&list, &format!("member-{index:05}@example.com"))
            .await;
    }

    // Rows: a fresh address, and one whose domain differs only in case from a fresh one —
    // neither names a member the list already holds, so neither may be served.
    let rows: [(&str, &str); 2] = [
        ("a fresh address", "overflow@example.com"),
        ("a fresh bare domain", "Overflow.Example.NET"),
    ];
    for (case, address) in rows {
        let (status, out) = harness
            .call(
                "RegisterMemberToAddressList",
                json!({ "AddressListId": list, "Address": address }),
            )
            .await;
        assert_eq!(
            status, 402,
            "case {case:?}: expected a register past the quota to answer 402, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ServiceQuotaExceededException",
            "case {case:?}: unexpected error code, got {out:?}"
        );
    }

    // An address the list already holds is still registrable at the ceiling, and the
    // membership is exactly what was registered.
    harness.register(&list, "member-00000@EXAMPLE.com").await;
    let listed = harness.list_all(&list, 1000).await;
    assert_eq!(
        listed.len(),
        MAX_ADDRESS_LIST_MEMBERS,
        "case membership-after-quota: expected {MAX_ADDRESS_LIST_MEMBERS} members, got {}",
        listed.len()
    );
    let unique: BTreeSet<&String> = listed.iter().collect();
    assert_eq!(
        unique.len(),
        listed.len(),
        "case membership-after-quota: every member must be reported once"
    );

    // A deregister really gives the slot back: once a member is gone the fresh address the
    // ceiling refused above is registrable, which is only true if the removal reached the
    // stored membership.
    harness.deregister(&list, "member-00000@example.com").await;
    harness.register(&list, "overflow@example.com").await;
    let listed = harness.list_all(&list, 1000).await;
    assert_eq!(
        listed.len(),
        MAX_ADDRESS_LIST_MEMBERS,
        "case slot-reused: expected {MAX_ADDRESS_LIST_MEMBERS} members, got {}",
        listed.len()
    );
    assert!(
        listed.contains(&"overflow@example.com".to_string())
            && !listed.contains(&"member-00000@example.com".to_string()),
        "case slot-reused: the deregistered member must be gone and the new one present, got \
         {} members starting {:?}",
        listed.len(),
        &listed[..3.min(listed.len())]
    );
}

/// RULE: a deregister drops the member the address names under the key the service stores an
/// address by — the domain folded to lower case, the local part as spelled — so the address
/// is afterwards no member of the list under ANY spelling of that same key, and every other
/// member is untouched.
///
/// Rows span the class: the same spelling, another domain casing, a bare domain in another
/// casing, a multi-label domain, and both ends of `Address`'s modelled 3..320 length.
#[tokio::test]
async fn deregister_drops_the_member_under_the_stored_key() {
    let harness = Harness::new();
    let list = harness.create_list("deregister").await;

    let (longest, longest_stored) = longest_address();
    // (case, as registered, the spelling the deregister names it by, as stored)
    let rows: [(&str, String, String, String); 6] = [
        (
            "the spelling it was registered with",
            "user@example.com".to_string(),
            "user@example.com".to_string(),
            "user@example.com".to_string(),
        ),
        (
            "another domain casing",
            "Owner@example.com".to_string(),
            "Owner@EXAMPLE.COM".to_string(),
            "Owner@example.com".to_string(),
        ),
        (
            "a bare domain in another casing",
            "Example.NET".to_string(),
            "EXAMPLE.net".to_string(),
            "example.net".to_string(),
        ),
        (
            "a multi-label domain",
            "Jane.Doe@mail.Example.ORG".to_string(),
            "Jane.Doe@MAIL.EXAMPLE.ORG".to_string(),
            "Jane.Doe@mail.example.org".to_string(),
        ),
        (
            "the shortest value the modelled length admits",
            "A.b".to_string(),
            "a.B".to_string(),
            "a.b".to_string(),
        ),
        (
            "the longest value the modelled length admits",
            longest,
            longest_stored.clone(),
            longest_stored,
        ),
    ];

    // A member that no row names, so every deregister below has a membership to leave
    // alone.
    harness.register(&list, "bystander@example.com").await;

    for (case, registered, deregistered, stored) in &rows {
        harness.register(&list, registered).await;
        let listed = harness.list_all(&list, 100).await;
        assert!(
            listed.contains(stored),
            "case {case:?}: expected {stored:?} to be a member before the deregister, got \
             {listed:?}"
        );

        harness.deregister(&list, deregistered).await;

        // The membership is gone from every read: the member lookup, under both spellings,
        // and the listing.
        for (lookup_case, lookup) in [("as registered", registered), ("as stored", stored)] {
            let (status, out) = harness
                .call(
                    "GetMemberOfAddressList",
                    json!({ "AddressListId": list, "Address": lookup }),
                )
                .await;
            assert_eq!(
                status, 404,
                "case {case:?} ({lookup_case}): expected GetMemberOfAddressList to answer 404 \
                 after the deregister, got {status} {out:?}"
            );
            assert_eq!(
                error_code(&out),
                "ResourceNotFoundException",
                "case {case:?} ({lookup_case}): unexpected error code, got {out:?}"
            );
        }

        let listed = harness.list_all(&list, 100).await;
        assert!(
            !listed.contains(stored),
            "case {case:?}: {stored:?} must no longer be listed, got {listed:?}"
        );
        assert_eq!(
            listed,
            vec!["bystander@example.com".to_string()],
            "case {case:?}: every other member must survive the deregister, got {listed:?}"
        );
    }
}

/// RULE: the local part of an address is case-SENSITIVE while its domain folds, so a
/// deregister whose local part differs from the stored one names a DIFFERENT member: it
/// removes nothing and the stored member stays readable.
#[tokio::test]
async fn a_deregister_requires_the_stored_local_part_casing() {
    let harness = Harness::new();
    let list = harness.create_list("deregister-casing").await;

    // (case, registered, the spellings whose local part differs)
    let rows: [(&str, &str, [&str; 3]); 3] = [
        (
            "a capitalised local part",
            "User@example.com",
            ["user@example.com", "USER@example.com", "uSer@EXAMPLE.COM"],
        ),
        (
            "an all-upper local part",
            "POSTMASTER@Example.ORG",
            [
                "postmaster@example.org",
                "Postmaster@example.org",
                "postMaster@EXAMPLE.ORG",
            ],
        ),
        (
            "a dotted mixed-case local part",
            "Jane.Doe@mail.Example.NET",
            [
                "jane.doe@mail.example.net",
                "JANE.DOE@mail.example.net",
                "Jane.doe@MAIL.EXAMPLE.NET",
            ],
        ),
    ];

    for (case, registered, others) in rows {
        harness.register(&list, registered).await;
        let (_, before) = harness
            .call(
                "GetMemberOfAddressList",
                json!({ "AddressListId": list, "Address": registered }),
            )
            .await;

        for other in others {
            harness.deregister(&list, other).await;
            let (status, after) = harness
                .call(
                    "GetMemberOfAddressList",
                    json!({ "AddressListId": list, "Address": registered }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: {other:?} differs from {registered:?} in its local part, so it \
                 must remove no member, got {status} {after:?}"
            );
            assert_eq!(
                after, before,
                "case {case:?}: the stored member must be unchanged after deregistering \
                 {other:?}, got {after:?}"
            );
        }
    }
}

/// RULE: `DeregisterMemberFromAddressList` is `@idempotent` — the call asks for the address
/// NOT to be a member, so an address the list does not hold (one nothing ever registered, or
/// one a previous call already removed) is served exactly like one this call removed, and
/// stores nothing.
#[tokio::test]
async fn deregistering_an_address_that_is_no_member_is_served() {
    let harness = Harness::new();
    let list = harness.create_list("idempotent-deregister").await;
    harness.register(&list, "member@example.com").await;

    let (longest, _) = longest_address();
    // Rows: addresses the list never held — an email address, a bare domain, a multi-label
    // domain, and both ends of the modelled 3..320 length.
    let unknown: [(&str, String); 5] = [
        ("an unknown email address", "nobody@example.com".to_string()),
        ("an unknown bare domain", "Unknown.Example.NET".to_string()),
        (
            "an unknown multi-label domain",
            "mail.unknown.example.org".to_string(),
        ),
        (
            "the shortest value the modelled length admits",
            "A.b".to_string(),
        ),
        ("the longest value the modelled length admits", longest),
    ];

    for (case, address) in &unknown {
        // Twice, because an idempotent call is answered the same however often it is made.
        for attempt in 1..=2 {
            let (status, out) = harness
                .call(
                    "DeregisterMemberFromAddressList",
                    json!({ "AddressListId": list, "Address": address }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?} (attempt {attempt}): deregistering an address that is no member \
                 must be served, got {status} {out:?}"
            );
            assert_eq!(
                out,
                json!({}),
                "case {case:?} (attempt {attempt}): the response models no members, got {out:?}"
            );
        }

        // The address the call named is still no member, and nothing was created for it.
        let (status, out) = harness
            .call(
                "GetMemberOfAddressList",
                json!({ "AddressListId": list, "Address": address }),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: a deregister must not create a member, got {status} {out:?}"
        );
    }

    // An address that WAS a member is served the same way once it is gone, however often
    // the call is repeated.
    for attempt in 1..=3 {
        harness.deregister(&list, "member@EXAMPLE.com").await;
        let (status, out) = harness
            .call(
                "GetMemberOfAddressList",
                json!({ "AddressListId": list, "Address": "member@example.com" }),
            )
            .await;
        assert_eq!(
            status, 404,
            "case already-removed (attempt {attempt}): the member must stay gone, got {status} \
             {out:?}"
        );
    }

    let listed = harness.list_all(&list, 100).await;
    assert!(
        listed.is_empty(),
        "case nothing-stored: no deregister may leave a member behind, got {listed:?}"
    );
}

/// RULE: a deregister drops the membership whole, so a later register of the same address
/// starts a new one, stamped with the instant of THAT register rather than the first.
#[tokio::test]
async fn re_registering_a_deregistered_address_starts_a_new_membership() {
    let harness = Harness::new();
    let list = harness.create_list("re-register").await;

    // Rows: an email address and a bare domain, each re-registered in another domain
    // casing, so the round trip goes through the stored key.
    let rows: [(&str, &str, &str); 2] = [
        ("an email address", "user@example.com", "user@EXAMPLE.COM"),
        ("a bare domain", "Example.NET", "example.net"),
    ];

    for (case, first, again) in rows {
        harness.register(&list, first).await;
        let (_, before) = harness
            .call(
                "GetMemberOfAddressList",
                json!({ "AddressListId": list, "Address": first }),
            )
            .await;
        let first_stamp = before["CreatedTimestamp"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: no CreatedTimestamp in {before:?}"));

        harness.deregister(&list, first).await;
        // Let the clock advance, so a membership that (wrongly) survived the deregister is
        // visible as an unmoved CreatedTimestamp.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        harness.register(&list, again).await;

        let (status, after) = harness
            .call(
                "GetMemberOfAddressList",
                json!({ "AddressListId": list, "Address": again }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the re-registered member to be readable, got {status} \
             {after:?}"
        );
        let second_stamp = after["CreatedTimestamp"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: no CreatedTimestamp in {after:?}"));
        assert!(
            second_stamp > first_stamp,
            "case {case:?}: expected a fresh CreatedTimestamp past {first_stamp}, got \
             {second_stamp}"
        );
    }
}
