// SPDX-License-Identifier: CC0-1.0

//! The CloudFront key group control plane, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront's key group surface states:
//!
//! * a key group is PERSISTED by the call that creates it, which MINTS its identity (an id
//!   and a version token) and records the instant it was written, so `GetKeyGroup` answers
//!   from that record rather than from the request being served — down to the members the
//!   create left out;
//! * `KeyGroupConfig$Name` is "a name to identify the key group": a name another group
//!   already holds cannot name a second one (`KeyGroupAlreadyExists`, 409), and the group
//!   that holds it is left exactly as it was;
//! * "To create a key group, you must specify at least one public key for the key group",
//!   and "the comment cannot be longer than 128 characters", so a configuration outside
//!   either documented bound is refused (`InvalidArgument`, 400) and stores nothing;
//! * the number of public keys ONE group names is bounded by the published quota, over
//!   which the create is refused (`TooManyPublicKeysInKeyGroup`, 400);
//! * the number of key groups the account holds is bounded, and a create past the bound is
//!   refused (`TooManyKeyGroups`, 400) and mints nothing;
//! * an id no key group holds is `NoSuchResource` (404);
//! * a `Distribution` reports, for each key group its configuration trusts, "the
//!   identifiers of the public keys in each key group that CloudFront can use to verify
//!   the signatures" — read from that key group's stored record.
//!
//! The table rows differ along everything these rules generalise over: how many public key
//! ids a group names (one up to the published quota), whether those ids are ones
//! `CreatePublicKey` minted or opaque strings a client submitted, whether a comment is
//! carried at all and whether it is empty, plain ASCII, XML metacharacters or exactly the
//! published 128-character bound.
//!
//! The harness (the router, the request helpers and the element scanner) is the
//! distribution lifecycle suite's, reused rather than copied.

use std::collections::BTreeSet;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    id_format_invalid_message, KEY_GROUP_ALREADY_EXISTS_MESSAGE,
    KEY_GROUP_WITHOUT_A_PUBLIC_KEY_MESSAGE, MAX_KEY_GROUPS, MAX_KEY_GROUP_COMMENT,
    MAX_PUBLIC_KEYS_IN_KEY_GROUP, NO_SUCH_KEY_GROUP_MESSAGE, TOO_MANY_KEY_GROUPS_MESSAGE,
    TOO_MANY_PUBLIC_KEYS_IN_KEY_GROUP_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, create_distribution, emulator, get_distribution, is_minted_uuid, is_version_token,
    Answer, Node, API_VERSION, HOST,
};
// The public key surface is this suite's neighbour: a key group names public key ids, and
// the ids a real `CreatePublicKey` minted are the ones a client puts in one — so they come
// from that call rather than from a literal written here. The wire-timestamp predicate is
// that suite's too: one reading of how this service writes an instant, for every suite.
use crate::public_key_lifecycle::{create_key, is_wire_timestamp, pem};
// The XML escaper is the crate's: one escaper for every suite, so no two of them can
// disagree about how a submitted value is written.
use crate::response_headers_policy_lifecycle::escape;

