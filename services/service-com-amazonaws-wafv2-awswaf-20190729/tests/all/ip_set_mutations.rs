// SPDX-License-Identifier: CC0-1.0

//! `UpdateIPSet` and `DeleteIPSet` against stored state, end to end over the wire.
//!
//! The rule asserted here: a mutating WAFv2 call leaves the stored resource in the
//! state the call describes, and every later read answers from that state.
//! Concretely, for an IP set —
//!
//! * `UpdateIPSet` "completely replaces the mutable specifications that you already
//!   have for the IP set with the ones that you provide to this call", so after a
//!   successful update `GetIPSet` answers EXACTLY the membership the call submitted
//!   — every block it named and no other — and a `Description` the call omitted is
//!   gone. The ORDER a read reports that membership in is the set's own, which
//!   `crate::ip_set_address_order` spans; here a read is compared against the
//!   membership, and against the previous read when nothing was meant to change.
//! * `DeleteIPSet` "deletes the specified IPSet", so after a successful delete
//!   `GetIPSet` for the same `(Scope, Name, Id)` is refused with
//!   `WAFNonexistentItemException` (HTTP 400) and the name is free again;
//! * a refused mutation changes nothing at all — not the contents, not the lock
//!   token.
//!
//! Every row below is driven through the router's front door with the harness in
//! [`crate::web_acl_lifecycle`], so what these tests observe is what an SDK
//! observes.

use serde_json::{json, Value};

use crate::web_acl_lifecycle::{is_model_uuid, visibility, Waf};

/// Create an IP set of `version` with `addresses` and `description`, and answer
/// `(id, arn, lock token)`.
///
/// Shared with the sibling suites that need a set with a chosen family, address
/// list and description (`crate::ip_set_listing`).
pub(crate) async fn create_ip_set(
    waf: &Waf,
    scope: &str,
    name: &str,
    version: &str,
    addresses: &[&str],
    description: Option<&str>,
) -> (String, String, String) {
    let mut request = json!({
        "Name": name,
        "Scope": scope,
        "IPAddressVersion": version,
        "Addresses": addresses,
    });
    if let Some(description) = description {
        request["Description"] = json!(description);
    }
    let created = waf.ok("CreateIPSet", request).await;
    let summary = &created["Summary"];
    (
        summary["Id"].as_str().expect("Summary.Id").to_string(),
        summary["ARN"].as_str().expect("Summary.ARN").to_string(),
        summary["LockToken"]
            .as_str()
            .expect("Summary.LockToken")
            .to_string(),
    )
}

/// `GetIPSet` for `(scope, name, id)`, as a 200.
pub(crate) async fn get_ip_set(waf: &Waf, scope: &str, name: &str, id: &str) -> Value {
    waf.ok(
        "GetIPSet",
        json!({ "Name": name, "Scope": scope, "Id": id }),
    )
    .await
}

/// The `Addresses` a `GetIPSet` answer reports, in the order it reported them.
///
/// Shared with the sibling suites that read an address list back
/// (`crate::ip_set_canonicalization`, `crate::ip_set_address_order`).
pub(crate) fn addresses(got: &Value) -> Vec<String> {
    got["IPSet"]["Addresses"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|address| address.as_str().unwrap_or_default().to_string())
        .collect()
}

/// The MEMBERSHIP a `GetIPSet` answer reports: its addresses as the set they name,
/// in one order so two sets of the same blocks compare equal.
pub(crate) fn membership(got: &Value) -> Vec<String> {
    let mut members = addresses(got);
    members.sort();
    members
}

/// `entries` as a set, the form [`membership`] compares against.
pub(crate) fn sorted(entries: &[&str]) -> Vec<String> {
    let mut sorted: Vec<String> = entries.iter().map(|entry| (*entry).to_string()).collect();
    sorted.sort();
    sorted
}

/// An address list as the set it names, for comparing one read against another.
pub(crate) fn membership_of(entries: &[String]) -> Vec<String> {
    let mut members = entries.to_vec();
    members.sort();
    members
}

