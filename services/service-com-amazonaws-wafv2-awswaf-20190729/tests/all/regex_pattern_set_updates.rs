// SPDX-License-Identifier: CC0-1.0

//! `UpdateRegexPatternSet` and `DeleteRegexPatternSet` against stored state, end to
//! end over the wire.
//!
//! The rule asserted here: a successful WAFv2 mutation leaves the stored resource in
//! the state the call describes and rotates its optimistic-lock token, while a
//! mutation whose token is not the resource's current one, or whose identity triple
//! names no stored resource, is refused and changes nothing. Concretely, for a regex
//! pattern set —
//!
//! * `UpdateRegexPatternSet` "completely replaces the mutable specifications that
//!   you already have for the regex pattern set with the ones that you provide to
//!   this call", so afterwards `GetRegexPatternSet` answers EXACTLY the submitted
//!   `RegularExpressionList`, in the submitted order and byte for byte, and a
//!   `Description` the call omitted is gone, while `Name`, `Id` and `ARN` are
//!   unchanged;
//! * `LockToken` is "a token used for optimistic locking … If a change has been
//!   made, the update fails with a WAFOptimisticLockException", so the token the
//!   caller spent stops working and the update answers a fresh `NextLockToken`,
//!   which is what the next read reports — hence of several updates racing on one
//!   token exactly one succeeds;
//! * a `(Scope, Name, Id)` triple naming no stored set is refused with
//!   `WAFNonexistentItemException`;
//! * `DeleteRegexPatternSet` "deletes the specified RegexPatternSet", so afterwards
//!   `GetRegexPatternSet` for the same triple is refused with
//!   `WAFNonexistentItemException` (HTTP 400), the name is free again and the ARN
//!   resolves to nothing — while a set a web ACL still references cannot be deleted
//!   at all (`WAFAssociatedItemException`).
//!
//! Every call here goes through the same front door a real client uses (the `Waf`
//! harness in [`crate::web_acl_lifecycle`]), so what these tests observe is what an
//! SDK observes.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::regex_pattern_set_lifecycle::{
    create_regex_pattern_set, get_regex_pattern_set, regex_pattern_set_arn,
};
use crate::web_acl_lifecycle::{is_model_uuid, rule, visibility, Waf};

/// `[{ "RegexString": … }, …]` — a `RegularExpressionList` over `patterns`.
fn regular_expression_list<S: AsRef<str>>(patterns: &[S]) -> Value {
    Value::Array(
        patterns
            .iter()
            .map(|pattern| json!({ "RegexString": pattern.as_ref() }))
            .collect(),
    )
}

/// An `UpdateRegexPatternSet` request for `(scope, name, id)` with `lock_token`.
fn update_request<S: AsRef<str>>(
    scope: &str,
    name: &str,
    id: &str,
    lock_token: &str,
    patterns: &[S],
    description: Option<&str>,
) -> Value {
    let mut request = json!({
        "Name": name,
        "Scope": scope,
        "Id": id,
        "LockToken": lock_token,
        "RegularExpressionList": regular_expression_list(patterns),
    });
    if let Some(description) = description {
        request["Description"] = json!(description);
    }
    request
}

/// One whole-replacement case: what the set held, and what the update submits.
struct Replacement {
    /// What the case is called in an assertion.
    case: &'static str,
    /// The scope the set lives in; it is part of the set's identity.
    scope: &'static str,
    /// The patterns the set is created with.
    before: Vec<String>,
    /// The description the set is created with.
    description_before: Option<&'static str>,
    /// The patterns the update submits, in the order it submits them.
    after: Vec<String>,
    /// The description the update submits, or `None` when it omits the member.
    description_after: Option<&'static str>,
}