// ---------------------------------------------------------------------------
// Calling the key group surface
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/key-group` with the `KeyGroupConfig` document `body` — the member the
/// operation binds to the HTTP payload.
pub(crate) async fn create_group(router: &MultiServiceRouter, body: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/key-group"),
        Some(body),
        None,
    )
    .await
}

/// `GET /2020-05-31/key-group/{Id}`.
pub(crate) async fn get_group(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/key-group/{id}"),
        None,
        None,
    )
    .await
}

// ---------------------------------------------------------------------------
// The configurations under test
// ---------------------------------------------------------------------------

/// One `KeyGroupConfig`, as a client submits it.
#[derive(Debug, Clone)]
pub(crate) struct Group {
    /// What this row is a case of, named in every assertion it drives.
    pub(crate) case: String,
    pub(crate) name: String,
    /// `Items`, the identifiers of the public keys in the group.
    pub(crate) items: Vec<String>,
    /// `Comment`, or `None` to leave the optional member out entirely.
    pub(crate) comment: Option<String>,
}

impl Group {
    /// The `KeyGroupConfig` XML a client sends for this configuration, in the model's
    /// member declaration order (`Name`, `Items`, `Comment`).
    pub(crate) fn xml(&self) -> String {
        let mut out = format!(
            "<KeyGroupConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">"
        );
        out.push_str(&format!("<Name>{}</Name>", escape(&self.name)));
        // `PublicKeyIdList`'s member is named `PublicKey` in XML.
        out.push_str("<Items>");
        for item in &self.items {
            out.push_str(&format!("<PublicKey>{}</PublicKey>", escape(item)));
        }
        out.push_str("</Items>");
        if let Some(comment) = &self.comment {
            out.push_str(&format!("<Comment>{}</Comment>", escape(comment)));
        }
        out.push_str("</KeyGroupConfig>");
        out
    }

    /// Assert that `config` — a `KeyGroupConfig` element the emulator serialised — reports
    /// exactly this configuration.
    pub(crate) fn assert_matches(&self, config: &Node, context: &str, raw: &str) {
        let actual = config.text_at("Name").map(str::to_string);
        assert_eq!(
            actual.as_deref(),
            Some(self.name.as_str()),
            "case {} ({context}): expected Name {:?}, got {actual:?} in {raw}",
            self.case,
            self.name
        );
        let items: Vec<String> = config
            .at("Items")
            .map(|items| {
                items
                    .children_named("PublicKey")
                    .into_iter()
                    .map(|item| item.text().to_string())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            items, self.items,
            "case {} ({context}): expected the public key ids {:?}, got {items:?} in {raw}",
            self.case, self.items
        );
        // An optional member the create left out is reported as the stored record has it:
        // absent, rather than as an empty element invented by the read.
        let actual = config.text_at("Comment").map(str::to_string);
        assert_eq!(
            actual,
            self.comment.clone(),
            "case {} ({context}): expected Comment {:?}, got {actual:?} in {raw}",
            self.case,
            self.comment
        );
    }
}

/// Upload `count` public keys and report the ids `CreatePublicKey` minted, in order.
pub(crate) async fn mint_public_keys(
    router: &MultiServiceRouter,
    tag: &str,
    count: usize,
) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for index in 0..count {
        let body = format!(
            "<PublicKeyConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <CallerReference>{tag}-{index}</CallerReference>\
             <Name>{tag}-{index}</Name>\
             <EncodedKey>{}</EncodedKey></PublicKeyConfig>",
            escape(&pem(1 + index % 3, 'K'))
        );
        let answer = create_key(router, &body).await;
        assert_eq!(
            answer.status, 201,
            "case minting {tag}-{index}: expected HTTP 201 from CreatePublicKey, got {} with {}",
            answer.status, answer.raw
        );
        ids.push(answer.root().text_at("Id").unwrap_or_default().to_string());
    }
    ids
}

/// The configurations every case below is run over.
///
/// The rows span what the rules generalise over: the number of public key ids a group
/// names — one, two, and exactly the published quota
/// ([`MAX_PUBLIC_KEYS_IN_KEY_GROUP`], the bound itself being admissible) — whether those
/// ids were minted by `CreatePublicKey` or are opaque strings a client submitted, and the
/// comment forms (absent, empty, plain ASCII, XML metacharacters, and the published
/// 128-character bound).
pub(crate) async fn table(router: &MultiServiceRouter) -> Vec<Group> {
    let minted = mint_public_keys(router, "key-group-table", MAX_PUBLIC_KEYS_IN_KEY_GROUP).await;
    let comments: [Option<String>; 5] = [
        None,
        Some(String::new()),
        Some("the key group for an emulator test".to_string()),
        Some("tags <key> & \"quotes\" and 'apostrophes'".to_string()),
        // "The comment cannot be longer than 128 characters": the bound itself is
        // admissible, so one row sits exactly on it.
        Some("c".repeat(MAX_KEY_GROUP_COMMENT)),
    ];
    let item_sets: Vec<Vec<String>> = vec![
        minted[..1].to_vec(),
        minted[..2].to_vec(),
        // The published quota on one group's key list, which is admissible.
        minted.clone(),
        // A client may submit any identifier; the group stores the list as submitted.
        vec!["K1OPAQUEIDAAAA".to_string()],
        vec![
            "K1OPAQUEIDAAAA".to_string(),
            "K2OPAQUEIDBBBB".to_string(),
            "K3OPAQUEIDCCCC".to_string(),
        ],
    ];

    item_sets
        .into_iter()
        .enumerate()
        .map(|(index, items)| Group {
            case: format!(
                "{} public key id(s) with comment {:?}",
                items.len(),
                comments[index % comments.len()]
                    .as_deref()
                    .map(|comment| comment.chars().take(12).collect::<String>())
            ),
            name: format!("key-group-{index}"),
            items,
            comment: comments[index % comments.len()].clone(),
        })
        .collect()
}

/// Create every row of `rows` through the API and report `(row, id, last_modified_time)` in
/// creation order.
async fn create_all(router: &MultiServiceRouter, rows: &[Group]) -> Vec<(Group, String, String)> {
    let mut created: Vec<(Group, String, String)> = Vec::new();
    let mut ids: BTreeSet<String> = BTreeSet::new();
    let mut etags: BTreeSet<String> = BTreeSet::new();
    for row in rows {
        let answer = create_group(router, &row.xml()).await;
        assert_eq!(
            answer.status, 201,
            "case {}: expected HTTP 201 from CreateKeyGroup, got {} with {}",
            row.case, answer.status, answer.raw
        );
        let id = answer.root().text_at("Id").unwrap_or_default().to_string();
        assert!(
            is_minted_uuid(&id),
            "case {}: expected a minted key group id, got {id:?}",
            row.case
        );
        assert!(
            ids.insert(id.clone()),
            "case {}: the id {id} repeated across creates",
            row.case
        );
        let etag = answer.header("etag").to_string();
        assert!(
            is_version_token(&etag),
            "case {}: expected a minted version token in ETag, got {etag:?}",
            row.case
        );
        assert!(
            etags.insert(etag.clone()),
            "case {}: the version token {etag} repeated across creates",
            row.case
        );
        let location = answer.header("location").to_string();
        assert_eq!(
            location,
            format!("https://{HOST}/{API_VERSION}/key-group/{id}"),
            "case {}: expected the new group's URI in Location, got {location:?}",
            row.case
        );
        let last_modified = answer
            .root()
            .text_at("LastModifiedTime")
            .unwrap_or_default()
            .to_string();
        assert!(
            is_wire_timestamp(&last_modified),
            "case {}: expected an ISO-8601 LastModifiedTime, got {last_modified:?} in {}",
            row.case,
            answer.raw
        );
        let config = answer
            .root()
            .at("KeyGroupConfig")
            .unwrap_or_else(|| panic!("case {}: no configuration in {}", row.case, answer.raw));
        row.assert_matches(config, "CreateKeyGroup", &answer.raw);
        created.push((row.clone(), id, last_modified));
    }
    created
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_persists_the_configuration_every_read_then_reports() {
    let router = emulator();
    let rows = table(&router).await;
    let created = create_all(&router, &rows).await;

    for (row, id, last_modified) in &created {
        let read = get_group(&router, id).await;
        assert_eq!(
            read.status, 200,
            "case {}: expected HTTP 200 from GetKeyGroup, got {} with {}",
            row.case, read.status, read.raw
        );
        assert_eq!(
            read.root().text_at("Id"),
            Some(id.as_str()),
            "case {}: expected the stored id {id} in the read, got {}",
            row.case,
            read.raw
        );
        // "The date and time when the key group was last modified": written by the create,
        // so a read reports that same instant rather than the instant of the read.
        assert_eq!(
            read.root().text_at("LastModifiedTime"),
            Some(last_modified.as_str()),
            "case {}: expected the stored LastModifiedTime {last_modified}, got {}",
            row.case,
            read.raw
        );
        let config = read
            .root()
            .at("KeyGroupConfig")
            .unwrap_or_else(|| panic!("case {}: no configuration in {}", row.case, read.raw));
        row.assert_matches(config, "GetKeyGroup", &read.raw);
        assert!(
            is_version_token(read.header("etag")),
            "case {}: expected a version token in the read's ETag, got {:?}",
            row.case,
            read.header("etag")
        );

        // A read must not change what a later caller sees: the same request answers the
        // same record again, at the same version.
        let again = get_group(&router, id).await;
        assert_eq!(
            again.raw, read.raw,
            "case {}: a repeated read must answer the same record, got {}",
            row.case, again.raw
        );
        assert_eq!(
            again.header("etag"),
            read.header("etag"),
            "case {}: a repeated read must answer at the same version",
            row.case
        );
    }
}

#[tokio::test]
async fn a_name_identifies_one_key_group() {
    let router = emulator();
    let minted = mint_public_keys(&router, "key-group-name", 3).await;
    let row = Group {
        case: "the group the account already holds".to_string(),
        name: "key-group-already-taken".to_string(),
        items: minted[..1].to_vec(),
        comment: Some("the first create's comment".to_string()),
    };
    let created = create_group(&router, &row.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201, got {} with {}",
        row.case, created.status, created.raw
    );
    let id = created.root().text_at("Id").unwrap_or_default().to_string();
    let etag = created.header("etag").to_string();

    // Every create under the taken name is refused, whatever else it says: the name is
    // what identifies the group, so the request describes no second one.
    let variants = [
        Group {
            case: "the same configuration again".to_string(),
            ..row.clone()
        },
        Group {
            case: "a different key list".to_string(),
            items: minted.clone(),
            ..row.clone()
        },
        Group {
            case: "a different comment".to_string(),
            comment: Some("another comment".to_string()),
            ..row.clone()
        },
        Group {
            case: "no comment at all".to_string(),
            comment: None,
            ..row.clone()
        },
    ];
    for variant in &variants {
        let answer = create_group(&router, &variant.xml()).await;
        assert_eq!(
            answer.status, 409,
            "case {}: expected HTTP 409 under a taken name, got {} with {}",
            variant.case, answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "KeyGroupAlreadyExists",
            "case {}: expected KeyGroupAlreadyExists, got {}",
            variant.case,
            answer.raw
        );
        let message = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, KEY_GROUP_ALREADY_EXISTS_MESSAGE,
            "case {}: expected the taken-name sentence, got {}",
            variant.case, answer.raw
        );
    }

    // The group that holds the name is exactly as its own create left it.
    let read = get_group(&router, &id).await;
    assert_eq!(
        read.status, 200,
        "case {}: expected the stored group to be readable, got {} with {}",
        row.case, read.status, read.raw
    );
    assert_eq!(
        read.header("etag"),
        etag,
        "case {}: a refused create must not version the stored group",
        row.case
    );
    let config = read
        .root()
        .at("KeyGroupConfig")
        .unwrap_or_else(|| panic!("case {}: no configuration in {}", row.case, read.raw));
    row.assert_matches(config, "GetKeyGroup after the refusals", &read.raw);

    // The same configuration under a FRESH name is a second group, with its own id.
    let second = Group {
        case: "the same keys under another name".to_string(),
        name: "key-group-second-name".to_string(),
        ..row.clone()
    };
    let answer = create_group(&router, &second.xml()).await;
    assert_eq!(
        answer.status, 201,
        "case {}: expected HTTP 201, got {} with {}",
        second.case, answer.status, answer.raw
    );
    let other = answer.root().text_at("Id").unwrap_or_default().to_string();
    assert_ne!(
        other, id,
        "case {}: expected a second group with its own id",
        second.case
    );
}

#[tokio::test]
async fn a_configuration_outside_the_documented_bounds_is_refused() {
    let router = emulator();
    let minted = mint_public_keys(&router, "key-group-bounds", 3).await;

    // Each row is a configuration past one documented bound, with the code and the
    // sentence the operation models for it.
    let rows = [
        (
            Group {
                case: "no public key at all".to_string(),
                name: "key-group-empty-items".to_string(),
                items: Vec::new(),
                comment: None,
            },
            "InvalidArgument",
            KEY_GROUP_WITHOUT_A_PUBLIC_KEY_MESSAGE,
        ),
        (
            Group {
                case: "no public key and a comment".to_string(),
                name: "key-group-empty-items-commented".to_string(),
                items: Vec::new(),
                comment: Some("a group that names nothing".to_string()),
            },
            "InvalidArgument",
            KEY_GROUP_WITHOUT_A_PUBLIC_KEY_MESSAGE,
        ),
        (
            Group {
                case: "a comment one character past the bound".to_string(),
                name: "key-group-long-comment".to_string(),
                items: minted[..1].to_vec(),
                comment: Some("c".repeat(MAX_KEY_GROUP_COMMENT + 1)),
            },
            "InvalidArgument",
            // The refusal states the constraint and the length submitted.
            "",
        ),
        (
            Group {
                case: "one public key past the published quota".to_string(),
                name: "key-group-too-many-keys".to_string(),
                items: (0..=MAX_PUBLIC_KEYS_IN_KEY_GROUP)
                    .map(|index| format!("K{index}QUOTAIDAAAAA"))
                    .collect(),
                comment: None,
            },
            "TooManyPublicKeysInKeyGroup",
            TOO_MANY_PUBLIC_KEYS_IN_KEY_GROUP_MESSAGE,
        ),
    ];

    for (row, code, message) in &rows {
        let answer = create_group(&router, &row.xml()).await;
        assert_eq!(
            answer.status, 400,
            "case {}: expected HTTP 400, got {} with {}",
            row.case, answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            *code,
            "case {}: expected {code}, got {}",
            row.case,
            answer.raw
        );
        if !message.is_empty() {
            let actual = answer.root().find_text("Message").unwrap_or_default();
            assert_eq!(
                actual, *message,
                "case {}: expected the modelled sentence, got {}",
                row.case, answer.raw
            );
        }

        // The refusal stored nothing: the name it named is still free, so a configuration
        // that satisfies every bound takes it.
        let repaired = Group {
            case: format!("{} repaired", row.case),
            items: minted[..1].to_vec(),
            comment: None,
            ..row.clone()
        };
        let accepted = create_group(&router, &repaired.xml()).await;
        assert_eq!(
            accepted.status, 201,
            "case {}: expected the repaired configuration to be accepted, got {} with {}",
            repaired.case, accepted.status, accepted.raw
        );
        let id = accepted
            .root()
            .text_at("Id")
            .unwrap_or_default()
            .to_string();
        let read = get_group(&router, &id).await;
        let config = read.root().at("KeyGroupConfig").unwrap_or_else(|| {
            panic!("case {}: no configuration in {}", repaired.case, read.raw)
        });
        repaired.assert_matches(config, "GetKeyGroup", &read.raw);
    }
}

#[tokio::test]
async fn an_id_no_key_group_holds_is_not_found() {
    let router = emulator();
    let minted = mint_public_keys(&router, "key-group-missing", 1).await;
    let row = Group {
        case: "the one stored group".to_string(),
        name: "key-group-stored".to_string(),
        items: minted.clone(),
        comment: None,
    };
    let created = create_group(&router, &row.xml()).await;
    let stored_id = created.root().text_at("Id").unwrap_or_default().to_string();

    // Every id no group holds is the modelled not-found, whatever shape it has — one code
    // and one status for both conditions, so the MESSAGE is what tells an id in the shape a
    // key group id takes from a value that was never one.
    let id_format_invalid = id_format_invalid_message("Id");
    let cases = [
        (
            "a UUID no group holds",
            "f9a6a0b4-8d39-4a1e-9f67-1b0f7f5d2c3e".to_string(),
            NO_SUCH_KEY_GROUP_MESSAGE,
        ),
        (
            "an id from the public key namespace",
            minted[0].clone(),
            id_format_invalid.as_str(),
        ),
        (
            "an id from the distribution namespace",
            "E1AAAAAAAAAAAA".to_string(),
            id_format_invalid.as_str(),
        ),
        (
            "a value that is no id at all",
            "nope".to_string(),
            id_format_invalid.as_str(),
        ),
    ];
    for (case, id, expected) in &cases {
        let answer = get_group(&router, id).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404 for {id}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchResource",
            "case {case}: expected NoSuchResource for {id}, got {}",
            answer.raw
        );
        // The sentence names the resource the operation ADDRESSES: a key group.
        let message = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, *expected,
            "case {case}: expected {expected:?} for {id}, got {message:?} in {}",
            answer.raw
        );
    }

    // The refused reads changed nothing: the stored group still answers its own record.
    let read = get_group(&router, &stored_id).await;
    assert_eq!(
        read.status, 200,
        "case the one stored group: expected HTTP 200, got {} with {}",
        read.status, read.raw
    );
}

/// How many key groups ONE account holds is bounded ([`MAX_KEY_GROUPS`]): a group record is
/// what every later read answers from, so it cannot be evicted, and the create AT the bound
/// is accepted while every create PAST it is refused with the error the operation models
/// for a full account (`TooManyKeyGroups`, HTTP 400). The refusal mints nothing: the groups
/// already stored are readable and unchanged, and the name a refused create named is still
/// free.
#[tokio::test]
async fn a_create_past_the_account_bound_is_refused() {
    let router = emulator();

    // The bound is an ENTRY COUNT, so what matters is the create that fills it and the ones
    // past it; the rows filling it still vary the configuration's forms, so the count is
    // what refuses them rather than anything about one group.
    let mut filled: Vec<(String, Group)> = Vec::new();
    for index in 0..MAX_KEY_GROUPS {
        let row = Group {
            case: format!("group {index} of the bound"),
            name: format!("key-group-bound-{index}"),
            items: (0..=(index % MAX_PUBLIC_KEYS_IN_KEY_GROUP))
                .map(|key| format!("K{index}BOUND{key}AAAAA"))
                .collect(),
            comment: match index % 3 {
                0 => None,
                1 => Some(String::new()),
                _ => Some(format!("group {index}")),
            },
        };
        let answer = create_group(&router, &row.xml()).await;
        assert_eq!(
            answer.status, 201,
            "case {}: expected HTTP 201 within the bound, got {} with {}",
            row.case, answer.status, answer.raw
        );
        let id = answer.root().text_at("Id").unwrap_or_default().to_string();
        filled.push((id, row));
    }

    let past = [
        Group {
            case: "one group past the bound".to_string(),
            name: "key-group-past-the-bound".to_string(),
            items: vec!["K1PASTIDAAAAAA".to_string()],
            comment: None,
        },
        Group {
            case: "another group past the bound".to_string(),
            name: "key-group-past-the-bound-again".to_string(),
            items: vec![
                "K2PASTIDAAAAAA".to_string(),
                "K3PASTIDBBBBBB".to_string(),
            ],
            comment: Some("past the bound".to_string()),
        },
    ];
    for row in &past {
        // Twice: the first refusal must leave the name it named free, so the repeat is
        // refused by the bound again rather than by the name.
        for attempt in 1..=2 {
            let answer = create_group(&router, &row.xml()).await;
            assert_eq!(
                answer.status, 400,
                "case {} (attempt {attempt}): expected HTTP 400 past the bound, got {} with {}",
                row.case, answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "TooManyKeyGroups",
                "case {} (attempt {attempt}): expected TooManyKeyGroups, got {}",
                row.case,
                answer.raw
            );
            let message = answer.root().find_text("Message").unwrap_or_default();
            assert_eq!(
                message, TOO_MANY_KEY_GROUPS_MESSAGE,
                "case {} (attempt {attempt}): expected the full-account sentence, got {}",
                row.case, answer.raw
            );
        }
    }

    // A group stored before the refusals is still readable, at the configuration its own
    // create left behind.
    for (id, row) in [&filled[0], &filled[MAX_KEY_GROUPS - 1]] {
        let read = get_group(&router, id).await;
        assert_eq!(
            read.status, 200,
            "case {}: expected the stored group to be readable, got {} with {}",
            row.case, read.status, read.raw
        );
        let config = read
            .root()
            .at("KeyGroupConfig")
            .unwrap_or_else(|| panic!("case {}: no configuration in {}", row.case, read.raw));
        row.assert_matches(config, "GetKeyGroup after the refusals", &read.raw);
    }
}

/// The `DistributionConfig` XML of a distribution whose default cache behaviour trusts
/// `groups`, in the member order `DistributionConfig` declares.
fn distribution_trusting(reference: &str, groups: &[String]) -> String {
    let items: String = groups
        .iter()
        .map(|group| format!("<KeyGroup>{}</KeyGroup>", escape(group)))
        .collect();
    format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items><Origin><Id>primary</Id>\
         <DomainName>origin.example.com</DomainName>\
         <CustomOriginConfig><HTTPPort>80</HTTPPort><HTTPSPort>443</HTTPSPort>\
         <OriginProtocolPolicy>https-only</OriginProtocolPolicy></CustomOriginConfig>\
         </Origin></Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
         <TrustedKeyGroups><Enabled>true</Enabled><Quantity>{}</Quantity>\
         <Items>{items}</Items></TrustedKeyGroups>\
         <ViewerProtocolPolicy>https-only</ViewerProtocolPolicy></DefaultCacheBehavior>\
         <Comment>signed urls</Comment><Enabled>true</Enabled></DistributionConfig>",
        groups.len()
    )
}

/// The `(KeyGroupId, the public key ids it reports)` of every entry of a `Distribution`'s
/// active key group set, in payload order.
fn active_key_groups(distribution: &Node) -> Vec<(String, Vec<String>)> {
    distribution
        .at("ActiveTrustedKeyGroups/Items")
        .map(|items| {
            items
                .children_named("KeyGroup")
                .into_iter()
                .map(|entry| {
                    let keys = entry
                        .at("KeyPairIds/Items")
                        .map(|list| {
                            list.children_named("KeyPairId")
                                .into_iter()
                                .map(|key| key.text().to_string())
                                .collect()
                        })
                        .unwrap_or_default();
                    (
                        entry.text_at("KeyGroupId").unwrap_or_default().to_string(),
                        keys,
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// "A list of key groups, and the public keys in each key group, that CloudFront can use to
/// verify the signatures of signed URLs and signed cookies": the keys a distribution
/// reports for a group it trusts are the ones that GROUP's stored record names, and a
/// trusted id the account holds no group for contributes none.
#[tokio::test]
async fn a_distribution_reports_the_public_keys_of_the_key_groups_it_trusts() {
    let router = emulator();
    let minted = mint_public_keys(&router, "key-group-trusted", 3).await;

    // Two groups of different sizes, plus a trusted id no group holds: the reported sets
    // differ per group, so none of them can come from the distribution's own request.
    let rows = [
        Group {
            case: "a group of one key".to_string(),
            name: "key-group-trusted-one".to_string(),
            items: minted[..1].to_vec(),
            comment: None,
        },
        Group {
            case: "a group of three keys".to_string(),
            name: "key-group-trusted-three".to_string(),
            items: minted.clone(),
            comment: Some("three keys".to_string()),
        },
    ];
    let mut trusted: Vec<String> = Vec::new();
    let mut expected: Vec<(String, Vec<String>)> = Vec::new();
    for row in &rows {
        let answer = create_group(&router, &row.xml()).await;
        assert_eq!(
            answer.status, 201,
            "case {}: expected HTTP 201, got {} with {}",
            row.case, answer.status, answer.raw
        );
        let id = answer.root().text_at("Id").unwrap_or_default().to_string();
        trusted.push(id.clone());
        expected.push((id, row.items.clone()));
    }
    // An id no key group holds: the distribution still trusts it, and it contributes no
    // public key.
    let absent = "f9a6a0b4-8d39-4a1e-9f67-1b0f7f5d2c3e".to_string();
    trusted.push(absent.clone());
    expected.push((absent, Vec::new()));

    let created = create_distribution(
        &router,
        &distribution_trusting("key-group-trusting-distribution", &trusted),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case the trusting distribution: expected HTTP 201, got {} with {}",
        created.status, created.raw
    );
    let actual = active_key_groups(created.root());
    assert_eq!(
        actual, expected,
        "case the trusting distribution (CreateDistribution): expected the trusted groups' \
         stored keys {expected:?}, got {actual:?} in {}",
        created.raw
    );

    // The read answers from the same records, so it reports the same sets.
    let id = created.root().text_at("Id").unwrap_or_default().to_string();
    let read = get_distribution(&router, &id).await;
    assert_eq!(
        read.status, 200,
        "case the trusting distribution: expected HTTP 200 from GetDistribution, got {} with {}",
        read.status, read.raw
    );
    let actual = active_key_groups(read.root());
    assert_eq!(
        actual, expected,
        "case the trusting distribution (GetDistribution): expected {expected:?}, got \
         {actual:?} in {}",
        read.raw
    );
}