/// One whole-replacement case: what the set held, and what the update submits.
struct Replacement {
    /// What the case is called in an assertion.
    case: &'static str,
    /// The address family the set is created with; it is immutable thereafter.
    version: &'static str,
    /// The addresses the set is created with.
    before: &'static [&'static str],
    /// The description the set is created with.
    description_before: Option<&'static str>,
    /// The addresses the update submits.
    after: &'static [&'static str],
    /// The membership a read must report, as a set: the submitted entries for the
    /// single-notation IPv4 family, and their canonical notation for IPv6.
    stored_after: &'static [&'static str],
    /// The description the update submits, or `None` when it omits the member.
    description_after: Option<&'static str>,
}

/// `UpdateIPSet` replaces the whole mutable specification, and `GetIPSet` answers
/// what was submitted.
///
/// The rows differ along everything the replacement generalises over: the number
/// of addresses (fewer, more, none at all), the values, the ORDER of the same two
/// values, the address family, and whether the update carries a `Description` at
/// all. A `Description` the update omits reads back as `""`, the empty description
/// every WAFv2 read reports for an entity saved without one.
#[tokio::test]
async fn update_ip_set_replaces_the_whole_specification() {
    let cases: &[Replacement] = &[
        Replacement {
            case: "one address replaces two, and the description is dropped",
            version: "IPV4",
            before: &["10.0.0.0/24", "10.0.1.0/24"],
            description_before: Some("probe ipset"),
            after: &["172.16.0.0/16"],
            stored_after: &["172.16.0.0/16"],
            description_after: None,
        },
        Replacement {
            case: "two addresses replace one, and a description is added",
            version: "IPV4",
            before: &["192.0.2.0/24"],
            description_before: None,
            after: &["203.0.113.0/24", "198.51.100.0/24"],
            stored_after: &["203.0.113.0/24", "198.51.100.0/24"],
            description_after: Some("two ranges"),
        },
        Replacement {
            case: "the same two addresses, submitted in the opposite order",
            version: "IPV4",
            before: &["10.0.0.0/24", "10.0.1.0/24"],
            description_before: Some("ordered"),
            after: &["10.0.1.0/24", "10.0.0.0/24"],
            stored_after: &["10.0.1.0/24", "10.0.0.0/24"],
            description_after: Some("ordered"),
        },
        Replacement {
            case: "an empty list clears every address",
            version: "IPV4",
            before: &["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16"],
            description_before: Some("three ranges"),
            after: &[],
            stored_after: &[],
            description_after: Some("emptied"),
        },
        Replacement {
            case: "an IPv6 set is replaced by three IPv6 ranges",
            version: "IPV6",
            before: &["2001:db8::/32"],
            description_before: Some("v6 before"),
            after: &[
                "2001:db8:1::/48",
                "2001:db8:3::/48",
                "2001:db8:2::/48",
            ],
            // An IPv6 entry is stored in the one canonical notation of its value;
            // see `crate::ip_set_canonicalization`.
            stored_after: &[
                "2001:0db8:0001:0000:0000:0000:0000:0000/48",
                "2001:0db8:0003:0000:0000:0000:0000:0000/48",
                "2001:0db8:0002:0000:0000:0000:0000:0000/48",
            ],
            description_after: Some("v6 replaced"),
        },
        Replacement {
            case: "a CLOUDFRONT-scoped set is replaced too",
            version: "IPV4",
            before: &["10.1.0.0/16"],
            description_before: Some("global"),
            after: &["10.2.0.0/16", "10.3.0.0/16"],
            stored_after: &["10.2.0.0/16", "10.3.0.0/16"],
            description_after: None,
        },
    ];

    let waf = Waf::new();
    for (index, replacement) in cases.iter().enumerate() {
        let Replacement {
            case,
            version,
            before,
            description_before,
            after,
            stored_after,
            description_after,
        } = *replacement;
        let scope = if case.contains("CLOUDFRONT") {
            "CLOUDFRONT"
        } else {
            "REGIONAL"
        };
        let name = format!("replace-ips-{index}");
        let (id, _arn, lock) =
            create_ip_set(&waf, scope, &name, version, before, description_before).await;

        let mut request = json!({
            "Name": &name,
            "Scope": scope,
            "Id": &id,
            "LockToken": &lock,
            "Addresses": after,
        });
        if let Some(description) = description_after {
            request["Description"] = json!(description);
        }
        let updated = waf.ok("UpdateIPSet", request).await;
        let next_lock = updated["NextLockToken"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            is_model_uuid(&next_lock),
            "case {case:?}: NextLockToken must have the modelled form, got {updated}"
        );
        assert_ne!(
            next_lock, lock,
            "case {case:?}: a successful update must rotate the lock token, got \
             {updated}"
        );

        let got = get_ip_set(&waf, scope, &name, &id).await;
        assert_eq!(
            membership(&got),
            sorted(stored_after),
            "case {case:?}: expected exactly the submitted addresses as the set they \
             name, got {got}"
        );
        let stored_order = addresses(&got);
        assert_eq!(
            got["IPSet"]["Description"],
            json!(description_after.unwrap_or_default()),
            "case {case:?}: expected the submitted description (empty when the \
             update omitted it), got {got}"
        );
        assert_eq!(
            got["LockToken"],
            json!(&next_lock),
            "case {case:?}: GetIPSet must report the rotated lock token, got {got}"
        );
        // The immutable members survive the replacement.
        assert_eq!(
            got["IPSet"]["IPAddressVersion"],
            json!(version),
            "case {case:?}: IPAddressVersion is immutable, got {got}"
        );
        assert_eq!(
            got["IPSet"]["Id"],
            json!(&id),
            "case {case:?}: Id is immutable, got {got}"
        );

        // The token the caller just spent no longer works, and the refused retry
        // leaves the replacement in place.
        waf.refused(
            "UpdateIPSet",
            json!({
                "Name": &name,
                "Scope": scope,
                "Id": &id,
                "LockToken": &lock,
                "Addresses": ["10.9.9.0/24"],
            }),
            "WAFOptimisticLockException",
            case,
        )
        .await;
        let again = get_ip_set(&waf, scope, &name, &id).await;
        assert_eq!(
            addresses(&again),
            stored_order,
            "case {case:?}: a refused update must change nothing, got {again}"
        );
    }
}