/// `UpdateRegexPatternSet` replaces the whole mutable specification, and
/// `GetRegexPatternSet` answers what was submitted.
///
/// The rows differ along everything the replacement generalises over: the NUMBER of
/// patterns (fewer, more, none at all), their CONTENT (plain text, regex
/// metacharacters and escapes, non-ASCII scripts and an astral-plane character),
/// their ORDER (the same three patterns reordered), their SIZE (a one-character
/// pattern and a 512-character one — `RegexPatternString` is `@length(1..512)`, per
/// `smithy_query(query="shape",shape="Regex")`), the SCOPE, and whether the update
/// carries a `Description` at all. A `Description` the update omits reads back as
/// `""`, the empty description every WAFv2 read reports for an entity saved without
/// one.
#[tokio::test]
async fn update_regex_pattern_set_replaces_the_whole_specification() {
    let cases: Vec<Replacement> = vec![
        Replacement {
            case: "one pattern replaces two, and the description is dropped",
            scope: "REGIONAL",
            before: vec!["^/a$".to_string(), "^/b$".to_string()],
            description_before: Some("two paths"),
            after: vec!["^/only$".to_string()],
            description_after: None,
        },
        Replacement {
            case: "three patterns replace one, and a description is added",
            scope: "REGIONAL",
            before: vec!["^/one$".to_string()],
            description_before: None,
            after: vec![
                "^/x$".to_string(),
                "^/y$".to_string(),
                "^/z$".to_string(),
            ],
            description_after: Some("three paths"),
        },
        Replacement {
            case: "the same three patterns in the opposite order",
            scope: "REGIONAL",
            before: vec![
                "^/x$".to_string(),
                "^/y$".to_string(),
                "^/z$".to_string(),
            ],
            description_before: Some("ordered"),
            after: vec![
                "^/z$".to_string(),
                "^/y$".to_string(),
                "^/x$".to_string(),
            ],
            description_after: Some("reordered"),
        },
        Replacement {
            case: "an empty list clears every pattern",
            scope: "REGIONAL",
            before: vec!["^/gone$".to_string(), "^/also-gone$".to_string()],
            description_before: Some("to be emptied"),
            after: vec![],
            description_after: Some("emptied"),
        },
        Replacement {
            case: "regex metacharacters, classes and escapes",
            scope: "REGIONAL",
            before: vec!["plain".to_string()],
            description_before: None,
            after: vec![
                r"(?i)\bselect\b.*\bfrom\b".to_string(),
                r"[0-9]{3}-[0-9]{2}-[0-9]{4}".to_string(),
                r"\\x41+|\t\s*$".to_string(),
            ],
            description_after: Some("metacharacters"),
        },
        Replacement {
            case: "non-ASCII patterns across scripts, including an astral-plane one",
            scope: "REGIONAL",
            before: vec!["ascii-only".to_string()],
            description_before: Some("ascii before"),
            after: vec![
                "привет".to_string(),
                "東京+".to_string(),
                "🙈🙉🙊".to_string(),
                "cafe\u{0301}?".to_string(),
            ],
            description_after: Some("unicode-patterns"),
        },
        Replacement {
            case: "a one-character pattern, the modelled length floor",
            scope: "REGIONAL",
            before: vec!["^/longer$".to_string()],
            description_before: Some("before the floor"),
            after: vec!["a".to_string()],
            description_after: None,
        },
        Replacement {
            case: "a 512-character pattern, the modelled length ceiling",
            scope: "REGIONAL",
            before: vec!["z".to_string()],
            description_before: None,
            after: vec!["z".repeat(512)],
            description_after: Some("at the ceiling"),
        },
        Replacement {
            case: "a CLOUDFRONT-scoped set is replaced too",
            scope: "CLOUDFRONT",
            before: vec!["^/edge$".to_string()],
            description_before: Some("global"),
            after: vec!["^/global$".to_string(), r"\.js$".to_string()],
            description_after: None,
        },
    ];

    let waf = Waf::new();
    let mut minted: BTreeSet<String> = BTreeSet::new();
    for (index, replacement) in cases.iter().enumerate() {
        let Replacement {
            case,
            scope,
            before,
            description_before,
            after,
            description_after,
        } = replacement;
        let name = format!("updated-patterns-{index}");
        let (id, arn, lock) =
            create_regex_pattern_set(&waf, scope, &name, before, *description_before).await;
        assert!(
            minted.insert(lock.clone()),
            "case {case:?}: every create must mint a fresh LockToken, but {lock:?} \
             repeated"
        );

        let updated = waf
            .ok(
                "UpdateRegexPatternSet",
                update_request(scope, &name, &id, &lock, after, *description_after),
            )
            .await;
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
        assert!(
            minted.insert(next_lock.clone()),
            "case {case:?}: every update must mint a fresh LockToken, but \
             {next_lock:?} repeated"
        );

        let got = get_regex_pattern_set(&waf, scope, &name, &id).await;
        let stored = &got["RegexPatternSet"];
        assert_eq!(
            stored["RegularExpressionList"],
            regular_expression_list(after),
            "case {case:?}: expected exactly the submitted patterns in the submitted \
             order, got {got}"
        );
        assert_eq!(
            stored["Description"],
            json!(description_after.unwrap_or_default()),
            "case {case:?}: expected the submitted description (empty when the update \
             omitted it), got {got}"
        );
        assert_eq!(
            got["LockToken"],
            json!(&next_lock),
            "case {case:?}: GetRegexPatternSet must report the rotated lock token, got \
             {got}"
        );
        // The immutable members survive the replacement.
        assert_eq!(
            stored["Name"],
            json!(&name),
            "case {case:?}: Name is immutable, got {got}"
        );
        assert_eq!(
            stored["Id"],
            json!(&id),
            "case {case:?}: Id is immutable, got {got}"
        );
        assert_eq!(
            stored["ARN"],
            json!(&arn),
            "case {case:?}: ARN is immutable, got {got}"
        );
        assert_eq!(
            arn,
            regex_pattern_set_arn(scope, &name, &id),
            "case {case:?}: expected the regexpatternset ARN of this scope, got {arn:?}"
        );

        // The token the caller just spent no longer works, and the refused retry
        // leaves the replacement in place.
        waf.refused(
            "UpdateRegexPatternSet",
            update_request(scope, &name, &id, &lock, &["^/retry$"], None),
            "WAFOptimisticLockException",
            case,
        )
        .await;
        let again = get_regex_pattern_set(&waf, scope, &name, &id).await;
        assert_eq!(
            again, got,
            "case {case:?}: a refused update must change nothing, got {again}"
        );

        // The rotated token is itself spendable, so an update chains: the second one
        // replaces the first replacement and rotates the token again.
        let chained = waf
            .ok(
                "UpdateRegexPatternSet",
                update_request(scope, &name, &id, &next_lock, &["^/chained$"], Some("chained")),
            )
            .await;
        let chained_lock = chained["NextLockToken"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            minted.insert(chained_lock.clone()),
            "case {case:?}: a chained update must mint a fresh LockToken, but \
             {chained_lock:?} repeated"
        );
        let got = get_regex_pattern_set(&waf, scope, &name, &id).await;
        assert_eq!(
            got["RegexPatternSet"]["RegularExpressionList"],
            regular_expression_list(&["^/chained$"]),
            "case {case:?}: the chained update must be the stored state, got {got}"
        );
        assert_eq!(
            got["LockToken"],
            json!(&chained_lock),
            "case {case:?}: the chained update's token must be the current one, got \
             {got}"
        );
    }
}

