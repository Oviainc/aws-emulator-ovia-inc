// SPDX-License-Identifier: CC0-1.0

//! The ONE rule both `@idempotencyToken`-scoped creates fold their `Tags` into the
//! request a `ClientToken` is scoped to by, driven through BOTH operations that share it.
//!
//! `CreateApplication` and `CreateTrustedTokenIssuer` each judge a reused `ClientToken`
//! by comparing the whole request it first carried with the one it carries now, and a tag
//! set carries no order the caller chose: the pairs go in SORTED, key then value, each as
//! its own part. So two requests that differ only in the ORDER their tags arrived are the
//! same request — a replay, answering with the first call's ARN and minting nothing —
//! while two requests whose tag SETS differ are two requests, which the reused token is
//! refused for with `ConflictException`.
//!
//! `application_assignments.rs` and `trusted_token_issuers.rs` own the idempotency slice
//! of each operation on its own. This suite is the one that asserts the shared fold on
//! both at once, over the same table, so a change to it that reached one create and not
//! the other fails here. The `Tag`/`TagList` constraints the rows span are from
//! `smithy_query(query="shape",shape="Tag,TagList")`; the harness is the one
//! `ssoadmin_permission_sets.rs` owns.

use serde_json::{json, Value};

use crate::ssoadmin_permission_sets::{call, error_code, organization_router};

/// The provider a customer managed application is created under: the `custom` entry of
/// the published catalog, the one `CreateApplication` serves.
const PROVIDER_CUSTOM: &str = "arn:aws:sso::aws:applicationProvider/custom";

/// The only value the model's `TrustedTokenIssuerType` enum admits
/// (`smithy_query(query="shape",shape="TrustedTokenIssuerType")`).
const ISSUER_TYPE: &str = "OIDC_JWT";

/// The only value the model's `JwksRetrievalOption` enum admits
/// (`smithy_query(query="shape",shape="OidcJwtConfiguration")`).
const JWKS_RETRIEVAL_OPTION: &str = "OPEN_ID_DISCOVERY";