/// A refused `UpdateIPSet` changes nothing: not the addresses, not the
/// description, not the lock token.
///
/// One row per way the request can be wrong — an unknown name, an unknown id, a
/// stale lock token, an address that is not CIDR, and an address of the other
/// family — each with the error code the model gives that failure. (A `Scope`
/// outside the enum is refused by the request boundary itself, before any handler
/// runs, so it is not a row here.)
#[tokio::test]
async fn a_refused_update_leaves_the_stored_ip_set_untouched() {
    let waf = Waf::new();
    let before: &[&str] = &["10.0.0.0/24", "10.0.1.0/24"];
    let (id, _arn, lock) = create_ip_set(
        &waf,
        "REGIONAL",
        "refused-ips",
        "IPV4",
        before,
        Some("kept"),
    )
    .await;
    let stale = "11111111-2222-3333-4444-555555555555";
    let absent_id = "66666666-7777-8888-9999-aaaaaaaaaaaa";
    // The set as the store holds it, order included: what every read below must
    // still answer.
    let stored = addresses(&get_ip_set(&waf, "REGIONAL", "refused-ips", &id).await);
    assert_eq!(
        membership_of(&stored),
        sorted(before),
        "the set under test must hold exactly the blocks it was created with, got \
         {stored:?}"
    );

    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a name that names no set",
            json!({
                "Name": "no-such-ips",
                "Scope": "REGIONAL",
                "Id": &id,
                "LockToken": &lock,
                "Addresses": ["172.16.0.0/16"],
            }),
            "WAFNonexistentItemException",
        ),
        (
            "an id that names no set",
            json!({
                "Name": "refused-ips",
                "Scope": "REGIONAL",
                "Id": absent_id,
                "LockToken": &lock,
                "Addresses": ["172.16.0.0/16"],
            }),
            "WAFNonexistentItemException",
        ),
        (
            "the set of the other scope",
            json!({
                "Name": "refused-ips",
                "Scope": "CLOUDFRONT",
                "Id": &id,
                "LockToken": &lock,
                "Addresses": ["172.16.0.0/16"],
            }),
            "WAFNonexistentItemException",
        ),
        (
            "a lock token that was never issued",
            json!({
                "Name": "refused-ips",
                "Scope": "REGIONAL",
                "Id": &id,
                "LockToken": stale,
                "Addresses": ["172.16.0.0/16"],
            }),
            "WAFOptimisticLockException",
        ),
        (
            "an address that is not CIDR notation",
            json!({
                "Name": "refused-ips",
                "Scope": "REGIONAL",
                "Id": &id,
                "LockToken": &lock,
                "Addresses": ["172.16.0.0"],
            }),
            "WAFInvalidParameterException",
        ),
        (
            "a /0 range, the one range WAF documents as unsupported",
            json!({
                "Name": "refused-ips",
                "Scope": "REGIONAL",
                "Id": &id,
                "LockToken": &lock,
                "Addresses": ["0.0.0.0/0"],
            }),
            "WAFInvalidParameterException",
        ),
        (
            "an address of the other family",
            json!({
                "Name": "refused-ips",
                "Scope": "REGIONAL",
                "Id": &id,
                "LockToken": &lock,
                "Addresses": ["2001:db8::/32"],
            }),
            "WAFInvalidParameterException",
        ),
        (
            "one bad address among good ones",
            json!({
                "Name": "refused-ips",
                "Scope": "REGIONAL",
                "Id": &id,
                "LockToken": &lock,
                "Addresses": ["172.16.0.0/16", "nonsense", "10.8.0.0/16"],
            }),
            "WAFInvalidParameterException",
        ),
    ];

    for (case, request, code) in cases {
        waf.refused("UpdateIPSet", request, code, case).await;
        let got = get_ip_set(&waf, "REGIONAL", "refused-ips", &id).await;
        assert_eq!(
            addresses(&got),
            stored,
            "case {case:?}: a refused update must not replace the addresses, got \
             {got}"
        );
        assert_eq!(
            got["IPSet"]["Description"],
            json!("kept"),
            "case {case:?}: a refused update must not clear the description, got \
             {got}"
        );
        assert_eq!(
            got["LockToken"],
            json!(&lock),
            "case {case:?}: a refused update must not rotate the lock token, got \
             {got}"
        );
    }
}