/// A refused `UpdateRegexPatternSet` changes nothing: not the patterns, not the
/// description, not the lock token.
///
/// One row per way the request can miss — a name that names no set, a well-formed id
/// that was never minted, another set's id under this name, another set's name under
/// this id, the right pair in the wrong scope, and a token that was never issued —
/// each with the error code the model gives that failure. (A `Scope` outside the
/// enum and a malformed `Id` or `LockToken` are refused by the request boundary
/// itself, before any handler runs, so they are not rows here.)
#[tokio::test]
async fn a_refused_update_leaves_the_stored_set_untouched() {
    let waf = Waf::new();
    let before: &[&str] = &["^/kept$", "^/also-kept$"];
    let (id, _arn, lock) =
        create_regex_pattern_set(&waf, "REGIONAL", "refused-patterns", before, Some("kept")).await;
    let (other_id, _other_arn, _other_lock) =
        create_regex_pattern_set(&waf, "REGIONAL", "other-patterns", &["^/other$"], None).await;
    let never_minted = "66666666-7777-8888-9999-aaaaaaaaaaaa";
    let never_issued = "11111111-2222-3333-4444-555555555555";
    let replacement: &[&str] = &["^/replaced$"];

    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a name that names no set",
            update_request(
                "REGIONAL",
                "no-such-patterns",
                &id,
                &lock,
                replacement,
                None,
            ),
            "WAFNonexistentItemException",
        ),
        (
            "an id that was never minted",
            update_request(
                "REGIONAL",
                "refused-patterns",
                never_minted,
                &lock,
                replacement,
                None,
            ),
            "WAFNonexistentItemException",
        ),
        (
            "another set's id under this name",
            update_request(
                "REGIONAL",
                "refused-patterns",
                &other_id,
                &lock,
                replacement,
                None,
            ),
            "WAFNonexistentItemException",
        ),
        (
            "another set's name under this id",
            update_request(
                "REGIONAL",
                "other-patterns",
                &id,
                &lock,
                replacement,
                None,
            ),
            "WAFNonexistentItemException",
        ),
        (
            "the right name and id in the other scope",
            update_request(
                "CLOUDFRONT",
                "refused-patterns",
                &id,
                &lock,
                replacement,
                None,
            ),
            "WAFNonexistentItemException",
        ),
        (
            "a lock token that was never issued",
            update_request(
                "REGIONAL",
                "refused-patterns",
                &id,
                never_issued,
                replacement,
                None,
            ),
            "WAFOptimisticLockException",
        ),
    ];

    for (case, request, code) in cases {
        waf.refused("UpdateRegexPatternSet", request, code, case)
            .await;
        let got = get_regex_pattern_set(&waf, "REGIONAL", "refused-patterns", &id).await;
        assert_eq!(
            got["RegexPatternSet"]["RegularExpressionList"],
            regular_expression_list(before),
            "case {case:?}: a refused update must not replace the patterns, got {got}"
        );
        assert_eq!(
            got["RegexPatternSet"]["Description"],
            json!("kept"),
            "case {case:?}: a refused update must not clear the description, got {got}"
        );
        assert_eq!(
            got["LockToken"],
            json!(&lock),
            "case {case:?}: a refused update must not rotate the lock token, got {got}"
        );
    }

    // The other set is untouched by every refusal aimed at this one.
    let got = get_regex_pattern_set(&waf, "REGIONAL", "other-patterns", &other_id).await;
    assert_eq!(
        got["RegexPatternSet"]["RegularExpressionList"],
        regular_expression_list(&["^/other$"]),
        "a refused update must not touch another set, got {got}"
    );
}