/// One row: a tag set, and the sets that are a DIFFERENT set rather than a reordering of
/// it.
struct Row {
    label: &'static str,
    tags: Vec<(String, String)>,
    /// Sets whose content differs from `tags`, so the same token carrying one of them is
    /// a conflict.
    different: Vec<(&'static str, Vec<(String, String)>)>,
}

fn pairs(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    tags.iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

impl Row {
    fn new(
        label: &'static str,
        tags: &[(&str, &str)],
        different: &[(&'static str, &[(&str, &str)])],
    ) -> Self {
        Self {
            label,
            tags: pairs(tags),
            different: different
                .iter()
                .map(|(why, set)| (*why, pairs(set)))
                .collect(),
        }
    }

    /// Orderings of this row's tag set that must all fingerprint alike: as sent, reversed,
    /// sorted by key, sorted by key descending, and rotated by one. A set of fewer than
    /// two tags has only itself, which still has to replay.
    fn reorderings(&self) -> Vec<(String, Vec<(String, String)>)> {
        let mut orderings = vec![("as the first call sent them".to_string(), self.tags.clone())];
        if self.tags.len() < 2 {
            return orderings;
        }
        let mut reversed = self.tags.clone();
        reversed.reverse();
        orderings.push(("reversed".to_string(), reversed));
        let mut ascending = self.tags.clone();
        ascending.sort();
        orderings.push(("sorted by key ascending".to_string(), ascending.clone()));
        let mut descending = ascending;
        descending.reverse();
        orderings.push(("sorted by key descending".to_string(), descending));
        let mut rotated = self.tags.clone();
        rotated.rotate_left(1);
        orderings.push(("rotated by one".to_string(), rotated));
        orderings
    }
}

/// The rows the rule is asserted over.
///
/// They differ along what it generalises over: the NUMBER of tags (none, one, several, and
/// the 75 `TagList` admits at its upper bound), the ORDER they are sent in, the LENGTH of
/// a key at the `TagKey` 128-character bound and of a value at both `TagValue` bounds
/// (`length 0..256`, so an empty value is in), the CHARACTER CLASSES both patterns admit
/// (`^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`: letters of any script, separators, digits and
/// `_ . : / = + - @`), and — through the `different` column — the ways a tag set can
/// really change: a value, a key, a tag added, a tag dropped, and a set whose
/// concatenation matches but whose pairs do not.
fn rows() -> Vec<Row> {
    // A `TagKey` at its 128-character upper bound, and a `TagValue` at its 256-character
    // one.
    let key_at_bound = "k".repeat(128);
    let value_at_bound = "v".repeat(256);
    let mut rows = vec![
        Row::new(
            "no Tags member at all",
            &[],
            &[("one tag added", &[("Owner", "platform")])],
        ),
        Row::new(
            "a single tag",
            &[("Owner", "platform")],
            &[
                ("its value changed", &[("Owner", "finance")]),
                ("its key changed", &[("owner", "platform")]),
                ("a second tag added", &[("Owner", "platform"), ("Env", "prod")]),
                ("every tag dropped", &[]),
            ],
        ),
        Row::new(
            "three tags whose sent order is neither ascending nor descending",
            &[("zone", "eu"), ("Env", "staging"), ("owner", "platform")],
            &[
                (
                    "one value changed",
                    &[("zone", "us"), ("Env", "staging"), ("owner", "platform")],
                ),
                ("one tag dropped", &[("zone", "eu"), ("Env", "staging")]),
            ],
        ),
        Row::new(
            "a key at its 128-character bound beside an empty value",
            &[("filled", "value"), (key_at_bound.as_str(), "")],
            &[(
                "the bounded key given a value",
                &[("filled", "value"), (key_at_bound.as_str(), "v")],
            )],
        ),
        Row::new(
            "keys and values spanning the character classes the patterns admit",
            &[
                ("cost centre", "42"),
                ("path/to:thing", "a=b+c-d@e.f"),
                ("Caf\u{e9}_\u{41e}\u{442}\u{434}\u{435}\u{43b}", "na\u{ef}ve"),
                ("padded", value_at_bound.as_str()),
            ],
            &[(
                "the separator in a key changed",
                &[
                    ("cost_centre", "42"),
                    ("path/to:thing", "a=b+c-d@e.f"),
                    ("Caf\u{e9}_\u{41e}\u{442}\u{434}\u{435}\u{43b}", "na\u{ef}ve"),
                    ("padded", value_at_bound.as_str()),
                ],
            )],
        ),
        // Each key and each value is its own part, so a set whose pairs concatenate to the
        // same text is still a different set.
        Row::new(
            "a pair whose key and value could be re-split",
            &[("a", "bc")],
            &[
                ("the same text split differently", &[("ab", "c")]),
                ("the same text as one key", &[("abc", "")]),
            ],
        ),
    ];

    // The 75 tags `TagList` admits at its upper bound, sent in descending key order.
    let many: Vec<(String, String)> = (0..75)
        .rev()
        .map(|index| (format!("key-{index:02}"), format!("value-{index}")))
        .collect();
    let mut one_value_changed = many.clone();
    one_value_changed[40].1 = "value-changed".to_string();
    let mut one_dropped = many.clone();
    one_dropped.pop();
    rows.push(Row {
        label: "75 tags, the most TagList admits, in descending key order",
        tags: many,
        different: vec![
            ("one of the 75 values changed", one_value_changed),
            ("one of the 75 tags dropped", one_dropped),
        ],
    });
    rows
}

/// The `Tags` member of a request, or no member at all for an empty set.
fn tags_member(tags: &[(String, String)]) -> Option<Value> {
    if tags.is_empty() {
        return None;
    }
    Some(Value::Array(
        tags.iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect(),
    ))
}

/// One of the two creates that share the tag fold, as this suite drives it.
#[derive(Clone, Copy)]
struct Create {
    /// The operation name on the wire.
    operation: &'static str,
    /// The output member carrying the minted resource's ARN.
    arn_member: &'static str,
}

const CREATES: &[Create] = &[
    Create {
        operation: "CreateApplication",
        arn_member: "ApplicationArn",
    },
    Create {
        operation: "CreateTrustedTokenIssuer",
        arn_member: "TrustedTokenIssuerArn",
    },
];

impl Create {
    /// The request body: everything but `Tags` held fixed across a row, so the tag set is
    /// the only thing a replay varies.
    fn request(
        &self,
        instance_arn: &str,
        name: &str,
        client_token: &str,
        tags: &[(String, String)],
    ) -> Value {
        let mut body = match self.operation {
            "CreateApplication" => json!({
                "InstanceArn": instance_arn,
                "ApplicationProviderArn": PROVIDER_CUSTOM,
                "Name": name,
                "ClientToken": client_token,
            }),
            _ => json!({
                "InstanceArn": instance_arn,
                "Name": name,
                "TrustedTokenIssuerType": ISSUER_TYPE,
                "TrustedTokenIssuerConfiguration": {
                    "OidcJwtConfiguration": {
                        "IssuerUrl": "https://issuer.example.com/oidc",
                        "ClaimAttributePath": "email",
                        "IdentityStoreAttributePath": "emails.primary.value",
                        "JwksRetrievalOption": JWKS_RETRIEVAL_OPTION,
                    }
                },
                "ClientToken": client_token,
            }),
        };
        if let Some(tags) = tags_member(tags) {
            body["Tags"] = tags;
        }
        body
    }
}

/// Both creates fold a tag set into the request their `ClientToken` is scoped to the same
/// way: reordering the tags is a replay, changing the set is a conflict.
#[tokio::test]
async fn both_creates_judge_a_reused_client_token_by_the_same_sorted_tag_fold() {
    let (router, instance_arn) = organization_router();

    for create in CREATES {
        for (index, row) in rows().into_iter().enumerate() {
            let name = format!("{}-row-{index}", create.operation);
            let token = format!("tag-fold-{}-{index}", create.operation);
            let case = format!("{} with {}", create.operation, row.label);

            let (status, body) = call(
                &router,
                create.operation,
                &create.request(&instance_arn, &name, &token, &row.tags),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the first create must answer 200, got {status} {body}"
            );
            let first = body[create.arn_member]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: the create minted no ARN: {body}"))
                .to_string();

            // Every ordering of the SAME set is the same request: a replay.
            for (ordering, tags) in row.reorderings() {
                let (status, replayed) = call(
                    &router,
                    create.operation,
                    &create.request(&instance_arn, &name, &token, &tags),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}, tags {ordering}: a replay must answer 200, got {status} \
                     {replayed}"
                );
                assert_eq!(
                    replayed[create.arn_member].as_str(),
                    Some(first.as_str()),
                    "case {case:?}, tags {ordering}: a replay must answer the first ARN \
                     {first:?}, got {replayed}"
                );
            }

            // A set whose CONTENT differs is another request under the same token.
            for (why, tags) in &row.different {
                let (status, refused) = call(
                    &router,
                    create.operation,
                    &create.request(&instance_arn, &name, &token, tags),
                )
                .await;
                assert_eq!(
                    status, 400,
                    "case {case:?}, {why}: the reused token must be refused 400, got {status} \
                     {refused}"
                );
                assert_eq!(
                    error_code(&refused),
                    "ConflictException",
                    "case {case:?}, {why}: expected ConflictException, got {refused}"
                );
            }

            // The conflicts minted nothing: the token still replays to the first ARN.
            let (status, still) = call(
                &router,
                create.operation,
                &create.request(&instance_arn, &name, &token, &row.tags),
            )
            .await;
            assert_eq!(
                still[create.arn_member].as_str(),
                Some(first.as_str()),
                "case {case:?}: after the conflicts the token must still replay to {first:?}, \
                 got {status} {still}"
            );
        }
    }
}