/// A deleted IP set is GONE: `GetIPSet` for the same triple is refused, and the
/// name is free for a fresh set that is not a duplicate.
///
/// The rows differ in what happened to the set before the delete — straight after
/// the create, after an update rotated its token, after a web ACL referenced it
/// and was itself deleted — and in the scope it lives in, because the delete must
/// drop the entry and every index that pointed at it in each case.
#[tokio::test]
async fn a_deleted_ip_set_is_no_longer_readable() {
    let cases: &[(&str, &str, bool, bool)] = &[
        // (case, scope, update before deleting, reference from a web ACL first)
        ("straight after the create", "REGIONAL", false, false),
        ("after an update rotated the token", "REGIONAL", true, false),
        ("after a web ACL referenced it", "REGIONAL", false, true),
        ("a CLOUDFRONT-scoped set", "CLOUDFRONT", false, false),
    ];

    let waf = Waf::new();
    for (index, (case, scope, update_first, reference_first)) in cases.iter().enumerate() {
        let name = format!("deleted-ips-{index}");
        let (id, arn, lock) = create_ip_set(
            &waf,
            scope,
            &name,
            "IPV4",
            &["10.4.0.0/16"],
            Some("to be deleted"),
        )
        .await;
        let mut lock = lock;

        if *update_first {
            let updated = waf
                .ok(
                    "UpdateIPSet",
                    json!({
                        "Name": &name,
                        "Scope": scope,
                        "Id": &id,
                        "LockToken": &lock,
                        "Addresses": ["10.5.0.0/16", "10.6.0.0/16"],
                    }),
                )
                .await;
            lock = updated["NextLockToken"]
                .as_str()
                .expect("NextLockToken")
                .to_string();
        }

        if *reference_first {
            let acl_name = format!("deleted-ips-acl-{index}");
            let created = waf
                .ok(
                    "CreateWebACL",
                    json!({
                        "Name": &acl_name,
                        "Scope": scope,
                        "DefaultAction": { "Allow": {} },
                        "VisibilityConfig": visibility(&acl_name),
                        "Rules": [crate::web_acl_lifecycle::rule(
                            "reference",
                            json!({ "IPSetReferenceStatement": { "ARN": &arn } }),
                        )],
                    }),
                )
                .await;
            let acl_id = created["Summary"]["Id"].as_str().expect("Summary.Id");
            let acl_lock = created["Summary"]["LockToken"]
                .as_str()
                .expect("Summary.LockToken");
            // While the web ACL references it, the set cannot be deleted.
            waf.refused(
                "DeleteIPSet",
                json!({
                    "Name": &name,
                    "Scope": scope,
                    "Id": &id,
                    "LockToken": &lock,
                }),
                "WAFAssociatedItemException",
                case,
            )
            .await;
            waf.ok(
                "DeleteWebACL",
                json!({
                    "Name": &acl_name,
                    "Scope": scope,
                    "Id": acl_id,
                    "LockToken": acl_lock,
                }),
            )
            .await;
        }

        // A delete carrying a token that is not the current one leaves the set
        // alive.
        waf.refused(
            "DeleteIPSet",
            json!({
                "Name": &name,
                "Scope": scope,
                "Id": &id,
                "LockToken": "11111111-2222-3333-4444-555555555555",
            }),
            "WAFOptimisticLockException",
            case,
        )
        .await;
        get_ip_set(&waf, scope, &name, &id).await;

        waf.ok(
            "DeleteIPSet",
            json!({
                "Name": &name,
                "Scope": scope,
                "Id": &id,
                "LockToken": &lock,
            }),
        )
        .await;

        // Gone, and gone on every read path: by the same triple, and by the name
        // its index entry was keyed under.
        let (status, body) = waf
            .call(
                "GetIPSet",
                json!({ "Name": &name, "Scope": scope, "Id": &id }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: GetIPSet after DeleteIPSet must answer 400, got status \
             {status} and body {body}"
        );
        let error_type = body["__type"].as_str().unwrap_or_default();
        assert!(
            error_type.ends_with("WAFNonexistentItemException"),
            "case {case:?}: expected WAFNonexistentItemException, got {error_type:?} \
             in body {body}"
        );
        // Deleting it twice is refused the same way: the entry is really gone, not
        // merely hidden from the read path.
        waf.refused(
            "DeleteIPSet",
            json!({
                "Name": &name,
                "Scope": scope,
                "Id": &id,
                "LockToken": &lock,
            }),
            "WAFNonexistentItemException",
            case,
        )
        .await;

        // The name is free again, so a fresh set under it is not a duplicate — and
        // it is a NEW resource, with its own id.
        let (fresh_id, _fresh_arn, _fresh_lock) = create_ip_set(
            &waf,
            scope,
            &name,
            "IPV4",
            &["10.7.0.0/16"],
            Some("recreated"),
        )
        .await;
        assert_ne!(
            fresh_id, id,
            "case {case:?}: a set recreated under a deleted set's name is a new \
             resource"
        );
        let got = get_ip_set(&waf, scope, &name, &fresh_id).await;
        assert_eq!(
            got["IPSet"]["Addresses"],
            json!(["10.7.0.0/16"]),
            "case {case:?}: the recreated set must answer its own addresses, got \
             {got}"
        );
    }
}