/// Updates racing on ONE token have exactly one winner: the first to be applied
/// rotates the token, so every other call's token is no longer the set's current one
/// and is refused with `WAFOptimisticLockException`.
///
/// The rows differ in how many callers race (two, three, five), because the
/// invariant is "one winner" whatever the count, and each racer submits its own
/// patterns so the stored set afterwards identifies the winner.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn updates_racing_on_one_token_have_a_single_winner() {
    for racers in [2usize, 3, 5] {
        let case = format!("{racers} updates sharing one lock token");
        let waf = Arc::new(Waf::new());
        let name = format!("raced-patterns-{racers}");
        let (id, _arn, lock) = create_regex_pattern_set(
            &waf,
            "REGIONAL",
            &name,
            &["^/before$"],
            Some("before the race"),
        )
        .await;

        let mut pending = Vec::with_capacity(racers);
        for racer in 0..racers {
            let pattern = format!("^/racer-{racer}$");
            let waf = Arc::clone(&waf);
            let request = update_request(
                "REGIONAL",
                &name,
                &id,
                &lock,
                std::slice::from_ref(&pattern),
                None,
            );
            pending.push(tokio::spawn(async move {
                let outcome = waf.call("UpdateRegexPatternSet", request).await;
                (pattern, outcome)
            }));
        }
        let mut outcomes = Vec::with_capacity(racers);
        for racer in pending {
            outcomes.push(racer.await.expect("the racing call completes"));
        }

        let winners: Vec<&String> = outcomes
            .iter()
            .filter(|(_, (status, _))| *status == 200)
            .map(|(pattern, _)| pattern)
            .collect();
        assert_eq!(
            winners.len(),
            1,
            "case {case:?}: expected exactly one winner, got {outcomes:?}"
        );
        for (pattern, (status, body)) in &outcomes {
            if *status == 200 {
                continue;
            }
            assert_eq!(
                *status, 400,
                "case {case:?}: a loser must answer 400, got status {status} and body \
                 {body} for {pattern:?}"
            );
            let error_type = body["__type"].as_str().unwrap_or_default();
            assert!(
                error_type.ends_with("WAFOptimisticLockException"),
                "case {case:?}: a loser must answer WAFOptimisticLockException, got \
                 {error_type:?} in body {body}"
            );
        }

        // The stored set is the winner's replacement, and its token is the one the
        // winner was handed.
        let winner = winners[0];
        let winning_token = outcomes
            .iter()
            .find(|(pattern, (status, _))| pattern == winner && *status == 200)
            .map(|(_, (_, body))| {
                body["NextLockToken"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .unwrap_or_default();
        let got = get_regex_pattern_set(&waf, "REGIONAL", &name, &id).await;
        assert_eq!(
            got["RegexPatternSet"]["RegularExpressionList"],
            regular_expression_list(&[winner]),
            "case {case:?}: the stored set must be the winner's replacement, got {got}"
        );
        assert_eq!(
            got["LockToken"],
            json!(&winning_token),
            "case {case:?}: the current token must be the winner's NextLockToken, got \
             {got}"
        );
    }
}

/// A `RegexPatternSetReferenceStatement` naming `arn`, as a web ACL rule.
fn reference_rule(arn: &str) -> Value {
    rule(
        "match-patterns",
        json!({
            "RegexPatternSetReferenceStatement": {
                "ARN": arn,
                "FieldToMatch": { "QueryString": {} },
                "TextTransformations": [{ "Priority": 0, "Type": "NONE" }],
            }
        }),
    )
}

/// A deleted regex pattern set is GONE: `GetRegexPatternSet` for the same triple is
/// refused, the name is free for a fresh set that is not a duplicate, and the ARN no
/// longer resolves as a rule reference — while a set a web ACL still references
/// cannot be deleted at all.
///
/// The rows differ in what happened to the set before the delete — straight after
/// the create, after an update rotated its token, after a web ACL referenced it and
/// was itself deleted — and in the scope it lives in, because the delete must drop
/// the entry and every index that pointed at it in each case.
#[tokio::test]
async fn a_deleted_regex_pattern_set_is_no_longer_readable() {
    // (case, scope, update before deleting, reference from a web ACL first)
    let cases: &[(&str, &str, bool, bool)] = &[
        ("straight after the create", "REGIONAL", false, false),
        ("after an update rotated the token", "REGIONAL", true, false),
        ("after a web ACL referenced it", "REGIONAL", false, true),
        ("a CLOUDFRONT-scoped set", "CLOUDFRONT", false, false),
    ];

    let waf = Waf::new();
    for (index, (case, scope, update_first, reference_first)) in cases.iter().enumerate() {
        let name = format!("deleted-patterns-{index}");
        let (id, arn, lock) = create_regex_pattern_set(
            &waf,
            scope,
            &name,
            &["^/to-be-deleted$"],
            Some("to be deleted"),
        )
        .await;
        let mut lock = lock;

        if *update_first {
            let updated = waf
                .ok(
                    "UpdateRegexPatternSet",
                    update_request(scope, &name, &id, &lock, &["^/updated$"], None),
                )
                .await;
            lock = updated["NextLockToken"]
                .as_str()
                .expect("NextLockToken")
                .to_string();
        }

        if *reference_first {
            let acl_name = format!("deleted-patterns-acl-{index}");
            let created = waf
                .ok(
                    "CreateWebACL",
                    json!({
                        "Name": &acl_name,
                        "Scope": scope,
                        "DefaultAction": { "Allow": {} },
                        "VisibilityConfig": visibility(&acl_name),
                        "Rules": [reference_rule(&arn)],
                    }),
                )
                .await;
            let acl_id = created["Summary"]["Id"].as_str().expect("Summary.Id");
            let acl_lock = created["Summary"]["LockToken"]
                .as_str()
                .expect("Summary.LockToken");
            // While the web ACL references it, the set cannot be deleted.
            waf.refused(
                "DeleteRegexPatternSet",
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
            get_regex_pattern_set(&waf, scope, &name, &id).await;
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

        // A delete carrying a token that is not the current one leaves the set alive.
        waf.refused(
            "DeleteRegexPatternSet",
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
        get_regex_pattern_set(&waf, scope, &name, &id).await;

        waf.ok(
            "DeleteRegexPatternSet",
            json!({
                "Name": &name,
                "Scope": scope,
                "Id": &id,
                "LockToken": &lock,
            }),
        )
        .await;

        // Gone, and gone on every read path: by the same triple, by the name its
        // index entry was keyed under, and by the ARN a rule could reference.
        let (status, body) = waf
            .call(
                "GetRegexPatternSet",
                json!({ "Name": &name, "Scope": scope, "Id": &id }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: GetRegexPatternSet after the delete must answer 400, got \
             status {status} and body {body}"
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
            "DeleteRegexPatternSet",
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
        // And an update of the deleted set is refused too.
        waf.refused(
            "UpdateRegexPatternSet",
            update_request(scope, &name, &id, &lock, &["^/after-delete$"], None),
            "WAFNonexistentItemException",
            case,
        )
        .await;
        // The ARN resolves to nothing, so a rule can no longer reference it.
        let acl_name = format!("dangling-reference-acl-{index}");
        waf.refused(
            "CreateWebACL",
            json!({
                "Name": &acl_name,
                "Scope": scope,
                "DefaultAction": { "Allow": {} },
                "VisibilityConfig": visibility(&acl_name),
                "Rules": [reference_rule(&arn)],
            }),
            "WAFNonexistentItemException",
            case,
        )
        .await;

        // The name is free again, so a fresh set under it is not a duplicate — and
        // it is a NEW resource, with its own id.
        let (fresh_id, _fresh_arn, _fresh_lock) = create_regex_pattern_set(
            &waf,
            scope,
            &name,
            &["^/recreated$"],
            Some("recreated"),
        )
        .await;
        assert_ne!(
            fresh_id, id,
            "case {case:?}: a set recreated under a deleted set's name is a new \
             resource"
        );
        let got = get_regex_pattern_set(&waf, scope, &name, &fresh_id).await;
        assert_eq!(
            got["RegexPatternSet"]["RegularExpressionList"],
            regular_expression_list(&["^/recreated$"]),
            "case {case:?}: the recreated set must answer its own patterns, got {got}"
        );
    }
}
